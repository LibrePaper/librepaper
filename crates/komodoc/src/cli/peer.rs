//! Reusable, headless automation peer for Komodoc documents.
//!
//! The peer deliberately keeps the share key in memory and out of all
//! serialised output.  A link is a complete credential: a cached bearer may
//! identify the caller for attribution, but never widens the role carried by
//! the link.

use std::time::{Duration, Instant};

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::cli::{link_key, stored_agent_token_for};
use crate::document::session;
use crate::http::{detail_of, KEY_HEADER};
use crate::room::encode_update;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RETRIES: usize = 3;

/// The authority conveyed by a document link.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Capabilities {
    pub role: String,
    pub can_read: bool,
    pub can_comment: bool,
    pub can_edit: bool,
    pub can_resolve: bool,
    pub can_delete: bool,
    pub can_checkpoint: bool,
}

impl Capabilities {
    fn from_document(document: &Value) -> Self {
        let role = document
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let can_edit = document
            .get("can_edit")
            .and_then(Value::as_bool)
            .unwrap_or(matches!(role.as_str(), "owner" | "editor"));
        let can_comment = document
            .get("can_comment")
            .and_then(Value::as_bool)
            .unwrap_or(matches!(role.as_str(), "owner" | "editor" | "commenter"));
        Self {
            can_read: document
                .get("can_read")
                .and_then(Value::as_bool)
                .unwrap_or(!role.is_empty()),
            can_comment,
            can_edit,
            can_resolve: document
                .get("can_resolve")
                .and_then(Value::as_bool)
                .unwrap_or(can_comment),
            can_delete: document
                .get("can_delete")
                .and_then(Value::as_bool)
                .unwrap_or(can_comment),
            can_checkpoint: document
                .get("can_checkpoint")
                .and_then(Value::as_bool)
                .unwrap_or(can_edit),
            role,
        }
    }
}

/// A parsed document link. `key` is intentionally private and never appears
/// in Debug, JSON, or error text.
#[derive(Clone, PartialEq, Eq)]
pub struct DocumentLink {
    server: String,
    slug: String,
    path: String,
    key: String,
}

impl std::fmt::Debug for DocumentLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocumentLink")
            .field("server", &self.server)
            .field("slug", &self.slug)
            .field("path", &self.path)
            .field("key", &"[redacted]")
            .finish()
    }
}

impl DocumentLink {
    /// Parse a pasted `/docs/<slug>#k=...` or `/raw/<slug>#k=...` link.
    /// Bare slugs are accepted when `default_server` is supplied.
    pub fn parse(input: &str, default_server: &str) -> Result<Self, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("a document link is required".into());
        }
        let (server, slug, path, key) = if let Ok(url) = url::Url::parse(input) {
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err("document links must use http or https".into());
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err("document links may not contain userinfo".into());
            }
            let server = url.origin().ascii_serialization();
            let segments: Vec<_> = url
                .path_segments()
                .map(|parts| parts.collect())
                .unwrap_or_default();
            let slug = match segments.as_slice() {
                [prefix, slug] if matches!(*prefix, "docs" | "raw" | "document") => *slug,
                [slug] => *slug,
                _ => return Err("the document link must identify one document".into()),
            };
            if slug.is_empty() {
                return Err("the document link has no document id".into());
            }
            let path = url
                .query_pairs()
                .find_map(|(name, value)| (name == "file").then_some(value.into_owned()))
                .unwrap_or_default();
            validate_file_path(&path)?;
            let key = key_from_url(&url);
            (server, slug.to_string(), path, key)
        } else {
            let server = default_server.trim().trim_end_matches('/');
            if server.is_empty() {
                return Err("a full document link or --server is required".into());
            }
            (
                server.to_string(),
                input.trim_matches('/').to_string(),
                String::new(),
                String::new(),
            )
        };
        if slug.is_empty() || slug.contains('/') || slug == "." || slug == ".." {
            return Err("the document link has an invalid document id".into());
        }
        Ok(Self {
            server,
            slug,
            path,
            key,
        })
    }

    pub fn server(&self) -> &str {
        &self.server
    }
    pub fn slug(&self) -> &str {
        &self.slug
    }
    pub fn has_key(&self) -> bool {
        !self.key.is_empty()
    }
    pub fn path(&self) -> &str {
        &self.path
    }
}

fn key_from_url(url: &url::Url) -> String {
    let fragment = url.fragment().unwrap_or_default();
    let from_fragment = link_key(&format!("#{}", fragment));
    if !from_fragment.is_empty() {
        return from_fragment;
    }
    url.query_pairs()
        .find(|(name, _)| name == "k" || name == "key")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

fn validate_file_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        if path.is_empty() {
            return Ok(());
        }
        return Err("the selected file path is invalid".into());
    }
    Ok(())
}

/// One source-and-annotation read from the authenticated v1 snapshot route.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    /// Canonical live source-tree revision used by anchored suggestions.
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub main: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_sha: String,
    #[serde(default)]
    pub comments: Vec<Value>,
    #[serde(default)]
    pub files: Value,
    #[serde(default)]
    pub texts: Value,
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// The result of a write, retaining the request id so callers can correlate
/// an acknowledgement or investigate an unknown post-submission outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationResult {
    pub request_id: String,
    pub status: u16,
    pub outcome: String,
    pub value: Value,
}

#[derive(Clone)]
pub struct AutomationPeer {
    link: DocumentLink,
    token: String,
    capabilities: Capabilities,
    client: reqwest::Client,
}

impl std::fmt::Debug for AutomationPeer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutomationPeer")
            .field("link", &self.link)
            .field("token", &"[redacted]")
            .field("capabilities", &self.capabilities)
            .field("client", &self.client)
            .finish()
    }
}

