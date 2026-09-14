//! Why a room write did not happen, as a value rather than as prose.
//!
//! Every mutator that can refuse answers with a [`WriteError`]. A caller
//! decides what to do -- stop a publication, close a socket, pick a status
//! code -- by matching on the variant, never by
//! reading the message. That is the whole point of the type: the wording of a
//! refusal is for the person reading it, so rewording one must not silently
//! move a route from 507 to 413 or turn a permanent refusal into a retry.

use crate::config::SizeRefusal;
use crate::storage::postgres::Error as CatalogError;

/// Why this server may not write a room it is holding open.
///
/// The cached `read_only` flag says *that* a room is fenced; this says what
/// fenced it, which is what decides whether a client should try again
/// elsewhere (the lease moved), give up (the document is gone) or wait for an
/// operator (this server could not read the room's own state and refuses to
/// write over what it cannot see).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FenceReason {
    /// Another server holds the lease. Reconnecting reaches the holder.
    HeldElsewhere = 0,
    /// The document was deleted. Nothing will accept writes for it again.
    Deleted = 1,
    /// This server could not read the room's durable state, so it will not
    /// write over it. Permanent for this instance: an operator has to look.
    UnreadableState = 2,
    /// A room copy that no process currently owns,
    /// `get` path. It never becomes writable.
    NotAuthoritative = 3,
    /// The stored state is already past the encoded ceiling, so no further
    /// write of it could ever be saved. Fenced so the sweeper stops retrying
    /// a permanent failure; what is stored stays readable.
    Oversized = 4,
    /// A durable agent source operation is awaiting reconciliation. Until
    /// its marker and authority are resolved, this room must not be served.
    AgentRecoveryPending = 5,
}

impl FenceReason {
    /// The reason stored in a room's atomic companion to `read_only`.
    /// An unknown byte reads as the common case rather than panicking.
    pub(crate) fn from_stored(byte: u8) -> Self {
        match byte {
            1 => Self::Deleted,
            2 => Self::UnreadableState,
            3 => Self::NotAuthoritative,
            4 => Self::Oversized,
            5 => Self::AgentRecoveryPending,
            _ => Self::HeldElsewhere,
        }
    }

    /// Whether a client can usefully try the same write again.
    fn temporary(self) -> bool {
        matches!(self, Self::HeldElsewhere | Self::NotAuthoritative)
    }

    fn message(self) -> &'static str {
        match self {
            // Kept verbatim: browsers show this on the socket close frame.
            Self::HeldElsewhere | Self::NotAuthoritative => {
                "this room is being written by another server; reconnect to continue editing"
            }
            Self::Deleted => "this document has been deleted",
            Self::UnreadableState => {
                "this document's stored state could not be read, so it is open read-only"
            }
            Self::Oversized => {
                "this document's saved state is past the largest this deployment can save, so \
                 it is open read-only"
            }
            Self::AgentRecoveryPending => {
                "this document has a pending agent operation and is awaiting recovery"
            }
        }
    }
}

/// Which figure ceiling a figure upload ran into. Separate from
/// [`SizeRefusal`], which is about the document's own text and snapshot: the
/// wording a person reads has to name the right ceiling, and the two are
/// configured independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FigureLimit {
    /// Past `max_asset`: this one file is too large.
    OneFile { ceiling: i64 },
    /// Past `max_assets`: the document keeps as many figure bytes as it may.
    Document { ceiling: i64 },
}

impl FigureLimit {
    fn message(self) -> String {
        match self {
            Self::OneFile { ceiling } => format!(
                "that figure is larger than the {} MB one file may be",
                ceiling >> 20
            ),
            Self::Document { ceiling } => format!(
                "this document has reached the {} MB it may keep in figures",
                ceiling >> 20
            ),
        }
    }
}

/// A ceiling the shared document itself has reached, refused on the socket
/// that wrote past it. These four messages are wire text: a close frame
/// carries nothing but a reason. These strings are part of the collaboration
/// protocol and must remain stable across clients.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentLimit {
    /// Past `max_document`: the visible source.
    Size,
    /// Past `max_files`.
    Files,
    /// The owner or deployment has no bytes left for this room's edits.
    #[allow(dead_code)]
    Quota,
    /// Past `E`: the encoded snapshot, history and metadata included.
    Encoded,
}

