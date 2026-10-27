//! One refusal, in one shape, before any transport spells it.
//!
//! A room write, an agent change and a catalogue write each fail in their own
//! enum. The server answers over HTTP, over the room socket and over MCP, and
//! each of those used to carry its own table from failure to status, retry
//! advice and code. A [`Refusal`] is what every one of those failures becomes
//! first, so a condition has one status, one retry answer and one reason, and
//! a transport only decides how to spell them.

use librepaper_engine::storage::postgres::{Error as CatalogError, QuotaScope};

use super::error::WriteError;
use super::source_change::AgentError;

/// Why a conflict is a conflict, as the wire names it. The default is the
/// plain "the state moved" answer; the others tell a caller what to do next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    /// The input was built against a state the document has left.
    Conflict,
    /// Files named in a change moved since it was planned.
    Changed,
    /// A pending suggestion sits in the way of the change.
    Suggestion,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::Changed => "changed",
            Self::Suggestion => "suggestion",
        }
    }
}

/// What kind of refusal this is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    NotFound,
    Forbidden,
    Unauthenticated,
    Conflict(Reason),
    Invalid,
    Quota(QuotaScope),
    RateLimited,
    /// A refusal about this moment: storage, a busy budget, a lost fence.
    Unavailable,
    /// A failure with no more specific answer.
    Internal,
}

/// Whether the refused work may have been written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Committed {
    /// Nothing was written. Sending the same work again is safe.
    No,
    /// The write may or may not have landed.
    Unknown,
}

/// A refused request, before a transport spells it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    pub kind: Kind,
    pub committed: Committed,
    /// What a client may read. Never storage context.
    pub message: String,
}

impl Refusal {
    pub fn new(kind: Kind, committed: Committed, message: impl Into<String>) -> Self {
        Self {
            kind,
            committed,
            message: message.into(),
        }
    }

    /// A refusal that is known to have written nothing.
    pub fn not_written(kind: Kind, message: impl Into<String>) -> Self {
        Self::new(kind, Committed::No, message)
    }

    /// The HTTP status of this refusal.
    pub fn status(&self) -> u16 {
        match self.kind {
            Kind::NotFound => 404,
            Kind::Forbidden => 403,
            Kind::Unauthenticated => 401,
            Kind::Conflict(_) => 409,
            Kind::Invalid => 400,
            Kind::Quota(_) => 507,
            Kind::RateLimited => 429,
            Kind::Unavailable => 503,
            Kind::Internal => 500,
        }
    }

    /// Whether the same request is worth sending again unchanged.
    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            Kind::RateLimited | Kind::Unavailable | Kind::Internal
        )
    }

    /// The machine-readable reason a client may key on.
    pub fn reason(&self) -> &'static str {
        match self.kind {
            Kind::NotFound => "not_found",
            Kind::Forbidden => "permission_denied",
            Kind::Unauthenticated => "authentication_required",
            Kind::Conflict(reason) => reason.as_str(),
            Kind::Invalid => "invalid_request",
            Kind::Quota(_) => "storage_budget",
            Kind::RateLimited => "request_budget",
            Kind::Unavailable | Kind::Internal => "service_unavailable",
        }
    }

    /// A catalogue failure from a route that names its own wording for the
    /// two cases the catalogue does not word for a client. The database and
    /// ownership failures carry context that belongs in the log, so they
    /// become `internal` here.
    pub fn from_catalog(error: CatalogError, not_found: &str, internal: &str) -> Self {
        match error {
            CatalogError::Invalid(why) => Self::not_written(Kind::Invalid, why),
            CatalogError::Conflict(why) => Self::not_written(Kind::Conflict(Reason::Conflict), why),
            CatalogError::Duplicate(of) => {
                Self::not_written(Kind::Conflict(Reason::Conflict), of.to_string())
            }
            CatalogError::NotFound => Self::not_written(Kind::NotFound, not_found),
            CatalogError::Quota(scope) => Self::not_written(Kind::Quota(scope), scope.to_string()),
            CatalogError::RateLimited => {
                Self::not_written(Kind::RateLimited, CatalogError::RateLimited.to_string())
            }
            CatalogError::Ownership(_) | CatalogError::Database(_) => {
                Self::new(Kind::Internal, Committed::Unknown, internal)
            }
        }
    }

    /// The same refusal with a different answer to "did it write".
    pub fn with_committed(mut self, committed: Committed) -> Self {
        self.committed = committed;
        self
    }
}