impl AutomationPeer {
    /// Open a link and report the effective role before any write is attempted.
    pub async fn open(link: DocumentLink) -> Result<Self, String> {
        let token = stored_agent_token_for(link.server());
        let client = new_client()?;
        let mut request = client.get(format!("{}/api/documents/{}", link.server(), link.slug()));
        if !token.is_empty() {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if !link.key.is_empty() {
            request = request.header(KEY_HEADER, &link.key);
        }
        let response = request
            .header("x-komodoc-automation", "1")
            .header("x-komodoc-client", "1")
            .send()
            .await
            .map_err(|err| format!("request failed: {err}"))?;
        let status = response.status().as_u16();
        let raw = response
            .bytes()
            .await
            .map_err(|err| format!("could not read document response: {err}"))?;
        let document: Value = serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"error": String::from_utf8_lossy(&raw)}));
        if status != 200 {
            return Err(format!(
                "could not open document ({}): {}",
                status,
                detail_of(&document)
            ));
        }
        Ok(Self {
            link,
            token,
            capabilities: Capabilities::from_document(&document),
            client,
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    pub fn link(&self) -> &DocumentLink {
        &self.link
    }

    pub async fn snapshot(&self) -> Result<Snapshot, String> {
        let endpoint = format!(
            "{}/api/documents/{}/snapshot",
            self.link.server(),
            self.link.slug()
        );
        let response = self.request(reqwest::Method::GET, &endpoint, None).await?;
        if !response.status().is_success() {
            return Err(http_error(response).await);
        }
        let raw = response
            .bytes()
            .await
            .map_err(|err| format!("could not read snapshot response: {err}"))?;
        let value: Value = serde_json::from_slice(&raw)
            .map_err(|err| format!("invalid snapshot response: {err}"))?;
        if value["version"] != 1 || value["protocol"] != "komodoc.snapshot.v1" {
            return Err("server does not support the document snapshot format".into());
        }
        let mut snapshot: Snapshot = serde_json::from_value(value)
            .map_err(|err| format!("invalid snapshot response: {err}"))?;
        snapshot.capabilities = self.capabilities.clone();
        if snapshot.source_sha.is_empty() {
            snapshot.source_sha = source_sha(&snapshot.source);
        }
        Ok(snapshot)
    }
    pub async fn source(&self) -> Result<String, String> {
        let snapshot = self.snapshot().await?;
        if self.link.path.is_empty() {
            return Ok(snapshot.source);
        }
        snapshot
            .texts
            .get(self.link.path.as_str())
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("remote file {:?} does not exist", self.link.path))
    }
    pub async fn comments(&self) -> Result<Vec<Value>, String> {
        Ok(self.snapshot().await?.comments)
    }

    pub async fn comment(
        &self,
        body: &str,
        exact: &str,
        request_id: &str,
    ) -> Result<OperationResult, String> {
        self.comment_at(body, exact, self.link.path(), request_id)
            .await
    }

    pub async fn comment_at(
        &self,
        body: &str,
        exact: &str,
        path: &str,
        request_id: &str,
    ) -> Result<OperationResult, String> {
        validate_file_path(path)?;
        let source = (!path.is_empty()).then(|| json!({"path":path,"exact":exact}));
        self.comment_with_source(body, exact, source, None, request_id)
            .await
    }

    /// Add a comment with the complete source anchor and captured revision.
    /// The revision is deliberately sent at the top level, where the REST
    /// comments protocol records it alongside the source anchor.
    pub async fn comment_with_source(
        &self,
        body: &str,
        exact: &str,
        source: Option<Value>,
        revision: Option<&str>,
        request_id: &str,
    ) -> Result<OperationResult, String> {
        let mut payload = json!({
            "type":"comment", "body":body, "exact":exact, "source":source,
            "motivation":"commenting", "request_id":request_id,
            "version":1, "protocol":"komodoc.room.v1"
        });
        if let Some(revision) = revision.filter(|value| !value.is_empty()) {
            payload["revision"] = json!(revision);
        }
        self.annotation("comment", request_id, payload).await
    }

    pub async fn reply(
        &self,
        comment_id: &str,
        body: &str,
        request_id: &str,
    ) -> Result<OperationResult, String> {
        self.annotation(
            "reply",
            request_id,
            json!({"type":"reply", "comment_id":comment_id, "body":body, "temp_id":request_id, "request_id":request_id, "version":1, "protocol":"komodoc.room.v1"}),
        )
        .await
    }

    pub async fn resolve(
        &self,
        comment_id: &str,
        resolved: bool,
        request_id: &str,
    ) -> Result<OperationResult, String> {
        self.annotation("resolve", request_id, json!({"type":"resolve", "comment_id":comment_id, "resolved":resolved, "temp_id":request_id, "request_id":request_id, "version":1, "protocol":"komodoc.room.v1"})).await
    }

    pub async fn delete(
        &self,
        comment_id: &str,
        request_id: &str,
    ) -> Result<OperationResult, String> {
        self.annotation(
            "delete",
            request_id,
            json!({"type":"delete", "comment_id":comment_id, "temp_id":request_id, "request_id":request_id, "version":1, "protocol":"komodoc.room.v1"}),
        )
        .await
    }

    async fn annotation(
        &self,
        kind: &str,
        request_id: &str,
        mut payload: Value,
    ) -> Result<OperationResult, String> {
        if request_id.trim().is_empty() {
            return Err("a non-empty request id is required".into());
        }
        payload_set_submission_id(&mut payload, stable_submission_id(kind, request_id));
        if matches!(kind, "comment" | "reply") && !self.capabilities.can_comment {
            return Err("this link cannot add annotations".into());
        }
        if kind == "delete" && !self.capabilities.can_delete {
            return Err("this link cannot delete annotations".into());
        }
        if kind == "resolve" && !self.capabilities.can_resolve {
            return Err("this link cannot resolve annotations".into());
        }
        let endpoint = format!(
            "{}/api/documents/{}/comments",
            self.link.server(),
            self.link.slug()
        );
        let mut last_error = String::new();
        for attempt in 0..RETRIES {
            match self
                .request(reqwest::Method::POST, &endpoint, Some(payload.clone()))
                .await
            {
                Ok(response) if response.status().is_success() => {
                    let status = response.status().as_u16();
                    match response.bytes().await {
                        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
                            Ok(value) => {
                                return Ok(OperationResult {
                                    request_id: request_id.to_string(),
                                    status,
                                    outcome: "success".into(),
                                    value,
                                });
                            }
                            Err(err) => {
                                last_error = format!("invalid annotation response: {err}");
                            }
                        },
                        Err(err) => {
                            last_error = format!("could not read annotation response: {err}");
                        }
                    }
                }
                Ok(response) => {
                    let status = response.status().as_u16();
                    let retryable = (500..=599).contains(&status);
                    match response.bytes().await {
                        Ok(raw) => {
                            let value = serde_json::from_slice::<Value>(&raw).unwrap_or_else(
                                |_| json!({"error": String::from_utf8_lossy(&raw)}),
                            );
                            if !retryable {
                                return Ok(OperationResult {
                                    request_id: request_id.to_string(),
                                    status,
                                    outcome: "error".into(),
                                    value,
                                });
                            }
                            last_error = format!("annotation request failed ({status})");
                        }
                        Err(err) => {
                            last_error = format!("could not read annotation response: {err}");
                        }
                    }
                }
                Err(err) => {
                    last_error = err;
                }
            }
            if attempt + 1 < RETRIES {
                tokio::time::sleep(Duration::from_millis(50 * (attempt as u64 + 1))).await;
            }
        }
        Err(last_error)
    }

    /// Update source through the same Yjs room used by browsers. The expected
    /// SHA is checked immediately before opening the room, so stale prepared
    /// edits produce an explicit result instead of replacing newer text.
    pub async fn edit_source(
        &self,
        body: &str,
        expected_sha: &str,
    ) -> Result<OperationResult, String> {
        self.edit_source_at(body, self.link.path(), expected_sha)
            .await
    }

    /// Edit a selected text file in a directory document using the same Yjs
    /// transaction as a browser. The path is a target within the link's
    /// existing authority; it does not change that authority.
    pub async fn edit_source_at(
        &self,
        body: &str,
        path: &str,
        expected_sha: &str,
    ) -> Result<OperationResult, String> {
        validate_file_path(path)?;
        if !self.capabilities.can_edit {
            return Err("this link cannot edit source".into());
        }
        if expected_sha.is_empty() {
            return Err("expected source SHA is required; read the snapshot first".into());
        }
        let request_id = format!("edit-{}", source_sha(body));
        let current = self.snapshot().await?;
        let expected_actual = if path.is_empty() {
            current.source_sha.clone()
        } else {
            snapshot_text_sha(&current.texts, path)
                .ok_or_else(|| format!("remote file {path:?} does not exist"))?
        };
        if !expected_sha.is_empty() && expected_actual != expected_sha {
            return Ok(OperationResult {
                request_id: request_id.clone(),
                status: 409,
                outcome: "stale-input".into(),
                value: json!({"expected_sha": expected_sha, "actual_sha": expected_actual}),
            });
        }
        let request = self.socket_request()?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let (socket, _) = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| "timed out joining document".to_string())?
        .map_err(|err| format!("could not join document: {err}"))?;
        use futures_util::{SinkExt, StreamExt};
        let (mut write, mut read) = socket.split();
        let mut submitted = false;
        let seq = 1_i64;
        tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            write.send(Message::Text(json!({"type":"y-open", "vector": encode_update(&session::encode_vector(&session::new_doc()))}).to_string().into())),
        )
        .await
        .map_err(|_| "timed out opening document".to_string())?
        .map_err(|err| err.to_string())?;
        let doc = session::new_doc();
        loop {
            let frame = match tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                read.next(),
            )
            .await
            {
                Ok(Some(Ok(frame))) => frame,
                Ok(Some(Err(err))) => {
                    if submitted {
                        return Ok(unknown_edit_result(
                            &request_id,
                            expected_sha,
                            body,
                            &format!("document session failed after submission: {err}"),
                        ));
                    }
                    return Err(err.to_string());
                }
                Ok(None) => {
                    if submitted {
                        return Ok(unknown_edit_result(
                            &request_id,
                            expected_sha,
                            body,
                            "document session closed before edit acknowledgement",
                        ));
                    }
                    return Err("document session closed before document state".into());
                }
                Err(_) => {
                    if submitted {
                        return Ok(unknown_edit_result(
                            &request_id,
                            expected_sha,
                            body,
                            "timed out waiting for document persistence acknowledgement",
                        ));
                    }
                    return Err("timed out waiting for document state".into());
                }
            };
            let Message::Text(raw) = frame else {
                if let Message::Close(reason) = frame {
                    let reason = reason
                        .as_ref()
                        .map(|close| close.reason.to_string())
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or_else(|| "document session closed".into());
                    if submitted {
                        return Ok(unknown_edit_result(
                            &request_id,
                            expected_sha,
                            body,
                            &reason,
                        ));
                    }
                    return Err(reason);
                }
                continue;
            };
            let message: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
            match message
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "y-state" => {
                    let bytes = if let Some(encoded) = message.get("update").and_then(Value::as_str)
                    {
                        crate::room::decode_update(encoded)
                            .ok_or_else(|| "invalid document state".to_string())?
                    } else if let Some(reference) = message.get("ref").and_then(Value::as_str) {
                        tokio::time::timeout(
                            deadline.saturating_duration_since(Instant::now()),
                            self.fetch_state(reference),
                        )
                        .await
                        .map_err(|_| "timed out fetching document state".to_string())??
                    } else {
                        Vec::new()
                    };
                    if !bytes.is_empty() {
                        session::apply_update(&doc, &bytes)?;
                    }
                    if message.get("ref").and_then(Value::as_str).is_some() {
                        let vector = encode_update(&session::encode_vector(&doc));
                        tokio::time::timeout(
                            deadline.saturating_duration_since(Instant::now()),
                            write.send(Message::Text(json!({"type":"y-sync", "vector":vector, "version":1, "protocol":"komodoc.room.v1"}).to_string().into())),
                        )
                        .await
                        .map_err(|_| "timed out requesting document catch-up".to_string())?
                        .map_err(|err| err.to_string())?;
                        continue;
                    }
                    if submitted {
                        continue;
                    }
                    if !path.is_empty() && !session::texts_of(&doc).contains_key(path) {
                        return Err(format!("remote file {path:?} does not exist"));
                    }
                    let old = if path.is_empty() {
                        session::text_of(&doc)
                    } else {
                        session::texts_of(&doc)
                            .get(path)
                            .cloned()
                            .unwrap_or_default()
                    };
                    // Recheck after the handshake: the HTTP snapshot may
                    // have become stale while this peer was joining.
                    if !expected_sha.is_empty() && source_sha(&old) != expected_sha {
                        let _ = write.send(Message::Close(None)).await;
                        return Ok(OperationResult {
                            request_id: request_id.clone(),
                            status: 409,
                            outcome: "stale-input".into(),
                            value: json!({"expected_sha": expected_sha, "actual_sha": source_sha(&old)}),
                        });
                    }
                    if old != body {
                        let before = session::encode_vector(&doc);
                        let edits = komodoc_text::diff(&old, body);
                        let applied = if path.is_empty() {
                            session::apply_edits(&doc, &edits);
                            true
                        } else {
                            session::apply_edits_at(&doc, path, &edits)
                        };
                        if !applied {
                            return Err(format!("remote file {path:?} does not exist"));
                        }
                        let update = session::encode_diff(&doc, &before)?;
                        let mut messages = crate::cli::sync::update_messages(&update, seq);
                        if messages.is_empty() {
                            return Err("could not frame empty document edit".into());
                        }
                        for message in &mut messages {
                            message["request_id"] = Value::String(request_id.clone());
                            message["version"] = json!(1);
                            message["protocol"] = json!("komodoc.room.v1");
                            submitted = true;
                            let raw = serde_json::to_string(message)
                                .map_err(|err| format!("could not encode document edit: {err}"))?;
                            match tokio::time::timeout(
                                deadline.saturating_duration_since(Instant::now()),
                                write.send(Message::Text(raw.into())),
                            )
                            .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(err)) => {
                                    return Ok(unknown_edit_result(
                                        &request_id,
                                        expected_sha,
                                        body,
                                        &format!(
                                            "document edit send failed after submission: {err}"
                                        ),
                                    ));
                                }
                                Err(_) => {
                                    return Ok(unknown_edit_result(
                                        &request_id,
                                        expected_sha,
                                        body,
                                        "timed out sending document edit after submission",
                                    ));
                                }
                            }
                        }
                    } else {
                        write.send(Message::Close(None)).await.ok();
                        return Ok(OperationResult {
                            request_id: request_id.clone(),
                            status: 200,
                            outcome: "no-op".into(),
                            value: json!({"source_sha": source_sha(&old)}),
                        });
                    }
                }
                "y-update" if !submitted => {
                    let bytes = message
                        .get("update")
                        .and_then(Value::as_str)
                        .and_then(crate::room::decode_update)
                        .ok_or("invalid document update")?;
                    session::apply_update(&doc, &bytes)?;
                }
                "y-ack" if message.get("seq").and_then(Value::as_i64) == Some(seq) => {
                    return Ok(OperationResult {
                        request_id: request_id.clone(),
                        status: 200,
                        outcome: "success".into(),
                        value: json!({"submitted_source_sha": source_sha(body)}),
                    });
                }
                "error" => {
                    return Err(message
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("document rejected the edit")
                        .to_string())
                }
                _ => {}
            }
        }
    }

    fn socket_request(&self) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
        let mut request = crate::cli::sync::socket_url(self.link.server(), self.link.slug())
            .into_client_request()
            .map_err(|err| err.to_string())?;
        if !self.token.is_empty() {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token)
                    .parse()
                    .map_err(|_| "invalid credential header".to_string())?,
            );
        }
        if !self.link.key.is_empty() {
            request.headers_mut().insert(
                crate::http::KEY_HEADER,
                self.link
                    .key
                    .parse()
                    .map_err(|_| "invalid credential header".to_string())?,
            );
        }
        request.headers_mut().insert(
            "x-komodoc-automation",
            "1".parse()
                .map_err(|_| "invalid automation header".to_string())?,
        );
        request.headers_mut().insert(
            "x-komodoc-client",
            "1".parse()
                .map_err(|_| "invalid client header".to_string())?,
        );
        Ok(request)
    }

    pub async fn checkpoint(&self, why: &str) -> Result<OperationResult, String> {
        if !self.capabilities.can_checkpoint {
            return Err("this link cannot request checkpoints".into());
        }
        use futures_util::{SinkExt, StreamExt};
        let request_id = random_request_id();
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            let (mut socket, _) = tokio_tungstenite::connect_async(self.socket_request()?)
                .await
                .map_err(|err| format!("could not join document: {err}"))?;
            socket
                .send(Message::Text(
                    json!({"type":"y-checkpoint", "why":why, "request_id":request_id})
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|err| err.to_string())?;
            while let Some(frame) = socket.next().await {
                let frame = frame.map_err(|err| err.to_string())?;
                let raw = match frame {
                    Message::Text(raw) => raw,
                    Message::Close(reason) => {
                        let reason = reason
                            .as_ref()
                            .map(|close| close.reason.to_string())
                            .filter(|reason| !reason.is_empty())
                            .unwrap_or_else(|| "document session closed".into());
                        return Err(reason);
                    }
                    _ => continue,
                };
                let message: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
                if message["type"] == "error" {
                    return Err(message["message"]
                        .as_str()
                        .unwrap_or("checkpoint rejected")
                        .to_string());
                }
                if message["type"] == "y-checkpoint" && message["request_id"] == request_id {
                    if message["durable"] != true {
                        return Err("checkpoint was not acknowledged as durable".into());
                    }
                    return Ok(OperationResult {
                        request_id,
                        status: 200,
                        outcome: "success".into(),
                        value: message,
                    });
                }
            }
            Err("document session closed before checkpoint acknowledgement".into())
        })
        .await
        .map_err(|_| "timed out waiting for checkpoint".to_string())?
    }

    async fn chat_request(
        &self,
        method: reqwest::Method,
        suffix: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let endpoint = format!(
            "{}/api/documents/{}/chat{}",
            self.link.server(),
            self.link.slug(),
            suffix
        );
        let mut request = self
            .client
            .request(method, endpoint)
            .header("x-komodoc-client", "1")
            .header("x-komodoc-automation", "1");
        if !self.token.is_empty() {
            request = request.header("authorization", format!("Bearer {}", self.token));
        }
        if !self.link.key.is_empty() {
            request = request.header(KEY_HEADER, &self.link.key);
        }
        if !token.is_empty() {
            request = request.header("x-komodoc-chat-token", token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("chat request failed: {err}"))?;
        if !response.status().is_success() {
            return Err(http_error(response).await);
        }
        response
            .json()
            .await
            .map_err(|err| format!("invalid chat response: {err}"))
    }

    pub async fn chat_create(&self) -> Result<Value, String> {
        self.chat_request(reqwest::Method::POST, "", "", Some(json!({})))
            .await
    }

    fn chat_socket_request(
        &self,
        conversation: &str,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
        let base = self.link.server().trim_end_matches('/');
        let base = base
            .strip_prefix("https://")
            .map(|rest| format!("wss://{rest}"))
            .or_else(|| {
                base.strip_prefix("http://")
                    .map(|rest| format!("ws://{rest}"))
            })
            .unwrap_or_else(|| format!("ws://{base}"));
        let url = format!(
            "{base}/api/documents/{}/chat/{conversation}/socket",
            self.link.slug()
        );
        let mut request = url.into_client_request().map_err(|err| err.to_string())?;
        if !self.token.is_empty() {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token)
                    .parse()
                    .map_err(|_| "invalid credential header")?,
            );
        }
        if !self.link.key.is_empty() {
            request.headers_mut().insert(
                KEY_HEADER,
                self.link
                    .key
                    .parse()
                    .map_err(|_| "invalid credential header")?,
            );
        }
        request
            .headers_mut()
            .insert("x-komodoc-automation", "1".parse().unwrap());
        request
            .headers_mut()
            .insert("x-komodoc-client", "1".parse().unwrap());
        Ok(request)
    }

    pub async fn chat_post(
        &self,
        conversation: &str,
        token: &str,
        text: &str,
        request_id: &str,
    ) -> Result<Value, String> {
        self.chat_post_with_results(conversation, token, text, request_id, None)
            .await
    }

    pub async fn chat_post_with_results(
        &self,
        conversation: &str,
        token: &str,
        text: &str,
        request_id: &str,
        results: Option<Value>,
    ) -> Result<Value, String> {
        let payload = chat_post_payload(request_id, text, results);
        self.chat_request(
            reqwest::Method::POST,
            &format!("/{conversation}"),
            token,
            Some(payload),
        )
        .await
        .map_err(|_| "timed out posting chat reply".to_string())
    }

    pub async fn chat_watch(
        &self,
        conversation: &str,
        token: &str,
        timeout: Duration,
        after: Option<u64>,
    ) -> Result<Value, String> {
        use futures_util::{SinkExt, StreamExt};
        validate_conversation(conversation, token)?;
        let operation = async {
            let (mut socket, _) =
                tokio_tungstenite::connect_async(self.chat_socket_request(conversation)?)
                    .await
                    .map_err(|err| format!("could not join chat: {err}"))?;
            socket
                .send(Message::Text(
                    json!({"type":"join","token":token,"role":"agent","after":after})
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|err| err.to_string())?;
            while let Some(frame) = socket.next().await {
                let frame = frame.map_err(|err| err.to_string())?;
                let raw = match frame {
                    Message::Text(raw) => raw,
                    Message::Close(reason) => {
                        let reason = reason
                            .as_ref()
                            .map(|close| close.reason.to_string())
                            .filter(|reason| !reason.is_empty())
                            .unwrap_or_else(|| "chat session closed".into());
                        return Err(reason);
                    }
                    _ => continue,
                };
                let event: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
                if event["type"] == "error" {
                    return Err(event["message"].as_str().unwrap_or("chat rejected").into());
                }
                if event["type"] == "message" && event["message"]["role"] == "user" {
                    let mut result =
                        json!({"messages":[event["message"].clone()],"timed_out":false});
                    if let Some(cursor) = event["message"]["cursor"].as_u64() {
                        result["next_cursor"] = json!(cursor);
                    }
                    return Ok(result);
                }
            }
            Err("chat closed while waiting for a message".into())
        };
        match tokio::time::timeout(timeout.max(Duration::from_secs(1)), operation).await {
            Ok(result) => result,
            Err(_) => Ok(json!({"messages":[],"timed_out":true})),
        }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response, String> {
        let mut request = self.client.request(method, endpoint);
        if !self.token.is_empty() {
            request = request.header("authorization", format!("Bearer {}", self.token));
        }
        if !self.link.key.is_empty() {
            request = request.header(KEY_HEADER, &self.link.key);
        }
        request = request.header("x-komodoc-automation", "1");
        request = request.header("x-komodoc-client", "1");
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .map_err(|err| format!("request failed: {err}"))
    }

    async fn asset_bytes(&self, sha: &str) -> Result<Vec<u8>, String> {
        if sha.len() != 64
            || !sha
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(format!("invalid asset digest {sha:?}"));
        }
        let endpoint = format!(
            "{}/api/documents/{}/assets/{sha}",
            self.link.server(),
            self.link.slug()
        );
        let response = self.request(reqwest::Method::GET, &endpoint, None).await?;
        if !response.status().is_success() {
            return Err(http_error(response).await);
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|err| format!("could not read asset {sha}: {err}"))
    }

    async fn fetch_state(&self, reference: &str) -> Result<Vec<u8>, String> {
        let target = state_reference(self.link.server(), reference)?;
        let response = self.request(reqwest::Method::GET, &target, None).await?;
        if !response.status().is_success() {
            return Err(http_error(response).await);
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|err| format!("could not fetch document state: {err}"))
    }
}

