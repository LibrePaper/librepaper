//! Who is writing.
//!
//! What used to be here -- `commit_source`, the receipt table, the revision
//! counter, the proposal-decision transaction -- is gone. Under
//! SPEC-server-is-a-log a write to the source is a flush of the log (§5.1)
//! and a write whose meaning depends on the source is a semantic command
//! (§7), and both are assembled by the sequencer rather than by one
//! catalogue method per kind of write. What remains is the identity the
//! authorization check at the commit boundary is made against.

use uuid::Uuid;

/// Who a document transaction is being made on behalf of.
///
/// Three spellings of one question, because a deployment admits three kinds
/// of writer: a signed-in account, an authenticated automation key, and
/// somebody holding an editor link. The link hash is checked against
/// `share_links` in the same transaction as the write, so revoking a link
/// refuses the next command rather than the next login.
#[derive(Clone, Debug)]
pub struct Authority {
    /// A stable name for this writer, used as the peer key on the batches it
    /// produces so an acknowledgement can find its way back.
    pub principal_key: String,
    pub account_id: Option<Uuid>,
    pub link_hash: Option<Vec<u8>>,
    /// The account session this request was authenticated under.
    pub session_generation: Option<i64>,
    /// Deployment ceilings resolved from the authenticated identity.
    pub policy_edit: bool,
    pub policy_comment: bool,
    /// In automation mode the account remains useful for attribution and
    /// session validity, while only the presented live link grants access.
    pub automation: bool,
}

impl Authority {
    /// An authority with no account and no link: the deployment itself,
    /// writing source of its own (§7.3). It is refused by the authorization
    /// check, which is correct -- nothing the deployment does on its own
    /// behalf goes through a document command.
    pub fn deployment(peer_key: impl Into<String>) -> Self {
        Self {
            principal_key: peer_key.into(),
            account_id: None,
            link_hash: None,
            session_generation: None,
            policy_edit: false,
            policy_comment: false,
            automation: false,
        }
    }

    pub(crate) fn mutation_authorization(&self) -> super::annotations::MutationAuthorization {
        super::annotations::MutationAuthorization {
            principal_key: self.principal_key.clone(),
            account_id: self.account_id,
            session_generation: self.session_generation,
            token_hash: self
                .link_hash
                .as_deref()
                .and_then(|hash| hash.try_into().ok()),
            policy_edit: self.policy_edit,
            policy_comment: self.policy_comment,
            automation: self.automation,
        }
    }
}
