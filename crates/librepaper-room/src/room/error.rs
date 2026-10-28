//! Why a room write did not happen, as a value rather than as prose.
//!
//! Every mutator that can refuse answers with a [`WriteError`]. A caller
//! decides what to do -- stop a bundle, close a socket, pick a status
//! code -- by matching on the variant, never by
//! reading the message. That is the whole point of the type: the wording of a
//! refusal is for the person reading it, so rewording one must not silently
//! move a route from 507 to 413 or turn a permanent refusal into a retry.

use librepaper_engine::storage::postgres::{DuplicateOf, Error as CatalogError, QuotaScope};

/// Whether the same write is worth sending again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Retry {
    /// Nothing about waiting helps.
    No,
    /// The refusal is about this moment, not about the work.
    Later,
}

/// Why a room write did not happen.
#[derive(Clone, Debug)]
pub enum WriteError {
    /// The document's projection breached a resource bound, so nothing that
    /// needs to know what it says can be answered (§9.3). Ingest and flush
    /// continue: an editor can still type and export locally. The string is
    /// the reason, which is shown, because "unreadable" on its own tells
    /// somebody nothing about whether to wait or to trim the document.
    Unreadable(String),
    /// This process lost the writer fence. Reconnecting reaches whoever
    /// holds it now (§10).
    Fenced(String),
    /// The memory budget had no room to read this document just now
    /// (§9.2). Purely about this moment.
    Busy,
    /// The document, checkpoint or comment named is not there.
    NotFound,
    /// The input describes a state the document has moved on from.
    Conflict(String),
    /// The input is not something this document could ever take.
    Invalid(String),
    /// A storage limit was reached. The scope says whose, so the answer is
    /// the same 507 the catalogue gives.
    Quota(QuotaScope),
    /// The account is writing faster than it is allowed to.
    RateLimited,
    /// The write would collide with something that already has that name.
    Duplicate(DuplicateOf),
    /// Storage failed. The string is context for the log; clients are told
    /// only that storage is unavailable.
    Storage(String),
}

impl WriteError {
    /// A refusal about this moment rather than about the work.
    pub fn is_temporary(&self) -> bool {
        self.retry() == Retry::Later
    }

    /// Whether the same write is worth sending again.
    pub fn retry(&self) -> Retry {
        match self {
            // Unreadable clears on a successful build, which an operator
            // can trigger after raising a limit and which happens on its own
            // after an editor trims the document (§9.3). So it is worth
            // trying again, unlike a refusal about the work itself.
            Self::Unreadable(_) | Self::Fenced(_) | Self::Busy => Retry::Later,
            Self::NotFound | Self::Invalid(_) | Self::Quota(_) | Self::Duplicate(_) => Retry::No,
            // The account's budget refills with time.
            Self::RateLimited => Retry::Later,
            // A stale input has to be rebuilt against the current document
            // before it can be sent again, so the same bytes never help.
            Self::Conflict(_) => Retry::No,
            Self::Storage(_) => Retry::Later,
        }
    }

    /// What a client is told. Never the storage context: an object key or a
    /// bucket error is for the log.
    pub fn client_message(&self) -> String {
        match self {
            Self::Unreadable(why) => why.clone(),
            Self::Fenced(_) => {
                // Kept verbatim: browsers show this on the socket close frame.
                "this room is being written by another server; reconnect to continue editing".into()
            }
            Self::Busy => "this deployment is busy; try again in a moment".into(),
            Self::NotFound => "not found".into(),
            Self::Conflict(why) | Self::Invalid(why) => why.clone(),
            Self::Quota(scope) => scope.to_string(),
            Self::RateLimited => CatalogError::RateLimited.to_string(),
            Self::Duplicate(of) => of.to_string(),
            Self::Storage(_) => "storage temporarily unavailable".into(),
        }
    }

    /// The detail worth logging, which is exactly what is kept from a client.
    pub fn log_context(&self) -> Option<&str> {
        match self {
            Self::Storage(context) | Self::Fenced(context) => Some(context.as_str()),
            _ => None,
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The log wants the cause; every other reader wants the message.
            Self::Storage(context) => write!(formatter, "storage failure: {context}"),
            other => formatter.write_str(&other.client_message()),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<CatalogError> for WriteError {
    fn from(error: CatalogError) -> Self {
        match error {
            CatalogError::NotFound => Self::NotFound,
            CatalogError::Invalid(why) => Self::Invalid(why),
            CatalogError::Conflict(why) => Self::Conflict(why),
            CatalogError::Quota(scope) => Self::Quota(scope),
            CatalogError::RateLimited => Self::RateLimited,
            CatalogError::Duplicate(of) => Self::Duplicate(of),
            CatalogError::Ownership(why) => Self::Storage(why),
            CatalogError::Database(err) => Self::Storage(err.to_string()),
        }
    }
}

impl From<librepaper_engine::log::SequencerError> for WriteError {
    /// The sequencer's refusals and the room's are the same refusals seen
    /// from two places, so the mapping is total and there is no catch-all:
    /// a variant added to one has to be answered for in the other.
    fn from(error: librepaper_engine::log::SequencerError) -> Self {
        use librepaper_engine::log::SequencerError;
        match error {
            SequencerError::Unreadable(why) => Self::Unreadable(why),
            SequencerError::Busy => Self::Busy,
            SequencerError::Fenced(why) => Self::Fenced(why),
            SequencerError::Trimmed => Self::Unreadable(SequencerError::Trimmed.to_string()),
            SequencerError::Storage(error) => Self::from(error),
            SequencerError::Loro(why) => Self::Storage(why),
        }
    }
}

impl From<librepaper_engine::log::CommandError> for WriteError {
    fn from(error: librepaper_engine::log::CommandError) -> Self {
        use librepaper_engine::log::CommandError;
        match error {
            CommandError::Conflict(why) => Self::Conflict(why),
            // A stale rendered selection is a conflict with a fact the
            // client needs: the digest it should re-render against. The
            // caller that has somewhere to put that digest should match on
            // `CommandError` before it reaches here.
            CommandError::StaleSelection { digest } => Self::Conflict(format!(
                "the document moved under that selection; it now reads {digest}"
            )),
            CommandError::Storage(error) => Self::from(error),
            CommandError::Sequencer(error) => Self::from(error),
        }
    }
}