pub(crate) fn chat_post_payload(request_id: &str, text: &str, results: Option<Value>) -> Value {
    let mut payload = json!({"id":request_id,"role":"agent","text":text});
    if let Some(results) = results {
        payload["context"] = json!({"results": results});
    }
    payload
}

fn state_reference(server: &str, reference: &str) -> Result<String, String> {
    let origin = url::Url::parse(server).map_err(|_| "invalid document server URL")?;
    let target = origin
        .join(reference)
        .map_err(|_| "invalid document state reference")?;
    if target.origin() != origin.origin()
        || !target.username().is_empty()
        || target.password().is_some()
    {
        return Err("refused to send document credentials to another origin".into());
    }
    Ok(target.to_string())
}

fn new_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| format!("could not create HTTP client: {err}"))
}

async fn http_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let value = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({"error": body}));
    format!(
        "document request failed ({}): {}",
        status.as_u16(),
        detail_of(&value)
    )
}

pub fn source_sha(source: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(source.as_bytes());
    hex::encode(digest.finalize())
}

fn snapshot_text_sha(texts: &Value, path: &str) -> Option<String> {
    texts.get(path).and_then(Value::as_str).map(source_sha)
}

fn unknown_edit_result(
    request_id: &str,
    expected_sha: &str,
    body: &str,
    reason: &str,
) -> OperationResult {
    OperationResult {
        request_id: request_id.to_string(),
        status: 504,
        outcome: "unknown".into(),
        value: json!({
            "reason": reason,
            "expected_sha": expected_sha,
            "submitted_sha": source_sha(body),
            "retry": "inspect the snapshot before deciding whether to retry",
        }),
    }
}

