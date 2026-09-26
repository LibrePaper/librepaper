//! SPEC-server-is-a-log §14.2 coverage for the pieces that are pure
//! sequencer state: ingest, the flush triggers, acknowledgement, join, and
//! (by way of the fixtures) the frame it all rides on.
//!
//! None of these tests open a database connection. `Sequencer::admit` is
//! the only production path that needs `Arc<PostgresCatalog>`, and it needs
//! it only for the two reads in §4.1 that establish where a document's log
//! starts. Everything a sequencer does afterward -- ingest, the flush
//! triggers, the buffer, relay, and (mostly) join -- is answered from state
//! the sequencer already holds. `Sequencer::from_parts` is the seam that
//! lets a test supply that state directly instead of reading it, and
//! `LogCatalog` is the seam that lets a test supply a document's rows and
//! base without a database: both are used by `admit` itself (see
//! `sequencer.rs`), not carved out for tests alone.
//!
//! What is deliberately NOT here, because it needs a database or a
//! not-yet-compiling neighbour:
//!
//! * The compaction coverage proof (§8.4 step 4) -- already unit-tested in
//!   `storage/worker.rs` against a real snapshot.
//! * Source-producing command evidence (§7.3/§7.4) -- `Command::transact`
//!   takes a real `sqlx::Transaction`, which cannot be faked; it needs
//!   `LIBREPAPER_TEST_POSTGRES_URL`.
//! * Lease loss during buffered typing -- exercises `begin_document_command`
//!   and `flush_log_row`'s fencing against a real `deployment_writer` row.
//! * The idle-deployment query count -- a property of a whole running
//!   server, not of one sequencer.

use std::borrow::Cow;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures_util::future::BoxFuture;
use loro::{ExportMode, Frontiers, LoroDoc, VersionVector};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::config::Configuration;
use crate::log::budget::DEFAULT_EXPANSION;
use crate::log::sequencer::{
    ack_targets, max_update_bytes, FlushReason, Ingested, LogCatalog, Role, Sequencer,
    SequencerError, FLUSH_MAX_AGE, FLUSH_QUIET, FLUSH_TRIGGER_BYTES,
};
use crate::log::{frame, Budget};
use crate::room::outgoing::{Outgoing, Receiver, Sender};
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::postgres::{self, Authority, FlushRow};

// -- fixtures ------------------------------------------------------------

/// A fake `LogCatalog`. Every method a test does not exercise panics if
/// called, which is the point: it proves the code path under test really
/// does not touch storage, rather than merely returning an empty answer
/// that would hide the same bug.
struct FakeCatalog {
    /// Behind locks so a test can change what storage holds WHILE a
    /// sequencer is part-way through reading it, which is what compaction
    /// does to a live document: it deletes the rows it folded into a base
    /// (§8.4 step 5) under whatever else happens to be running.
    coverage: Mutex<Vec<postgres::RowCoverage>>,
    rows: Mutex<Vec<postgres::LogRow>>,
    base: Mutex<Option<Vec<u8>>>,
    /// Every row a flush wrote, in order, for inspection after the fact.
    flushed: Mutex<Vec<Vec<u8>>>,
    next_sequence: AtomicI64,
    /// When set, `log_coverage` announces it has been entered and then
    /// waits to be released, so a test can force an ingest to land in the
    /// exact window §6.2 says is safe: after `join`'s coverage read and
    /// before it takes the lock.
    gate: Option<(Arc<Notify>, Arc<Notify>)>,
    /// When true, the next `flush_log_row` fails once (as if the response
    /// were lost), so a test can exercise §5.1's ambiguous-outcome path.
    /// One-shot: cleared on the failing call, so a retried flush after the
    /// error succeeds normally like a real reconnect would.
    fail_next_flush: std::sync::atomic::AtomicBool,
    /// Whether the write whose response `fail_next_flush` loses had
    /// nevertheless committed. That is what makes the outcome ambiguous
    /// rather than merely failed, and storing the row here -- with exactly
    /// the bytes and vector the flush handed over -- is how a test seeds
    /// the row `resolve_ambiguous_flush` re-reads without recomputing by
    /// hand the vector `flush` is about to write.
    lost_flush_committed: bool,
    /// What `log_sequence` answers when the ambiguous-outcome path re-reads
    /// it (§5.1). Only meaningful together with `fail_next_flush`.
    sequence_on_ambiguity: AtomicI64,
    /// §8.4's two triggers, as `PostgresCatalog` would report this
    /// deployment's. Small in the tests that want to cross them without
    /// writing a hundred rows.
    compaction_thresholds: (i64, i64),
    /// When set, `flush_log_row` announces it has been entered and then
    /// waits to be released. This is a database that has stopped answering,
    /// which is the whole workload SPEC-frugal §2 is about: it lets a test
    /// hold a write open and look at what the buffer and the pending
    /// reservations are doing meanwhile, without a sleep anywhere.
    flush_gate: Option<(Arc<Notify>, Arc<Notify>)>,
    /// When true, every `flush_log_row` fails, with no row written and the
    /// sequence left where it was. Distinct from `fail_next_flush`, which is
    /// one-shot and is about an AMBIGUOUS outcome; this is a write that
    /// plainly did not happen.
    fail_every_flush: std::sync::atomic::AtomicBool,
}

impl FakeCatalog {
    fn empty() -> Self {
        Self {
            coverage: Mutex::new(Vec::new()),
            rows: Mutex::new(Vec::new()),
            base: Mutex::new(None),
            flushed: Mutex::new(Vec::new()),
            next_sequence: AtomicI64::new(1),
            gate: None,
            fail_next_flush: std::sync::atomic::AtomicBool::new(false),
            lost_flush_committed: false,
            sequence_on_ambiguity: AtomicI64::new(0),
            compaction_thresholds: (100, 16 * 1024 * 1024),
            flush_gate: None,
            fail_every_flush: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl LogCatalog for FakeCatalog {
    fn log_coverage(
        &self,
        _document_id: Uuid,
    ) -> BoxFuture<'_, postgres::Result<Vec<postgres::RowCoverage>>> {
        // Sampled before the gate, so a test that holds a join here and
        // then deletes rows reproduces the real ordering: the coverage
        // query answered, and only afterwards did compaction land.
        let coverage = self.coverage.lock().unwrap().clone();
        let gate = self.gate.clone();
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            Ok(coverage)
        })
    }

    fn log_rows(
        &self,
        _document_id: Uuid,
        after: i64,
        through: Option<i64>,
    ) -> BoxFuture<'_, postgres::Result<Vec<postgres::LogRow>>> {
        let through = through.unwrap_or(i64::MAX);
        let rows: Vec<_> = self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.update_sequence > after && row.update_sequence <= through)
            .cloned()
            .collect();
        Box::pin(async move { Ok(rows) })
    }

    fn flush_log_row<'a>(
        &'a self,
        _document_id: Uuid,
        row: FlushRow<'a>,
        scratch: super::pending::Reservation,
    ) -> BoxFuture<'a, postgres::Result<i64>> {
        if self.fail_next_flush.swap(false, Ordering::SeqCst) {
            if self.lost_flush_committed {
                let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst);
                self.rows.lock().unwrap().push(postgres::LogRow {
                    update_sequence: sequence,
                    update_bytes: row.update_bytes.to_vec(),
                    vector: row.vector.to_vec(),
                    created_at: time::OffsetDateTime::now_utc(),
                });
                self.flushed.lock().unwrap().push(row.update_bytes.to_vec());
            }
            return Box::pin(async {
                Err(postgres::Error::Conflict(
                    "simulated: the response to this write was lost".into(),
                ))
            });
        }
        let gate = self.flush_gate.clone();
        let failing = self.fail_every_flush.load(Ordering::SeqCst);
        Box::pin(async move {
            let _scratch = scratch;
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            if failing {
                return Err(postgres::Error::Conflict(
                    "simulated: this write did not happen".into(),
                ));
            }
            self.flushed.lock().unwrap().push(row.update_bytes.to_vec());
            Ok(self.next_sequence.fetch_add(1, Ordering::SeqCst))
        })
    }

    fn log_sequence(&self, _document_id: Uuid) -> BoxFuture<'_, postgres::Result<i64>> {
        let answer = self.sequence_on_ambiguity.load(Ordering::SeqCst);
        Box::pin(async move { Ok(answer) })
    }

    fn persistence_connection(
        &self,
        _scratch: super::pending::Reservation,
    ) -> BoxFuture<'_, postgres::Result<postgres::PersistenceConnection>> {
        Box::pin(async { unimplemented!("a semantic command needs PostgreSQL") })
    }

    fn begin_document_command<'a>(
        &'a self,
        _connection: &'a mut postgres::PersistenceConnection,
        _document_id: Uuid,
        _authority: &'a Authority,
        _rung: super::sequencer::Rung,
    ) -> BoxFuture<'a, postgres::Result<sqlx::Transaction<'a, sqlx::Postgres>>> {
        Box::pin(async {
            unimplemented!("a semantic command needs a real transaction; it needs LIBREPAPER_TEST_POSTGRES_URL")
        })
    }

    fn insert_log_row<'a>(
        &'a self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _document_id: Uuid,
        _row: FlushRow<'a>,
    ) -> BoxFuture<'a, postgres::Result<i64>> {
        Box::pin(async {
            unimplemented!("a semantic command needs a real transaction; it needs LIBREPAPER_TEST_POSTGRES_URL")
        })
    }

    fn read_base(
        &self,
        _document_id: Uuid,
        _blobs: Arc<dyn BlobStore>,
        _expanded_limit: u64,
    ) -> BoxFuture<'_, std::result::Result<Vec<u8>, String>> {
        let base = self.base.lock().unwrap().clone();
        Box::pin(async move { base.ok_or_else(|| "no base configured for this fake".to_string()) })
    }

    fn compaction_thresholds(&self) -> (i64, i64) {
        self.compaction_thresholds
    }
}

/// A throwaway object store. Nothing in this file calls `build` or
/// `read_base` successfully, so this is here only to give `Sequencer` a
/// value for a field it always has; a leaked temp directory is fine for a
/// short-lived test binary.
fn blobs() -> Arc<dyn BlobStore> {
    let dir = tempfile::tempdir().expect("a temp directory for the unused blob store");
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    Arc::new(FsStore::new(path, false))
}

/// A pending budget no test reaches by accident. Cases that are ABOUT the
/// pending bound build their own, deliberately small one and share it across
/// sequencers, which is what makes the bound deployment-wide.
fn generous_pending() -> Arc<crate::log::PendingBudget> {
    crate::log::PendingBudget::new(u64::MAX / 2, u64::MAX / 2)
}

/// A sequencer with no base, no rows and an empty buffer -- admitted
/// without I/O, per `Sequencer::from_parts`' own doc comment.
fn bare_sequencer(catalog: Arc<dyn LogCatalog>) -> Sequencer {
    sequencer_sharing_pending(catalog, generous_pending())
}

/// The same, against a pending budget the caller supplies -- so two of them
/// can be built against ONE budget and the deployment-wide half of the bound
/// can be seen at all.
fn sequencer_sharing_pending(
    catalog: Arc<dyn LogCatalog>,
    pending: Arc<crate::log::PendingBudget>,
) -> Sequencer {
    Sequencer::from_parts(
        Uuid::new_v4(),
        "test-doc".to_string(),
        Uuid::nil(),
        catalog,
        blobs(),
        Arc::new(Configuration::default()),
        Budget::new(u64::MAX / 2, DEFAULT_EXPANSION),
        pending,
        "deployment.peer.test".to_string(),
        crate::log::ledger::StorageLedger::new(),
        1,
        VersionVector::default(),
        0,
        0,
        0,
        VersionVector::default(),
        false,
        Arc::new(std::sync::OnceLock::new()),
    )
}

/// A causally-linked source of update batches, standing in for one editor's
/// outbox: each call exports exactly the ops since the last call, which is
/// what one `doc-update` looks like on the wire.
// `pub(super)`, not private: `log::recovery` (SPEC-server-is-a-log §14.1)
// reuses this as the same causally-linked batch source rather than building
// a second one. Nothing outside `crate::log` sees it either way.
pub(super) struct Outbox {
    doc: LoroDoc,
    at: VersionVector,
}

impl Outbox {
    pub(super) fn new() -> Self {
        let doc = LoroDoc::new();
        let _ = doc.get_text("t");
        Self {
            doc,
            at: VersionVector::default(),
        }
    }

    pub(super) fn export_since_last(&mut self) -> Vec<u8> {
        self.doc.commit();
        let batch = self
            .doc
            .export(ExportMode::Updates {
                from: Cow::Borrowed(&self.at),
            })
            .expect("exporting from a covered vector never fails");
        self.at = self.doc.oplog_vv();
        batch
    }

    /// One keystroke's worth of edit, batched.
    pub(super) fn edit(&mut self) -> Vec<u8> {
        let text = self.doc.get_text("t");
        text.insert_utf16(text.len_utf16(), "x").unwrap();
        self.export_since_last()
    }

