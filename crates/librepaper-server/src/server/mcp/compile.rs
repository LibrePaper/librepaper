//! `document_compile`: ask the owner's open browser tab to compile a file.
//!
//! The server never compiles. It sends a `compile_request` frame on the
//! owner's room socket and waits for the tab to answer on
//! `POST /api/documents/{slug}/compile/{id}`. Pending requests and finished
//! results live in memory only: a restart forgets them, and a caller whose
//! request was forgotten simply asks again.
//!
//! An `lpt_` agent token authenticates as the owner's account, so a request
//! is relayed only to a socket whose principal equals the caller's, and a
//! reply is accepted only from the principal the request was sent for. A
//! reply made with an agent token is refused, so an agent cannot answer its
//! own request.

use std::collections::HashMap;
use std::time::Duration;

use super::*;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use librepaper_engine::storage::outgoing::Outgoing;
use tokio::sync::watch;

/// How long a tab has to answer a request.
const REQUEST_TTL_SECONDS: i64 = 600;
/// How long a finished result can be fetched.
const KEEP_FINISHED_SECONDS: i64 = 3600;
const MAX_FINISHED: usize = 64;
const MAX_PENDING_PER_PRINCIPAL: usize = 16;
const MAX_REPLY_BYTES: usize = 96 * 1024;
const MAX_DIAGNOSTICS: usize = 16 * 1024;
const MAX_LOG: usize = 16 * 1024;
const MAX_PROVENANCE: usize = 16 * 1024;
const MAX_MESSAGE: usize = 1024;
const MAX_REVISION: usize = 128;
const DEFAULT_WAIT_MS: u64 = 20_000;
const MAX_WAIT_MS: u64 = 25_000;
const MAX_RESULT_WAIT_MS: u64 = 10_000;
const NO_TAB: &str = "Open this project in a browser signed in as its owner, then try again.";

type Slot = watch::Receiver<Option<Value>>;

struct Entry {
    slug: String,
    principal: String,
    path: String,
    expires_at: i64,
    finished_at: Option<i64>,
    result: watch::Sender<Option<Value>>,
}

/// What became of a tab's reply.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Completion {
    Accepted,
    /// Unknown, finished, expired, or made for another principal or document.
    NotFound,
    TooLarge,
    Invalid,
}

#[derive(Default)]
pub(crate) struct CompileHub {
    entries: std::sync::Mutex<HashMap<String, Entry>>,
}

fn prune(entries: &mut HashMap<String, Entry>, now: i64) {
    entries.retain(|_, entry| match entry.finished_at {
        Some(at) => now - at < KEEP_FINISHED_SECONDS,
        None => entry.expires_at > now,
    });
    let mut finished: Vec<(i64, String)> = entries
        .iter()
        .filter_map(|(id, entry)| entry.finished_at.map(|at| (at, id.clone())))
        .collect();
    if finished.len() > MAX_FINISHED {
        finished.sort();
        for (_, id) in finished.iter().take(finished.len() - MAX_FINISHED) {
            entries.remove(id);
        }
    }
}

fn pending_result(id: &str, path: &str) -> Value {
    json!({
        "compile_id": id,
        "status": "pending",
        "path": path,
        "output": "none",
        "current": false,
    })
}

fn expired_result(id: &str, path: &str) -> Value {
    json!({
        "compile_id": id,
        "status": "unavailable",
        "path": path,
        "output": "none",
        "current": false,
        "error": "timeout",
        "message": "The editor tab did not answer in time.",
    })
}

