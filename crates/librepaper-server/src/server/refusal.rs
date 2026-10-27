//! The three ways a [`Refusal`] reaches a client: an HTTP reply, a room
//! socket error frame and an MCP failure. Each is one function over the same
//! value, so a condition has the same status, retry advice and reason on
//! every transport and a route cannot invent its own table.

use super::*;
use librepaper_room::room::refusal::{Committed, Kind, Refusal};

/// An HTTP answer for a refusal.
pub(crate) fn http(refusal: &Refusal) -> Reply {
    http_with(refusal, &[])
}

/// The same, keeping whatever correlation fields the route's clients read: a
/// request id, a temporary id, the SHA a rendering was for, the paths that
/// moved. Those are the route's own wire contract; the refusal decides the
/// status, the message, the reason and the retry advice.
pub(crate) fn http_with(refusal: &Refusal, fields: &[(&str, Value)]) -> Reply {
    let mut payload = json!({
        "error": refusal.message,
        "reason": refusal.reason(),
        "retryable": refusal.retryable(),
    });
    for (name, value) in fields {
        payload[*name] = value.clone();
    }
    write_json(refusal.status(), &payload)
}

/// An HTTP answer for a catalogue failure. A database or ownership failure
/// carries context that goes to the log under `what` and never to the client,
/// who is told `internal`. `not_found` is the route's own wording for a name
/// that matches nothing.
pub(crate) fn catalog(
    error: librepaper_engine::storage::postgres::Error,
    what: &str,
    not_found: &str,
    internal: &str,
) -> Reply {
    use librepaper_engine::storage::postgres::Error;
    if matches!(error, Error::Ownership(_) | Error::Database(_)) {
        tracing::warn!("{what}: {error:?}");
    }
    http(&Refusal::from_catalog(error, not_found, internal))
}

/// The ids a client clears pending state by. An id that is not set is left
/// out of the frame, and an empty one counts as not set.
#[derive(Clone, Debug, Default)]
pub(crate) struct FrameIds {
    temp_id: Option<String>,
    request_id: Option<String>,
}

impl FrameIds {
    /// The ids of the message being answered.
    pub(crate) fn of(incoming: &RoomMessage) -> Self {
        Self::new(incoming.temp_id(), incoming.request_id())
    }

    pub(crate) fn new(temp_id: &str, request_id: &str) -> Self {
        Self::default().temp(temp_id).request(request_id)
    }

    pub(crate) fn temp(mut self, temp_id: &str) -> Self {
        self.temp_id = (!temp_id.is_empty()).then(|| temp_id.to_owned());
        self
    }

    pub(crate) fn request(mut self, request_id: &str) -> Self {
        self.request_id = (!request_id.is_empty()).then(|| request_id.to_owned());
        self
    }
}

/// A room socket error frame for a refusal. It always carries the frame
/// version and protocol, the status its HTTP sibling would answer with, and
/// whichever ids the frame was a reply to.
pub(crate) fn socket(refusal: &Refusal, ids: FrameIds) -> Value {
    let mut frame = json!({
        "type": "error",
        "message": refusal.message,
        "status": refusal.status(),
        "version": 1,
        "protocol": super::socket::PROTOCOL,
    });
    if refusal.retryable() {
        frame["retryable"] = json!(true);
    }
    if let Some(temp_id) = ids.temp_id {
        frame["temp_id"] = json!(temp_id);
    }
    if let Some(request_id) = ids.request_id {
        frame["request_id"] = json!(request_id);
    }
    frame
}

/// An MCP failure for a refusal. A storage failure whose commit state is
/// unknown is `outcome_unknown`, which tells an agent to inspect the
/// document before sending the work again.
pub(crate) fn mcp(refusal: &Refusal) -> mcp::Failure {
    mcp::Failure::of_refusal(refusal)
}