    /// An edit padded to at least `bytes` wide. The filler is random, not
    /// one repeated character: Loro's run-length encoding would otherwise
    /// collapse a repeated character back under the trigger it is meant to
    /// cross.
    pub(super) fn edit_at_least(&mut self, bytes: usize) -> Vec<u8> {
        let filler = hex::encode(crate::auth::random_bytes(bytes / 2 + 1));
        let text = self.doc.get_text("t");
        text.insert_utf16(text.len_utf16(), &filler).unwrap();
        self.export_since_last()
    }

    /// What this outbox's own document currently reads, for a test to
    /// compare against what a server it sent batches to ends up holding.
    pub(super) fn text(&self) -> String {
        self.doc.get_text("t").to_string()
    }

    /// This outbox's own vector -- what it has sent and can vouch for --
    /// distinct from `export_since_last`'s bookkeeping of what it has
    /// already exported once.
    pub(super) fn vector(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    /// Every op after `vector`, without touching this outbox's own
    /// bookkeeping of what it has already exported once -- what a client
    /// resending its whole unsent history after a `doc-gap` does.
    pub(super) fn export_from(&self, vector: &VersionVector) -> Vec<u8> {
        self.doc
            .export(ExportMode::Updates {
                from: Cow::Borrowed(vector),
            })
            .expect("exporting from a covered vector never fails")
    }
}

// -- ingest (§5, §14.2) ---------------------------------------------------

/// A batch that carries no changes never becomes a row.
///
/// This is the one an `is_empty()` check cannot catch, and it went
/// unnoticed until a retry test asked why an idempotent command had written
/// a second row. A Loro `Updates` export is never zero-length: the format
/// writes its header and envelope whatever is inside, so a fork that
/// diverged from its parent by nothing at all still exports 22 bytes.
/// `ImportBlobMetadata::change_num` is what the format uses to say "this
/// covers zero changes", and it is the only honest test.
///
/// It matters here more than anywhere, because §6.2 step 6 has EVERY client
/// send exactly such a batch on EVERY join: catch-up exports
/// `Updates { from: vector }`, and a client that was already up to date has
/// nothing to put in it. One reconnect would otherwise cost one row, which
/// is what a whole minute of somebody typing costs (§5.2).
#[tokio::test]
async fn a_batch_carrying_no_changes_is_acknowledged_without_becoming_a_row() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:writer", "account:writer", 1, outbox.edit())
            .await,
        Ingested::Accepted
    ));

    // Exporting again with nothing new is what a rejoin does.
    let nothing = outbox.export_since_last();
    assert!(
        !nothing.is_empty(),
        "sanity: a no-op Loro export still carries its envelope, which is \
         the whole reason this case needs a test"
    );
    assert_eq!(
        LoroDoc::decode_import_blob_meta(&nothing, true)
            .expect("a well-formed batch")
            .change_num,
        0,
        "sanity: and it carries no changes"
    );

    let before = sequencer.log_state().await.buffered;
    assert!(
        matches!(
            sequencer
                .ingest(1, "account:writer", "account:writer", 2, nothing)
                .await,
            Ingested::Accepted,
        ),
        "a batch with nothing in it is accepted, not refused: nothing is wrong with it"
    );
    assert_eq!(
        sequencer.log_state().await.buffered,
        before,
        "an empty batch reached the buffer and would have become a row"
    );
}

/// §5 step 2: "the sender is within its per-principal rate".
///
/// This is the one of the three ingest bounds that is about a person rather
/// than about bytes, and it is the one that was silently dropped once
/// already when the sequencer was first written: the config key and the
/// refusal type both survived while nothing counted. So this asserts more
/// than the refusal -- that the bound is retryable rather than fatal, that
/// it is charged per PEER and not per socket, and that one peer's spending
/// is not charged to another.
///
/// The bucket's clock is pinned for the whole case (`set_rate_clock`), so
/// nothing here turns on how long the loop below takes to run. It used to
/// turn on exactly that: a fixed window reset on a wall-clock minute
/// boundary, and a loop that straddled one saw its allowance handed back
/// mid-assertion. That flake was the bug reporting itself, and
/// `an_exhausted_allowance_comes_back_only_as_the_bucket_refills` is where
/// the behaviour it was complaining about is pinned down.
#[tokio::test]
async fn a_peer_past_its_minute_allowance_is_refused_retryably_and_others_are_not() {
    let config = Configuration::default();
    let allowance = config.session.updates_per_minute;
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    sequencer.set_rate_clock(Duration::from_secs(1));
    let mut outbox = Outbox::new();

    for sent in 1..=allowance {
        let outcome = sequencer
            .ingest(1, "account:writer", "account:writer", sent, outbox.edit())
            .await;
        assert!(
            matches!(outcome, Ingested::Accepted),
            "update {sent} of an allowance of {allowance} was refused"
        );
    }
    let over = sequencer
        .ingest(
            1,
            "account:writer",
            "account:writer",
            allowance + 1,
            outbox.edit(),
        )
        .await;
    assert!(
        matches!(over, Ingested::Retryable(_)),
        "one past the allowance was not refused retryably: {over:?}"
    );

    // The same person on a second socket is the same person. A bound kept
    // per socket would be no bound at all, because a client in a resend
    // loop reconnects.
    //
    // A FRESH outbox, not the one above: the two refused batches were
    // dropped rather than queued (§5 step 4), so that outbox is now
    // causally ahead of the log and its next export would be answered with
    // `doc-gap` before the rate check ever ran. That is correct behaviour
    // and it would hide what this case is about.
    let mut second_tab = Outbox::new();
    let same_person = sequencer
        .ingest(2, "account:writer", "account:writer", 1, second_tab.edit())
        .await;
    assert!(
        matches!(same_person, Ingested::Retryable(_)),
        "a second socket let one peer past its own allowance: {same_person:?}"
    );

    // Somebody else is unaffected, which is what "per-principal" means.
    let mut other = Outbox::new();
    let somebody_else = sequencer
        .ingest(3, "account:other", "account:other", 1, other.edit())
        .await;
    assert!(
        matches!(somebody_else, Ingested::Accepted),
        "one peer's allowance was charged to another: {somebody_else:?}"
    );
}

/// §9.1 says token bucket, and this is the difference between a bucket and
/// the fixed window that stood here before.
///
/// A window resets: it hands the whole allowance back at once when its
/// boundary passes, so the same caller can spend `allowance` just before it
/// and `allowance` again just after -- twice the bound inside whatever
/// fraction of a second separates the two bursts. A bucket has no boundary
/// to reach. Waiting buys exactly what the waiting was worth, which is what
/// the middle of this case measures: one second of an `updates_per_minute`
/// allowance is `updates_per_minute / 60` updates and not one more.
///
/// The clock is pinned rather than slept through, so "no time passed"
/// really is no time and "one second passed" really is one second. See
/// `Sequencer::set_rate_clock`.
#[tokio::test]
async fn an_exhausted_allowance_comes_back_only_as_the_bucket_refills() {
    let config = Configuration::default();
    let allowance = config.session.updates_per_minute;
    let a_second_of_it = allowance / 60;
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    sequencer.set_rate_clock(Duration::from_secs(1));

    let mut outbox = Outbox::new();
    for sent in 1..=allowance {
        let outcome = sequencer
            .ingest(1, "account:writer", "account:writer", sent, outbox.edit())
            .await;
        assert!(
            matches!(outcome, Ingested::Accepted),
            "update {sent} of an allowance of {allowance} was refused"
        );
    }

    // No time has passed at all, so nothing has refilled. Under a window
    // this is where a boundary could fall and admit the whole second burst.
    let mut immediately_again = Outbox::new();
    for attempt in 1..=allowance {
        let outcome = sequencer
            .ingest(
                1,
                "account:writer",
                "account:writer",
                attempt,
                immediately_again.edit(),
            )
            .await;
        assert!(
            matches!(outcome, Ingested::Retryable(_)),
            "attempt {attempt} of a second allowance, with no time passed, was admitted: {outcome:?}"
        );
    }

    // One second buys one second's worth. Not a boundary's worth.
    sequencer.set_rate_clock(Duration::from_secs(2));
    let mut after_a_second = Outbox::new();
    for sent in 1..=a_second_of_it {
        let outcome = sequencer
            .ingest(
                1,
                "account:writer",
                "account:writer",
                sent,
                after_a_second.edit(),
            )
            .await;
        assert!(
            matches!(outcome, Ingested::Accepted),
            "update {sent} of the {a_second_of_it} a second refills was refused: {outcome:?}"
        );
    }
    let mut one_too_many = Outbox::new();
    let over = sequencer
        .ingest(
            1,
            "account:writer",
            "account:writer",
            1,
            one_too_many.edit(),
        )
        .await;
    assert!(
        matches!(over, Ingested::Retryable(_)),
        "a second of waiting gave back more than a second's worth: {over:?}"
    );

    // And the refusal really is temporary: a full period restores the lot.
    sequencer.set_rate_clock(Duration::from_secs(62));
    let mut later = Outbox::new();
    let welcome_back = sequencer
        .ingest(1, "account:writer", "account:writer", 1, later.edit())
        .await;
    assert!(
        matches!(welcome_back, Ingested::Accepted),
        "the bucket never refilled: {welcome_back:?}"
    );
}

#[tokio::test]
async fn reconnecting_under_a_new_peer_key_does_not_refill_an_allowance() {
    let config = Configuration::default();
    let allowance = config.session.updates_per_minute;
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    // Pinned so that the time the loop below takes cannot itself refill the
    // bucket. What this case is about is who the allowance belongs to, not
    // how fast it comes back.
    sequencer.set_rate_clock(Duration::from_secs(1));
    let mut outbox = Outbox::new();

    // A socket's peer key is its socket id, because `client_seq` counts per
    // connection and two connections sharing an ack address would collide
    // their sequence namespaces. The allowance is charged to the principal
    // beside it instead, which is the whole point of this case.
    for sent in 1..=allowance {
        let outcome = sequencer
            .ingest(1, "1", "account:writer", sent, outbox.edit())
            .await;
        assert!(
            matches!(outcome, Ingested::Accepted),
            "update {sent} of an allowance of {allowance} was refused"
        );
    }
    let over = sequencer
        .ingest(1, "1", "account:writer", allowance + 1, outbox.edit())
        .await;
    assert!(
        matches!(over, Ingested::Retryable(_)),
        "one past the allowance was not refused retryably: {over:?}"
    );

    // The reconnect: a new socket id, and therefore a new peer key and a
    // `client_seq` counting from one again, which is exactly what a client
    // stuck in a resend loop does. Only the principal is unchanged, so only
    // the principal can hold the bound.
    //
    // A FRESH outbox, for the reason the neighbouring case gives: the
    // refused batches were dropped rather than queued, so the first outbox
    // is now causally ahead of the log and its next export would be answered
    // with `doc-gap` before the rate check ever ran.
    let mut after_reconnect = Outbox::new();
    let same_person_again = sequencer
        .ingest(2, "2", "account:writer", 1, after_reconnect.edit())
        .await;
    assert!(
        matches!(same_person_again, Ingested::Retryable(_)),
        "reconnecting refilled a principal's allowance: {same_person_again:?}"
    );

    // Somebody else, arriving on their own socket, is untouched by it.
    let mut other = Outbox::new();
    let somebody_else = sequencer
        .ingest(3, "3", "net:198.51.100.7", 1, other.edit())
        .await;
    assert!(
        matches!(somebody_else, Ingested::Accepted),
        "one principal's allowance was charged to another: {somebody_else:?}"
    );
}

#[tokio::test]
async fn a_complete_batch_is_accepted_without_building_a_cache() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let batch = Outbox::new().edit();
    let outcome = sequencer
        .ingest(1, "account:writer", "account:writer", 1, batch)
        .await;
    assert!(
        matches!(outcome, Ingested::Accepted),
        "expected Accepted, got {outcome:?}"
    );
    assert!(
        !sequencer.is_warm().await,
        "ingest must not decode a document to accept a batch (§4.1)"
    );
}

#[tokio::test]
async fn a_batch_whose_start_vector_is_not_covered_is_a_gap_not_an_append() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let mut outbox = Outbox::new();
    let _first = outbox.edit();
    // The second batch's start vector is the first batch's end vector,
    // which the sequencer has never seen because the first was never
    // ingested. §5 step 4: refuse, do not append.
    let second = outbox.edit();
    let outcome = sequencer
        .ingest(1, "account:writer", "account:writer", 2, second)
        .await;
    match outcome {
        Ingested::Gap { vector } => {
            assert_eq!(
                vector,
                VersionVector::default().encode(),
                "the head is still empty, so the client should export from nothing"
            );
        }
        other => panic!("expected a gap, got {other:?}"),
    }
    assert_eq!(
        sequencer.log_state().await.buffered,
        0,
        "a refused gap must not be appended"
    );
}

#[tokio::test]
async fn a_corrupted_batch_is_invalid_rather_than_imported() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let mut batch = Outbox::new().edit();
    // Flip a byte in the body, not the outermost framing, so this exercises
    // the checksum rather than a bounds check.
    let middle = batch.len() / 2;
    batch[middle] ^= 0xff;
    let outcome = sequencer
        .ingest(1, "account:writer", "account:writer", 1, batch)
        .await;
    assert!(
        matches!(outcome, Ingested::Invalid(_)),
        "expected Invalid, got {outcome:?}"
    );
    assert_eq!(
        sequencer.log_state().await.buffered,
        0,
        "an invalid batch must not be appended"
    );
}