/// The result object for one accepted reply, or why the reply is refused.
fn build_result(id: &str, path: &str, body: &Value) -> Result<Value, Completion> {
    let fields = body.as_object().ok_or(Completion::Invalid)?;
    if let Some(error) = fields.get("error") {
        let code = error
            .as_str()
            .filter(|code| {
                matches!(
                    *code,
                    "run_unavailable" | "run_disabled" | "busy" | "no_renderer" | "failed"
                )
            })
            .ok_or(Completion::Invalid)?;
        let message = match fields.get("message") {
            None | Some(Value::Null) => "",
            Some(Value::String(message)) => message,
            Some(_) => return Err(Completion::Invalid),
        };
        if message.len() > MAX_MESSAGE {
            return Err(Completion::TooLarge);
        }
        return Ok(json!({
            "compile_id": id,
            "status": "unavailable",
            "path": path,
            "output": "none",
            "current": false,
            "error": code,
            "message": message,
        }));
    }
    let ok = fields
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or(Completion::Invalid)?;
    let revision = fields
        .get("revision")
        .and_then(Value::as_str)
        .filter(|revision| !revision.is_empty() && revision.len() <= MAX_REVISION)
        .ok_or(Completion::Invalid)?;
    let output = fields
        .get("output")
        .and_then(Value::as_str)
        .filter(|output| matches!(*output, "pdf" | "html" | "none"))
        .ok_or(Completion::Invalid)?;
    let diagnostics = match fields.get("diagnostics") {
        None | Some(Value::Null) => json!([]),
        Some(diagnostics) if diagnostics.is_array() => diagnostics.clone(),
        Some(_) => return Err(Completion::Invalid),
    };
    if diagnostics.to_string().len() > MAX_DIAGNOSTICS {
        return Err(Completion::TooLarge);
    }
    let log = match fields.get("log") {
        None | Some(Value::Null) => "",
        Some(Value::String(log)) => log,
        Some(_) => return Err(Completion::Invalid),
    };
    if log.len() > MAX_LOG {
        return Err(Completion::TooLarge);
    }
    let provenance = match fields.get("provenance") {
        None | Some(Value::Null) => json!({}),
        Some(provenance) if provenance.is_object() => provenance.clone(),
        Some(_) => return Err(Completion::Invalid),
    };
    if provenance.to_string().len() > MAX_PROVENANCE {
        return Err(Completion::TooLarge);
    }
    Ok(json!({
        "compile_id": id,
        "status": if ok { "succeeded" } else { "failed" },
        "path": path,
        "output": output,
        "revision": revision,
        "current": false,
        "diagnostics": diagnostics,
        "log": log,
        "provenance": provenance,
    }))
}

impl CompileHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record a new pending request for `principal`.
    pub(super) fn register(
        &self,
        slug: &str,
        principal: &str,
        path: &str,
        now: i64,
    ) -> Result<(String, Slot), Failure> {
        let mut entries = self.lock();
        prune(&mut entries, now);
        let pending = entries
            .values()
            .filter(|entry| entry.finished_at.is_none() && entry.principal == principal)
            .count();
        if pending >= MAX_PENDING_PER_PRINCIPAL {
            return Err(Failure::new(
                "budget_exceeded",
                "too many compiles are waiting for the editor tab; wait for one to finish",
            ));
        }
        let id = format!(
            "compile_{}",
            hex::encode(librepaper_base::util::random_bytes(16))
        );
        let (result, receiver) = watch::channel(None);
        entries.insert(
            id.clone(),
            Entry {
                slug: slug.to_owned(),
                principal: principal.to_owned(),
                path: path.to_owned(),
                expires_at: now + REQUEST_TTL_SECONDS,
                finished_at: None,
                result,
            },
        );
        Ok((id, receiver))
    }

    /// Forget a request that could not be delivered.
    pub(super) fn forget(&self, id: &str) {
        self.lock().remove(id);
    }

    /// The path and result slot of a request this caller made.
    pub(super) fn lookup(
        &self,
        slug: &str,
        id: &str,
        principal: &str,
        now: i64,
    ) -> Option<(String, Slot)> {
        let mut entries = self.lock();
        prune(&mut entries, now);
        let entry = entries
            .get(id)
            .filter(|entry| entry.slug == slug && entry.principal == principal)?;
        Some((entry.path.clone(), entry.result.subscribe()))
    }

    /// Take a tab's reply. Only a pending, unexpired request for this
    /// document and principal accepts one.
    pub(super) fn complete(
        &self,
        slug: &str,
        id: &str,
        principal: &str,
        body: &Value,
        now: i64,
    ) -> Completion {
        let mut entries = self.lock();
        prune(&mut entries, now);
        let Some(entry) = entries.get_mut(id).filter(|entry| {
            entry.slug == slug && entry.principal == principal && entry.finished_at.is_none()
        }) else {
            return Completion::NotFound;
        };
        match build_result(id, &entry.path, body) {
            Ok(result) => {
                entry.finished_at = Some(now);
                entry.result.send_replace(Some(result));
                prune(&mut entries, now);
                Completion::Accepted
            }
            Err(refusal) => refusal,
        }
    }
}