fn payload_set_submission_id(payload: &mut Value, id: String) {
    payload["temp_id"] = Value::String(id);
}

fn stable_submission_id(kind: &str, request_id: &str) -> String {
    let digest = Sha256::digest(format!("komodoc-room-v1\0{kind}\0{request_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let encoded = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &encoded[0..8],
        &encoded[8..12],
        &encoded[12..16],
        &encoded[16..20],
        &encoded[20..]
    )
}

/// Standard Yjs awareness update containing a single provider-neutral user
/// state. The browser consumes this using y-protocols' awareness decoder.
pub fn awareness_update(client_id: u32, clock: u32, name: &str, color: &str) -> Vec<u8> {
    let state =
        serde_json::to_vec(&json!({"user":{"name":name,"color":color}})).unwrap_or_default();
    let mut out = Vec::new();
    put_varuint(&mut out, 1);
    put_varuint(&mut out, client_id);
    put_varuint(&mut out, clock);
    put_varuint(&mut out, state.len() as u32);
    out.extend_from_slice(&state);
    out
}

/// Derive the awareness client id from a Yrs replica id. It stays stable over
/// reconnects while the peer owns its document, so reconnects update one
/// presence entry instead of leaving ghosts behind.
pub fn awareness_client_id(doc: &yrs::Doc) -> u32 {
    let digest = Sha256::digest(doc.client_id().to_string().as_bytes());
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// Provider-neutral automation commands. Every command accepts the pasted
/// document link as its first argument and prints one JSON object, making the
/// CLI suitable for an agent process without a shell parser.
#[derive(Subcommand, Clone, Debug)]
pub enum AgentCommand {
    /// Read and reply to a private sidebar conversation.
    Chat {
        #[command(subcommand)]
        command: ChatCommand,
    },
    /// Print the source and annotations visible through this link.
    Read { link: String },
    /// Print only source from the authenticated snapshot.
    Source { link: String },
    /// Print only annotations from the authenticated snapshot.
    Comments { link: String },
    /// Print effective capabilities before attempting a write.
    Capabilities { link: String },
    /// Compile the live source locally and print diagnostics as JSON.
    Diagnostics { link: String },
    /// Add a source annotation.
    Comment {
        link: String,
        #[arg(long)]
        body: String,
        #[arg(long, default_value = "")]
        exact: String,
        #[arg(long, default_value = "")]
        prefix: String,
        #[arg(long, default_value = "")]
        suffix: String,
        #[arg(long)]
        position: Option<i64>,
        #[arg(long, default_value = "")]
        path: String,
        /// Revision on which this source anchor was captured.
        #[arg(long)]
        revision: Option<String>,
        #[arg(long, value_name = "ID")]
        request_id: Option<String>,
    },
    /// Reply to an annotation.
    Reply {
        link: String,
        comment_id: String,
        #[arg(long)]
        body: String,
        #[arg(long, value_name = "ID")]
        request_id: Option<String>,
    },
    /// Resolve or reopen an annotation.
    Resolve {
        link: String,
        comment_id: String,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        resolved: bool,
        #[arg(long, value_name = "ID")]
        request_id: Option<String>,
    },
    /// Delete an annotation under the link's deletion policy.
    Delete {
        link: String,
        comment_id: String,
        #[arg(long, value_name = "ID")]
        request_id: Option<String>,
    },
    /// Replace the main source after checking its source SHA.
    Edit {
        link: String,
        #[arg(long, conflicts_with = "source")]
        file: Option<std::path::PathBuf>,
        #[arg(long, conflicts_with = "file")]
        source: Option<String>,
        /// Remote path inside a directory document; defaults to its main file.
        #[arg(long, default_value = "")]
        path: String,
        /// SHA from the snapshot being edited; required for stale-input safety.
        #[arg(long)]
        expected_sha: String,
    },
    /// Request a durable checkpoint.
    Checkpoint {
        link: String,
        #[arg(long, default_value = "agent")]
        why: String,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum ChatCommand {
    /// Create a private conversation; keep its returned token private.
    Create { link: String },
    /// Stay connected until the next live user message arrives.
    Watch {
        link: String,
        #[arg(long)]
        conversation: String,
        /// Conversation credential; defaults to KOMODOC_CHAT_TOKEN.
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        after: Option<u64>,
        #[arg(long, default_value_t = 25, value_parser = clap::value_parser!(u64).range(0..=300))]
        timeout: u64,
    },
    /// Post an agent reply to a private conversation.
    Post {
        link: String,
        #[arg(long)]
        conversation: String,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        message: String,
        #[arg(long)]
        request_id: Option<String>,
        /// JSON result identifiers to relay under context.results.
        #[arg(long, value_name = "JSON")]
        results: Option<String>,
    },
}

fn validate_conversation(conversation: &str, token: &str) -> Result<(), String> {
    if conversation.is_empty()
        || conversation.len() > 128
        || !conversation
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("invalid conversation identifier".into());
    }
    if token.is_empty() {
        return Err("provide --token or KOMODOC_CHAT_TOKEN for this conversation".into());
    }
    Ok(())
}

fn chat_token(token: Option<String>) -> Result<String, String> {
    let token = token
        .or_else(|| std::env::var("KOMODOC_CHAT_TOKEN").ok())
        .unwrap_or_default();
    if token.is_empty() {
        return Err("provide --token or KOMODOC_CHAT_TOKEN for this conversation".into());
    }
    Ok(token)
}

pub async fn run_cli(command: AgentCommand) -> Result<(), String> {
    let link_text = match &command {
        AgentCommand::Chat { command } => match command {
            ChatCommand::Create { link }
            | ChatCommand::Watch { link, .. }
            | ChatCommand::Post { link, .. } => link,
        },
        AgentCommand::Read { link }
        | AgentCommand::Source { link }
        | AgentCommand::Comments { link }
        | AgentCommand::Capabilities { link }
        | AgentCommand::Diagnostics { link }
        | AgentCommand::Comment { link, .. }
        | AgentCommand::Reply { link, .. }
        | AgentCommand::Resolve { link, .. }
        | AgentCommand::Delete { link, .. }
        | AgentCommand::Edit { link, .. }
        | AgentCommand::Checkpoint { link, .. } => link,
    };
    let link = DocumentLink::parse(link_text, "")?;
    let peer = AutomationPeer::open(link).await?;
    match command {
        AgentCommand::Chat { command } => {
            let value = match command {
                ChatCommand::Create { .. } => peer.chat_create().await?,
                ChatCommand::Watch {
                    conversation,
                    token,
                    after,
                    timeout,
                    ..
                } => {
                    peer.chat_watch(
                        &conversation,
                        &chat_token(token)?,
                        Duration::from_secs(timeout),
                        after,
                    )
                    .await?
                }
                ChatCommand::Post {
                    conversation,
                    token,
                    message,
                    request_id,
                    results,
                    ..
                } => {
                    let results = results
                        .map(|raw| {
                            serde_json::from_str(&raw)
                                .map_err(|err| format!("invalid --results JSON: {err}"))
                        })
                        .transpose()?;
                    peer.chat_post_with_results(
                        &conversation,
                        &chat_token(token)?,
                        &message,
                        &request_id.unwrap_or_else(random_request_id),
                        results,
                    )
                    .await?
                }
            };
            println!(
                "{}",
                serde_json::to_string(&value).map_err(|err| err.to_string())?
            );
        }
        AgentCommand::Read { .. } => {
            let mut snapshot = peer.snapshot().await?;
            let path = peer.link.path();
            if !path.is_empty() {
                snapshot.source = snapshot
                    .texts
                    .get(path)
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("remote file {path:?} does not exist"))?
                    .to_string();
                snapshot.source_sha = source_sha(&snapshot.source);
            }
            let mut value = serde_json::to_value(snapshot).map_err(|err| err.to_string())?;
            if !path.is_empty() {
                value["path"] = json!(path);
            }
            println!(
                "{}",
                serde_json::to_string(&value).map_err(|err| err.to_string())?
            );
        }
        AgentCommand::Source { .. } => println!(
            "{}",
            serde_json::to_string(&peer.source().await?).map_err(|err| err.to_string())?
        ),
        AgentCommand::Comments { .. } => println!(
            "{}",
            serde_json::to_string(&peer.snapshot().await?.comments)
                .map_err(|err| err.to_string())?
        ),
        AgentCommand::Capabilities { .. } => println!(
            "{}",
            serde_json::to_string(peer.capabilities()).map_err(|err| err.to_string())?
        ),
        AgentCommand::Diagnostics { .. } => {
            let snapshot = peer.snapshot().await?;
            println!("{}", diagnostics_json_for_peer(&peer, &snapshot).await?);
        }
        AgentCommand::Comment {
            body,
            exact,
            prefix,
            suffix,
            position,
            path,
            revision,
            request_id,
            ..
        } => {
            let id = request_id.unwrap_or_else(random_request_id);
            let path = if path.is_empty() {
                peer.link.path().to_string()
            } else {
                path
            };
            validate_file_path(&path)?;
            let source = (!path.is_empty()).then(|| {
                json!({
                    "path": path,
                    "exact": exact,
                    "prefix": prefix,
                    "suffix": suffix,
                    "position": position,
                })
            });
            print_result(
                peer.comment_with_source(&body, &exact, source, revision.as_deref(), &id)
                    .await?,
            )?;
        }
        AgentCommand::Reply {
            comment_id,
            body,
            request_id,
            ..
        } => {
            let id = request_id.unwrap_or_else(random_request_id);
            print_result(peer.reply(&comment_id, &body, &id).await?)?;
        }
        AgentCommand::Resolve {
            comment_id,
            resolved,
            request_id,
            ..
        } => {
            let id = request_id.unwrap_or_else(random_request_id);
            print_result(peer.resolve(&comment_id, resolved, &id).await?)?;
        }
        AgentCommand::Delete {
            comment_id,
            request_id,
            ..
        } => {
            let id = request_id.unwrap_or_else(random_request_id);
            print_result(peer.delete(&comment_id, &id).await?)?;
        }
        AgentCommand::Edit {
            source,
            file,
            path,
            expected_sha,
            ..
        } => {
            let body = match (source, file) {
                (Some(source), None) => source,
                (None, Some(file)) => std::fs::read_to_string(&file)
                    .map_err(|err| format!("could not read {}: {err}", file.display()))?,
                _ => return Err("provide exactly one of --source or --file".into()),
            };
            let path = if path.is_empty() {
                peer.link.path().to_string()
            } else {
                path
            };
            print_result(peer.edit_source_at(&body, &path, &expected_sha).await?)?;
        }
        AgentCommand::Checkpoint { why, .. } => print_result(peer.checkpoint(&why).await?)?,
    }
    Ok(())
}

fn print_result(result: OperationResult) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(&result).unwrap_or_else(|_| "{\"outcome\":\"error\"}".into())
    );
    if result.status >= 400 || matches!(result.outcome.as_str(), "error" | "stale-input") {
        return Err(format!("automation operation failed ({})", result.status));
    }
    Ok(())
}