// -- flush triggers (§5 step 6, §14.2) ------------------------------------

#[tokio::test]
async fn nothing_is_due_before_anything_arrives() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    assert_eq!(sequencer.flush_due().await, None);
}

#[tokio::test]
async fn a_single_update_at_the_byte_trigger_flushes_alone_because_the_trigger_is_not_a_cap() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let mut outbox = Outbox::new();
    // Comfortably past FLUSH_TRIGGER_BYTES (1 MiB) and comfortably under
    // the log quota (32 MiB by default): large enough that a real
    // deployment would see it as the "single large paste" case §5 step 6
    // describes, without this test's runtime being dominated by generating
    // data close to the quota.
    let big = outbox.edit_at_least(FLUSH_TRIGGER_BYTES + FLUSH_TRIGGER_BYTES / 4);
    assert!(
        big.len() < max_update_bytes(Arc::new(Configuration::default()).log_quota_bytes),
        "sanity: still under the hard cap"
    );
    let outcome = sequencer
        .ingest(1, "account:writer", "account:writer", 1, big)
        .await;
    assert!(
        matches!(outcome, Ingested::Accepted),
        "a single update under MAX_UPDATE_BYTES is accepted even past the trigger size, got {outcome:?}"
    );
    assert_eq!(
        sequencer.flush_due().await,
        Some(FlushReason::Bytes),
        "one oversized batch must flush alone, not wait for a second one"
    );
}

#[tokio::test]
async fn a_buffer_flushes_at_the_quiet_period_even_while_small() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let batch = Outbox::new().edit();
    sequencer
        .ingest(1, "account:writer", "account:writer", 1, batch)
        .await;
    assert_eq!(
        sequencer.flush_due().await,
        None,
        "not due the instant it arrives"
    );
    tokio::time::sleep(FLUSH_QUIET + Duration::from_millis(300)).await;
    assert_eq!(sequencer.flush_due().await, Some(FlushReason::Quiet));
}

#[tokio::test]
async fn a_buffer_flushes_at_the_max_age_even_while_someone_keeps_typing() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let mut outbox = Outbox::new();
    let mut client_seq = 0i64;
    client_seq += 1;
    let outcome = sequencer
        .ingest(
            1,
            "account:writer",
            "account:writer",
            client_seq,
            outbox.edit(),
        )
        .await;
    assert!(matches!(outcome, Ingested::Accepted));

    let start = tokio::time::Instant::now();
    // Keep the buffer's last arrival recent -- well under FLUSH_QUIET --
    // while its oldest batch ages past FLUSH_MAX_AGE. Only the max-age
    // branch, and not the quiet one, can explain a trigger shaped like
    // this: it proves the two conditions are independent, not that a long
    // enough wait always looks the same.
    loop {
        tokio::time::sleep(Duration::from_secs(4)).await;
        client_seq += 1;
        let outcome = sequencer
            .ingest(
                1,
                "account:writer",
                "account:writer",
                client_seq,
                outbox.edit(),
            )
            .await;
        assert!(matches!(outcome, Ingested::Accepted));
        if start.elapsed() >= FLUSH_MAX_AGE + Duration::from_millis(500) {
            break;
        }
    }
    assert_eq!(sequencer.flush_due().await, Some(FlushReason::MaxAge));
}

// -- acknowledgement (§5.1, §14.2) ----------------------------------------

#[tokio::test]
async fn a_flushed_row_acknowledges_each_peer_by_its_highest_client_seq_in_it() {
    let catalog = Arc::new(FakeCatalog::empty());
    let sequencer = bare_sequencer(catalog.clone());
    let mut writer_a = Outbox::new();
    let mut writer_b = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 10, writer_a.edit())
            .await,
        Ingested::Accepted
    ));
    assert!(matches!(
        sequencer
            .ingest(2, "account:b", "account:b", 40, writer_b.edit())
            .await,
        Ingested::Accepted
    ));
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 11, writer_a.edit())
            .await,
        Ingested::Accepted
    ));

    let written = sequencer
        .flush(FlushReason::MaxAge)
        .await
        .expect("flush succeeds against the fake catalog")
        .expect("something was buffered");
    assert_eq!(written, 1);

    let flushed = catalog.flushed.lock().unwrap();
    assert_eq!(flushed.len(), 1, "one flush writes one row");
    let batches = frame::decode(&flushed[0]).expect("the row this sequencer wrote is well framed");
    let targets = ack_targets(
        batches
            .iter()
            .map(|batch| (batch.peer_key.as_str(), batch.client_seq)),
    );

    // account:a sent client_seq 10 then 11 in the same row. A single ack
    // naming 11 is what "acknowledgements name a contiguous prefix" means:
    // the client does not need a separate ack for 10 to know it is covered.
    assert_eq!(targets.get("account:a"), Some(&11));
    assert_eq!(targets.get("account:b"), Some(&40));
    assert_eq!(
        targets.len(),
        2,
        "no peer appears that did not send anything"
    );
}

/// Every `doc-ack` queued for one subscriber, as the `upTo` each names.
/// Relayed updates and close frames are skipped: what a test in this
/// section is asking is which of a peer's `client_seq` values the server
/// has told it are safe.
fn drained_acks(rx: &mut Receiver) -> Vec<i64> {
    let mut acks = Vec::new();
    while let Ok(queued) = rx.try_recv() {
        let text = match queued.into_parts().0 {
            Outgoing::Text(text) => text,
            Outgoing::SharedText(text) => text.to_string(),
            Outgoing::Close(_) => continue,
        };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("a JSON frame");
        if frame["type"] == "doc-ack" {
            acks.push(frame["upTo"].as_i64().expect("upTo is a number"));
        }
    }
    acks
}

fn drained_frames(rx: &mut Receiver) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    while let Ok(queued) = rx.try_recv() {
        let text = match queued.into_parts().0 {
            Outgoing::Text(text) => text,
            Outgoing::SharedText(text) => text.to_string(),
            Outgoing::Close(_) => continue,
        };
        frames.push(serde_json::from_str(&text).expect("a JSON frame"));
    }
    frames
}

/// §5.1, the acknowledgement-safety invariant: a `doc-ack` must never name a
/// `client_seq` whose bytes are not in a committed row. The browser clears
/// its unacknowledged map up to `upTo` and reads that as "my work is safe",
/// so an ack for work that was never stored throws away the only copy.
///
/// A retryable refusal is where that could go wrong, and NOT for the reason
/// the code used to give: `Retryable` does not close the socket (only
/// `MAX_CONSECUTIVE_REFUSALS` of them in a row does, in `server::socket`),
/// so the peer goes on sending afterwards. What makes it safe is that a
/// refused batch is never merged into `head_vector` -- `ingest` returns
/// before that merge -- so the peer's NEXT batch starts from a vector the
/// head does not cover and comes back `Gap` instead of being appended past
/// the hole. Recovery is the client re-exporting from the vector the gap
/// reports, under a fresh `client_seq`, and that is the batch the row holds.
///
/// So this walks the whole path rather than the refusal alone: refused,
/// gapped, recovered, flushed, acknowledged. The bucket's clock is pinned
/// throughout, so nothing here turns on how long the loop takes to run.
#[tokio::test]
async fn a_refused_batch_is_never_acknowledged_and_its_peer_recovers_through_a_gap() {
    let config = Configuration::default();
    let allowance = config.session.updates_per_minute;
    let catalog = Arc::new(FakeCatalog::empty());
    let sequencer = bare_sequencer(catalog.clone());
    sequencer.set_rate_clock(Duration::from_secs(1));

    let (tx, mut rx) = Sender::channel(4096, 1 << 20, None, None);
    sequencer
        .join(1, Role::Editor, "account:writer", tx, None)
        .await
        .expect("join succeeds");

    let mut outbox = Outbox::new();
    for sent in 1..=allowance {
        assert!(
            matches!(
                sequencer
                    .ingest(1, "account:writer", "account:writer", sent, outbox.edit())
                    .await,
                Ingested::Accepted
            ),
            "update {sent} of an allowance of {allowance} was refused"
        );
    }

    let refused_seq = allowance + 1;
    let refused = sequencer
        .ingest(
            1,
            "account:writer",
            "account:writer",
            refused_seq,
            outbox.edit(),
        )
        .await;
    assert!(
        matches!(refused, Ingested::Retryable(_)),
        "the fixture failed to exhaust the allowance: {refused:?}"
    );

    // The clock moves so the bucket is full again. Otherwise the next
    // attempt would be refused for the rate too, and this case would prove
    // nothing about what happens to a peer that is allowed to keep sending
    // after a refusal -- which it is, because a retryable refusal does not
    // close the socket.
    sequencer.set_rate_clock(Duration::from_secs(62));

    let gapped_seq = allowance + 2;
    let gapped = sequencer
        .ingest(
            1,
            "account:writer",
            "account:writer",
            gapped_seq,
            outbox.edit(),
        )
        .await;
    let recover_from = match gapped {
        Ingested::Gap { vector } => {
            VersionVector::decode(&vector).expect("the gap reports a well-formed vector")
        }
        other => panic!(
            "a batch built on top of a refused one was not gapped, so the refused work could \
             be acknowledged through a hole in the log: {other:?}"
        ),
    };

    let recovered_seq = allowance + 3;
    let recovered = sequencer
        .ingest(
            1,
            "account:writer",
            "account:writer",
            recovered_seq,
            outbox.export_from(&recover_from),
        )
        .await;
    assert!(
        matches!(recovered, Ingested::Accepted),
        "re-exporting from the vector the gap reported is the documented recovery and it was \
         refused: {recovered:?}"
    );

    sequencer
        .flush(FlushReason::MaxAge)
        .await
        .expect("flush succeeds against the fake catalog")
        .expect("something was buffered");

    let flushed = catalog.flushed.lock().unwrap().clone();
    assert_eq!(flushed.len(), 1, "one flush writes one row");
    let batches = frame::decode(&flushed[0]).expect("the row this sequencer wrote is well framed");
    let in_the_row: Vec<i64> = batches.iter().map(|batch| batch.client_seq).collect();
    assert!(
        !in_the_row.contains(&refused_seq) && !in_the_row.contains(&gapped_seq),
        "a batch the sequencer refused reached the row"
    );
    assert!(
        in_the_row.contains(&recovered_seq),
        "the recovery batch is what carries the refused work, and it is not in the row"
    );

    assert_eq!(
        drained_acks(&mut rx),
        vec![recovered_seq],
        "the only acknowledgement names the recovered batch: the refused and gapped ones were \
         never stored under their own seq and must never be named"
    );

    // And the ack is honest about the work, not just about the number: the
    // row really does carry everything this outbox ever typed, including
    // what the refusal and the gap dropped, because the recovery batch
    // re-exported it.
    let replayed = LoroDoc::new();
    for batch in &batches {
        replayed
            .import(&batch.bytes)
            .expect("the row's batches import in order");
    }
    assert_eq!(
        replayed.get_text("t").to_string(),
        outbox.text(),
        "an acknowledgement named work the row does not actually hold"
    );
}