/// Wait up to `wait` for a result. A request nobody answered by its expiry
/// reports `timeout`; one still running reports `pending`.
async fn wait_for(id: &str, path: &str, mut slot: Slot, wait: Duration) -> Value {
    let outcome = tokio::time::timeout(wait, slot.wait_for(|value| value.is_some()))
        .await
        .map(|received| received.map(|value| value.clone()));
    match outcome {
        Ok(Ok(Some(result))) => result,
        Ok(Ok(None)) | Err(_) => pending_result(id, path),
        Ok(Err(_)) => expired_result(id, path),
    }
}

impl Server {
    /// Whether the compiled revision is the one the server holds now.
    async fn compile_current(&self, slug: &str, mut result: Value) -> Value {
        let current = match result["revision"].as_str() {
            Some(revision) => match self.rooms.get(slug).await {
                Ok(room) => room
                    .agent_query_snapshot()
                    .await
                    .is_ok_and(|snapshot| snapshot.tree_digest == revision),
                Err(_) => false,
            },
            None => false,
        };
        result["current"] = json!(current);
        result
    }

    /// The `document_compile` tool.
    pub(super) async fn mcp_compile(
        &self,
        who: &Viewer,
        slug: &str,
        args: &Value,
    ) -> Result<Value, Failure> {
        let path = args["path"].as_str().unwrap_or_default();
        if !render::valid_path(path) {
            return Err(Failure::new("invalid_params", "path is not a project file"));
        }
        let run = args["run"].as_bool().unwrap_or(false);
        if run && !who.at_least(Role::Editor) {
            let mut failure = Failure::new("forbidden", "running code needs an Editor token");
            failure.committed = Committed::No;
            return Err(failure);
        }
        let wait = args["wait_ms"]
            .as_u64()
            .unwrap_or(DEFAULT_WAIT_MS)
            .min(MAX_WAIT_MS);
        let no_tab = || Failure::new("renderer_unavailable", NO_TAB);
        if !who.id.is_signed_in() {
            return Err(no_tab());
        }
        // The most recently connected socket is the tab the owner just
        // used. Socket ids only grow.
        let mut tabs: Vec<_> = self
            .connections
            .lock()
            .await
            .iter()
            .filter(|(_, connection)| {
                connection.slug == slug
                    && connection.principal == who.id.id
                    && connection.chat.is_none()
            })
            .map(|(socket, connection)| (*socket, connection.tx.clone()))
            .collect();
        tabs.sort_by_key(|tab| std::cmp::Reverse(tab.0));
        if tabs.is_empty() {
            return Err(no_tab());
        }
        let now = now_unix();
        let (id, slot) = self.compile.register(slug, &who.id.id, path, now)?;
        let frame = json!({
            "type": "compile_request",
            "id": id,
            "path": path,
            "output": args.get("output").filter(|output| output.is_string()),
            "run": run,
            "expires_at": now + REQUEST_TTL_SECONDS,
        })
        .to_string();
        if !tabs
            .iter()
            .any(|(_, tx)| tx.try_send(Outgoing::Text(frame.clone())).is_ok())
        {
            self.compile.forget(&id);
            return Err(no_tab());
        }
        let result = wait_for(&id, path, slot, Duration::from_millis(wait)).await;
        Ok(self.compile_current(slug, result).await)
    }

