//! An agent takes back its own pending tracked changes, for instance to undo
//! a bad `document_propose` pass. Only changes the same author made, and only
//! while they are pending: accepting a change, or rejecting someone else's,
//! stays the editor's act in the web app.

use std::sync::Arc;

use uuid::Uuid;

use super::source_change::{client_seq_of, label_for_request};
use librepaper_document::document::track;
use librepaper_engine::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use librepaper_engine::storage::postgres::{NewLabel, PostgresCatalog};

const MAX_WITHDRAWN: usize = 100;

/// What a withdrawal answers with: the ids it rejected. A replayed one does
/// not know them, because only the label of the original transaction
/// survives, so it answers with none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawReceipt {
    pub withdrawn: Vec<String>,
    pub replay: bool,
}

/// Refuses the first requested id that is not a pending change by `who`.
/// `pending` is every pending change of the document.
fn check_ownership(
    pending: &[track::ResolvedChange],
    ids: &[String],
    who: &str,
) -> Result<(), String> {
    for id in ids {
        let Some(change) = pending.iter().find(|change| &change.id == id) else {
            return Err(format!(
                "change {id} is not pending; it may already have been accepted or rejected"
            ));
        };
        if change.who != who {
            return Err(format!(
                "change {id} was not made by you; only an editor can reject it"
            ));
        }
    }
    Ok(())
}

/// Rejects the agent's own pending changes by id.
pub struct AgentWithdrawCommand {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    request_id: Uuid,
    change_ids: Vec<String>,
    /// The stable author key stored with each change, `agent:<name>`.
    who: String,
    author_account_id: Option<Uuid>,
    author_label: String,
    // filled in by evaluate, consumed by transact.
    withdrawn: Vec<String>,
}

impl AgentWithdrawCommand {
    /// `who` is the full author key, `agent:<name>`.
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        request_id: Uuid,
        change_ids: Vec<String>,
        who: String,
        author_account_id: Option<Uuid>,
        author_label: String,
    ) -> Result<Self, String> {
        if change_ids.is_empty() {
            return Err("name at least one change to withdraw".into());
        }
        if change_ids.len() > MAX_WITHDRAWN {
            return Err("change count is outside the limit".into());
        }
        Ok(Self {
            catalog,
            document_id,
            request_id,
            change_ids,
            who,
            author_account_id,
            author_label,
            withdrawn: Vec::new(),
        })
    }
}

impl Command for AgentWithdrawCommand {
    type Output = WithdrawReceipt;

    fn name(&self) -> &'static str {
        "agent-withdraw"
    }

    fn replay(
        &mut self,
    ) -> futures_util::future::BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>>
    {
        Box::pin(async move {
            let existing = label_for_request(&self.catalog, self.document_id, self.request_id)
                .await
                .map_err(CommandError::from)?;
            Ok(existing.map(|_| WithdrawReceipt {
                withdrawn: Vec::new(),
                replay: true,
            }))
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        let doc = head.doc();
        let mut pending: Vec<track::ResolvedChange> = Vec::new();
        for entry in head.projection.projection.files.values() {
            if entry.kind == "text" {
                pending.extend(track::resolve_changes(doc, &entry.id));
            }
        }
        check_ownership(&pending, &self.change_ids, &self.who).map_err(CommandError::Conflict)?;
        let ids = self.change_ids.clone();
        let prepared = head
            .prepare(client_seq_of(self.request_id), |draft| {
                track::reject_many(draft, &ids);
                Ok(())
            })
            .map_err(CommandError::Conflict)?;
        self.withdrawn = ids;
        Ok(prepared)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> futures_util::future::BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let after_digest = evidence
                .after_digest
                .clone()
                .unwrap_or_else(|| evidence.before_digest.clone());
            self.catalog
                .insert_label(
                    tx,
                    &NewLabel {
                        id: librepaper_engine::storage::postgres::new_id(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier: evidence
                            .after_frontier
                            .clone()
                            .unwrap_or_else(|| evidence.before_frontier.clone()),
                        tree_digest: Some(
                            *after_digest
                                .parse::<librepaper_base::digest::Digest>()
                                .map_err(|_| CommandError::Conflict("invalid tree digest".into()))?
                                .as_bytes(),
                        ),
                        label: None,
                        reason: "agent-withdraw".to_string(),
                        request_id: Some(self.request_id),
                        author_account_id: self.author_account_id,
                        author_label: self.author_label.clone(),
                    },
                )
                .await?;
            Ok(WithdrawReceipt {
                withdrawn: std::mem::take(&mut self.withdrawn),
                replay: false,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use librepaper_document::document::session;

    const TEXT: &str = "Intro. Alpha beta gamma. Outro.";

    fn tracked(doc: &loro::LoroDoc, who: &str, author: &str, at: usize, delete: usize) -> String {
        let by = track::Author { who, author };
        let ids = track::apply_tracked_edits(
            doc,
            "a.md",
            &[session::Edit {
                at,
                delete,
                insert: "X".into(),
            }],
            &by,
            Some("run"),
            1_000,
        )
        .expect("the tracked edit fits");
        doc.commit();
        ids.into_iter().next().expect("one change")
    }

    fn pending(doc: &loro::LoroDoc) -> Vec<track::ResolvedChange> {
        track::resolve_changes(doc, "a.md")
    }

    fn new_doc() -> loro::LoroDoc {
        let doc = session::new_doc();
        assert!(session::put_text_with_id(&doc, "a.md", "a.md", TEXT));
        doc
    }

    #[test]
    fn own_pending_changes_pass_the_ownership_check() {
        let doc = new_doc();
        let id = tracked(&doc, "agent:Bot", "Bot", 7, 5);
        assert_eq!(
            check_ownership(&pending(&doc), &[id], "agent:Bot"),
            Ok(())
        );
    }

    #[test]
    fn another_authors_change_is_refused() {
        let doc = new_doc();
        let id = tracked(&doc, "agent:Other", "Other", 7, 5);
        let error = check_ownership(&pending(&doc), &[id], "agent:Bot").unwrap_err();
        assert!(error.contains("was not made by you"), "{error}");
    }

    #[test]
    fn an_unknown_change_is_refused() {
        let doc = new_doc();
        let error =
            check_ownership(&pending(&doc), &["nope".to_string()], "agent:Bot").unwrap_err();
        assert!(error.contains("is not pending"), "{error}");
    }

    #[test]
    fn rejecting_a_withdrawn_change_restores_the_text() {
        let doc = new_doc();
        let id = tracked(&doc, "agent:Bot", "Bot", 7, 5);
        assert_ne!(
            super::super::comments::text_for_file_id(&doc, "a.md").as_deref(),
            Some(TEXT)
        );
        assert_eq!(track::reject_many(&doc, &[id.clone()]), 1);
        doc.commit();
        assert_eq!(
            super::super::comments::text_for_file_id(&doc, "a.md").as_deref(),
            Some(TEXT)
        );
        // Once decided, the change is no longer pending.
        let error = check_ownership(&pending(&doc), &[id], "agent:Bot").unwrap_err();
        assert!(error.contains("is not pending"), "{error}");
    }
}