/// §5.1's ambiguous outcome, successful branch: the database response was
/// lost, the re-read proves the row committed, and only then does the flush
/// retire and acknowledge.
///
/// The branch where the sequence itself is unaccountable has its own case
/// (`a_fenced_flush_stops_ingest_and_closes_every_open_socket`). This is the
/// side where being wrong is silent: acknowledging on a lost response
/// without proving the row is this flush's own tells a client its work is
/// safe when it may be nowhere. `expected + 1` alone does not prove that --
/// a semantic command's row lands the same way -- so the second half of
/// this case is the same lost response over a row that exists and is
/// somebody else's, which must acknowledge nothing.
///
/// The matching row is seeded by the fake itself (`lost_flush_committed`),
/// from the bytes and vector the flush handed it, rather than recomputed
/// here -- a hand-written vector would be testing this test's arithmetic
/// instead of the sequencer's comparison.
#[tokio::test]
async fn a_flush_whose_response_was_lost_acknowledges_only_the_row_it_proves_is_its_own() {
    let catalog = Arc::new(FakeCatalog {
        fail_next_flush: std::sync::atomic::AtomicBool::new(true),
        lost_flush_committed: true,
        // `expected` for a fresh `bare_sequencer` is 0, so `expected + 1` is
        // what a log holding this flush's own row reads back as.
        sequence_on_ambiguity: AtomicI64::new(1),
        ..FakeCatalog::empty()
    });
    let sequencer = bare_sequencer(catalog.clone());

    let (tx, mut rx) = Sender::channel(64, 1 << 20, None, None);
    sequencer
        .join(1, Role::Editor, "account:writer", tx, None)
        .await
        .expect("join succeeds");

    let mut outbox = Outbox::new();
    for sent in [7, 8] {
        assert!(matches!(
            sequencer
                .ingest(1, "account:writer", "account:writer", sent, outbox.edit())
                .await,
            Ingested::Accepted
        ));
    }

    let written = sequencer
        .flush(FlushReason::MaxAge)
        .await
        .expect("the row is really there, so the flush resolves rather than failing")
        .expect("something was buffered");
    assert_eq!(written, 1, "the flush claims the row the re-read found");
    assert!(
        sequencer.fenced().await.is_none(),
        "this server's own write landed; nothing about it says another writer holds the log"
    );
    assert_eq!(
        catalog.flushed.lock().unwrap().len(),
        1,
        "the lost response must not become a second copy of the same row"
    );
    assert_eq!(
        sequencer.log_state().await.buffered,
        0,
        "a resolved flush retires its batches"
    );
    assert_eq!(
        drained_acks(&mut rx),
        vec![8],
        "one acknowledgement, naming the peer's highest seq in the row that was proved to exist"
    );

    // Now the same lost response over a row that is not this flush's. The
    // log is at `expected + 1` again, so the sequence check says nothing;
    // only the row's vector distinguishes a semantic command's write (or
    // another writer's) from this one's. Nothing may be acknowledged here:
    // these batches are in no row anywhere.
    catalog.rows.lock().unwrap().push(postgres::LogRow {
        update_sequence: 2,
        update_bytes: b"somebody else's row".to_vec(),
        vector: b"a vector this flush never wrote".to_vec(),
        created_at: time::OffsetDateTime::now_utc(),
    });
    catalog.sequence_on_ambiguity.store(2, Ordering::SeqCst);
    catalog.fail_next_flush.store(true, Ordering::SeqCst);
    assert!(matches!(
        sequencer
            .ingest(1, "account:writer", "account:writer", 9, outbox.edit())
            .await,
        Ingested::Accepted
    ));

    let error = sequencer
        .flush(FlushReason::MaxAge)
        .await
        .expect_err("the row at the next sequence is somebody else's");
    assert!(
        matches!(error, SequencerError::Fenced(_)),
        "expected Fenced, got {error:?}"
    );
    assert!(
        drained_acks(&mut rx).is_empty(),
        "work that is in no row of this log was acknowledged as durable"
    );
}

#[tokio::test]
async fn a_join_reports_head_and_durable_vectors_separately_while_a_flush_is_pending() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let (tx, _rx) = Sender::channel(64, 1 << 20, None, None);
    sequencer
        .join(1, Role::Editor, "account:writer", tx, None)
        .await
        .expect("join succeeds");

    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:writer", "account:writer", 1, outbox.edit())
            .await,
        Ingested::Accepted
    ));
    let joined = sequencer
        .join(2, Role::Editor, "account:reconnected", subscriber(), None)
        .await
        .expect("reconnect join succeeds");

    assert_eq!(joined.vector, outbox.doc.oplog_vv().encode());
    assert_eq!(
        joined.durable_vector,
        VersionVector::default().encode(),
        "the join head includes the buffered update while durableVector stays at the log row"
    );
}

#[tokio::test]
async fn a_successful_flush_notifies_every_editor_but_not_readers_of_the_committed_vector() {
    let catalog = Arc::new(FakeCatalog::empty());
    let sequencer = bare_sequencer(catalog.clone());
    let (writer_tx, mut writer_rx) = Sender::channel(64, 1 << 20, None, None);
    let (peer_tx, mut peer_rx) = Sender::channel(64, 1 << 20, None, None);
    let (reader_tx, mut reader_rx) = Sender::channel(64, 1 << 20, None, None);
    sequencer
        .join(1, Role::Editor, "account:writer", writer_tx, None)
        .await
        .expect("writer join succeeds");
    sequencer
        .join(2, Role::Editor, "account:peer", peer_tx, None)
        .await
        .expect("peer join succeeds");
    sequencer
        .join(3, Role::Reader, "account:reader", reader_tx, None)
        .await
        .expect("reader join succeeds");
    let mut outbox = Outbox::new();
    let batch = outbox.edit();
    assert!(matches!(
        sequencer
            .ingest(1, "account:writer", "account:writer", 1, batch)
            .await,
        Ingested::Accepted
    ));
    // The author reconnects after its original socket and peer key disappear.
    // Durable coverage is document-wide, while the old doc-ack remains tied
    // to the old transmission identity.
    assert!(!sequencer.unsubscribe(1).await);
    let (reconnected_tx, mut reconnected_rx) = Sender::channel(64, 1 << 20, None, None);
    sequencer
        .join(
            4,
            Role::Editor,
            "account:reconnected-peer-key",
            reconnected_tx,
            None,
        )
        .await
        .expect("reconnected editor join succeeds");
    // Ingest relays to the other editor and announces a source change to the
    // reader. The durable frame is emitted only after the row commits.
    let _ = drained_frames(&mut peer_rx);
    let _ = drained_frames(&mut reconnected_rx);
    let _ = drained_frames(&mut reader_rx);
    assert!(drained_frames(&mut writer_rx).is_empty());

    sequencer
        .flush(FlushReason::Barrier)
        .await
        .expect("flush succeeds")
        .expect("the buffered batch is written");

    let writer_frames = drained_frames(&mut writer_rx);
    let peer_frames = drained_frames(&mut peer_rx);
    let reconnected_frames = drained_frames(&mut reconnected_rx);
    let reader_frames = drained_frames(&mut reader_rx);
    let reconnected_durable = reconnected_frames
        .iter()
        .find(|frame| frame["type"] == "doc-durable")
        .expect("the reconnected author receives the durable notification");
    let peer_durable = peer_frames
        .iter()
        .find(|frame| frame["type"] == "doc-durable")
        .expect("every editor receives the durable notification");
    assert_eq!(reconnected_durable["vector"], peer_durable["vector"]);
    assert_eq!(
        reconnected_durable["vector"],
        serde_json::Value::String(
            base64::engine::general_purpose::STANDARD.encode(outbox.doc.oplog_vv().encode())
        )
    );
    assert!(
        reader_frames
            .iter()
            .all(|frame| frame["type"] != "doc-durable"),
        "durable log vectors are editor state, not reader notifications"
    );
    assert!(
        writer_frames
            .iter()
            .all(|frame| frame["type"] != "doc-durable"),
        "the old author socket is gone before the commit"
    );
    assert!(
        reconnected_frames
            .iter()
            .all(|frame| frame["type"] != "doc-ack"),
        "the reconnected peer key does not receive the old author's transmission acknowledgement"
    );
    assert!(
        peer_frames.iter().all(|frame| frame["type"] != "doc-ack"),
        "an unrelated editor does not receive the author's transmission acknowledgement"
    );
}

#[tokio::test]
async fn a_failed_flush_does_not_advance_or_broadcast_durable_coverage() {
    let catalog = Arc::new(FakeCatalog {
        fail_next_flush: std::sync::atomic::AtomicBool::new(true),
        ..FakeCatalog::empty()
    });
    let sequencer = bare_sequencer(catalog);
    let (tx, mut rx) = Sender::channel(64, 1 << 20, None, None);
    sequencer
        .join(1, Role::Editor, "account:writer", tx, None)
        .await
        .expect("join succeeds");
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:writer", "account:writer", 1, outbox.edit())
            .await,
        Ingested::Accepted
    ));
    assert!(drained_frames(&mut rx).is_empty());

    assert!(sequencer.flush(FlushReason::Barrier).await.is_err());
    assert!(
        drained_frames(&mut rx)
            .iter()
            .all(|frame| frame["type"] != "doc-durable"),
        "a failed write cannot claim a durable vector"
    );
    let joined = sequencer
        .join(2, Role::Editor, "account:reconnected", subscriber(), None)
        .await
        .expect("join succeeds after the failed flush");
    assert_eq!(joined.durable_vector, VersionVector::default().encode());
}

// -- join (§6.2, §14.2) ----------------------------------------------------

/// A sequencer with a base and two rows: `base_vv` covers only the base,
/// `v1` covers the base plus row 1, `v2` covers everything. Building this
/// costs no I/O; `log_coverage`, `log_rows` and `read_base` all answer from
/// the fixture's own fields.
struct JoinFixture {
    sequencer: Sequencer,
    row1: Vec<u8>,
    row2: Vec<u8>,
    base_vv: VersionVector,
    v1: VersionVector,
    v2: VersionVector,
}

fn join_fixture() -> JoinFixture {
    let doc = LoroDoc::new();
    let text = doc.get_text("t");
    text.insert_utf16(0, "base").unwrap();
    doc.commit();
    let base_vv = doc.oplog_vv();
    let text = doc.get_text("t");
    text.insert_utf16(text.len_utf16(), "row1").unwrap();
    doc.commit();
    let v1 = doc.oplog_vv();
    let text = doc.get_text("t");
    text.insert_utf16(text.len_utf16(), "row2").unwrap();
    doc.commit();
    let v2 = doc.oplog_vv();

    let row1 = b"row-1-payload".to_vec();
    let row2 = b"row-2-payload".to_vec();
    let row = |sequence: i64, vector: &VersionVector, payload: &[u8]| postgres::LogRow {
        update_sequence: sequence,
        update_bytes: frame::encode(&[frame::Batch {
            peer_key: "account:writer".into(),
            client_seq: sequence,
            bytes: payload.to_vec(),
        }]),
        vector: vector.encode(),
        created_at: time::OffsetDateTime::now_utc(),
    };
    let coverage = |sequence: i64, vector: &VersionVector| postgres::RowCoverage {
        update_sequence: sequence,
        vector: vector.encode(),
        bytes: 32,
    };

    let catalog = Arc::new(FakeCatalog {
        coverage: Mutex::new(vec![coverage(1, &v1), coverage(2, &v2)]),
        rows: Mutex::new(vec![row(1, &v1, &row1), row(2, &v2, &row2)]),
        base: Mutex::new(Some(b"base-blob".to_vec())),
        flushed: Mutex::new(Vec::new()),
        next_sequence: AtomicI64::new(3),
        gate: None,
        ..FakeCatalog::empty()
    });
    let sequencer = Sequencer::from_parts(
        Uuid::new_v4(),
        "join-fixture".to_string(),
        Uuid::nil(),
        catalog,
        blobs(),
        Arc::new(Configuration::default()),
        Budget::new(u64::MAX / 2, DEFAULT_EXPANSION),
        generous_pending(),
        "deployment.peer.test".to_string(),
        crate::log::ledger::StorageLedger::new(),
        3,
        v2.clone(),
        0,
        16,
        2,
        base_vv.clone(),
        true,
        Arc::new(std::sync::OnceLock::new()),
    );
    JoinFixture {
        sequencer,
        row1,
        row2,
        base_vv,
        v1,
        v2,
    }
}

fn subscriber() -> Sender {
    Sender::channel(64, 1 << 20, None, None).0
}

#[tokio::test]
async fn a_join_from_an_empty_vector_gets_the_base_and_every_row() {
    let fixture = join_fixture();
    let joined = fixture
        .sequencer
        .join(1, Role::Editor, "account:reader", subscriber(), None)
        .await
        .expect("join succeeds");
    assert_eq!(joined.vector, fixture.v2.encode());
    assert_eq!(joined.base.as_deref(), Some(b"base-blob".as_slice()));
    assert!(
        !joined.from_rows,
        "the base was not covered, so this is doc-state plus doc-rows, not doc-rows alone (§6.2 step 5)"
    );
    assert_eq!(joined.batches, vec![fixture.row1, fixture.row2]);
    assert!(
        !fixture.sequencer.is_warm().await,
        "a join never builds a cache (§6.2)"
    );
}

#[tokio::test]
async fn a_join_covering_only_the_base_gets_every_row_as_doc_rows() {
    let fixture = join_fixture();
    let client = fixture.base_vv.encode();
    let joined = fixture
        .sequencer
        .join(
            1,
            Role::Editor,
            "account:reader",
            subscriber(),
            Some(&client),
        )
        .await
        .expect("join succeeds");
    assert_eq!(
        joined.base, None,
        "the client already has the base, so it is not sent again"
    );
    assert!(joined.from_rows);
    assert_eq!(joined.batches, vec![fixture.row1, fixture.row2]);
    assert!(!fixture.sequencer.is_warm().await);
}

#[tokio::test]
async fn a_join_missing_only_the_newest_row_gets_just_that_row() {
    let fixture = join_fixture();
    let client = fixture.v1.encode();
    let joined = fixture
        .sequencer
        .join(
            1,
            Role::Editor,
            "account:reader",
            subscriber(),
            Some(&client),
        )
        .await
        .expect("join succeeds");
    assert_eq!(joined.base, None);
    assert!(joined.from_rows);
    assert_eq!(
        joined.batches,
        vec![fixture.row2],
        "row 1 is already covered and must not be resent"
    );
    assert!(!fixture.sequencer.is_warm().await);
}