/// Compile a snapshot with the same native engine used by publishing. The
/// command intentionally returns a JSON list so an agent can pass each entry
/// directly into an explain task without parsing human-oriented compiler
/// output.
#[allow(dead_code)]
pub(crate) fn diagnostics_json(snapshot: &Snapshot) -> Result<String, String> {
    diagnostics_json_with_files(snapshot, &[])
}

async fn diagnostics_json_for_peer(
    peer: &AutomationPeer,
    snapshot: &Snapshot,
) -> Result<String, String> {
    // A `?file=` link selects a directory document's file. Keep that choice
    // attached to the same snapshot while compiling, rather than silently
    // switching back to the tree's main file.
    let selected = snapshot_for_path(snapshot, peer.link.path())?;
    let mut assets = Vec::new();
    if let Some(files) = selected.files.as_object() {
        for (path, entry) in files {
            if entry.get("kind").and_then(Value::as_str) != Some("asset") {
                continue;
            }
            let Some(sha) = entry.get("sha").and_then(Value::as_str) else {
                continue;
            };
            assets.push((path.clone(), peer.asset_bytes(sha).await?));
        }
    }
    diagnostics_json_with_files(&selected, &assets)
}

pub(crate) fn snapshot_for_path(snapshot: &Snapshot, path: &str) -> Result<Snapshot, String> {
    if path.is_empty() {
        return Ok(snapshot.clone());
    }
    let source = snapshot
        .texts
        .get(path)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("remote file {path:?} does not exist"))?;
    let mut selected = snapshot.clone();
    selected.main = path.to_string();
    selected.source = source.to_string();
    selected.source_sha = source_sha(source);
    // Directory snapshots report the main file's format. A file query can
    // select another supported format, so resolve the renderer from its own
    // extension just as the reader does for the document tree.
    if let Some(format) = crate::document::render::document_format(path) {
        selected.format = format.to_string();
    }
    Ok(selected)
}

