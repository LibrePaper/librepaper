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
//!
//! Opening a room also converts the records that older releases wrote into the
//! current format, as one server-authored command, before the room is shared. A
//! document with no old records pays one read.
//!
//! The heal a history trim runs before its shallow snapshot is here too: it
//! gives each stale record fresh cursors, so the snapshot cannot strand it.

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

/// Heals the anchor records of every text in the document. A history trim runs
/// it before the shallow snapshot, which keeps no history that a stale cursor
/// could resolve through. Evaluated against head under the sequencer's lock.
/// Its answer carries the frontier the healing left, read under that same
/// lock, so the trim can refuse a snapshot of anything that landed after it.
struct HealAnchors {
    healed: usize,
}

impl SequencerCommand for HealAnchors {
    type Output = (usize, Vec<u8>);

    fn name(&self) -> &'static str {
        "anchors-heal"
    }

    fn authority(&self) -> Rung {
        Rung::Editor
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        let mut healed = 0;
        let prepared = head
            .prepare(client_seq_of(Uuid::now_v7()), |draft| {
                for file_id in anchors::text_file_ids(draft) {
                    healed += anchors::heal_stale(draft, &file_id).len();
                    // The next file's reads resolve cursors, which commits the
                    // pending writes, so commit this file's first.
                    draft.commit();
                }
                Ok(())
            })
            .map_err(CommandError::Conflict)?;
        self.healed = healed;
        Ok(prepared)
    }

    fn transact<'a>(
        &'a mut self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        // The frontier after the healing's own batch, or the head's when there
        // was nothing to heal.
        let frontier = evidence
            .after_frontier
            .clone()
            .unwrap_or_else(|| evidence.before_frontier.clone());
        Box::pin(async move { Ok((self.healed, frontier)) })
    }
}

/// Converts the anchor records that older releases wrote into the current
/// format. Evaluated against head under the sequencer's lock, like the other
/// anchor commands.
struct UpgradeAnchors {
    upgraded: usize,
}

impl SequencerCommand for UpgradeAnchors {
    type Output = usize;

    fn name(&self) -> &'static str {
        "anchors-upgrade"
    }

    fn authority(&self) -> Rung {
        Rung::Editor
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        let mut upgraded = 0;
        let prepared = head
            .prepare(client_seq_of(Uuid::now_v7()), |draft| {
                upgraded = anchors::upgrade_records(draft).len();
                Ok(())
            })
            .map_err(CommandError::Conflict)?;
        self.upgraded = upgraded;
        Ok(prepared)
    }

    fn transact<'a>(
        &'a mut self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move { Ok(self.upgraded) })
    }
}

/// Heals the anchor records of `room`'s document under the caller's authority.
/// Returns how many records were changed and the encoded frontier the healing
/// left.
pub(crate) async fn heal_anchors(
    room: &Room,
    authority: &Authority,
) -> Result<(usize, Vec<u8>), CommandError> {
    let mut command = HealAnchors { healed: 0 };
    room.command(authority, &mut command).await
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
/// as every other reader reads them, so no whole collection is held. Records in
/// an older format are converted first, so they count as present.
pub(crate) async fn backfill_anchors(room: &Room) -> Result<usize, WriteError> {
    let legacy = room
        .log()
        .with_head(|doc| anchors::has_legacy_records(doc))
        .await?;
    if legacy {
        let authority = owner_authority(room).await?;
        room.command(&authority, &mut UpgradeAnchors { upgraded: 0 })
            .await
            .map_err(WriteError::from)?;
    }
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