#[tokio::test]
async fn no_batch_is_lost_between_a_join_reply_and_the_first_relay_under_concurrent_ingest() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let catalog = Arc::new(FakeCatalog {
        gate: Some((entered.clone(), release.clone())),
        ..FakeCatalog::empty()
    });
    let sequencer = Arc::new(bare_sequencer(catalog));

    let join = {
        let sequencer = sequencer.clone();
        tokio::spawn(async move {
            let (tx, rx) = Sender::channel(64, 1 << 20, None, None);
            let joined = sequencer
                .join(1, Role::Editor, "account:reader", tx, None)
                .await
                .expect("join succeeds");
            (joined, rx)
        })
    };

    // The join is now inside its coverage read (§6.2 step 2), before it has
    // registered a subscriber or taken the lock. This is exactly the
    // window the spec calls out: "a row written meanwhile is relayed to
    // this subscriber anyway, because registration and the reply happen
    // together."
    entered.notified().await;
    let batch = Outbox::new().edit();
    let outcome = sequencer
        .ingest(2, "account:writer", "account:writer", 1, batch.clone())
        .await;
    assert!(matches!(outcome, Ingested::Accepted));
    release.notify_one();

    let (joined, mut rx) = join.await.expect("the join task does not panic");
    let in_reply = joined.batches.contains(&batch);
    let in_relay = !in_reply && rx.try_recv().is_ok();
    assert!(
        in_reply ^ in_relay,
        "expected exactly one delivery of the batch, in_reply={in_reply} in_relay={in_relay}"
    );
}

// -- compaction accounting (§8.4, §9.1, §9.2) -----------------------------

/// `note_compacted` must trust the `row_bytes` its caller reports rather
/// than assuming compaction leaves nothing behind. Typing continues while
/// compaction runs (§8.4), so rows can land above `through` between the
/// flush that establishes it and the base's activation; `activate_log_base`
/// recomputes `documents.uncompacted_update_bytes` from what survives, and
/// `storage::worker::compact` reads that back and passes it here. Zeroing
/// it unconditionally would under-report `log_bytes()` from that moment,
/// under-enforcing the §9.1 log quota and under-reserving the §9.2 budget
/// for every later build.
#[tokio::test]
async fn note_compacted_uses_the_row_bytes_it_is_given_rather_than_assuming_zero() {
    let sequencer = bare_sequencer(Arc::new(FakeCatalog::empty()));
    let mut gate = sequencer.compaction_gate().await;
    gate.note_compacted(5, &VersionVector::default().encode(), 1000, 250, 2);
    drop(gate);
    let state = sequencer.log_state().await;
    assert_eq!(
        state.log_bytes, 1250,
        "base_bytes (1000) plus the row_bytes the caller reports (250), never zero"
    );
}

/// Two rows, no base yet, and an optional gate on the coverage read: the
/// state a document is in the first time compaction runs on it.
///
/// No base is the case that matters. `join` treats a document with none as
/// "the client covers the base by definition", so if a base is activated
/// while a join is part-way through, the join answers from a `has_base`
/// that is still false -- no base in the reply -- while the rows it was
/// about to send have already been deleted.
fn compaction_fixture(
    gate: Option<(Arc<Notify>, Arc<Notify>)>,
) -> (
    Arc<Sequencer>,
    Arc<FakeCatalog>,
    VersionVector,
    Vec<u8>,
    Vec<u8>,
) {
    let doc = LoroDoc::new();
    let text = doc.get_text("t");
    text.insert_utf16(0, "one").unwrap();
    doc.commit();
    let v1 = doc.oplog_vv();
    let text = doc.get_text("t");
    text.insert_utf16(text.len_utf16(), "two").unwrap();
    doc.commit();
    let v2 = doc.oplog_vv();

    let row1 = b"row-1-payload".to_vec();
    let row2 = b"row-2-payload".to_vec();
    let row = |sequence: i64, vector: &VersionVector, payload: &[u8]| postgres::LogRow {
        update_sequence: sequence,
        update_bytes: frame::encode(&[frame::Batch {
            peer_key: "account:writer".into(),
            client_seq: sequence,
            bytes: payload.to_vec(),
        }]),
        vector: vector.encode(),
        created_at: time::OffsetDateTime::now_utc(),
    };
    let coverage = |sequence: i64, vector: &VersionVector| postgres::RowCoverage {
        update_sequence: sequence,
        vector: vector.encode(),
        bytes: 32,
    };
    let catalog = Arc::new(FakeCatalog {
        coverage: Mutex::new(vec![coverage(1, &v1), coverage(2, &v2)]),
        rows: Mutex::new(vec![row(1, &v1, &row1), row(2, &v2, &row2)]),
        gate,
        next_sequence: AtomicI64::new(3),
        ..FakeCatalog::empty()
    });
    let sequencer = Arc::new(Sequencer::from_parts(
        Uuid::new_v4(),
        "compaction-fixture".to_string(),
        Uuid::nil(),
        catalog.clone(),
        blobs(),
        Arc::new(Configuration::default()),
        Budget::new(u64::MAX / 2, DEFAULT_EXPANSION),
        generous_pending(),
        "deployment.peer.test".to_string(),
        crate::log::ledger::StorageLedger::new(),
        3,
        v2.clone(),
        0,
        32,
        2,
        VersionVector::default(),
        false,
        Arc::new(std::sync::OnceLock::new()),
    ));
    (sequencer, catalog, v2, row1, row2)
}

/// What compaction's catalogue transaction does, in the order it does it:
/// the base becomes readable, the rows it covers go, and the coverage list
/// they were on goes with them (§8.4 step 5).
fn activate_base_in_storage(catalog: &FakeCatalog) {
    *catalog.base.lock().unwrap() = Some(b"compacted-base".to_vec());
    catalog.rows.lock().unwrap().clear();
    catalog.coverage.lock().unwrap().clear();
}

// -- compaction against a live document (§8.4 step 5) ---------------------

/// §8.4 step 5 cannot land in the middle of a join.
///
/// Activating a base is two writes -- the catalogue's transaction and the
/// sequencer's own copy of what the base covers -- and between them the two
/// disagree. `join` reads the row coverage, then that copy, then the rows,
/// so a compaction that slips in between leaves it holding a coverage list
/// whose rows are gone and a `has_base` that is still false. This pins the
/// gate that makes the two writes one event: the activation waits for the
/// join, and the join answers entirely from the state it started in.
#[tokio::test]
async fn a_base_activation_waits_for_a_join_that_is_already_reading_the_log() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (sequencer, catalog, v2, row1, row2) =
        compaction_fixture(Some((entered.clone(), release.clone())));

    let joining = {
        let sequencer = sequencer.clone();
        tokio::spawn(async move {
            sequencer
                .join(1, Role::Editor, "account:reader", subscriber(), None)
                .await
        })
    };
    // The join has answered its coverage read and is holding the
    // transaction gate: the exact window the report is about.
    entered.notified().await;

    let compacting = {
        let sequencer = sequencer.clone();
        let catalog = catalog.clone();
        let v2 = v2.clone();
        tokio::spawn(async move {
            let mut gate = sequencer.compaction_gate().await;
            activate_base_in_storage(&catalog);
            gate.note_compacted(2, &v2.encode(), 4096, 0, 0);
        })
    };

    // Every chance to get in. Without the gate the task above is three
    // synchronous statements behind one lock acquisition and would be done
    // by now.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(
        !compacting.is_finished(),
        "the activation ran while a join was mid-flight"
    );
    assert_eq!(
        catalog.rows.lock().unwrap().len(),
        2,
        "no row may be deleted while a join is between its coverage read and its row read"
    );

    release.notify_one();
    let joined = joining
        .await
        .expect("the join task did not panic")
        .expect("join succeeds");
    compacting.await.expect("the compaction task did not panic");

    // The pre-compaction log, whole: every row, and no base, because there
    // was none when this join started.
    assert_eq!(joined.vector, v2.encode());
    assert!(joined.base.is_none());
    assert_eq!(
        joined.batches,
        vec![row1, row2],
        "the reply carries the bytes for every operation its vector claims"
    );
}

/// The negative control for the test above: the same interleaving with
/// nothing serialising it, which is what compaction did before it took the
/// gate.
///
/// Nothing here touches the sequencer at all -- only storage changes,
/// exactly as the catalogue transaction changes it -- and that alone is
/// enough. The reply names a head vector covering both rows and carries
/// neither: no base, because the sequencer still believes there is none,
/// and no rows, because they have been deleted. §6.2 step 3 has the client
/// trust that vector as the baseline for its own catch-up export, so it
/// would believe itself synced and never ask again.
#[tokio::test]
async fn a_base_activated_with_nothing_serialising_it_answers_a_join_with_no_bytes_at_all() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (sequencer, catalog, v2, _row1, _row2) =
        compaction_fixture(Some((entered.clone(), release.clone())));

    let joining = {
        let sequencer = sequencer.clone();
        tokio::spawn(async move {
            sequencer
                .join(1, Role::Editor, "account:reader", subscriber(), None)
                .await
        })
    };
    entered.notified().await;
    activate_base_in_storage(&catalog);
    release.notify_one();

    let joined = joining
        .await
        .expect("the join task did not panic")
        .expect("join succeeds");
    assert_eq!(joined.vector, v2.encode());
    assert!(
        joined.base.is_none() && joined.batches.is_empty(),
        "this is the loss the gate prevents; if this reply is whole, the interleaving \
         being modelled here is no longer the one compaction performs"
    );
}

// -- the writer lease, lost (§10) ------------------------------------------

/// §10: "a lost writer lease must refuse ingest and commands, close
/// sockets; a new owner loads sequencers from the log." A `Fenced` error
/// surfaces this precisely when the ambiguous-outcome re-read of §5.1 finds
/// the log at a sequence this process cannot account for: not its own
/// expected value (nothing was written; retry), and not exactly one past it
/// with a matching row (this process's own write landed after all) -- some
/// other value, which only another writer holding the log could produce.
#[tokio::test]
async fn a_fenced_flush_stops_ingest_and_closes_every_open_socket() {
    let catalog = Arc::new(FakeCatalog {
        fail_next_flush: std::sync::atomic::AtomicBool::new(true),
        // `expected` for a fresh `bare_sequencer` is 0 (next_sequence is 1).
        // 99 is neither `expected` (nothing committed; retry) nor
        // `expected + 1` (this process's own write landed); it is what a
        // completely different writer's progress looks like.
        sequence_on_ambiguity: AtomicI64::new(99),
        ..FakeCatalog::empty()
    });
    let sequencer = Arc::new(bare_sequencer(catalog));

    let (tx, mut rx) = Sender::channel(64, 1 << 20, None, None);
    sequencer
        .join(1, Role::Editor, "account:writer", tx, None)
        .await
        .expect("join succeeds");

    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:writer", "account:writer", 1, outbox.edit())
            .await,
        Ingested::Accepted
    ));

    let error = sequencer
        .flush(FlushReason::Barrier)
        .await
        .expect_err("the catalog reports a sequence this process cannot account for");
    assert!(
        matches!(error, SequencerError::Fenced(_)),
        "expected Fenced, got {error:?}"
    );

    assert!(
        sequencer.fenced().await.is_some(),
        "the sequencer marks itself fenced so a later caller does not have to \
         rediscover this from another failed flush"
    );

    // Ingest refuses from here on -- and does so without any further trip
    // to the catalog, which `FakeCatalog::flush_log_row` succeeding again
    // on a second call would otherwise mask.
    assert!(
        matches!(
            sequencer
                .ingest(1, "account:writer", "account:writer", 2, outbox.edit())
                .await,
            Ingested::Refused(_)
        ),
        "a fenced sequencer refuses ingest rather than buffering work it can never make durable"
    );

    // The socket that was open when the fence was discovered was told to
    // close, not left believing itself synced against a process that can no
    // longer write for it.
    let queued = rx
        .try_recv()
        .expect("a close frame was queued for the open subscriber");
    assert!(
        matches!(queued.into_parts().0, Outgoing::Close(_)),
        "the subscriber's socket must be closed, not merely told about an error"
    );
}

/// Once compaction fencing is observed, every state-reading entry point must
/// refuse the stale in-memory view. In particular, `join` must check before
/// registering its subscriber; a reconnect must go to a new owner.
#[tokio::test]
async fn every_sequencer_read_refuses_after_compaction_fence() {
    let (sequencer, catalog, _, _, _) = compaction_fixture(None);
    {
        let mut gate = sequencer.compaction_gate().await;
        activate_base_in_storage(&catalog);
        gate.fence("activation committed but reconciliation failed".into());
    }

    assert!(sequencer.projection_if_warm().await.is_none());
    assert!(matches!(
        sequencer.projection().await,
        Err(SequencerError::Fenced(_))
    ));
    assert!(matches!(
        sequencer.with_fork_at(&Frontiers::default(), |_| ()).await,
        Err(SequencerError::Fenced(_))
    ));
    assert!(matches!(
        sequencer.with_head(|_| ()).await,
        Err(SequencerError::Fenced(_))
    ));
    assert!(matches!(
        sequencer
            .snapshot_at_log_vector(crate::log::sequencer::SnapshotMode::Full)
            .await,
        Err(SequencerError::Fenced(_))
    ));
    assert!(matches!(
        sequencer
            .join(999, Role::Editor, "fenced-reader", subscriber(), None)
            .await,
        Err(SequencerError::Fenced(_))
    ));
    assert_eq!(sequencer.log_state().await.subscribers, 0);

    // `projection_if_warm` must also reject an actually warm cache; using a
    // cold sequencer alone would make `None` ambiguous with its normal result.
    let warm = bare_sequencer(Arc::new(FakeCatalog::empty()));
    warm.projection().await.expect("empty fake document builds");
    {
        let mut gate = warm.compaction_gate().await;
        gate.fence("warm cache is no longer authoritative".into());
    }
    assert!(warm.projection_if_warm().await.is_none());
    assert!(matches!(
        warm.projection().await,
        Err(SequencerError::Fenced(_))
    ));
}