fn diagnostics_json_with_files(
    snapshot: &Snapshot,
    asset_files: &[(String, Vec<u8>)],
) -> Result<String, String> {
    let path = if snapshot.main.is_empty() {
        "main".to_string()
    } else {
        snapshot.main.clone()
    };
    let source = if snapshot.source.is_empty() {
        snapshot
            .texts
            .get(&path)
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        snapshot.source.as_str()
    };
    let format = snapshot.format.to_ascii_lowercase();
    let texts = snapshot
        .texts
        .as_object()
        .into_iter()
        .flat_map(|texts| texts.iter())
        .filter_map(|(path, value)| value.as_str().map(|text| (path.clone(), text.to_owned())))
        .collect();
    let mut compiled = match format.as_str() {
        "markdown" | "md" => komodoc_engine::citations::compile(
            &path,
            source,
            &snapshot.title,
            &texts,
            &komodoc_engine::markdown::no_assets,
        ),
        "html" | "htm" => komodoc_engine::html::compile(source, &snapshot.title),
        "typst" | "typ" => {
            let files: Vec<(String, Vec<u8>)> = snapshot
                .texts
                .as_object()
                .into_iter()
                .flat_map(|texts| texts.iter())
                .map(|(name, text)| {
                    (
                        name.clone(),
                        text.as_str().unwrap_or_default().as_bytes().to_vec(),
                    )
                })
                .collect();
            let mut files = files;
            files.extend(asset_files.iter().cloned());
            let (compiled, _) = crate::document::render::read_and_note_from_files(
                &path,
                source,
                &snapshot.title,
                &files,
            );
            compiled
        }
        other => {
            return Err(format!(
            "unsupported document format {other:?}; diagnostics supports markdown, typst, and html"
            ))
        }
    };
    if !matches!(format.as_str(), "markdown" | "md") {
        compiled
            .diagnostics
            .extend(komodoc_engine::bib::library(&path, &format, source, &texts).diagnostics);
    }
    let mut output = Vec::with_capacity(compiled.diagnostics.len());
    for diagnostic in compiled.diagnostics {
        let diagnostic_path = if diagnostic.file.is_empty() {
            path.clone()
        } else {
            diagnostic.file.clone()
        };
        let diagnostic_source = if diagnostic.file.is_empty() {
            source
        } else {
            snapshot
                .texts
                .get(&diagnostic.file)
                .and_then(Value::as_str)
                .unwrap_or(source)
        };
        let source_line = diagnostic
            .line
            .checked_sub(1)
            .and_then(|line| diagnostic_source.lines().nth(line))
            .unwrap_or_default();
        output.push(json!({
            "severity": diagnostic.severity.as_str(),
            "path": diagnostic_path,
            "line": diagnostic.line,
            "column": diagnostic.column,
            "message": diagnostic.message,
            "source_line": source_line,
        }));
    }
    serde_json::to_string(&output).map_err(|err| format!("could not encode diagnostics: {err}"))
}

