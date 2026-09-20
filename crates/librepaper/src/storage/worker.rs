//! Background work, in this process, with no queue table.
//!
//! There are four kinds of it -- compaction, archives, deletion and the
//! sweep of superseded compaction bases -- and what they have in common is
//! that none of them needs a row to remember it (SPEC-server-is-a-log §8.6).
//! Durable state is what survives a restart:
//!
//! | task | the state that remembers it |
//! |---|---|
//! | compaction | `documents.uncompacted_*` over the threshold |
//! | archive | a label with `archive_requested_at` and no `archive_key` |
//! | deletion | `documents.status = 'deleting'` |
//! | superseded bases | a `superseded_bases.delete_after` in the past |
//!
//! At startup the worker scans those four once and enqueues what it finds.
//! Afterwards it is woken by the events that create the state: an idle
//! deployment issues no queries beyond the lease connection, which is the
//! last test in §14.2. If the bounded queue is full a task is dropped, and
//! the scan or the next trigger for that document finds it again -- the
//! state is still there, which is the whole point of keeping it there and
//! not in a queue.
//!
//! The one timer is the backoff on a task that FAILED, which schedules its
//! own retry. That is not a periodic scan and it does not run on an idle
//! deployment: nothing is failing on one. Without it a failed deletion or
//! archive would wait for the next process start, because unlike compaction
//! nothing else ever asks for them again.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use loro::LoroDoc;
use time::OffsetDateTime;
use uuid::Uuid;

use super::blob::BlobStore;
use super::collaboration::CollaborationStorage;
use super::postgres::PostgresCatalog;
use crate::log::{FlushReason, Registry};

/// How long a deleted document stays in the trash before it is purged.
pub use super::maintenance::DELETION_GRACE;

/// How many tasks may wait at once. Small on purpose: a backlog larger than
/// this is not a queue, it is a scan that has not run yet.
const QUEUE: usize = 256;

/// How long a failed task waits before it is tried again, doubling per
/// consecutive failure up to [`BACKOFF_CEILING`].
///
/// The retry is scheduled rather than left to chance. Compaction would be
/// re-triggered by the next flush anyway, but a deletion and an archive are
/// named only by durable state that nothing else looks at until the next
/// startup, so a transient blob-store failure on either one used to park it
/// until somebody restarted the process.
const BACKOFF: Duration = Duration::from_secs(60);

/// Where the doubling stops. A task failing for an hour is failing for a
/// reason a faster retry will not fix, and the point of the ceiling is that
/// it still retries at all: the state naming the task is durable, so the
/// deployment should recover on its own once whatever broke is fixed,
/// without an operator noticing.
const BACKOFF_CEILING: Duration = Duration::from_secs(60 * 60);

