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
use loro::{ExportMode, LoroDoc, VersionVector};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::config::Configuration;
use crate::log::budget::DEFAULT_EXPANSION;
use crate::log::sequencer::{
    ack_targets, FlushReason, Ingested, LogCatalog, Role, Sequencer, SequencerError, FLUSH_MAX_AGE,
    FLUSH_QUIET, FLUSH_TRIGGER_BYTES, MAX_UPDATE_BYTES,
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
        }
    }
}

impl LogCatalog for FakeCatalog {
    fn log_head(&self, _document_id: Uuid) -> BoxFuture<'_, postgres::Result<postgres::LogHead>> {
        Box::pin(async {
            unimplemented!("admission is bypassed by Sequencer::from_parts in these tests")
        })
    }

    fn log_base(
        &self,
        _document_id: Uuid,
    ) -> BoxFuture<'_, postgres::Result<Option<postgres::LogBase>>> {
        Box::pin(async {
            unimplemented!("admission is bypassed by Sequencer::from_parts in these tests")
        })
    }

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
        Box::pin(async move {
            self.flushed.lock().unwrap().push(row.update_bytes.to_vec());
            Ok(self.next_sequence.fetch_add(1, Ordering::SeqCst))
        })
    }

    fn log_sequence(&self, _document_id: Uuid) -> BoxFuture<'_, postgres::Result<i64>> {
        let answer = self.sequence_on_ambiguity.load(Ordering::SeqCst);
        Box::pin(async move { Ok(answer) })
    }

    fn begin_document_command<'a>(
        &'a self,
        _document_id: Uuid,
        _authority: &'a Authority,
        _rung: super::sequencer::Rung,
    ) -> BoxFuture<'a, postgres::Result<sqlx::Transaction<'static, sqlx::Postgres>>> {
        Box::pin(async {
            unimplemented!("a semantic command needs a real transaction; it needs LIBREPAPER_TEST_POSTGRES_URL")
        })
    }

    fn insert_log_row<'a>(
        &'a self,
        _tx: &'a mut sqlx::Transaction<'static, sqlx::Postgres>,
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

/// A sequencer with no base, no rows and an empty buffer -- admitted
/// without I/O, per `Sequencer::from_parts`' own doc comment.
fn bare_sequencer(catalog: Arc<dyn LogCatalog>) -> Sequencer {
    Sequencer::from_parts(
        Uuid::new_v4(),
        "test-doc".to_string(),
        catalog,
        blobs(),
        Arc::new(Configuration::default()),
        Budget::new(u64::MAX / 2, DEFAULT_EXPANSION),
        "deployment.peer.test".to_string(),
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
    // MAX_UPDATE_BYTES (4 MiB): large enough that a real deployment would
    // see it as the "single large paste" case §5 step 6 describes, without
    // this test's runtime being dominated by generating a full 4 MiB of
    // random filler.
    let big = outbox.edit_at_least(FLUSH_TRIGGER_BYTES + FLUSH_TRIGGER_BYTES / 4);
    assert!(
        big.len() < MAX_UPDATE_BYTES,
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
    let targets = ack_targets(&batches);

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
        writer_frames.iter().all(|frame| frame["type"] != "doc-durable"),
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
        catalog,
        blobs(),
        Arc::new(Configuration::default()),
        Budget::new(u64::MAX / 2, DEFAULT_EXPANSION),
        "deployment.peer.test".to_string(),
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
        catalog.clone(),
        blobs(),
        Arc::new(Configuration::default()),
        Budget::new(u64::MAX / 2, DEFAULT_EXPANSION),
        "deployment.peer.test".to_string(),
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
        Arc::new(FakeCatalog::empty()),
        blobs(),
        Arc::new(Configuration::default()),
        budget,
        "deployment.peer.test".to_string(),
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
    let transient = crate::log::budget::estimate(
        log_bytes,
        crate::log::budget::BUILD_TRANSIENT_EXPANSION,
    );
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