/// The MCP code a refusal is spelled with.
pub(crate) fn mcp_code(refusal: &Refusal) -> &'static str {
    match (refusal.kind, refusal.committed) {
        (Kind::NotFound, _) => "not_found",
        (Kind::Forbidden | Kind::Unauthenticated, _) => "permission_changed",
        (Kind::Conflict(_), _) => "conflict",
        (Kind::Invalid, _) => "invalid_params",
        (Kind::Quota(_) | Kind::RateLimited, _) => "budget_exceeded",
        (Kind::Unavailable, Committed::No) => "unavailable",
        (Kind::Unavailable, Committed::Unknown) => "outcome_unknown",
        (Kind::Internal, _) => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use librepaper_engine::storage::postgres::QuotaScope;
    use librepaper_room::room::refusal::Reason;
    use librepaper_room::room::WriteError;

    async fn body_of(reply: Reply) -> (u16, Value) {
        let status = reply.status().as_u16();
        let bytes = to_bytes(reply.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// The HTTP literal `write_json` produced for these refusals before the
    /// projection existed.
    #[tokio::test]
    async fn http_bodies_equal_what_the_old_tables_wrote() {
        let cases = [
            (
                Refusal::from(WriteError::NotFound),
                404,
                json!({"error": "not found", "reason": "not_found", "scope": "request", "retryable": false}),
            ),
            (
                Refusal::from(WriteError::Conflict("moved".into())),
                409,
                json!({"error": "moved", "reason": "conflict", "scope": "request", "retryable": false}),
            ),
            (
                Refusal::from(WriteError::Invalid("bad".into())),
                400,
                json!({"error": "bad", "reason": "invalid_request", "scope": "request", "retryable": false}),
            ),
            (
                Refusal::from(WriteError::Storage("db.internal".into())),
                503,
                json!({"error": "storage temporarily unavailable", "reason": "service_unavailable", "scope": "request", "retryable": true}),
            ),
            (
                Refusal::not_written(Kind::Conflict(Reason::Changed), "files changed"),
                409,
                json!({"error": "files changed", "reason": "changed", "scope": "request", "retryable": false}),
            ),
            (
                Refusal::not_written(Kind::Forbidden, "no"),
                403,
                json!({"error": "no", "reason": "permission_denied", "scope": "request", "retryable": false}),
            ),
            (
                Refusal::not_written(Kind::Unauthenticated, "sign in"),
                401,
                json!({"error": "sign in", "reason": "authentication_required", "scope": "request", "retryable": false}),
            ),
            (
                Refusal::not_written(Kind::Quota(QuotaScope::Owner), "full"),
                507,
                json!({"error": "full", "reason": "storage_budget", "scope": "request", "retryable": false}),
            ),
            (
                Refusal::not_written(Kind::RateLimited, "slow down"),
                429,
                json!({"error": "slow down", "reason": "request_budget", "scope": "request", "retryable": true}),
            ),
            (
                Refusal::new(Kind::Internal, Committed::Unknown, "could not save that"),
                500,
                json!({"error": "could not save that", "reason": "service_unavailable", "scope": "request", "retryable": true}),
            ),
        ];
        for (refusal, status, expected) in cases {
            let (got_status, got) = body_of(http(&refusal)).await;
            assert_eq!(got_status, status, "{refusal:?}");
            assert_eq!(got, expected, "{refusal:?}");
        }
    }

    #[test]
    fn socket_frames_carry_version_protocol_and_the_ids_in_scope() {
        let frame = socket(
            &Refusal::from(WriteError::Storage("db.internal".into())),
            FrameIds::new("t1", "r1"),
        );
        assert_eq!(
            frame,
            json!({
                "type": "error",
                "message": "storage temporarily unavailable",
                "status": 503,
                "version": 1,
                "protocol": "librepaper.room.v3",
                "retryable": true,
                "temp_id": "t1",
                "request_id": "r1",
            })
        );
        let frame = socket(
            &Refusal::not_written(Kind::Forbidden, "editing is not permitted"),
            FrameIds::default().request("r1"),
        );
        assert_eq!(
            frame,
            json!({
                "type": "error",
                "message": "editing is not permitted",
                "status": 403,
                "version": 1,
                "protocol": "librepaper.room.v3",
                "request_id": "r1",
            })
        );
        let frame = socket(
            &Refusal::not_written(
                Kind::RateLimited,
                "too many chat messages; try again shortly",
            ),
            FrameIds::new("t1", ""),
        );
        assert_eq!(frame["status"], json!(429));
        assert_eq!(frame["retryable"], json!(true));
        assert_eq!(frame["temp_id"], json!("t1"));
        assert!(frame.get("request_id").is_none());
    }

    #[test]
    fn every_kind_has_a_socket_frame_with_its_status() {
        let kinds = [
            (Kind::NotFound, 404, false),
            (Kind::Forbidden, 403, false),
            (Kind::Unauthenticated, 401, false),
            (Kind::Conflict(Reason::Conflict), 409, false),
            (Kind::Invalid, 400, false),
            (Kind::Quota(QuotaScope::Owner), 507, false),
            (Kind::RateLimited, 429, true),
            (Kind::Unavailable, 503, true),
            (Kind::Internal, 500, true),
        ];
        for (kind, status, retryable) in kinds {
            let frame = socket(&Refusal::not_written(kind, "m"), FrameIds::default());
            let mut expected = json!({
                "type": "error",
                "message": "m",
                "status": status,
                "version": 1,
                "protocol": "librepaper.room.v3",
            });
            if retryable {
                expected["retryable"] = json!(true);
            }
            assert_eq!(frame, expected, "{kind:?}");
        }
    }

    #[test]
    fn mcp_failures_keep_the_codes_agents_match_on() {
        let cases = [
            (Refusal::from(WriteError::NotFound), "not_found"),
            (Refusal::from(WriteError::Conflict("x".into())), "conflict"),
            (
                Refusal::from(WriteError::Invalid("x".into())),
                "invalid_params",
            ),
            (Refusal::from(WriteError::Busy), "unavailable"),
            (
                Refusal::from(WriteError::Storage("x".into())),
                "outcome_unknown",
            ),
            (
                Refusal::not_written(Kind::Forbidden, "x"),
                "permission_changed",
            ),
            (
                Refusal::not_written(Kind::Quota(QuotaScope::Deployment), "x"),
                "budget_exceeded",
            ),
            (
                Refusal::not_written(Kind::RateLimited, "x"),
                "budget_exceeded",
            ),
            (
                Refusal::new(Kind::Internal, Committed::Unknown, "x"),
                "internal",
            ),
        ];
        for (refusal, code) in cases {
            let failure = mcp(&refusal);
            assert_eq!(failure.code, code, "{refusal:?}");
            assert_eq!(failure.message, refusal.message, "{refusal:?}");
            assert_eq!(failure.committed, refusal.committed, "{refusal:?}");
        }
    }
}