    /// `document_result` with `kind: "compile"`.
    pub(super) async fn mcp_compile_result(
        &self,
        who: &Viewer,
        slug: &str,
        args: &Value,
    ) -> Result<Value, Failure> {
        let id = args["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or_else(|| Failure::new("invalid_params", "id is required"))?;
        let wait = args["wait_ms"]
            .as_u64()
            .unwrap_or(0)
            .min(MAX_RESULT_WAIT_MS);
        let (path, slot) = self
            .compile
            .lookup(slug, id, &who.id.id, now_unix())
            .ok_or_else(|| Failure::new("not_found", "no such compile result"))?;
        let result = wait_for(id, &path, slot, Duration::from_millis(wait)).await;
        Ok(self.compile_current(slug, result).await)
    }

    /// `POST /api/documents/{slug}/compile/{id}`: the tab's answer.
    pub(crate) async fn handle_compile_reply(
        &self,
        request: Request<Body>,
        context: &RequestContext,
        slug: &str,
        id: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(404, "not found");
        }
        let headers = request.headers().clone();
        if !header_of(&headers, "content-type").is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
        }) {
            return plain(415, "expected application/json");
        }
        if cross_site_refused(&headers, &context.arrival) {
            return plain(403, "origin refused");
        }
        let auth = match self
            .authorize::<access::rung::Read>(context, &headers, slug, None)
            .await
        {
            Ok(auth) => auth,
            Err(refusal) => return access::deny(&refusal),
        };
        let who = &auth.viewer;
        // An agent token acts as the owner's account; it must not be able to
        // answer the request it made.
        if who.token.is_some() || who.automation || Server::is_automation(&headers) {
            return plain(404, "not found");
        }
        let body = match to_bytes(request.into_body(), MAX_REPLY_BYTES).await {
            Ok(body) => body,
            Err(_) => return plain(413, "reply is too large"),
        };
        let Ok(body) = serde_json::from_slice::<Value>(&body) else {
            return plain(400, "invalid JSON");
        };
        match self
            .compile
            .complete(slug, id, &who.id.id, &body, now_unix())
        {
            Completion::Accepted => {
                let mut response = Response::new(Body::empty());
                *response.status_mut() = StatusCode::NO_CONTENT;
                set(&mut response, "cache-control", "no-store");
                response
            }
            Completion::NotFound => plain(404, "not found"),
            Completion::TooLarge => plain(413, "reply exceeds its limit"),
            Completion::Invalid => plain(400, "invalid compile reply"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn success() -> Value {
        json!({
            "revision": "abc",
            "ok": true,
            "output": "pdf",
            "diagnostics": [{"severity": "error", "message": "x"}],
            "log": "done",
            "provenance": {"engine": "pdftex"},
        })
    }

    fn registered(hub: &CompileHub) -> (String, Slot) {
        hub.register("doc", "owner", "paper.tex", NOW)
            .map_err(|failure| failure.message)
            .unwrap()
    }

    #[test]
    fn a_reply_from_the_requesting_principal_is_accepted() {
        let hub = CompileHub::default();
        let (id, slot) = registered(&hub);
        assert_eq!(
            hub.complete("doc", &id, "owner", &success(), NOW + 1),
            Completion::Accepted
        );
        let result = slot.borrow().clone().unwrap();
        assert_eq!(result["status"], "succeeded");
        assert_eq!(result["path"], "paper.tex");
        assert_eq!(result["output"], "pdf");
        assert_eq!(result["revision"], "abc");
        // It cannot be answered twice.
        assert_eq!(
            hub.complete("doc", &id, "owner", &success(), NOW + 2),
            Completion::NotFound
        );
    }

    #[test]
    fn a_reply_from_anyone_else_or_for_nothing_is_refused() {
        let hub = CompileHub::default();
        let (id, _slot) = registered(&hub);
        assert_eq!(
            hub.complete("doc", &id, "someone-else", &success(), NOW),
            Completion::NotFound
        );
        assert_eq!(
            hub.complete("other-doc", &id, "owner", &success(), NOW),
            Completion::NotFound
        );
        assert_eq!(
            hub.complete("doc", "compile_unknown", "owner", &success(), NOW),
            Completion::NotFound
        );
        assert!(hub.lookup("doc", &id, "someone-else", NOW).is_none());
        assert!(hub.lookup("other-doc", &id, "owner", NOW).is_none());
    }

    #[test]
    fn an_expired_request_takes_no_reply() {
        let hub = CompileHub::default();
        let (id, _slot) = registered(&hub);
        let late = NOW + REQUEST_TTL_SECONDS;
        assert_eq!(
            hub.complete("doc", &id, "owner", &success(), late),
            Completion::NotFound
        );
        assert!(hub.lookup("doc", &id, "owner", late).is_none());
    }

    #[test]
    fn replies_over_a_limit_are_refused_and_leave_the_request_pending() {
        let hub = CompileHub::default();
        let (id, _slot) = registered(&hub);
        let mut big_log = success();
        big_log["log"] = json!("x".repeat(MAX_LOG + 1));
        assert_eq!(
            hub.complete("doc", &id, "owner", &big_log, NOW),
            Completion::TooLarge
        );
        let mut big_diagnostics = success();
        big_diagnostics["diagnostics"] = json!(vec!["x".repeat(1024); 17]);
        assert_eq!(
            hub.complete("doc", &id, "owner", &big_diagnostics, NOW),
            Completion::TooLarge
        );
        let mut big_provenance = success();
        big_provenance["provenance"] = json!({"k": "x".repeat(MAX_PROVENANCE)});
        assert_eq!(
            hub.complete("doc", &id, "owner", &big_provenance, NOW),
            Completion::TooLarge
        );
        let mut bad_output = success();
        bad_output["output"] = json!("exe");
        assert_eq!(
            hub.complete("doc", &id, "owner", &bad_output, NOW),
            Completion::Invalid
        );
        assert_eq!(
            hub.complete("doc", &id, "owner", &success(), NOW),
            Completion::Accepted
        );
    }

    #[test]
    fn a_tab_that_cannot_compile_reports_unavailable() {
        let hub = CompileHub::default();
        let (id, slot) = registered(&hub);
        let reply = json!({"error": "run_disabled", "message": "The owner has not allowed it."});
        assert_eq!(
            hub.complete("doc", &id, "owner", &reply, NOW),
            Completion::Accepted
        );
        let result = slot.borrow().clone().unwrap();
        assert_eq!(result["status"], "unavailable");
        assert_eq!(result["error"], "run_disabled");
        assert_eq!(
            hub.complete("doc", &id, "owner", &json!({"error": "bogus"}), NOW),
            Completion::NotFound
        );
    }

    #[test]
    fn finished_results_are_kept_but_bounded() {
        let hub = CompileHub::default();
        let mut first = None;
        for index in 0..(MAX_FINISHED + 3) {
            let (id, _slot) = registered(&hub);
            let at = NOW + index as i64;
            assert_eq!(
                hub.complete("doc", &id, "owner", &success(), at),
                Completion::Accepted
            );
            first.get_or_insert(id);
        }
        let now = NOW + 100;
        assert!(hub.lookup("doc", &first.unwrap(), "owner", now).is_none());
        let kept = hub
            .lock()
            .values()
            .filter(|e| e.finished_at.is_some())
            .count();
        assert_eq!(kept, MAX_FINISHED);
        let (id, _slot) = registered(&hub);
        hub.complete("doc", &id, "owner", &success(), now);
        let (_, slot) = hub.lookup("doc", &id, "owner", now).unwrap();
        assert!(slot.borrow().is_some());
        // An hour later it is gone.
        assert!(hub
            .lookup("doc", &id, "owner", now + KEEP_FINISHED_SECONDS)
            .is_none());
    }

    #[test]
    fn one_principal_cannot_queue_without_bound() {
        let hub = CompileHub::default();
        let slots: Vec<_> = (0..MAX_PENDING_PER_PRINCIPAL)
            .map(|_| registered(&hub))
            .collect();
        assert!(hub.register("doc", "owner", "paper.tex", NOW).is_err());
        assert!(hub.register("doc", "another", "paper.tex", NOW).is_ok());
        drop(slots);
    }

    #[tokio::test]
    async fn an_unanswered_wait_is_pending_and_an_answer_ends_it() {
        let hub = CompileHub::default();
        let (id, slot) = registered(&hub);
        let pending = wait_for(&id, "paper.tex", slot.clone(), Duration::from_millis(10)).await;
        assert_eq!(pending["status"], "pending");
        assert_eq!(pending["compile_id"], json!(id));
        hub.complete("doc", &id, "owner", &success(), NOW);
        let done = wait_for(&id, "paper.tex", slot, Duration::from_millis(10)).await;
        assert_eq!(done["status"], "succeeded");
    }

    #[tokio::test]
    async fn a_forgotten_request_reports_a_timeout() {
        let hub = CompileHub::default();
        let (id, slot) = registered(&hub);
        hub.forget(&id);
        let result = wait_for(&id, "paper.tex", slot, Duration::from_millis(10)).await;
        assert_eq!(result["status"], "unavailable");
        assert_eq!(result["error"], "timeout");
    }
}
