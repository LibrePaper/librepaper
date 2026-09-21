//! Labels: naming a moment of this document.
//!
//! A label (SPEC-server-is-a-log §7.1, §8.2) is the whole of what used to be
//! a checkpoint: no eager archive, no tree of its own, just three things that
//! together name a state -- the vector the log held, the frontier
//! `fork_at` reconstructs from, and the projection digest that says two
//! labels name the same document. Restore -- moving the live document to a
//! past state -- is `server::history::Restore`, which rebuilds it from a
//! label's own archived projection rather than by forking the live CRDT.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use uuid::Uuid;

use super::*;
use crate::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use crate::storage::postgres::{Authority, LabelRecord, NewLabel, PostgresCatalog};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attribution {
    display: String,
    account: Option<String>,
}
impl Attribution {
    pub fn account(id: &str, display: &str) -> Self {
        if id.is_empty() {
            Self::unattributed(display)
        } else {
            Self {
                display: display.into(),
                account: Some(id.into()),
            }
        }
    }
    pub fn unattributed(display: &str) -> Self {
        Self {
            display: display.into(),
            account: None,
        }
    }
    pub fn display(&self) -> &str {
        &self.display
    }
    pub fn account_id(&self) -> Option<&str> {
        self.account.as_deref()
    }
}
impl From<&str> for Attribution {
    fn from(v: &str) -> Self {
        Self::unattributed(v)
    }
}
impl From<&String> for Attribution {
    fn from(v: &String) -> Self {
        Self::unattributed(v)
    }
}
impl From<&Attribution> for Attribution {
    fn from(v: &Attribution) -> Self {
        v.clone()
    }
}

fn account_of(by: &Attribution) -> Option<Uuid> {
    by.account_id().and_then(|id| Uuid::parse_str(id).ok())
}

/// Takes a label: names the head state without touching it.
///
/// §7.1 lists no precondition for this command -- it always succeeds against
/// whatever head it finds -- and §7.3 does not apply to it either: a label
/// produces no source. What `evaluate` does is read the three things a label
/// names a state by, under the same lock every other command reads them
/// under, so that they are exactly what the row this command's flush writes
/// (if any) actually covers.
struct TakeLabel {
    document_id: Uuid,
    reason: String,
    label: Option<String>,
    by: Attribution,
    request_id: Option<Uuid>,
    catalog: Arc<PostgresCatalog>,
    vector: Vec<u8>,
    frontier: Vec<u8>,
    digest: [u8; 32],
}

impl Command for TakeLabel {
    type Output = LabelRecord;

    fn name(&self) -> &'static str {
        "label"
    }

    // §7.2: a retry with the same `request_id` finds the `document_labels`
    // row `transact` wrote and returns it without inserting a second one.
    // `insert_label` also checks this inside the transaction (so a race
    // between two concurrent retries still lands on one row), but that
    // check cannot answer step 0 -- by the time `transact` runs, the
    // sequencer has already materialized head and locked it for this
    // command, which a genuine replay must not do.
    fn replay(&mut self) -> BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>> {
        Box::pin(async move {
            let Some(request_id) = self.request_id else {
                return Ok(None);
            };
            self.catalog
                .label_by_request(self.document_id, request_id)
                .await
                .map_err(CommandError::from)
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        self.vector = head.vector.encode();
        self.frontier = head.frontier.encode();
        self.digest = head.projection.projection.digest_bytes();
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let input = NewLabel {
                id: Uuid::now_v7(),
                document_id: self.document_id,
                source_sequence: evidence.source_sequence,
                vector: self.vector.clone(),
                frontier: self.frontier.clone(),
                tree_digest: Some(self.digest),
                label: self.label.clone(),
                reason: self.reason.clone(),
                request_id: self.request_id,
                author_account_id: account_of(&self.by),
                author_label: self.by.display().to_string(),
            };
            self.catalog
                .insert_label(tx, &input)
                .await
                .map_err(CommandError::from)
        })
    }
}

impl Room {
    /// An account signing a version with its own name rather than with the
    /// id it authenticated as.
    ///
    /// A signed-in caller carries the catalogue uuid as its id, so a version
    /// attributed straight from that id is one the timeline can only call
    /// "Unknown editor". `display` is what the caller already knew to show --
    /// a pseudonym, a link label -- and stands when the catalogue cannot
    /// answer.
    pub(crate) async fn signed_by(&self, account_id: &str, display: &str) -> Attribution {
        let name = match Uuid::parse_str(account_id) {
            Ok(id) => self.catalog().account_display_name(id).await,
            Err(_) => String::new(),
        };
        Attribution::account(account_id, if name.is_empty() { display } else { &name })
    }

    /// Takes a label: records the head state under a reason and an author,
    /// without touching it. `request_id`, when given, is the retry key of
    /// §7.2 -- a second call with the same id returns the first call's row
    /// rather than writing a second one.
    pub async fn take_label(
        &self,
        reason: &str,
        label: Option<String>,
        by: impl Into<Attribution>,
        authority: &Authority,
        request_id: Option<Uuid>,
    ) -> Result<LabelRecord, WriteError> {
        self.take_label_reporting_replay(reason, label, by, authority, request_id)
            .await
            .map(|(record, _)| record)
    }

    /// [`Self::take_label`], and also whether §7.2's retry record answered
    /// it rather than a fresh commit. Callers that speak to a client over a
    /// request/response protocol want this; see
    /// [`crate::log::sequencer::Sequencer::command_reporting_replay`].
    pub async fn take_label_reporting_replay(
        &self,
        reason: &str,
        label: Option<String>,
        by: impl Into<Attribution>,
        authority: &Authority,
        request_id: Option<Uuid>,
    ) -> Result<(LabelRecord, bool), WriteError> {
        let mut command = TakeLabel {
            document_id: self.document_id,
            reason: reason.into(),
            label,
            by: by.into(),
            request_id,
            catalog: self.catalog().clone(),
            vector: Vec::new(),
            frontier: Vec::new(),
            digest: [0; 32],
        };
        self.command_reporting_replay(authority, &mut command)
            .await
            .map_err(WriteError::from)
    }

    /// One page of the timeline, newest first.
    pub async fn label_page(
        &self,
        after: Option<i64>,
        limit: i64,
    ) -> Result<Vec<LabelRecord>, WriteError> {
        self.catalog()
            .label_page(self.document_id, after, limit)
            .await
            .map_err(WriteError::from)
    }
}
