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
//! | superseded bases | a `document_snapshots.delete_after` in the past |
//!
//! At startup the worker scans those four in bounded pages and enqueues what
//! it finds.
//! Afterwards it is woken by the events that create the state: an idle
//! deployment issues no queries beyond the lease connection, which is the
//! last test in §14.2. A full bounded queue sets one coalesced overflow bit;
//! when the worker reaches it, the worker walks the durable state again in
//! bounded pages. Wake-ups therefore remain recoverable without allocating a
//! waiter per sender or polling an idle database.
//!
//! Wake-ups that need a future deadline instead of an immediate task (a
//! failure's backoff, a deletion's grace period, a superseded base's grace)
//! go through one bounded, deduplicating map, [`Deadlines`]. The
//! worker's own `select!` sleeps on the earliest entry in that map; there is
//! no timer anywhere else, and an idle deployment with nothing failing and
//! nothing scheduled leaves the map empty, so the worker simply blocks on
//! `recv()` and issues no query. A map that fills up does not grow past its
//! bound: it refuses the new deadline, remembers the earliest time it
//! refused, and wakes the worker to rescan durable state at that time
//! instead, which finds the refused work again. That rescan, not the map, is
//! what stays the source of truth for what work exists.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use loro::LoroDoc;
use time::OffsetDateTime;
use uuid::Uuid;

use super::blob::BlobStore;
use super::collaboration::CollaborationStorage;
use super::postgres::{PendingWorkCursor, PostgresCatalog};
use super::schedule::Deadlines;
use crate::log::{FlushReason, Registry};

/// How long a deleted document stays in the trash before it is purged.
pub use super::maintenance::DELETION_GRACE;

/// How many tasks may wait at once. Small on purpose: a backlog larger than
/// this is not a queue, it is a scan that has not run yet.
const QUEUE: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Task {
    /// One bounded page of durable work discovered during startup.
    Scan(PendingWorkCursor),
    /// §8.4. The document's backlog is over the threshold.
    Compact(Uuid),
    /// §8.5. Somebody asked for this label's plain-source archive, named by
    /// the label alone. The startup scan and the HTTP path used to name this
    /// task with the label's document id as well, and the scan only ever
    /// knows `Uuid::nil()` for that half, so the same archive request came
    /// out as two different `Task` values and no map could dedupe them.
    /// `Worker::archive` looks the document up from the label row, so the
    /// document id was never needed here.
    Archive(Uuid),
    /// The document has been in the trash long enough.
    Delete(Uuid),
    /// §8.4 step 5. A compaction base that was replaced is out of the grace
    /// that kept it readable for a download already in flight.
    ///
    /// Alone among the four this names no document: one pass clears every
    /// row that is due, so a `Uuid` here would only make the queue hold the
    /// same work several times over and defeat the deduplication that keys
    /// a deadline by the task itself.
    SweepSupersededBases,
}

/// How many superseded bases one sweep pass takes. The catalogue refuses a
/// batch outside 1..=500; a pass that fills its batch asks for itself again
/// rather than raising the bound.
const SWEEP_BATCH: i64 = 500;

/// What an operator can see of the worker's own overload, for the aggregate
/// cost snapshot. Counters only: a refusal here is never a lost task, so the
/// question these answer is not "what broke" but "is this deployment asking
/// for background work faster than it can run it".
#[derive(Default)]
struct Meter {
    /// Wake-ups the bounded channel had no room for, each one folded into
    /// the rescan bit instead.
    refused_wake_ups: AtomicU64,
    /// Entries in the worker's deadline map, published by the worker after
    /// every task so a reader does not need the worker itself.
    deadlines: AtomicUsize,
    /// Deadlines the map had no room for, each one folded into its earliest
    /// refusal time instead.
    refused_deadlines: AtomicU64,
}

/// What the worker is driven by. Cloneable, so any part of the server can
/// ask for work without holding the worker itself.
#[derive(Clone)]
pub struct Handle {
    tx: tokio::sync::mpsc::Sender<Task>,
    /// A full queue does not justify allocating a waiter for every wake-up.
    /// Durable state is the work list, so one bit is enough to ask the worker
    /// to run its bounded startup scan again after the queue drains.
    rescan: Arc<AtomicBool>,
    meter: Arc<Meter>,
}