impl DocumentLimit {
    fn message(self) -> &'static str {
        match self {
            Self::Size => "this document has reached its size limit",
            Self::Files => "this document has reached its file limit",
            Self::Quota => "this document has reached its storage quota",
            Self::Encoded => super::ENCODED_CEILING_REFUSAL,
        }
    }
}

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
    /// This server may not write this room at all.
    ReadOnly(FenceReason),
    /// The caller's rights, or the session those rights were granted in,
    /// no longer admit this write.
    /// Past a configured ceiling. No retry helps (track 10's type).
    Size(SizeRefusal),
    /// Past one of the figure ceilings.
    Figure(FigureLimit),
    /// A ceiling on the shared document itself, refused on the socket.
    Document(DocumentLimit),
    /// The peer wrote faster than a person can. Temporary by nature.
    RateLimited,
    /// This server momentarily has no capacity at all; the peer is asked to
    /// reconnect rather than told its document is too large.
    /// The document, checkpoint or comment named is not there.
    NotFound,
    /// The input describes a state the document has moved on from.
    Conflict(String),
    /// The input is not something this document could ever take.
    Invalid(String),
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
            Self::ReadOnly(reason) if reason.temporary() => Retry::Later,
            Self::ReadOnly(_) => Retry::No,
            Self::NotFound | Self::Invalid(_) => Retry::No,
            Self::Size(_) | Self::Figure(_) | Self::Document(_) => Retry::No,
            Self::RateLimited => Retry::Later,
            // A stale input has to be rebuilt against the current document
            // before it can be sent again, so the same bytes never help.
            Self::Conflict(_) => Retry::No,
            Self::Storage(_) => Retry::Later,
        }
    }

    /// The HTTP status this refusal is answered with. The one table every
    /// route reads, so a status is a property of the variant and not of
    /// whichever handler happened to receive it.
    pub fn status(&self) -> u16 {
        match self {
            Self::ReadOnly(FenceReason::Deleted) | Self::NotFound => 404,
            Self::ReadOnly(_) => 503,
            Self::Size(_) | Self::Figure(_) => 413,
            Self::Document(DocumentLimit::Quota) => 507,
            Self::Document(_) => 413,
            Self::RateLimited => 429,
            Self::Conflict(_) => 409,
            Self::Invalid(_) => 400,
            Self::Storage(_) => 503,
        }
    }

    /// What a client is told. Never the storage context: an object key or a
    /// bucket error is for the log.
    pub fn client_message(&self) -> String {
        match self {
            Self::ReadOnly(reason) => reason.message().to_string(),
            Self::Size(refusal) => refusal.message(),
            Self::Figure(limit) => limit.message(),
            Self::Document(limit) => limit.message().to_string(),
            Self::RateLimited => "too many updates".into(),
            Self::NotFound => "not found".into(),
            Self::Conflict(why) | Self::Invalid(why) => why.clone(),
            Self::Storage(_) => "storage temporarily unavailable".into(),
        }
    }

    /// The detail worth logging, which is exactly what is kept from a client.
    pub fn log_context(&self) -> Option<&str> {
        match self {
            Self::Storage(context) => Some(context.as_str()),
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

/// The room's own storage paths still report failures as strings. Those are
/// storage context by construction: every refusal that a caller has to tell
/// apart is built as a variant at the point that decides it.
impl From<String> for WriteError {
    fn from(context: String) -> Self {
        Self::Storage(context)
    }
}

impl From<CatalogError> for WriteError {
    fn from(error: CatalogError) -> Self {
        match error {
            CatalogError::NotFound => Self::NotFound,
            CatalogError::Invalid(why) => Self::Invalid(why),
            CatalogError::Conflict(why) => Self::Conflict(why),
            CatalogError::Database(err) => Self::Storage(err.to_string()),
        }
    }
}

/// A suggestion acceptance keeps its own error type because one of its
/// outcomes -- a passage that no longer places -- is what opens the merge
/// editor in the browser, and no other write has that outcome. It converts
/// both ways so that a handler classifies exactly one kind of error.
impl From<WriteError> for super::AcceptError {
    fn from(error: WriteError) -> Self {
        match error {
            // A storage failure is the room failing, not the caller being
            // refused, and the caller is told so with its context kept for
            // the log.
            WriteError::Storage(context) => Self::Failed(context),
            other => Self::Refused(other.client_message()),
        }
    }
}

impl From<super::AcceptError> for WriteError {
    fn from(error: super::AcceptError) -> Self {
        match error {
            super::AcceptError::Refused(text) => Self::Invalid(text),
            super::AcceptError::Stale => {
                Self::Conflict("the passage has changed since this was suggested".into())
            }
            super::AcceptError::Failed(context) => Self::Storage(context),
        }
    }
}