fn random_request_id() -> String {
    format!("agent-{}", hex::encode(crate::auth::random_bytes(12)))
}

fn put_varuint(out: &mut Vec<u8>, mut value: u32) {
    while value > 0x7f {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pasted_link_without_exposing_key() {
        let link = DocumentLink::parse("https://docs.example/docs/abc#k=secret", "").unwrap();
        assert_eq!(link.server(), "https://docs.example");
        assert_eq!(link.slug(), "abc");
        assert!(link.has_key());
        assert!(!format!("{link:?}").contains("secret"));
    }

    #[test]
    fn awareness_contains_sync_identity() {
        let bytes = awareness_update(4, 1, "agent (sync)", "#3366cc");
        assert!(bytes.len() > 10);
        assert!(String::from_utf8_lossy(&bytes).contains("agent (sync)"));
    }

    #[test]
    fn automation_peer_debug_redacts_bearer() {
        let peer = AutomationPeer {
            link: DocumentLink::parse("https://docs.example/docs/paper#k=share-key", "")
                .expect("link"),
            token: "bearer-that-must-not-leak".into(),
            capabilities: Capabilities::default(),
            client: new_client().expect("client"),
        };
        let debug = format!("{peer:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("bearer-that-must-not-leak"));
    }

    #[test]
    fn unknown_edit_result_contains_recovery_identifiers() {
        let result = unknown_edit_result("edit-1", "expected", "changed", "socket closed");
        assert_eq!(result.status, 504);
        assert_eq!(result.outcome, "unknown");
        assert_eq!(result.request_id, "edit-1");
        assert_eq!(result.value["expected_sha"], "expected");
        assert_eq!(result.value["submitted_sha"], source_sha("changed"));
        assert_eq!(result.value["reason"], "socket closed");
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    #[test]
    fn references_never_send_credentials_off_origin() {
        for reference in [
            "https://evil.test/state",
            "//evil.test/state",
            "https://user@komodoc.test/state",
        ] {
            assert!(state_reference("https://komodoc.test", reference).is_err());
        }
        for reference in [".evil.test/state", "/api/state", "state"] {
            let target = state_reference("https://komodoc.test", reference).unwrap();
            assert_eq!(
                url::Url::parse(&target).unwrap().host_str(),
                Some("komodoc.test")
            );
        }
    }
}