/// The wait after `failures` consecutive failures of one task.
fn backoff(failures: u32) -> Duration {
    BACKOFF
        .saturating_mul(
            1u32.checked_shl(failures.saturating_sub(1))
                .unwrap_or(u32::MAX),
        )
        .min(BACKOFF_CEILING)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Task {
    /// §8.4. The document's backlog is over the threshold.
    Compact(Uuid),
    /// §8.5. Somebody asked for this label's plain-source archive.
    Archive { document: Uuid, label: Uuid },
    /// The document has been in the trash long enough.
    Delete(Uuid),
    /// §8.4 step 5. A compaction base that was replaced is out of the grace
    /// that kept it readable for a download already in flight.
    ///
    /// Alone among the four this names no document: one pass clears every
    /// row that is due, so a `Uuid` here would only make the queue hold the
    /// same work several times over and defeat the `cooling` map, which
    /// keys a backoff by the task itself.
    SweepSupersededBases,
}

/// How many superseded bases one sweep pass takes. The catalogue refuses a
/// batch outside 1..=500; a pass that fills its batch asks for itself again
/// rather than raising the bound.
const SWEEP_BATCH: i64 = 500;

/// What the worker is driven by. Cloneable, so any part of the server can
/// ask for work without holding the worker itself.
#[derive(Clone)]
pub struct Handle {
    tx: tokio::sync::mpsc::Sender<Task>,
}

impl Handle {
    /// Asks for a task. A full queue is not an error: the durable state that
    /// named this task is still there, and the next scan finds it.
    pub fn ask(&self, task: Task) {
        if self.tx.try_send(task).is_err() {
            ::log::debug!("background queue is full; {task:?} will be rediscovered");
        }
    }
}

pub struct Worker {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    sequencers: Arc<Registry>,
    config: Arc<crate::config::Configuration>,
    rx: tokio::sync::mpsc::Receiver<Task>,
    handle: Handle,
    /// Tasks that failed recently: when the retry they already have
    /// scheduled comes due, and how many times in a row they have failed.
    cooling: std::collections::HashMap<Task, (tokio::time::Instant, u32)>,
}

impl Worker {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        blobs: Arc<dyn BlobStore>,
        sequencers: Arc<Registry>,
        config: Arc<crate::config::Configuration>,
    ) -> (Self, Handle) {
        let (tx, rx) = tokio::sync::mpsc::channel(QUEUE);
        let handle = Handle { tx };
        (
            Self {
                catalog,
                blobs,
                sequencers,
                config,
                rx,
                handle: handle.clone(),
                cooling: std::collections::HashMap::new(),
            },
            handle,
        )
    }

    /// The startup scan, then the queue. Runs until the process ends.
    pub async fn run(mut self) {
        self.scan().await;
        while let Some(task) = self.rx.recv().await {
            // Something asked for this task again before its own retry came
            // due. Dropping it is right: the retry below is already
            // scheduled, and running now would defeat the backoff.
            if let Some((due, _)) = self.cooling.get(&task) {
                if tokio::time::Instant::now() < *due {
                    continue;
                }
            }
            match self.execute(task).await {
                Ok(()) => {
                    self.cooling.remove(&task);
                }
                Err(error) => {
                    let failures = self.cooling.get(&task).map_or(0, |(_, n)| *n) + 1;
                    let wait = backoff(failures);
                    ::log::warn!(
                        "background task {task:?} failed ({failures} in a row); \
                         trying again in {}s: {error}",
                        wait.as_secs()
                    );
                    self.cooling
                        .insert(task, (tokio::time::Instant::now() + wait, failures));
                    let handle = self.handle.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(wait).await;
                        handle.ask(task);
                    });
                }
            }
        }
    }

    /// The one scan there is. Everything it finds is already durable; nothing
    /// it misses is lost, because the state that named it is still there.
    pub async fn scan(&self) {
        let pending = match self.catalog.pending_background_work().await {
            Ok(pending) => pending,
            Err(error) => {
                ::log::warn!("could not scan for background work: {error}");
                return;
            }
        };
        for document in pending.compaction {
            self.handle.ask(Task::Compact(document));
        }
        for document in pending.deleting {
            self.handle.ask(Task::Delete(document));
        }
        for label in pending.archives {
            // The label's own row says which document it belongs to; the
            // scan returns ids only, so the task looks it up when it runs.
            self.handle.ask(Task::Archive {
                document: Uuid::nil(),
                label,
            });
        }
        if pending.superseded_bases {
            self.handle.ask(Task::SweepSupersededBases);
        }
    }

    async fn execute(&self, task: Task) -> Result<(), String> {
        match task {
            Task::Compact(document) => self.compact(document).await,
            Task::Archive { label, .. } => self.archive(label).await,
            Task::Delete(document) => self.delete(document).await,
            Task::SweepSupersededBases => self.sweep_superseded_bases().await,
        }
    }

    /// §8.4 step 5's other half. The base a compaction replaced stays
    /// readable for seven days and is then nobody's: no route reads it, no
    /// backup enumerates it, and the row that names it is the only record
    /// that it exists. Without this the object store grows by one stale base
    /// per compaction and never shrinks.
    async fn sweep_superseded_bases(&self) -> Result<(), String> {
        let removed =
            super::maintenance::Maintenance::new(self.catalog.clone(), self.blobs.clone())
                .delete_superseded_bases(SWEEP_BATCH)
                .await?;
        if removed as i64 >= SWEEP_BATCH {
            // More were due than one batch takes. Asking again is progress,
            // not a timer: the pass that finds nothing left stops.
            self.handle.ask(Task::SweepSupersededBases);
        }
        Ok(())
    }

    /// §8.4, all five steps. Coverage is proved before a row is deleted.
    async fn compact(&self, document_id: Uuid) -> Result<(), String> {
        // Admitted if it is not already resident, rather than skipped.
        //
        // Skipping was wrong in the one case that matters most: the startup
        // scan. `pending_background_work` finds documents over the threshold
        // from durable state, and right after a boot none of them is
        // resident, so every single one was dropped on the floor and the
        // only compaction that ever ran was on whatever happened to be open
        // at that instant. A log over §9.1's quota then refuses updates
        // "waiting to be compacted" and waits for ever.
        //
        // Admission itself is one head read (§4.1); the document this then
        // builds is the one compaction cannot avoid building, because a base
        // is an export of it. Going through the registry rather than
        // decoding privately here is what keeps there being exactly one
        // sequencer for a document (§4.3): the base this activates has to
        // land in the accounting of whatever sequencer a reader may already
        // be holding, and a private copy could not do that. It is retired
        // again by the next housekeeping pass, which drops any sequencer
        // with no subscribers and an empty buffer.
        let sequencer = match self.sequencers.resident(document_id).await {
            Some(sequencer) => sequencer,
            None => {
                let Some(document) = self
                    .catalog
                    .document(document_id)
                    .await
                    .map_err(|error| error.to_string())?
                else {
                    return Ok(());
                };
                if document.status != "active" {
                    // A document on its way out is `Task::Delete`'s, and
                    // compacting it would only write a base for the deletion
                    // to collect.
                    return Ok(());
                }
                self.sequencers
                    .get(document_id, &document.slug)
                    .await
                    .map_err(|error| error.to_string())?
            }
        };

        // 1. Flush, so the buffer is empty and `through` is a real row.
        sequencer
            .flush(FlushReason::Barrier)
            .await
            .map_err(|error| error.to_string())?;

        // 2 and 3. An entry whose `oplog_vv()` equals the log vector, and the
        //    snapshot exported from it. An entry that is ahead is not used.
        let Some((through, snapshot, log_vector, changes)) = sequencer
            .snapshot_at_log_vector()
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if through <= 0 {
            return Ok(());
        }

        // 4. Prove coverage. Both halves: the snapshot's own header, and a
        //    scratch document loaded from it. Because gaps are refused at
        //    ingest the entry has no pending operations, so this is a check
        //    and not a hope -- and a mismatch aborts rather than deleting
        //    rows the base does not hold.
        prove_coverage(&snapshot, &log_vector, changes)?;

        // 5a. Write the blob. No lock is held for this: it is compression
        //     and an object-store round trip, and nothing references the
        //     object until 5b activates it.
        let storage = CollaborationStorage::new(self.catalog.clone(), self.blobs.clone());
        let written = storage
            .write_base(document_id, &snapshot)
            .await
            .map_err(|error| error.to_string())?;

        // 5b. Activate the base, delete the rows it covers, reset the
        //     counters, and move the sequencer's own copy of all three --
        //     under the sequencer's compaction gate, so nothing reads the
        //     base and the rows while they disagree. The gate is what makes
        //     the catalogue transaction and the in-memory update one event
        //     as far as a join or a cache build is concerned; see
        //     `Sequencer::compaction_gate` for what each half would
        //     otherwise return.
        let mut gate = sequencer.compaction_gate().await;
        let base = self
            .catalog
            .activate_log_base(
                document_id,
                through,
                &log_vector,
                written,
                super::collaboration::superseded_base_deadline(),
            )
            .await
            .map_err(|error| error.to_string())?;
        if let Some(base) = base {
            // `activate_log_base` deletes rows `<= through` and recomputes
            // `documents.uncompacted_update_*` from what is left, in the
            // same transaction. Typing continues while compaction runs, so
            // rows can land above `through` between the flush in step 1 and
            // this activation; reading the counters back is the only way to
            // learn what survived compaction, rather than assuming zero
            // (SPEC-server-is-a-log §8.4, §9.1, §9.2).
            let head = self
                .catalog
                .log_head(document_id)
                .await
                .map_err(|error| error.to_string())?;
            gate.note_compacted(
                through,
                &base.vector,
                base.snapshot_bytes.max(0) as u64,
                head.uncompacted_bytes.max(0) as u64,
                head.uncompacted_count.max(0),
            );
            drop(gate);
            // This compaction just superseded a base and started its grace.
            // Nothing else will ever ask about that row, so the moment one
            // is created is the moment to look at whether any earlier one
            // has come due -- one indexed query, and only on a deployment
            // that is compacting, which is not an idle one (§14.2).
            self.handle.ask(Task::SweepSupersededBases);
        }
        Ok(())
    }

    /// §8.5. An archive is produced on request from the projection at the
    /// label's frontier, keyed by its tree digest so two labels naming one
    /// document name one object.
    async fn archive(&self, label_id: Uuid) -> Result<(), String> {
        let Some(label) = self
            .catalog
            .label_by_id(label_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if label.archive_key.is_some() {
            return Ok(());
        }
        let document = self
            .catalog
            .document(label.document_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("the document this label belongs to is gone")?;
        let sequencer = self
            .sequencers
            .get(label.document_id, &document.slug)
            .await
            .map_err(|error| error.to_string())?;
        let frontier = loro::Frontiers::decode(&label.frontier)
            .map_err(|error| format!("label frontier: {error}"))?;
        let projected = match sequencer.projection_at(&frontier).await {
            Ok(projected) => projected,
            Err(error) => {
                let message = error.to_string();
                let _ = self
                    .catalog
                    .note_label_archive_error(label_id, &message)
                    .await;
                return Err(message);
            }
        };

        let mut files = Vec::new();
        for (path, entry) in &projected.projection.files {
            match entry.kind.as_str() {
                "text" => files.push(super::source_archive::SourceFile::Inline {
                    path: path.clone(),
                    bytes: projected
                        .texts
                        .get(path)
                        .cloned()
                        .unwrap_or_default()
                        .into_bytes(),
                }),
                _ => {
                    // `assets_by_digests` takes the hex digest, the same form
                    // the projection already carries, and decodes it itself;
                    // no need to round-trip through raw bytes here.
                    let found = self
                        .catalog
                        .assets_by_digests(label.document_id, std::slice::from_ref(&entry.digest))
                        .await
                        .map_err(|error| error.to_string())?;
                    let Some(asset) = found.into_iter().next() else {
                        continue;
                    };
                    // The row we just looked up is the source of truth for
                    // its own digest; decoding `entry.digest` a second time
                    // would only be checking the projection against itself.
                    let Ok(digest) = <[u8; 32]>::try_from(asset.digest.as_slice()) else {
                        continue;
                    };
                    files.push(super::source_archive::SourceFile::Asset {
                        path: path.clone(),
                        asset_id: asset.id,
                        digest,
                        bytes: asset.byte_length.max(0) as u64,
                        media_type: asset.media_type,
                    });
                }
            }
        }
        if files.is_empty() {
            let message = "that label holds no files to archive".to_string();
            let _ = self
                .catalog
                .note_label_archive_error(label_id, &message)
                .await;
            return Err(message);
        }
        let main = if projected.projection.main.is_empty() {
            files[0].path().to_string()
        } else {
            projected.projection.main.clone()
        };
        let format = crate::document::render::document_format(&main)
            .map(str::to_string)
            .unwrap_or_else(|| document.source_format.clone());
        let encoded = super::source_archive::encode(
            super::source_archive::SourceArchive {
                source_format: format,
                main_path: main,
                files,
            },
            super::source_archive::ArchiveLimits {
                source_bytes: self.config.max_document,
                files: self.config.max_files,
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())?;

        // Keyed by the projection digest, so a label naming a document
        // another label already archived names the same object.
        let key = format!(
            "documents/{}/labels/{}.tar.zst",
            label.document_id,
            projected.projection.digest()
        );
        let bytes = encoded.bytes.len() as i64;
        self.blobs
            .put_new(&key, encoded.bytes, "application/zstd")
            .await
            .map_err(|error| error.to_string())?;
        self.catalog
            .attach_label_archive(label_id, &key, bytes)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// The purge of a document whose grace period is up. Blobs first, then
    /// the row: an object left behind is found by the orphan sweeper, while
    /// a row left behind pointing at deleted bytes is a listing entry that
    /// cannot be opened.
    async fn delete(&self, document_id: Uuid) -> Result<(), String> {
        let Some(document) = self
            .catalog
            .document(document_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if document.status != "deleting" {
            return Ok(());
        }
        let Some(deleted_at) = document.deleted_at else {
            return Ok(());
        };
        if OffsetDateTime::now_utc() < deleted_at + DELETION_GRACE {
            return Ok(());
        }
        let mut keys: HashSet<String> = HashSet::new();
        for asset in self
            .catalog
            .document_asset_keys(document_id)
            .await
            .map_err(|error| error.to_string())?
        {
            keys.insert(asset);
        }
        if let Ok(Some(base)) = self.catalog.log_base(document_id).await {
            keys.insert(base.snapshot_key);
        }
        // Every base still inside its grace, not just the newest: the row
        // that names each one is about to cascade away with the document.
        if let Ok(superseded) = self.catalog.superseded_base_keys(document_id).await {
            keys.extend(superseded);
        }
        if !keys.is_empty() {
            // `delete` takes the whole batch at once now, so one deleted
            // document is one call instead of one round trip per blob.
            let keys: Vec<String> = keys.into_iter().collect();
            let count = keys.len();
            if let Err(error) = self.blobs.delete(&keys).await {
                ::log::warn!("could not delete {count} blob(s) for {document_id}: {error}");
            }
        }
        self.catalog
            .finish_document_deletion(document_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// §8.4 step 4. The snapshot must cover exactly what the log covers -- not
/// more, not less -- before a single row is deleted.
///
/// Two independent checks, because they fail differently. The header says
/// what the encoder claimed; the scratch document says what a decoder
/// actually finds. A fabricated or truncated snapshot passes at most one.
pub(crate) fn prove_coverage(
    snapshot: &[u8],
    log_vector: &[u8],
    changes: usize,
) -> Result<(), String> {
    let expected = loro::VersionVector::decode(log_vector)
        .map_err(|error| format!("the log vector will not decode: {error}"))?;
    let header = LoroDoc::decode_import_blob_meta(snapshot, true)
        .map_err(|error| format!("the snapshot has no readable header: {error}"))?;
    if header.partial_end_vv != expected {
        return Err("the snapshot's header does not cover the log".into());
    }
    let scratch = LoroDoc::new();
    scratch
        .import(snapshot)
        .map_err(|error| format!("the snapshot will not load: {error}"))?;
    if scratch.oplog_vv() != expected {
        return Err("the loaded snapshot does not cover the log".into());
    }
    if scratch.len_changes() != changes {
        return Err(format!(
            "the loaded snapshot holds {} changes and the entry held {changes}",
            scratch.len_changes()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task that keeps failing has to keep being retried, and has to stop
    /// getting faster about it. The ceiling is the part worth pinning: an
    /// unbounded doubling reaches days, which is indistinguishable from
    /// giving up.
    #[test]
    fn the_backoff_doubles_and_then_stops_doubling() {
        assert_eq!(backoff(1), BACKOFF);
        assert_eq!(backoff(2), BACKOFF * 2);
        assert_eq!(backoff(3), BACKOFF * 4);
        assert_eq!(backoff(7), BACKOFF_CEILING);
        assert_eq!(backoff(1_000), BACKOFF_CEILING);
        // Failure counts start at one; a zero would be a caller's bug, and
        // it must not underflow into the ceiling.
        assert_eq!(backoff(0), BACKOFF);
    }

    fn document_with(text: &str) -> LoroDoc {
        let doc = crate::document::session::new_doc();
        doc.set_peer_id(7).unwrap();
        doc.get_text("body").insert(0, text).unwrap();
        doc.commit();
        doc
    }

    #[test]
    fn coverage_accepts_a_snapshot_of_exactly_the_log() {
        let doc = document_with("hello");
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        assert_eq!(
            prove_coverage(&snapshot, &doc.oplog_vv().encode(), doc.len_changes()),
            Ok(())
        );
    }

    #[test]
    fn coverage_rejects_a_snapshot_that_is_behind_the_log() {
        let doc = document_with("hello");
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        // The log has moved on; the snapshot has not.
        doc.get_text("body").insert(5, " again").unwrap();
        doc.commit();
        let error = prove_coverage(&snapshot, &doc.oplog_vv().encode(), doc.len_changes())
            .expect_err("a short snapshot must not pass");
        assert!(
            error.contains("does not cover"),
            "unexpected refusal: {error}"
        );
    }

    #[test]
    fn coverage_rejects_a_fabricated_snapshot() {
        let doc = document_with("hello");
        let vector = doc.oplog_vv().encode();
        assert!(prove_coverage(b"not a snapshot at all", &vector, 1).is_err());
        let mut truncated = doc.export(loro::ExportMode::Snapshot).unwrap();
        truncated.truncate(truncated.len() / 2);
        assert!(prove_coverage(&truncated, &vector, doc.len_changes()).is_err());
    }

    #[test]
    fn coverage_rejects_a_snapshot_with_the_wrong_change_count() {
        let doc = document_with("hello");
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        let error = prove_coverage(&snapshot, &doc.oplog_vv().encode(), doc.len_changes() + 1)
            .expect_err("a change count that disagrees must not pass");
        assert!(error.contains("changes"), "unexpected refusal: {error}");
    }
}