/// §10, "subscriber too slow: closed; reconnects and reconciles", and §4.5,
/// "a subscriber whose queue overflows is closed and reconnects".
///
/// Removing the subscription alone is not that. The socket stays open, the
/// reader loop goes on answering pings, and the person at the keyboard sees
/// a connected editor that has quietly stopped receiving other people's
/// text and its own acknowledgements. Nothing on the socket side notices:
/// §12 deleted the per-socket housekeeping tick that used to re-check room
/// membership and cut the transport, so the close has to come from here.
#[tokio::test]
async fn a_subscriber_too_slow_for_relay_is_closed_rather_than_silently_detached() {
    let sequencer = Arc::new(bare_sequencer(Arc::new(FakeCatalog::empty())));

    // Wide enough to take the close frame, far too narrow for the relayed
    // update: overflow is the point of the fixture, and a close that could
    // not fit either would prove nothing about what the code chose to send.
    let (tx, mut rx) = Sender::channel(64, 512, None, None);
    sequencer
        .join(2, Role::Editor, "account:slow", tx, None)
        .await
        .expect("join succeeds");

    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(
                1,
                "account:writer",
                "account:writer",
                1,
                outbox.edit_at_least(4096)
            )
            .await,
        Ingested::Accepted
    ));

    assert_eq!(
        sequencer.subscribers().await,
        0,
        "a subscriber that could not take the relay is dropped"
    );
    let queued = rx
        .try_recv()
        .expect("the dropped subscriber was told its socket is going away");
    assert!(
        matches!(queued.into_parts().0, Outgoing::Close(_)),
        "an unsubscribed socket that stays open is an editor that has stopped \
         collaborating with nothing to say so"
    );
}

/// The close must not be admitted by the same budget whose exhaustion is the
/// reason it is being sent. Four bytes of queue admits neither the relayed
/// update nor the close reason, so an ordinary `try_send` of the close is
/// refused too: the subscriber would be unsubscribed and its socket left open
/// answering pings forever, with no relay and no acknowledgements. The more
/// overloaded a subscriber was, the less likely it would be to be told to go.
#[tokio::test]
async fn a_subscriber_too_slow_even_for_the_close_frame_is_still_closed() {
    let sequencer = Arc::new(bare_sequencer(Arc::new(FakeCatalog::empty())));

    let (tx, mut rx) = Sender::channel(1, 4, None, None);
    sequencer
        .join(2, Role::Editor, "account:slow", tx, None)
        .await
        .expect("join succeeds");

    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(
                1,
                "account:writer",
                "account:writer",
                1,
                outbox.edit_at_least(4096)
            )
            .await,
        Ingested::Accepted
    ));

    assert_eq!(
        sequencer.subscribers().await,
        0,
        "a subscriber that could not take the relay is dropped"
    );
    let queued = rx
        .try_recv()
        .expect("the close reaches the queue on the slot reserved for it");
    assert!(
        matches!(queued.into_parts().0, Outgoing::Close(_)),
        "a close refused by the budget leaves a socket that never learns it \
         has stopped collaborating"
    );
}

/// §4.5 for the case §9.2 says is the steady state: an evicted, reader-only
/// document. The notification must go out even though no cache is resident to
/// compute a digest from, and building one here is exactly what §5 and §4.3
/// forbid on the typing path -- so the frame carries a null digest and the
/// reader refetches, which is what it does with the digest anyway.
#[tokio::test]
async fn a_reader_on_a_cold_document_is_told_the_source_changed_without_a_digest() {
    let sequencer = Arc::new(bare_sequencer(Arc::new(FakeCatalog::empty())));

    let (tx, mut rx) = Sender::channel(64, 64 * 1024, None, None);
    sequencer
        .join(2, Role::Reader, "account:reader", tx, None)
        .await
        .expect("join succeeds");

    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(
                1,
                "account:writer",
                "account:writer",
                1,
                outbox.edit_at_least(64)
            )
            .await,
        Ingested::Accepted
    ));

    let queued = rx
        .try_recv()
        .expect("a reader on a cold document is still told the source changed");
    let Outgoing::SharedText(text) = queued.into_parts().0 else {
        unreachable!("source-changed is a shared text frame");
    };
    let frame: serde_json::Value = serde_json::from_str(&text).expect("source-changed is JSON");
    assert_eq!(frame["type"], "source-changed");
    assert!(
        frame["digest"].is_null(),
        "a digest would mean a LoroDoc was built on the ingest path: {frame}"
    );
    assert!(
        rx.try_recv().is_err(),
        "the reader gets the one notification and no relay of its own"
    );
    assert!(
        !sequencer.log_state().await.warm,
        "notifying a reader must not warm the document"
    );
}

// -- SPEC-server-is-a-log Priority 3: the build-peak reservation ---------

/// A sequencer admitted with no rows and no base actually to import (so
/// `build` finishes at once), but told -- through `from_parts`' `base_bytes`
/// -- that its log weighs `log_bytes`, so the two reservations `build` takes
/// are sized as they would be for a real log of that weight.
fn sequencer_with_logged_weight(log_bytes: u64, budget: Arc<Budget>) -> Sequencer {
    Sequencer::from_parts(
        Uuid::new_v4(),
        "test-doc".to_string(),
        Uuid::nil(),
        Arc::new(FakeCatalog::empty()),
        blobs(),
        Arc::new(Configuration::default()),
        budget,
        generous_pending(),
        "deployment.peer.test".to_string(),
        crate::log::ledger::StorageLedger::new(),
        1,
        VersionVector::default(),
        log_bytes,
        0,
        0,
        VersionVector::default(),
        // `has_base: false` keeps `build` from calling `read_base`, which
        // this fake has nothing configured for; `log_bytes` above is what
        // the estimate is computed from regardless of whether there is
        // really a base to read.
        false,
        Arc::new(std::sync::OnceLock::new()),
    )
}

/// Without the build-peak reservation, a budget with room for the resident
/// estimate and a little to spare was enough to build. With it, `build`
/// also needs `BUILD_TRANSIENT_EXPANSION` more for the duration of the
/// import, and a budget that has not got that much free answers `busy`
/// instead of building -- which is the point: the transient cost of
/// producing the document is now accounted for, not just the cost of
/// holding it afterward.
#[tokio::test]
async fn a_build_needs_more_than_the_resident_estimate_while_it_runs() {
    let log_bytes = 1_000_000u64;
    let resident = crate::log::budget::estimate(log_bytes, DEFAULT_EXPANSION);
    // Room for the resident reservation plus a small margin, but nowhere
    // near enough for the additional transient reservation a build now
    // takes on top of it.
    let budget = Budget::new(resident + 8, DEFAULT_EXPANSION);
    let sequencer = sequencer_with_logged_weight(log_bytes, budget);

    let error = sequencer
        .projection()
        .await
        .expect_err("the budget has room for the resident estimate but not the build's peak");
    assert!(
        matches!(error, SequencerError::Busy),
        "expected Busy, got {error:?}"
    );
}

/// With enough room for the whole peak, the build succeeds, and once it has
/// finished the transient reservation is gone: what the budget holds
/// afterward is exactly the resident estimate, not the peak the build ran
/// at while it was producing it.
#[tokio::test]
async fn the_transient_reservation_is_released_once_the_entry_is_cached() {
    let log_bytes = 1_000_000u64;
    let resident = crate::log::budget::estimate(log_bytes, DEFAULT_EXPANSION);
    let transient =
        crate::log::budget::estimate(log_bytes, crate::log::budget::BUILD_TRANSIENT_EXPANSION);
    let budget = Budget::new(resident + transient, DEFAULT_EXPANSION);
    let sequencer = sequencer_with_logged_weight(log_bytes, budget.clone());

    sequencer
        .projection()
        .await
        .expect("room for the full build peak");

    assert_eq!(
        budget.used(),
        resident,
        "only the resident reservation should still be held once the cache exists"
    );
}

// -- SPEC-frugal §2: the deployment-wide pending-source bound -------------
//
// These are about the bound itself, so none of them uses `bare_sequencer`'s
// effectively-infinite pending budget: each builds a small one and, where the
// claim is deployment-wide, shares one budget between two sequencers. What
// makes that a real test rather than a rename is the sharing -- a per-document
// cap would pass every case below except the ones that add a second document.

use crate::log::pending::{scratch_for, PendingBudget};
use crate::log::sequencer::{max_row_bytes, BUFFER_CEILING_BYTES};

/// The reason a retryable refusal names, or a panic saying what came back
/// instead. Written once because nearly every case here asserts on it, and a
/// case that asserted only "not accepted" would pass if the update had been
/// refused for the rate bucket or the log quota rather than for the bound
/// under test.
fn refusal_reason(outcome: &Ingested) -> &'static str {
    match outcome {
        Ingested::Retryable(retry) => retry.reason,
        other => panic!("expected a retryable refusal, got {other:?}"),
    }
}

/// Everything this outbox holds that the server's head does not cover.
///
/// What a real client sends after a refusal, and the only correct thing to
/// send: a refused batch is not merged into the head, so the outbox's own
/// "since I last exported" bookmark has run ahead of what the server can
/// place, and anything exported from it comes back `Gap`. Recovery is an
/// export from the server's vector, which is what `catchUp` does in the
/// browser (`project-session.js`) and what these tests do here.
async fn resend_from_head(sequencer: &Sequencer, outbox: &Outbox) -> Vec<u8> {
    let head = sequencer.log_state().await.head_vector;
    let from = if head.is_empty() {
        VersionVector::default()
    } else {
        VersionVector::decode(&head).expect("the head vector the sequencer just encoded")
    };
    outbox.export_from(&from)
}

/// Keeps typing into `sequencer` until something is refused, and answers with
/// what refused it and how many updates got in first. Bounded, so a bound
/// that silently stopped working fails the test rather than hanging it.
async fn type_until_refused(
    sequencer: &Sequencer,
    outbox: &mut Outbox,
    peer: &str,
    each: usize,
) -> (Ingested, i64) {
    let mut sent = 0;
    for seq in 1..10_000 {
        let outcome = sequencer
            .ingest(1, peer, peer, seq, outbox.edit_at_least(each))
            .await;
        if !matches!(outcome, Ingested::Accepted) {
            return (outcome, sent);
        }
        sent += 1;
    }
    panic!("nothing refused {sent} updates; the bound under test is not enforced");
}

/// The case SPEC-frugal §2 is written about: documents that are each well
/// inside their own 4 MiB buffer ceiling, which together reach a limit the
/// deployment actually has.
///
/// Both halves matter. Without the first assertion this would pass against a
/// per-document cap, which is what already existed; without the second it
/// would pass against a bound that refused the update and kept it anyway.
#[tokio::test]
async fn documents_that_are_each_admissible_still_reach_one_shared_deployment_limit() {
    let pending = PendingBudget::new(128 * 1024, 8 * 1024 * 1024);
    let first = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());
    let second = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());
    let (mut one, mut two) = (Outbox::new(), Outbox::new());

    // Fill the first document to well under its own ceiling.
    let (refused_first, sent) = type_until_refused(&first, &mut one, "account:a", 4096).await;
    assert_eq!(refusal_reason(&refused_first), "pending_budget");
    let held = first.log_state().await;
    assert!(
        held.buffered_charge < BUFFER_CEILING_BYTES,
        "this document is nowhere near its own 4 MiB ceiling ({} bytes over {sent} updates); \
         the refusal has to be the deployment's",
        held.buffered_charge,
    );

    // A second, untouched document is refused by the same pool, which is the
    // whole claim: the bound is the deployment's and not this document's.
    let before = second.log_state().await;
    let refused_second = second
        .ingest(2, "account:b", "account:b", 1, two.edit_at_least(4096))
        .await;
    assert_eq!(refusal_reason(&refused_second), "pending_budget");

    // And nothing about the refusal touched the document.
    let after = second.log_state().await;
    assert_eq!(after.buffered, 0, "a refused update was buffered anyway");
    assert_eq!(after.buffered_charge, 0);
    assert_eq!(
        after.head_vector, before.head_vector,
        "a refused update moved the head, so the next one from that peer would be \
         acknowledged against operations the server never received",
    );
    assert_eq!(after.log_vector, before.log_vector);
    assert!(
        second.flush_due().await.is_none(),
        "a refused update left something for a flush to write",
    );
    assert!(pending.refused_retained() >= 2);
}

