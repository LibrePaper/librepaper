//! Anchor records for the comments a room already holds.
//!
//! A comment made before anchor records existed has no record in the document,
//! so it reads as unresolved. Opening a room writes the missing records, as one
//! server-authored command, before the room is shared. No edit can reach the
//! room before then, because no socket has joined it. A document with no
//! passage comments costs page reads and no build. A document with passage
//! comments is built when its room opens, because checking for a record reads
//! the document. When every comment has its record, that costs reads and no
//! write.

use futures_util::future::BoxFuture;
use loro::Frontiers;
use uuid::Uuid;

use librepaper_document::document::anchors;
use librepaper_engine::log::sequencer::{
    Command as SequencerCommand, CommandError, Evidence, Head, PreparedSource, Rung,
};
use librepaper_engine::storage::postgres::Authority;

use super::agent::client_seq_of;
use super::comments::{self, COMMENT_PAGE_MAX};
use super::{Room, WriteError};

/// A comment's record, as the backfill found it missing: what the passage is,
/// and the start and end cursors captured at the state the comment was made
/// against, when the passage is still in that state.
struct Missing {
    id: String,
    file_id: String,
    exact: String,
    prefix: String,
    suffix: String,
    cursors: Option<(Vec<u8>, Vec<u8>)>,
}

/// Writes the records that are missing. It is evaluated against head under the
/// sequencer's lock, so a record that another open wrote meanwhile is kept.
struct BackfillAnchors {
    missing: Vec<Missing>,
    written: usize,
}

impl SequencerCommand for BackfillAnchors {
    type Output = usize;

    fn name(&self) -> &'static str {
        "anchors-backfill"
    }

    fn authority(&self) -> Rung {
        Rung::Editor
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        let missing = std::mem::take(&mut self.missing);
        let mut written = 0;
        let prepared = head
            .prepare(client_seq_of(Uuid::now_v7()), |draft| {
                let mut files: Vec<&str> = Vec::new();
                for item in &missing {
                    if draft.get_map(anchors::ANCHORS).get(&item.id).is_some() {
                        continue;
                    }
                    let wrote = match &item.cursors {
                        Some((start, end)) => anchors::create_anchor_with_cursors(
                            draft,
                            &item.file_id,
                            &item.id,
                            start,
                            end,
                            &item.exact,
                            &item.prefix,
                            &item.suffix,
                        ),
                        None => anchors::create_detached_anchor(
                            draft,
                            &item.file_id,
                            &item.id,
                            &item.exact,
                            &item.prefix,
                            &item.suffix,
                        ),
                    };
                    if wrote {
                        written += 1;
                        if item.cursors.is_some() && !files.contains(&item.file_id.as_str()) {
                            files.push(item.file_id.as_str());
                        }
                    }
                }
                // The collapse check reads what was just written, so commit first.
                draft.commit();
                for file_id in files {
                    // A captured passage can have been deleted since its state
                    // was forked, which leaves its record collapsed. Those are
                    // marked detached.
                    anchors::mark_collapsed(draft, file_id);
                }
                Ok(())
            })
            .map_err(CommandError::Conflict)?;
        self.written = written;
        Ok(prepared)
    }

    fn transact<'a>(
        &'a mut self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move { Ok(self.written) })
    }
}

/// The authority the backfill writes under. The editor rung needs the owner or
/// a grant, and an actor without an account has neither, so the write is made
/// as the document's owner, on their live session.
///
/// The deployment's publisher ceiling lives in the server, not in this crate's
/// configuration, so the owner's editor rung is assumed here. Closing that gap
/// means passing the ceiling in.
async fn owner_authority(room: &Room) -> Result<Authority, WriteError> {
    let catalog = room.catalog();
    let document = catalog
        .document(room.document_id)
        .await?
        .ok_or(WriteError::NotFound)?;
    let owner = catalog
        .account(document.owner_id)
        .await?
        .ok_or(WriteError::NotFound)?;
    Ok(Authority {
        principal_key: owner.id.to_string(),
        account_id: Some(owner.id),
        link_hash: None,
        session_generation: Some(owner.session_generation),
        policy_edit: true,
        policy_comment: true,
        automation: false,
    })
}

/// Writes an anchor record for each comment of `room`'s document that has none.
/// Returns how many records were written. Comments are read a page at a time,
/// as every other reader reads them, so no whole collection is held.
pub(crate) async fn backfill_anchors(room: &Room) -> Result<usize, WriteError> {
    let mut missing: Vec<Missing> = Vec::new();
    let mut after = None;
    loop {
        let batch =
            comments::page(room.catalog(), room.document_id, after, COMMENT_PAGE_MAX).await?;
        let (next, complete) = (batch.next, batch.complete);
        let passages: Vec<_> = batch
            .comments
            .iter()
            .filter(|comment| comment.source().is_some())
            .collect();
        if !passages.is_empty() {
            let absent: Vec<usize> = room
                .log()
                .with_head(|doc| {
                    passages
                        .iter()
                        .enumerate()
                        .filter(|(_, comment)| {
                            doc.get_map(anchors::ANCHORS).get(&comment.id).is_none()
                        })
                        .map(|(index, _)| index)
                        .collect::<Vec<usize>>()
                })
                .await?;
            for index in absent {
                let comment = passages[index];
                let (Some(source), Some(anchor)) =
                    (comment.source(), comment.original_anchor.as_ref())
                else {
                    continue;
                };
                let Ok(frontier) = Frontiers::decode(&anchor.frontier) else {
                    continue;
                };
                // The pair is made as `create_anchor` makes it, so the record
                // resolves to the same span the comment was made on. A state
                // that cannot be rebuilt, such as a trimmed history, gets no
                // record. It stays unresolved, which is true, rather than
                // reading as deleted.
                let file_id = source.file_id.0.clone();
                let (from, to) = (source.start_utf16 as usize, source.end_utf16 as usize);
                let Ok(cursors) = room
                    .log()
                    .with_fork_at(&frontier, move |doc| {
                        anchors::capture_cursors(doc, &file_id, from, to)
                    })
                    .await
                else {
                    continue;
                };
                missing.push(Missing {
                    id: comment.id.clone(),
                    file_id: source.file_id.0.clone(),
                    exact: source.exact.clone(),
                    prefix: source.prefix.clone(),
                    suffix: source.suffix.clone(),
                    cursors,
                });
            }
        }
        if complete {
            break;
        }
        match next {
            Some(position) => after = Some(position),
            None => break,
        }
    }
    if missing.is_empty() {
        return Ok(0);
    }
    let authority = owner_authority(room).await?;
    let mut command = BackfillAnchors {
        missing,
        written: 0,
    };
    room.command(&authority, &mut command)
        .await
        .map_err(WriteError::from)
}
