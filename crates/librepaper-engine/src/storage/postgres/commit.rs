//! Who is writing.
//!
//! What used to be here -- `commit_source`, the receipt table, the revision
//! counter -- is gone. A write to the
//! source is a flush of the log, and a write whose meaning depends on the
//! source is a semantic command. Both are assembled by the sequencer rather
//! than by one catalogue method per kind of write. What remains is the identity the
//! authorization check at the commit boundary is made against.

use uuid::Uuid;

/// Who a document transaction is being made on behalf of.
///
/// A deployment admits three kinds of writer: a signed-in account, an
/// authenticated automation key, and somebody holding a link. The
/// transaction that writes checks this value against the live account,
/// grants and `share_links`, so revoking a link or ending a session refuses
/// the next command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Authority {
    /// The writer's stable name. It is used for attribution and identity
    /// comparisons and is not read by the SQL check.
    pub principal_key: String,
    pub account_id: Option<Uuid>,
    /// The account session generation the request was authenticated under.
    pub session_generation: Option<i64>,
    /// SHA-256 digest of the presented link or agent token.
    pub link_hash: Option<[u8; 32]>,
    /// Deployment ceilings resolved from the authenticated identity.
    pub policy_edit: bool,
    pub policy_comment: bool,
    /// In automation mode the account is used for attribution and session
    /// validity, and only the presented live link grants access.
    pub automation: bool,
}