/// A refusal must leave no reservation behind, or a deployment would lose a
/// little capacity on every refusal and end up permanently unable to accept
/// anything -- the failure that turns a slow minute into a dead process.
#[tokio::test]
async fn a_refused_update_leaves_no_reservation_behind() {
    let pending = PendingBudget::new(64 * 1024, 8 * 1024 * 1024);
    let sequencer = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());
    let mut outbox = Outbox::new();
    let (refused, _) = type_until_refused(&sequencer, &mut outbox, "account:a", 4096).await;
    assert_eq!(refusal_reason(&refused), "pending_budget");

    let after_first = pending.retained_used();
    // The same recovery a refused client makes, over and over: re-export
    // everything the server's head does not cover, and be refused again.
    for seq in 5_000..5_010 {
        let resend = resend_from_head(&sequencer, &outbox).await;
        let outcome = sequencer
            .ingest(1, "account:a", "account:a", seq, resend)
            .await;
        assert_eq!(refusal_reason(&outcome), "pending_budget");
    }
    assert_eq!(
        pending.retained_used(),
        after_first,
        "ten more refusals moved the reservation, so refusing is not free",
    );
    assert_eq!(
        pending.retained_used(),
        sequencer.log_state().await.buffered_charge as u64,
        "the pool holds exactly what the buffer holds",
    );
}

/// Racing documents must not oversubscribe the shared allowance. Every task
/// asks at once against one budget; what is held afterwards has to be inside
/// the limit, and it has to be exactly the sum of what the buffers kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_admission_near_the_limit_cannot_oversubscribe() {
    let limit = 96 * 1024;
    let pending = PendingBudget::new(limit, 8 * 1024 * 1024);
    let sequencers: Vec<Arc<Sequencer>> = (0..8)
        .map(|_| {
            Arc::new(sequencer_sharing_pending(
                Arc::new(FakeCatalog::empty()),
                pending.clone(),
            ))
        })
        .collect();

    let mut tasks = Vec::new();
    for sequencer in &sequencers {
        let sequencer = sequencer.clone();
        tasks.push(tokio::spawn(async move {
            let mut outbox = Outbox::new();
            for seq in 1..=8 {
                let _ = sequencer
                    .ingest(1, "account:a", "account:a", seq, outbox.edit_at_least(4096))
                    .await;
            }
        }));
    }
    for task in tasks {
        task.await.expect("an ingesting task");
    }

    assert!(
        pending.retained_used() <= limit,
        "{} bytes are reserved against a limit of {limit}",
        pending.retained_used(),
    );
    let mut buffered = 0u64;
    for sequencer in &sequencers {
        buffered += sequencer.log_state().await.buffered_charge as u64;
    }
    assert_eq!(
        pending.retained_used(),
        buffered,
        "the pool and the buffers disagree, so a charge was taken or released twice",
    );
}

/// The pending bound is about bytes the server is holding, not about whether
/// it has decoded them. A cold document and a warm one are charged alike --
/// the accounting hangs off the buffer, which both have, and not off the
/// cache, which only one does.
#[tokio::test]
async fn pending_is_charged_whether_or_not_the_document_is_warm() {
    let pending = PendingBudget::new(1024 * 1024, 8 * 1024 * 1024);
    let cold = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());
    let warm = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());
    // A projection builds the entry, which is what makes this one warm.
    warm.projection().await.expect("an empty document builds");
    assert!(warm.is_warm().await);
    assert!(!cold.is_warm().await);

    let (mut a, mut b) = (Outbox::new(), Outbox::new());
    let batch = a.edit_at_least(4096);
    let expected = batch.len() + crate::log::frame::overhead("account:a");
    assert!(matches!(
        cold.ingest(1, "account:a", "account:a", 1, batch).await,
        Ingested::Accepted
    ));
    assert_eq!(pending.retained_used(), expected as u64);

    let batch = b.edit_at_least(4096);
    let also = batch.len() + crate::log::frame::overhead("account:a");
    assert!(matches!(
        warm.ingest(1, "account:a", "account:a", 1, batch).await,
        Ingested::Accepted
    ));
    assert_eq!(
        pending.retained_used(),
        (expected + also) as u64,
        "a warm document's pending bytes are charged the same as a cold one's",
    );
    assert!(!cold.is_warm().await, "ingest must not warm a document");
}

/// The per-document ceiling is still the per-document ceiling. With a
/// deployment pool far larger than one document's allowance, one document
/// filling up is refused by its own cap and not by the deployment's -- which
/// is what stops one document from spending the whole deployment allowance.
#[tokio::test]
async fn the_per_document_ceiling_still_applies_when_the_deployment_has_room() {
    let pending = PendingBudget::new(64 * 1024 * 1024, 16 * 1024 * 1024);
    let sequencer = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());
    let mut outbox = Outbox::new();
    // 256 KiB a time, so the 4 MiB ceiling is reached in a few dozen updates
    // rather than a few thousand.
    let (refused, _) = type_until_refused(&sequencer, &mut outbox, "account:a", 256 * 1024).await;
    assert_eq!(
        refusal_reason(&refused),
        "document_buffer",
        "the deployment pool has sixty megabytes free; this can only be the document's own cap",
    );
    let held = sequencer.log_state().await;
    assert!(held.buffered_charge <= BUFFER_CEILING_BYTES);
    assert!(
        pending.retained_used() < pending.retained_limit() / 2,
        "one document reached its cap with most of the deployment pool still free",
    );
}

/// §5's zero-change catch-up: every client sends one on every join. It is not
/// work, produces no row, and must not need pending capacity -- otherwise a
/// deployment under pressure could not be rejoined at all, which is exactly
/// when clients are reconnecting.
///
/// And its acknowledgement must stay what it was: transport bookkeeping, with
/// nothing durable claimed. The durable vector is what says otherwise, and it
/// does not move.
#[tokio::test]
async fn an_empty_catch_up_needs_no_pending_capacity_and_claims_no_durability() {
    // A pool with nothing left in it at all.
    let pending = PendingBudget::new(1024, 8 * 1024 * 1024);
    let _full = pending.try_retain(1024).expect("the whole pool");
    let sequencer = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());

    let mut outbox = Outbox::new();
    let _ = outbox.edit();
    let nothing = outbox.export_since_last();
    assert_eq!(
        LoroDoc::decode_import_blob_meta(&nothing, true)
            .expect("well formed")
            .change_num,
        0,
        "sanity: this is the empty catch-up a rejoin sends",
    );

    let before = sequencer.log_state().await;
    assert!(
        matches!(
            sequencer
                .ingest(1, "account:a", "account:a", 1, nothing)
                .await,
            Ingested::Accepted
        ),
        "an empty catch-up was refused for want of room to hold nothing",
    );
    let after = sequencer.log_state().await;
    assert_eq!(after.buffered, 0);
    assert_eq!(after.buffered_charge, 0);
    assert_eq!(
        after.log_vector, before.log_vector,
        "acknowledging an empty catch-up must not claim the durable log moved",
    );
    assert_eq!(pending.retained_used(), 1024, "nothing else was reserved");
}

/// A flush stalled in the database holds its scratch and nothing else: the
/// buffer it snapshotted is still charged, because those bytes are still in
/// memory and still unsaved, and edits that arrive during the stall are
/// charged on top.
#[tokio::test]
async fn a_stalled_flush_holds_its_scratch_while_the_buffer_stays_charged() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let catalog = Arc::new(FakeCatalog {
        flush_gate: Some((entered.clone(), release.clone())),
        ..FakeCatalog::empty()
    });
    let pending = PendingBudget::new(1024 * 1024, 8 * 1024 * 1024);
    let sequencer = Arc::new(sequencer_sharing_pending(catalog.clone(), pending.clone()));
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 1, outbox.edit_at_least(4096))
            .await,
        Ingested::Accepted
    ));
    let first_charge = pending.retained_used();
    assert!(first_charge > 0);

    let flushing = {
        let sequencer = sequencer.clone();
        tokio::spawn(async move { sequencer.flush(FlushReason::MaxAge).await })
    };
    entered.notified().await;

    assert!(
        pending.scratch_used() > 0,
        "a write in flight is holding no scratch, so nothing accounted for the row",
    );
    assert_eq!(
        pending.retained_used(),
        first_charge,
        "the buffered bytes are still in memory and still unsaved; retiring them before \
         the write returns would be claiming a durability that has not happened",
    );

    // Typing continues during the stall (§10) and is charged on top.
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 2, outbox.edit_at_least(4096))
            .await,
        Ingested::Accepted
    ));
    let during = pending.retained_used();
    assert!(during > first_charge);

    release.notify_one();
    flushing
        .await
        .expect("the flush task")
        .expect("the write succeeds once released")
        .expect("a row was written");

    assert_eq!(
        pending.scratch_used(),
        0,
        "the row was written and its scratch was not given back",
    );
    assert_eq!(
        pending.retained_used(),
        during - first_charge,
        "retiring the committed prefix released exactly its charge; the edit that arrived \
         during the write is still buffered and still charged",
    );
    assert_eq!(sequencer.log_state().await.buffered, 1);
}

/// A write that plainly did not happen must change nothing: no durable
/// advancement, the work still buffered and still charged, and the temporary
/// allocations given back so the next attempt can have them.
#[tokio::test]
async fn a_failed_flush_keeps_the_work_charged_and_releases_only_its_scratch() {
    let catalog = Arc::new(FakeCatalog::empty());
    catalog
        .fail_every_flush
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let pending = PendingBudget::new(1024 * 1024, 8 * 1024 * 1024);
    let sequencer = sequencer_sharing_pending(catalog.clone(), pending.clone());
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 1, outbox.edit_at_least(4096))
            .await,
        Ingested::Accepted
    ));
    let before = sequencer.log_state().await;
    let charged = pending.retained_used();

    assert!(
        sequencer.flush(FlushReason::MaxAge).await.is_err(),
        "the fake refused this write",
    );

    let after = sequencer.log_state().await;
    assert_eq!(after.buffered, before.buffered, "work was dropped");
    assert_eq!(
        after.log_vector, before.log_vector,
        "durable coverage advanced on a write that did not happen",
    );
    assert_eq!(after.next_sequence, before.next_sequence);
    assert_eq!(pending.retained_used(), charged, "the work is still held");
    assert_eq!(
        pending.scratch_used(),
        0,
        "the failed attempt's temporary allocations were not released",
    );
    assert!(catalog.flushed.lock().unwrap().is_empty());

    // And it can be written once storage comes back, which is what makes the
    // retained charge a delay rather than a leak.
    catalog
        .fail_every_flush
        .store(false, std::sync::atomic::Ordering::SeqCst);
    sequencer
        .flush(FlushReason::MaxAge)
        .await
        .expect("storage is back")
        .expect("a row");
    assert_eq!(pending.retained_used(), 0);
    assert_eq!(pending.scratch_used(), 0);
}

/// A flush whose future is dropped mid-write gives its scratch back and
/// leaves the buffered work alone. Cancellation is the one path no explicit
/// release statement covers, which is why the reservations are `Drop` types
/// and why this asserts on both pools rather than only the one that moved.
#[tokio::test]
async fn a_cancelled_flush_releases_its_scratch_and_keeps_the_work() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let catalog = Arc::new(FakeCatalog {
        flush_gate: Some((entered.clone(), release.clone())),
        ..FakeCatalog::empty()
    });
    let pending = PendingBudget::new(1024 * 1024, 8 * 1024 * 1024);
    let sequencer = Arc::new(sequencer_sharing_pending(catalog.clone(), pending.clone()));
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 1, outbox.edit_at_least(4096))
            .await,
        Ingested::Accepted
    ));
    let charged = pending.retained_used();

    let flushing = {
        let sequencer = sequencer.clone();
        tokio::spawn(async move { sequencer.flush(FlushReason::MaxAge).await })
    };
    entered.notified().await;
    assert!(pending.scratch_used() > 0);
    flushing.abort();
    let _ = flushing.await;
    // The abort unwinds the task; give the runtime the turn it needs to drop
    // the future and with it every guard the flush was holding.
    tokio::task::yield_now().await;

    assert_eq!(
        pending.scratch_used(),
        0,
        "an abandoned write is still holding the memory it was writing from",
    );
    assert_eq!(
        pending.retained_used(),
        charged,
        "cancelling the write threw away the work it was writing",
    );
    assert_eq!(sequencer.log_state().await.buffered, 1);

    // And the surviving work can be retried: the cancelled flush left neither
    // a held transaction gate nor a half-retired buffer behind. The gate is
    // released first so this attempt is not stalled the way the last one was.
    release.notify_waiters();
    let sequencer_for_retry = sequencer.clone();
    let retry = tokio::spawn(async move { sequencer_for_retry.flush(FlushReason::MaxAge).await });
    entered.notified().await;
    release.notify_one();
    retry
        .await
        .expect("the retry task")
        .expect("the retried write succeeds")
        .expect("a row");
    assert_eq!(pending.retained_used(), 0, "the retry wrote the work");
    assert_eq!(pending.scratch_used(), 0);
}