impl Handle {
    /// Asks for a task without ever waiting or allocating a sender task.
    ///
    /// A full queue sets one rescan bit. The worker consumes that bit after
    /// the current task and walks durable state in bounded pages. Every
    /// background task is named by that durable state, so retaining one bit
    /// is sufficient and keeps overload memory bounded. In particular, this
    /// method must remain non-blocking: the worker itself asks for follow-up
    /// work while it is the only consumer of this channel.
    pub fn ask(&self, task: Task) {
        match self.tx.try_send(task) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(task)) => {
                self.meter.refused_wake_ups.fetch_add(1, Ordering::Relaxed);
                if !self.rescan.swap(true, Ordering::AcqRel) {
                    ::log::debug!(
                        "background worker queue full; retaining durable wake-up for {task:?}"
                    );
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(task)) => {
                ::log::debug!("background worker stopped before accepting {task:?}");
            }
        }
    }

    fn take_rescan(&self) -> bool {
        self.rescan.swap(false, Ordering::AcqRel)
    }

    /// The operator-only view of background overload. Every number here is
    /// against a stated bound, because a count on its own does not say
    /// whether a deployment is near anything: `queued` against `queue`, and
    /// `deadlines` against `deadline_capacity`. The two refusal counters
    /// rise when a bound was reached and the work was handed back to the
    /// durable rescan; they are not errors, but a deployment where they
    /// climb steadily is one whose background work is behind.
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "queued": QUEUE - self.tx.capacity(),
            "queue": QUEUE,
            "refused_wake_ups": self.meter.refused_wake_ups.load(Ordering::Relaxed),
            "deadlines": self.meter.deadlines.load(Ordering::Relaxed),
            "deadline_capacity": super::schedule::DEADLINES,
            "refused_deadlines": self.meter.refused_deadlines.load(Ordering::Relaxed),
            "rescan_pending": self.rescan.load(Ordering::Acquire),
        })
    }
}

struct ScanPage {
    tasks: Vec<Task>,
    next: Option<PendingWorkCursor>,
    superseded_base_due: Option<OffsetDateTime>,
}

fn enqueue_overflow_scan(handle: &Handle, local: &mut VecDeque<Task>) {
    // A cursor continuation already walks durable state through the end of
    // this pass. Do not consume the bit yet: it may describe a newer row
    // inserted behind an earlier cursor, which needs a fresh full pass.
    if local.iter().any(|task| matches!(task, Task::Scan(_))) {
        return;
    }
    if handle.take_rescan() {
        local.push_back(Task::Scan(PendingWorkCursor::default()));
    }
}

fn enqueue_due_rescan(handle: &Handle, local: &mut VecDeque<Task>, rescan: bool) {
    if !rescan {
        return;
    }
    if local.iter().any(|task| matches!(task, Task::Scan(_))) {
        // A cursor continuation cannot stand in for a fresh full pass.
        handle.rescan.store(true, Ordering::Release);
    } else {
        local.push_back(Task::Scan(PendingWorkCursor::default()));
    }
}

pub struct Worker {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    sequencers: Arc<Registry>,
    config: Arc<crate::config::Configuration>,
    rx: tokio::sync::mpsc::Receiver<Task>,
    handle: Handle,
    /// The bounded map of future work: a failure's backoff, a deletion's
    /// grace period, and a superseded base's grace, all in the one place
    /// the worker's `select!` sleeps on.
    deadlines: Deadlines,
}