impl From<&WriteError> for Refusal {
    fn from(error: &WriteError) -> Self {
        let message = error.client_message();
        match error {
            WriteError::NotFound => Self::not_written(Kind::NotFound, message),
            WriteError::Conflict(_) => Self::not_written(Kind::Conflict(Reason::Conflict), message),
            WriteError::Invalid(_) => Self::not_written(Kind::Invalid, message),
            // Refused before anything was applied.
            WriteError::Unreadable(_) | WriteError::Busy => {
                Self::not_written(Kind::Unavailable, message)
            }
            // The fence can be lost at the commit boundary, and a storage
            // failure can come from the commit itself.
            WriteError::Fenced(_) | WriteError::Storage(_) => {
                Self::new(Kind::Unavailable, Committed::Unknown, message)
            }
        }
    }
}

impl From<WriteError> for Refusal {
    fn from(error: WriteError) -> Self {
        Self::from(&error)
    }
}

impl From<&AgentError> for Refusal {
    fn from(error: &AgentError) -> Self {
        match error {
            AgentError::Invalid(why) => Self::not_written(Kind::Invalid, why.clone()),
            AgentError::Conflict(why) => {
                Self::not_written(Kind::Conflict(Reason::Conflict), why.clone())
            }
            AgentError::Changed(_) => Self::not_written(
                Kind::Conflict(Reason::Changed),
                "files changed since the change was planned",
            ),
            AgentError::Suggestion { .. } => Self::not_written(
                Kind::Conflict(Reason::Suggestion),
                "a pending suggestion is in the way",
            ),
            AgentError::Storage(_) => Self::new(
                Kind::Unavailable,
                Committed::Unknown,
                "could not apply the change",
            ),
        }
    }
}

impl From<AgentError> for Refusal {
    fn from(error: AgentError) -> Self {
        Self::from(&error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(kind: Kind) -> Refusal {
        Refusal::not_written(kind, "m")
    }

    #[test]
    fn each_kind_has_the_status_retry_and_reason_the_old_tables_gave() {
        let table = [
            (Kind::NotFound, 404, false, "not_found"),
            (Kind::Forbidden, 403, false, "permission_denied"),
            (Kind::Unauthenticated, 401, false, "authentication_required"),
            (Kind::Conflict(Reason::Conflict), 409, false, "conflict"),
            (Kind::Conflict(Reason::Changed), 409, false, "changed"),
            (Kind::Conflict(Reason::Suggestion), 409, false, "suggestion"),
            (Kind::Invalid, 400, false, "invalid_request"),
            (Kind::Quota(QuotaScope::Owner), 507, false, "storage_budget"),
            (
                Kind::Quota(QuotaScope::Deployment),
                507,
                false,
                "storage_budget",
            ),
            (Kind::RateLimited, 429, true, "request_budget"),
            (Kind::Unavailable, 503, true, "service_unavailable"),
            (Kind::Internal, 500, true, "service_unavailable"),
        ];
        for (kind, status, retryable, reason) in table {
            let refusal = refusal(kind);
            assert_eq!(refusal.status(), status, "{kind:?}");
            assert_eq!(refusal.retryable(), retryable, "{kind:?}");
            assert_eq!(refusal.reason(), reason, "{kind:?}");
        }
    }

    #[test]
    fn a_write_error_keeps_its_old_status_and_retry_advice() {
        let cases = [
            (WriteError::NotFound, 404, false, Committed::No),
            (WriteError::Conflict("x".into()), 409, false, Committed::No),
            (WriteError::Invalid("x".into()), 400, false, Committed::No),
            (WriteError::Unreadable("x".into()), 503, true, Committed::No),
            (WriteError::Busy, 503, true, Committed::No),
            (
                WriteError::Fenced("x".into()),
                503,
                true,
                Committed::Unknown,
            ),
            (
                WriteError::Storage("x".into()),
                503,
                true,
                Committed::Unknown,
            ),
        ];
        for (error, status, retryable, committed) in cases {
            let refusal = Refusal::from(&error);
            assert_eq!(refusal.status(), status, "{error:?}");
            assert_eq!(refusal.retryable(), retryable, "{error:?}");
            assert_eq!(refusal.committed, committed, "{error:?}");
            assert_eq!(refusal.message, error.client_message(), "{error:?}");
        }
    }

    #[test]
    fn an_agent_error_keeps_its_http_wording() {
        let changed = Refusal::from(AgentError::Changed(vec!["a".into()]));
        assert_eq!(changed.reason(), "changed");
        assert_eq!(
            changed.message,
            "files changed since the change was planned"
        );
        let storage = Refusal::from(AgentError::Storage("db.internal".into()));
        assert_eq!(storage.status(), 503);
        assert_eq!(storage.committed, Committed::Unknown);
        assert!(!storage.message.contains("db.internal"));
    }
}