/// The claim at the centre of the design: a deployment whose pending pool is
/// completely full can still write what is in it. A single undifferentiated
/// pool fails exactly here, and fails permanently -- there would be no room
/// left to encode the row that would free the room.
#[tokio::test]
async fn work_can_still_be_flushed_with_the_pending_pool_completely_full() {
    let catalog = Arc::new(FakeCatalog::empty());
    // Scratch sized as the configuration check requires: enough for one
    // maximum row. Retained deliberately tiny, and then filled.
    let pending = PendingBudget::new(
        64 * 1024,
        scratch_for(max_row_bytes(Configuration::default().log_quota_bytes) as u64),
    );
    let sequencer = sequencer_sharing_pending(catalog.clone(), pending.clone());
    let mut outbox = Outbox::new();
    let (refused, sent) = type_until_refused(&sequencer, &mut outbox, "account:a", 4096).await;
    assert_eq!(refusal_reason(&refused), "pending_budget");
    assert!(
        sent > 0,
        "nothing was accepted, so there is nothing to drain"
    );

    sequencer
        .flush(FlushReason::Bytes)
        .await
        .expect("a full pending pool must not stop a flush")
        .expect("a row was written");

    assert_eq!(
        pending.retained_used(),
        0,
        "the flush wrote everything it held and released every charge",
    );
    assert_eq!(pending.scratch_used(), 0);
    // And the refused work is accepted on the client's next attempt, without
    // the limit having been raised or a single edit discarded. This is the
    // whole recovery: the pressure cleared because the buffer drained, and
    // the re-export the client makes carries exactly what was refused.
    let resend = resend_from_head(&sequencer, &outbox).await;
    assert!(
        !resend.is_empty(),
        "sanity: the refused work is still in the client's document",
    );
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 9_000, resend)
            .await,
        Ingested::Accepted
    ));
    assert!(pending.retained_used() > 0, "the resent work is now held");
}

/// Progress across documents, not just within one. After the pressure clears
/// every document that was holding work writes it -- which is what
/// `Registry::housekeep` does on its ordinary pass, and is the recovery an
/// operator actually sees.
#[tokio::test]
async fn every_document_drains_once_the_pressure_clears() {
    let pending = PendingBudget::new(
        128 * 1024,
        scratch_for(max_row_bytes(Configuration::default().log_quota_bytes) as u64) * 2,
    );
    let catalogs: Vec<Arc<FakeCatalog>> = (0..3).map(|_| Arc::new(FakeCatalog::empty())).collect();
    let sequencers: Vec<Sequencer> = catalogs
        .iter()
        .map(|catalog| sequencer_sharing_pending(catalog.clone(), pending.clone()))
        .collect();

    // Spread the deployment allowance across all three, then keep going until
    // the pool refuses somebody.
    let mut outboxes: Vec<Outbox> = (0..3).map(|_| Outbox::new()).collect();
    let mut refused = false;
    for seq in 1..1000 {
        for (index, sequencer) in sequencers.iter().enumerate() {
            let outcome = sequencer
                .ingest(
                    1,
                    "account:a",
                    "account:a",
                    seq,
                    outboxes[index].edit_at_least(2048),
                )
                .await;
            if matches!(outcome, Ingested::Retryable(_)) {
                assert_eq!(refusal_reason(&outcome), "pending_budget");
                refused = true;
            }
        }
        if refused {
            break;
        }
    }
    assert!(refused, "the shared pool never refused anything");
    for sequencer in &sequencers {
        assert!(
            sequencer.log_state().await.buffered > 0,
            "all three hold work"
        );
    }

    for sequencer in &sequencers {
        sequencer
            .flush(FlushReason::MaxAge)
            .await
            .expect("each document can drain")
            .expect("a row");
    }
    assert_eq!(pending.retained_used(), 0);
    for catalog in &catalogs {
        assert_eq!(catalog.flushed.lock().unwrap().len(), 1);
    }
}

/// A sequencer dropped from the registry keeps paying for the work it is
/// still holding, and stops paying the moment the last reference to it goes.
/// Releasing on removal instead would under-report the bytes the process is
/// actually holding -- which is the same as not bounding them.
#[tokio::test]
async fn a_sequencers_charges_live_exactly_as_long_as_the_sequencer_does() {
    let pending = PendingBudget::new(1024 * 1024, 8 * 1024 * 1024);
    let sequencer = Arc::new(sequencer_sharing_pending(
        Arc::new(FakeCatalog::empty()),
        pending.clone(),
    ));
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 1, outbox.edit_at_least(4096))
            .await,
        Ingested::Accepted
    ));
    let charged = pending.retained_used();
    assert!(charged > 0);

    // What a registry retirement looks like from here: one reference goes,
    // another is still held by whoever is mid-operation on it.
    let still_held = sequencer.clone();
    drop(sequencer);
    assert_eq!(
        pending.retained_used(),
        charged,
        "the bytes are still in memory, so they must still be charged",
    );

    drop(still_held);
    assert_eq!(
        pending.retained_used(),
        0,
        "the last reference went and the buffer with it, but the charge stayed",
    );
}

/// The row a flush writes is exactly what the buffer's charge predicted.
/// Everything above rests on that: the scratch reservation is computed from
/// the charge before the row exists, so a charge that under-counted framing
/// would under-reserve every flush on a document full of small updates.
#[tokio::test]
async fn the_buffer_charge_is_exactly_the_row_the_flush_writes() {
    let catalog = Arc::new(FakeCatalog::empty());
    let pending = PendingBudget::new(1024 * 1024, 8 * 1024 * 1024);
    let sequencer = sequencer_sharing_pending(catalog.clone(), pending.clone());
    let mut outbox = Outbox::new();
    // Many small updates, which is the shape framing dominates.
    for seq in 1..=32 {
        assert!(matches!(
            sequencer
                .ingest(1, "account:a-fairly-long-peer-key", "p", seq, outbox.edit())
                .await,
            Ingested::Accepted
        ));
    }
    let charge = sequencer.log_state().await.buffered_charge;
    sequencer
        .flush(FlushReason::MaxAge)
        .await
        .expect("flush")
        .expect("a row");
    let flushed = catalog.flushed.lock().unwrap();
    assert_eq!(
        flushed[0].len(),
        crate::log::frame::ROW_HEADER_BYTES + charge,
        "the row is not the size the charge said it would be, so every scratch \
         reservation taken from that charge is the wrong size too",
    );
}

/// A semantic command that produces no source and writes no rows of its own.
/// Enough to reach §7's row assembly, which is the part the pending bound
/// touches: `transact` is never called in these cases, because the scratch
/// refusal happens before the transaction is opened -- which is the point.
struct NoopCommand;

impl crate::log::Command for NoopCommand {
    type Output = ();

    fn name(&self) -> &'static str {
        "test.noop"
    }

    fn evaluate(
        &mut self,
        _head: &crate::log::Head<'_>,
    ) -> std::result::Result<Option<crate::log::PreparedSource>, crate::log::CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a crate::log::Evidence,
    ) -> BoxFuture<'a, std::result::Result<(), crate::log::CommandError>> {
        Box::pin(async {
            unimplemented!("this command is only ever refused before its transaction is opened")
        })
    }
}

/// §7's row carries the buffered prefix, so a command is a persistence path
/// like any other and is admitted like one. When there is no scratch for the
/// row it would write, it is refused through the ordinary temporary-command
/// error -- BEFORE the transaction is opened, so there are no effects to roll
/// back and no authorization decision already made.
///
/// The fake catalogue proves that last part by construction: its
/// `begin_document_command` panics, so this test passing at all is the
/// evidence that nothing reached it.
#[tokio::test]
async fn a_command_with_no_scratch_is_refused_before_it_opens_a_transaction() {
    let pending = PendingBudget::new(1024 * 1024, 64 * 1024);
    let sequencer = sequencer_sharing_pending(Arc::new(FakeCatalog::empty()), pending.clone());
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "account:a", "account:a", 1, outbox.edit_at_least(4096))
            .await,
        Ingested::Accepted
    ));
    let charged = pending.retained_used();

    // Every byte of scratch spoken for by something else.
    let held = pending
        .try_scratch(64 * 1024)
        .expect("the whole scratch pool");

    let authority = Authority {
        principal_key: "account:a".into(),
        account_id: None,
        link_hash: None,
    };
    let error = sequencer
        .command(&authority, &mut NoopCommand)
        .await
        .expect_err("there is no scratch for this command's row");
    assert!(
        matches!(&error, crate::log::CommandError::Conflict(why) if why.contains("retry")),
        "resource exhaustion has to come back as a temporary, retryable refusal: {error:?}",
    );
    assert_eq!(
        pending.retained_used(),
        charged,
        "the refused command dropped somebody else's buffered work",
    );
    assert_eq!(sequencer.log_state().await.buffered, 1);

    // And once the scratch comes back the same command reaches its
    // transaction, which the fake refuses to provide -- so this half asserts
    // only that the budget is no longer what stops it.
    drop(held);
    assert_eq!(pending.scratch_used(), 0);
    let reached_the_transaction = std::panic::AssertUnwindSafe(async {
        let _ = sequencer.command(&authority, &mut NoopCommand).await;
    });
    assert!(
        futures_util::FutureExt::catch_unwind(reached_the_transaction)
            .await
            .is_err(),
        "with scratch available the command should have got as far as asking the \
         catalogue for a transaction, which this fake panics on",
    );
}

/// §5.1's ambiguous outcome, from the accounting's side: the response was
/// lost, the re-read proves the row committed, the buffer retires -- and its
/// charges are released exactly once.
///
/// Once is the thing worth asserting. A release on the failure path plus a
/// release on the resolution path would both run here, and a counter that
/// went down twice for one payload does not merely mis-report: it would
/// underflow the pool and let the deployment hold unbounded pending bytes
/// while reading as empty, which is the bound quietly ceasing to exist.
#[tokio::test]
async fn an_ambiguous_commit_that_resolves_releases_each_charge_exactly_once() {
    let catalog = Arc::new(FakeCatalog {
        fail_next_flush: std::sync::atomic::AtomicBool::new(true),
        lost_flush_committed: true,
        sequence_on_ambiguity: AtomicI64::new(1),
        ..FakeCatalog::empty()
    });
    let pending = PendingBudget::new(1024 * 1024, 8 * 1024 * 1024);
    let sequencer = sequencer_sharing_pending(catalog.clone(), pending.clone());
    let mut outbox = Outbox::new();
    for sent in 1..=3 {
        assert!(matches!(
            sequencer
                .ingest(
                    1,
                    "account:a",
                    "account:a",
                    sent,
                    outbox.edit_at_least(2048)
                )
                .await,
            Ingested::Accepted
        ));
    }
    assert!(pending.retained_used() > 0);

    let written = sequencer
        .flush(FlushReason::MaxAge)
        .await
        .expect("the re-read proves the row committed")
        .expect("something was buffered");
    assert_eq!(written, 1);

    assert_eq!(
        pending.retained_used(),
        0,
        "the retired batches did not release their charges",
    );
    assert_eq!(pending.scratch_used(), 0);
    assert_eq!(
        catalog.flushed.lock().unwrap().len(),
        1,
        "a lost response must not write the row twice",
    );

    // The pool is intact rather than merely reading zero: the whole of it can
    // be taken again, which an underflowed counter would not allow.
    let all = pending
        .try_retain(1024 * 1024)
        .expect("the pool is back to exactly its limit, neither short nor over-credited");
    assert!(pending.try_retain(1).is_err());
    drop(all);
}

/// Shutdown must wait for another document's scratch, without locking out
/// edits while it waits, then persist the complete buffer before returning.
#[tokio::test]
async fn shutdown_waits_for_scratch_and_drains_instead_of_reporting_empty() {
    let pending = PendingBudget::new(
        1024 * 1024,
        scratch_for(max_row_bytes(Configuration::default().log_quota_bytes) as u64),
    );
    let catalog = Arc::new(FakeCatalog::empty());
    let sequencer = Arc::new(sequencer_sharing_pending(catalog.clone(), pending.clone()));
    let mut outbox = Outbox::new();
    assert!(matches!(
        sequencer
            .ingest(1, "a", "a", 1, outbox.edit_at_least(1024))
            .await,
        Ingested::Accepted
    ));
    let held = pending
        .try_scratch(pending.scratch_limit())
        .expect("occupy all scratch");
    assert!(matches!(
        sequencer.flush(FlushReason::Quiet).await,
        Err(SequencerError::Busy)
    ));
    let closing = {
        let sequencer = sequencer.clone();
        tokio::spawn(async move { sequencer.flush(FlushReason::Shutdown).await })
    };
    while pending.refused_scratch() < 2 {
        tokio::task::yield_now().await;
    }
    assert!(
        !closing.is_finished(),
        "shutdown must not skip a buffered document"
    );
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            sequencer.ingest(1, "a", "a", 2, outbox.edit_at_least(1024))
        )
        .await
        .expect("scratch wait must not hold inner lock"),
        Ingested::Accepted
    ));
    drop(held);
    assert!(tokio::time::timeout(Duration::from_secs(2), closing)
        .await
        .expect("shutdown wakes after release")
        .expect("shutdown task")
        .expect("flush")
        .is_some());
    assert_eq!(sequencer.log_state().await.buffered, 0);
    assert_eq!(pending.retained_used(), 0);
    assert_eq!(pending.scratch_used(), 0);
    assert_eq!(catalog.flushed.lock().unwrap().len(), 1);
}