impl Worker {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        blobs: Arc<dyn BlobStore>,
        sequencers: Arc<Registry>,
        config: Arc<crate::config::Configuration>,
    ) -> (Self, Handle) {
        let (tx, rx) = tokio::sync::mpsc::channel(QUEUE);
        let handle = Handle {
            tx,
            rescan: Arc::new(AtomicBool::new(false)),
            meter: Arc::new(Meter::default()),
        };
        (
            Self {
                catalog,
                blobs,
                sequencers,
                config,
                rx,
                handle: handle.clone(),
                deadlines: Deadlines::new(),
            },
            handle,
        )
    }

    /// Start a bounded startup scan, then consume it, live wake-ups and
    /// deadlines that have come due.
    pub async fn run(mut self) {
        self.handle.ask(Task::Scan(PendingWorkCursor::default()));
        // Scan work stays local to the worker. A page is at most 3 * PAGE
        // tasks, and its continuation is one more task, so startup cannot
        // fill the channel or lose its cursor when live wake-ups saturate it.
        let mut local = VecDeque::new();
        loop {
            self.fire_due(&mut local);
            let Some(task) = (if let Some(task) = local.pop_front() {
                Some(task)
            } else {
                match self.deadlines.next() {
                    // A deadline is waiting: race it against the channel so
                    // a live wake-up is not blocked behind a sleep, but a
                    // sleep that wins loops back around to `fire_due` rather
                    // than being handled here, since firing is what turns a
                    // due deadline into a task in the first place.
                    Some(at) => tokio::select! {
                        received = self.rx.recv() => received,
                        _ = tokio::time::sleep_until(at) => continue,
                    },
                    // Nothing is scheduled. This is the idle path: no
                    // deadline, no timer, just a block on the channel.
                    None => self.rx.recv().await,
                }
            }) else {
                break;
            };

            if let Task::Scan(cursor) = task {
                if !self.deadlines.waiting(&task, tokio::time::Instant::now()) {
                    match self.scan(cursor).await {
                        Ok(page) => {
                            self.deadlines.clear(&task);
                            if let Some(due) = page.superseded_base_due {
                                self.schedule_sweep_at(due);
                            }
                            local.extend(page.tasks);
                            if let Some(next) = page.next {
                                local.push_back(Task::Scan(next));
                            }
                        }
                        Err(error) => self.failed(task, error),
                    }
                }
            } else {
                self.process(task).await;
            }

            // A live wake-up that arrived while the queue was full may have
            // been the only event naming a task. Restarting a scan is safe:
            // this loop drains all of its pages before receiving more work,
            // so early durable rows cannot starve later cursor pages.
            enqueue_overflow_scan(&self.handle, &mut local);
            self.publish();
        }
    }

    /// Publish what the deadline map holds, so the cost snapshot can read it
    /// without reaching into the worker. Done once per task rather than on
    /// every change: these are for an operator looking at a deployment, not
    /// for anything that makes a decision.
    fn publish(&self) {
        self.handle
            .meter
            .deadlines
            .store(self.deadlines.len(), Ordering::Relaxed);
        self.handle
            .meter
            .refused_deadlines
            .store(self.deadlines.refused(), Ordering::Relaxed);
    }

    /// Move everything whose deadline has come due into the local queue, and
    /// ask for a durable scan when a deadline this worker could not hold has
    /// come due.
    fn fire_due(&mut self, local: &mut VecDeque<Task>) {
        let due = self.deadlines.due(tokio::time::Instant::now());
        for task in due.tasks {
            if !local.contains(&task) {
                local.push_back(task);
            }
        }
        enqueue_due_rescan(&self.handle, local, due.rescan);
    }

    async fn process(&mut self, task: Task) {
        // Something asked for this task again before its own retry or grace
        // period came due. Dropping it is right: the deadline below is
        // already scheduled, and running now would defeat the backoff or
        // the grace period it represents.
        if self.deadlines.waiting(&task, tokio::time::Instant::now()) {
            // A plain Delete wake-up may have been queued before an owner
            // chose "delete forever". Re-read durable state before honoring
            // its old grace deadline. This preserves failure backoff while
            // allowing a hastened `deleted_at` to override grace, including
            // when the urgent wake-up was folded into a durable rescan.
            let grace_overridden = match task {
                Task::Delete(document) if self.deadlines.failures(&task) == 0 => {
                    match self.deletion_is_due(document).await {
                        Ok(due) => due,
                        Err(error) => {
                            self.failed(task, error);
                            return;
                        }
                    }
                }
                _ => false,
            };
            if !grace_overridden {
                return;
            }
        }
        match self.execute(task).await {
            Ok(()) => {
                self.deadlines.clear(&task);
            }
            Err(error) => self.failed(task, error),
        }
    }

    fn failed(&mut self, task: Task, error: String) {
        let before = self.deadlines.refused();
        let wait = self.deadlines.failed(task, tokio::time::Instant::now());
        if self.deadlines.refused() > before {
            // The map is full and this failure's retry did not fit. The
            // deadline is not lost: it is folded into the earliest refusal
            // time, which is when the worker will rescan durable state and
            // find this task again on its own.
            ::log::warn!(
                "background task {task:?} failed and the retry map is full \
                 ({} deadlines); durable state will be rescanned: {error}",
                self.deadlines.len()
            );
        } else {
            ::log::warn!(
                "background task {task:?} failed ({} in a row); \
                 trying again in {}s: {error}",
                self.deadlines.failures(&task),
                wait.as_secs()
            );
        }
    }

    /// Read one bounded page. The continuation sits behind the work from this
    /// page, so the queue stays bounded even with millions of durable tasks.
    async fn scan(&self, cursor: PendingWorkCursor) -> Result<ScanPage, String> {
        const PAGE: i64 = 64;
        let pending = match self.catalog.pending_background_work(cursor, PAGE).await {
            Ok(pending) => pending,
            Err(error) => {
                return Err(format!("could not scan for background work: {error}"));
            }
        };
        let mut next = cursor;
        if let Some(id) = pending.compaction.last() {
            next.compaction = *id;
        }
        if let Some(id) = pending.deleting.last() {
            next.deleting = *id;
        }
        if let Some(id) = pending.archives.last() {
            next.archives = *id;
        }
        next.compaction_done = cursor.compaction_done || (pending.compaction.len() as i64) < PAGE;
        next.deleting_done = cursor.deleting_done || (pending.deleting.len() as i64) < PAGE;
        next.archives_done = cursor.archives_done || (pending.archives.len() as i64) < PAGE;
        let continues = !next.compaction_done || !next.deleting_done || !next.archives_done;
        let mut tasks = Vec::with_capacity(
            pending.compaction.len() + pending.deleting.len() + pending.archives.len(),
        );
        tasks.extend(pending.compaction.into_iter().map(Task::Compact));
        tasks.extend(pending.deleting.into_iter().map(Task::Delete));
        for label in pending.archives {
            // The label's own row says which document it belongs to; the
            // scan returns ids only, so the task looks it up when it runs.
            tasks.push(Task::Archive(label));
        }
        Ok(ScanPage {
            tasks,
            next: continues.then_some(next),
            superseded_base_due: pending.superseded_base_due,
        })
    }

    async fn execute(&mut self, task: Task) -> Result<(), String> {
        match task {
            // Scans are expanded by `run` so their page work never has to
            // compete with live tasks for the bounded channel.
            Task::Scan(_) => Ok(()),
            Task::Compact(document) => self.compact(document).await,
            Task::Archive(label) => self.archive(label).await,
            Task::Delete(document) => self.delete(document).await,
            Task::SweepSupersededBases => self.sweep_superseded_bases().await,
        }
    }

    /// §8.4 step 5's other half. The base a compaction replaced stays
    /// readable for seven days and is then nobody's: no route reads it, no
    /// backup enumerates it, and the row that names it is the only record
    /// that it exists. Without this the object store grows by one stale base
    /// per compaction and never shrinks.
    async fn sweep_superseded_bases(&mut self) -> Result<(), String> {
        let removed =
            super::maintenance::Maintenance::new(self.catalog.clone(), self.blobs.clone())
                .delete_superseded_bases(SWEEP_BATCH)
                .await?;
        if removed as i64 >= SWEEP_BATCH {
            // More were due than one batch takes. Asking again is progress,
            // not a timer: the pass that finds nothing left stops.
            self.handle.ask(Task::SweepSupersededBases);
        } else if let Some(due) = sqlx::query_scalar::<_, Option<OffsetDateTime>>(
            "SELECT min(delete_after) FROM document_snapshots
             WHERE delete_after IS NOT NULL",
        )
        .fetch_one(self.catalog.pool())
        .await
        .map_err(|error| error.to_string())?
        {
            self.schedule_sweep_at(due);
        }
        Ok(())
    }

    fn schedule_sweep_at(&mut self, due: OffsetDateTime) {
        // Re-scans can observe the same future deadline many times while a
        // saturated queue drains. One timer per sweep deadline is now a
        // property of the map: `Deadlines::at` dedupes by `Task`, and
        // `Task::SweepSupersededBases` names no document, so every call
        // here collapses onto the one entry keyed by that task.
        self.schedule_at(due, Task::SweepSupersededBases);
    }

    fn schedule_at(&mut self, due: OffsetDateTime, task: Task) {
        let wait: Duration = (due - OffsetDateTime::now_utc())
            .try_into()
            .unwrap_or(Duration::ZERO);
        self.deadlines.at(task, tokio::time::Instant::now() + wait);
    }

    /// §8.4, all five steps. Coverage is proved before a row is deleted.
    async fn compact(&mut self, document_id: Uuid) -> Result<(), String> {
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
        let activation = self
            .catalog
            .activate_log_base(
                document_id,
                through,
                &log_vector,
                written,
                super::collaboration::superseded_base_deadline(),
            )
            .await;
        let activation = match activation {
            Ok(activation) => activation,
            Err(error) => {
                // A commit error is ambiguous: PostgreSQL may have made the
                // base durable before the connection failed. Reconcile while
                // the compaction gate is still held. If durable state cannot
                // be established, fence this sequencer so no join can observe
                // stale base metadata after covered rows disappeared.
                let recovered = async {
                    let base = self.catalog.log_base(document_id).await?;
                    let head = self.catalog.log_head(document_id).await?;
                    Ok::<_, crate::storage::postgres::Error>((base, head))
                }
                .await;
                match recovered {
                    Ok((Some(base), head))
                        if base.through_update_sequence == through && base.vector == log_vector =>
                    {
                        gate.note_compacted(
                            through,
                            &base.vector,
                            base.snapshot_bytes.max(0) as u64,
                            head.uncompacted_bytes.max(0) as u64,
                            head.uncompacted_count.max(0),
                        );
                        drop(gate);
                        self.handle.ask(Task::SweepSupersededBases);
                        return Ok(());
                    }
                    Ok(_) => return Err(error.to_string()),
                    Err(reconcile_error) => {
                        gate.fence(format!(
                            "compaction outcome is unknown after activation failed ({error}); durable state could not be reconciled ({reconcile_error})"
                        ));
                        return Err(error.to_string());
                    }
                }
            }
        };
        if let Some(activation) = activation {
            let base = activation.base;
            gate.note_compacted(
                through,
                &base.vector,
                base.snapshot_bytes.max(0) as u64,
                activation.uncompacted_bytes.max(0) as u64,
                activation.uncompacted_count.max(0),
            );
            drop(gate);
            // This compaction just superseded a base and started its grace.
            // Nothing else will ever ask about that row, so the moment one
            // is created is the moment to look at whether any earlier one
            // has come due -- one indexed query, and only on a deployment
            // that is compacting, which is not an idle one (§14.2).
            self.handle.ask(Task::SweepSupersededBases);
        } else {
            // A retry after an activation whose reply was lost reaches the
            // monotonic upsert as a no-op. Re-read under the still-held gate
            // so that retry repairs memory instead of silently leaving the
            // sequencer behind the durable base.
            match (
                self.catalog.log_base(document_id).await,
                self.catalog.log_head(document_id).await,
            ) {
                (Ok(Some(base)), Ok(head)) if base.through_update_sequence >= through => {
                    gate.note_compacted(
                        base.through_update_sequence,
                        &base.vector,
                        base.snapshot_bytes.max(0) as u64,
                        head.uncompacted_bytes.max(0) as u64,
                        head.uncompacted_count.max(0),
                    );
                }
                (Ok(_), Ok(_)) => {}
                (base, head) => {
                    let why = format!(
                        "compaction retry could not reconcile durable state (base: {:?}; head: {:?})",
                        base.err(),
                        head.err()
                    );
                    gate.fence(why.clone());
                    return Err(why);
                }
            }
        }
        Ok(())
    }

    /// §8.5. An archive is produced on request from the projection at the
    /// label's frontier, keyed by its tree digest so two labels naming one
    /// document name one object.
    async fn archive(&mut self, label_id: Uuid) -> Result<(), String> {
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

    /// Check whether a document is ready for purge.
    async fn deletion_is_due(&self, document_id: Uuid) -> Result<bool, String> {
        let Some(document) = self
            .catalog
            .document(document_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        Ok(document.status == "purging"
            || (document.status == "deleting"
                && document.deleted_at.is_some_and(|deleted_at| {
                    OffsetDateTime::now_utc() >= deleted_at + DELETION_GRACE
                })))
    }

    /// The purge of a document whose grace period is up. Blobs first, then
    /// the row: an object left behind is found by the orphan sweeper, while
    /// a row left behind pointing at deleted bytes is a listing entry that
    /// cannot be opened.
    async fn delete(&mut self, document_id: Uuid) -> Result<(), String> {
        let Some(document) = self
            .catalog
            .document(document_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if document.status != "deleting" && document.status != "purging" {
            return Ok(());
        }
        let Some(deleted_at) = document.deleted_at else {
            return Ok(());
        };
        if document.status == "deleting" {
            let due = deleted_at + DELETION_GRACE;
            if OffsetDateTime::now_utc() < due {
                self.schedule_at(due, Task::Delete(document_id));
                return Ok(());
            }
            if !self
                .catalog
                .claim_document_purge(document_id)
                .await
                .map_err(|error| error.to_string())?
            {
                // Restoration won the row update. No blob has been touched.
                return Ok(());
            }
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
        if let Some(base) = self
            .catalog
            .log_base(document_id)
            .await
            .map_err(|error| error.to_string())?
        {
            keys.insert(base.snapshot_key);
        }
        // Every retired snapshot still inside its grace, not just the newest:
        // the row that names each one is about to cascade away with the
        // document.
        keys.extend(
            self.catalog
                .superseded_base_keys(document_id)
                .await
                .map_err(|error| error.to_string())?,
        );
        if !keys.is_empty() {
            // `delete` takes the whole batch at once now, so one deleted
            // document is one call instead of one round trip per blob.
            let keys: Vec<String> = keys.into_iter().collect();
            let count = keys.len();
            self.blobs.delete(&keys).await.map_err(|error| {
                format!("could not delete {count} blob(s) for {document_id}: {error}")
            })?;
        }
        self.catalog
            .finish_document_deletion(document_id)
            .await
            .map_err(|error| error.to_string())?;
        // A hastened purge may complete before the old grace reminder fires.
        // Remove that armed entry so it cannot wake the worker for a row that
        // no longer exists; the normal `clear` call then has nothing to keep.
        self.deadlines.remove(&Task::Delete(document_id));
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

    #[tokio::test]
    async fn a_full_queue_keeps_one_rescan_bit_without_waiting_senders() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let handle = Handle {
            tx,
            rescan: Arc::new(AtomicBool::new(false)),
            meter: Arc::new(Meter::default()),
        };
        let first = Task::Compact(Uuid::new_v4());
        let dropped = Task::Compact(Uuid::new_v4());

        handle.ask(first);
        for _ in 0..10_000 {
            handle.ask(dropped);
        }
        assert_eq!(rx.try_recv(), Ok(first));

        // The old implementation spawned one `send` waiter per overflow.
        // Once the receiver made room, those waiters would refill it. A
        // bounded wake-up leaves only the durable-rescan bit behind.
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());
        assert!(handle.take_rescan());
        assert!(!handle.take_rescan());
    }

    #[test]
    fn overflow_rescan_waits_for_all_cursor_pages() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let handle = Handle {
            tx,
            rescan: Arc::new(AtomicBool::new(true)),
            meter: Arc::new(Meter::default()),
        };
        let mut local = VecDeque::from([Task::Scan(PendingWorkCursor::default())]);

        // A continuation means the current full scan still has pages to
        // visit. The overflow bit must survive it, or a newer durable row
        // could be missed after an earlier cursor page keeps returning work.
        enqueue_overflow_scan(&handle, &mut local);
        assert_eq!(local.len(), 1);
        assert!(handle.rescan.load(Ordering::Acquire));

        local.pop_front();
        enqueue_overflow_scan(&handle, &mut local);
        assert_eq!(
            local,
            VecDeque::from([Task::Scan(PendingWorkCursor::default())])
        );
        assert!(!handle.rescan.load(Ordering::Acquire));
    }

    #[test]
    fn due_rescan_is_enqueued_now_or_retained_behind_a_cursor() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let handle = Handle {
            tx,
            rescan: Arc::new(AtomicBool::new(false)),
            meter: Arc::new(Meter::default()),
        };
        let mut local = VecDeque::new();
        enqueue_due_rescan(&handle, &mut local, true);
        assert_eq!(
            local,
            VecDeque::from([Task::Scan(PendingWorkCursor::default())])
        );
        assert!(!handle.rescan.load(Ordering::Acquire));

        let handle = Handle {
            tx: tokio::sync::mpsc::channel(1).0,
            rescan: Arc::new(AtomicBool::new(false)),
            meter: Arc::new(Meter::default()),
        };
        let cursor = PendingWorkCursor {
            archives: Uuid::from_u128(42),
            ..Default::default()
        };
        let mut local = VecDeque::from([Task::Scan(cursor)]);
        let mut deadlines = Deadlines::new();
        let now = tokio::time::Instant::now();
        for _ in 0..super::super::schedule::DEADLINES {
            deadlines.at(
                Task::Delete(Uuid::new_v4()),
                now + Duration::from_secs(86400),
            );
        }
        deadlines.failed(Task::Archive(Uuid::new_v4()), now - Duration::from_secs(61));
        enqueue_due_rescan(&handle, &mut local, deadlines.due(now).rescan);
        assert_eq!(local.len(), 1);
        assert!(handle.rescan.load(Ordering::Acquire));

        enqueue_overflow_scan(&handle, &mut local);
        assert_eq!(local.pop_front(), Some(Task::Scan(cursor)));
        enqueue_overflow_scan(&handle, &mut local);
        assert_eq!(
            local.pop_front(),
            Some(Task::Scan(PendingWorkCursor::default()))
        );
        assert!(!handle.rescan.load(Ordering::Acquire));
        enqueue_due_rescan(&handle, &mut local, deadlines.due(now).rescan);
        assert!(local.is_empty(), "a spent overflow must not rescan forever");
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
    async fn delete_wakeups_preserve_failure_backoff_without_querying_storage() {
        let catalog = Arc::new(
            PostgresCatalog::connect(super::super::postgres::PostgresOptions::new(
                std::env::var("LIBREPAPER_TEST_POSTGRES_URL").expect("test database URL"),
            ))
            .await
            .unwrap(),
        );
        let directory = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> =
            Arc::new(super::super::blob::FsStore::new(directory.path(), false));
        let config = Arc::new(crate::config::Configuration::default());
        let registry = Registry::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            "test".into(),
        );
        let (mut worker, _) = Worker::new(catalog.clone(), blobs, registry, config);
        let task = Task::Delete(Uuid::new_v4());
        worker.deadlines.failed(task, tokio::time::Instant::now());
        let deadline = worker.deadlines.next();
        // Any accidental storage query now fails and increments the failure
        // streak. A duplicate wake-up must leave the pending retry untouched.
        catalog.close().await;
        worker.process(task).await;
        assert_eq!(worker.deadlines.failures(&task), 1);
        assert_eq!(worker.deadlines.next(), deadline);
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
