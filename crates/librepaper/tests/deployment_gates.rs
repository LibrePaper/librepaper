//! SPEC-server-is-a-log §14.2's three deployment-shaped tests: the items
//! that are about the DEPLOYMENT rather than about one document, and so
//! cannot be answered by a fake `LogCatalog` (see the header of
//! `src/log/tests.rs`, which explains why they live here instead). The
//! third of these -- "revocation mid-buffer flushes and closes" -- also
//! needs a real `Server` behind a real socket and the real sharing route
//! that revokes a link, none of which the fake catalogue has either: it has
//! no sharing, no sockets and no authority to revoke.
//!
//! All three need a real PostgreSQL and are gated behind
//! `LIBREPAPER_TEST_POSTGRES_URL`, the same variable the rest of the
//! catalogue coverage uses (`grep -rl LIBREPAPER_TEST_POSTGRES_URL
//! crates/librepaper/src`). Point it at a throwaway database, not
//! `librepaper_sqlx` (what the sqlx macros check offline builds against
//! and never run against) and not `librepaper` (a real local deployment):
//!
//! ```text
//! docker exec librepaper-postgres psql -U postgres -c 'CREATE DATABASE lp_gates'
//! export LIBREPAPER_TEST_POSTGRES_URL='postgresql://postgres:librepaper-local@127.0.0.1:55432/lp_gates'
//! cargo test -p librepaper --test deployment_gates -- --ignored --test-threads=1
//! ```
//!
//! `--test-threads=1` is mandatory for all three, for reasons that do not
//! all match: the lease test TRUNCATEs the database it is pointed at, like
//! every other catalogue test; the idle test counts every commit PostgreSQL
//! records against that database, so a second test running concurrently
//! would be indistinguishable from a real regression; the revocation test
//! also TRUNCATEs and, on top of that, binds a real loopback listener and
//! runs a real websocket handshake, neither of which needs isolation from a
//! neighbour but both of which are simply not worth paying to parallelize
//! for three tests.
//!
//! All three build the real production types (`storage::postgres::
//! PostgresCatalog`, `log::Registry`, `room::Rooms`, `storage::worker::
//! Worker`, and now `server::Server` behind its own `axum::serve`) rather
//! than a hand-made subset, per the brief for this file. Reaching them from
//! an integration-test binary (as opposed to a `#[cfg(test)]` module inside
//! the crate, which is where every other PostgreSQL-gated test in this
//! codebase lives) needs re-exports from `lib.rs`:
//!
//! ```text
//! pub use auth::{sign_device, GithubApp, Identity, Policy, PROVIDER_GITHUB};
//! pub use document::store::Store;
//! pub use room::{Room, Rooms};
//! pub use server::Server;
//! pub use storage::blob::{BlobStore, FsStore};
//! pub use storage::worker;
//! ```
//!
//! placed beside the existing `pub use storage::{postgres, source_archive};`.
//! That makes them reachable here as
//! `librepaper::{Rooms, BlobStore, FsStore, Server, Store, GithubApp, ...}`
//! and `librepaper::worker::Worker` (top-level, the way `pub use` re-exports
//! always surface, regardless of the private module path behind them) --
//! see this file's own `use` block. `librepaper::postgres`, `librepaper::log`
//! and `librepaper::log::sequencer` were already public and needed no change.

use std::sync::Arc;
use std::time::Duration;

use librepaper::config::Configuration;
use librepaper::log::sequencer::FlushReason;
use librepaper::log::{Ingested, Registry, SequencerError};
use librepaper::postgres::{
    Error as PgError, NewAccount, NewDocument, PostgresCatalog, PostgresOptions,
};
use librepaper::worker::Worker;
use librepaper::{sign_device, BlobStore, FsStore, GithubApp, Identity, Policy, Rooms, Server};
use librepaper::{Store, PROVIDER_GITHUB};
use loro::{ExportMode, LoroDoc, VersionVector};
use serde_json::json;
use uuid::Uuid;

// -- shared fixtures -------------------------------------------------------

/// A throwaway, leaked-temp-directory blob store. Nothing in either test
/// reads an asset back; a document created straight through the catalogue
/// (rather than through `document::store::Store`) never writes one either,
/// so this exists only because `Registry::new` needs a value for the field.
fn blobs() -> Arc<dyn BlobStore> {
    let dir = tempfile::tempdir().expect("a temp directory for the unused blob store");
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    Arc::new(FsStore::new(path, false))
}

async fn connected(url: &str) -> PostgresCatalog {
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .expect("connect to the test database");
    catalog.migrate().await.expect("run migrations");
    catalog
}

/// Wipes every row this file's fixtures could have left behind, the same
/// list `storage/postgres/mod.rs`'s own `truncate` uses. Safe only because
/// the brief for this file requires a database dedicated to it.
async fn truncate(catalog: &PostgresCatalog) {
    sqlx::query(
        "TRUNCATE document_proposal_hunks,document_proposals,document_labels,\
         document_updates,document_bases,document_assets,replies,annotations,\
         document_marks,share_links,grants,documents,accounts CASCADE",
    )
    .execute(catalog.pool())
    .await
    .expect("truncate the throwaway database");
}

async fn seed_account(catalog: &PostgresCatalog, tag: &str) -> Uuid {
    catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(format!("subject-{tag}")),
            handle: format!("handle-{tag}"),
            display_name: "Test Owner".into(),
            email: None,
        })
        .await
        .expect("create account")
        .id
}

/// A document with an empty log: no base, no rows. `Rooms::get` admits it
/// with nothing to replay, which is exactly what both tests want -- the
/// history under test is made entirely of updates this file ingests itself,
/// so there is nothing left over from a seeding path to confuse "the log is
/// consistent" with.
async fn seed_document(catalog: &PostgresCatalog, owner_id: Uuid, slug: &str) {
    catalog
        .create_document(NewDocument {
            slug: slug.to_string(),
            owner_id,
            ownership_mode: "owned".into(),
            title: "A Paper".into(),
            source_format: "markdown".into(),
            main_path: "paper.md".into(),
            settings: json!({"version": 1}),
        })
        .await
        .expect("create document");
}

/// One editor's outbox: a bare `LoroDoc` (no `files`/`paths` roots, exactly
/// like `log::tests::Outbox`) whose exports are causally linked batches, the
/// same shape a `doc-update` frame carries on the wire. Neither test reads
/// the document's schema, only its log, so there is no need for the shared
/// project shape here.
struct Outbox {
    doc: LoroDoc,
    at: VersionVector,
}

impl Outbox {
    fn new() -> Self {
        let doc = LoroDoc::new();
        let _ = doc.get_text("t");
        Self {
            doc,
            at: VersionVector::default(),
        }
    }

    fn edit(&mut self, text: &str) -> Vec<u8> {
        let handle = self.doc.get_text("t");
        handle.insert_utf16(handle.len_utf16(), text).unwrap();
        self.doc.commit();
        let batch = self
            .doc
            .export(ExportMode::Updates {
                from: std::borrow::Cow::Borrowed(&self.at),
            })
            .expect("exporting from a covered vector never fails");
        self.at = self.doc.oplog_vv();
        batch
    }
}

/// A catalogue's "no vector recorded yet" sentinel (literally empty bytes,
/// from `LogHead`/`LogBase` defaulting rather than encoding anything) and
/// the sequencer's own encoding of an empty `VersionVector` are not the same
/// bytes, though both mean the same thing: nothing has happened yet. The
/// sequencer's own `decode_vector` (private to `log::sequencer`) treats
/// empty bytes as `VersionVector::default()` for exactly this reason; this
/// mirrors that so a comparison against a row this test read straight from
/// the catalogue is meaningful.
fn decode_or_default(bytes: &[u8]) -> VersionVector {
    if bytes.is_empty() {
        VersionVector::default()
    } else {
        VersionVector::decode(bytes).expect("a stored vector decodes")
    }
}

// -- 1. lease loss during buffered typing (§10, §12, §14.2) ---------------

/// The writer lease and epoch are kept from the old design (ROOM.md's brief,
/// §12's "Kept" list). A flush fences on them in
/// `PostgresCatalog::begin_fenced_flush`; losing the fence mid-buffer must
/// not corrupt or silently drop what was already durable, and must not let
/// the incumbent process go on writing as though nothing happened.
///
/// This simulates the loss the way it actually happens: process A's
/// dedicated writer session goes away (here, by dropping its `WriterLease`,
/// which is what a dead connection looks like from the outside), which frees
/// the session-scoped advisory lock for process B to claim. B's claim bumps
/// `deployment_writer.epoch` durably. A's sequencer is told none of this --
/// per the design, "a lost lease is the sequencer's own concern the next
/// time it tries to flush" -- so A only discovers it when its buffered
/// typing tries to become a row.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; run with --test-threads=1"]
async fn lease_loss_during_buffered_typing_refuses_the_flush_and_the_log_is_consistent_after_re_admission(
) {
    let Ok(url) = std::env::var("LIBREPAPER_TEST_POSTGRES_URL") else {
        return;
    };

    let blobs = blobs();
    let config = Arc::new(Configuration::default());

    // -- process A claims the writer first and sets up the document ------
    let catalog_a = Arc::new(connected(&url).await);
    truncate(&catalog_a).await;
    let writer_a = catalog_a
        .claim_writer()
        .await
        .expect("A is the only writer so far, it must be able to claim the lease");
    let epoch_a = writer_a.epoch();

    let owner = seed_account(&catalog_a, "lease-owner").await;
    let slug = "lease-loss";
    seed_document(&catalog_a, owner, slug).await;

    let registry_a = Registry::new(
        catalog_a.clone(),
        blobs.clone(),
        config.clone(),
        "deployment-a".into(),
    );
    let rooms_a = Rooms::new(
        catalog_a.clone(),
        blobs.clone(),
        config.clone(),
        registry_a.clone(),
    );
    let room_a = rooms_a
        .get(slug)
        .await
        .expect("A can open the document it just created");

    // The durable head before anything is buffered. This is what
    // "re-admission is consistent" is measured against: nothing this test
    // buffers on A ever reaches a row, so a correct re-admission must land
    // exactly here, not one row short and not one row torn.
    let head_before = catalog_a
        .log_head(room_a.document_id)
        .await
        .expect("read the log head");
    assert_eq!(
        head_before.update_sequence, 0,
        "a fresh document's row counter has not moved"
    );

    // Somebody types. Nothing here crosses a flush trigger (30s max age, 5s
    // quiet, 1 MiB of bytes), so this sits in the buffer, relayed but not
    // yet durable -- exactly the state the spec calls "buffered typing".
    let mut outbox = Outbox::new();
    let batch = outbox.edit("the sentence nobody saved");
    assert!(matches!(
        room_a
            .ingest(1, "account:editor", "account:editor", 1, batch)
            .await,
        Ingested::Accepted
    ));
    let buffered_before = room_a.log().log_state().await;
    assert_eq!(
        buffered_before.buffered, 1,
        "the edit is buffered, not yet flushed"
    );

    // Process A's session dies without telling A. Dropping the lease is
    // what that looks like: it detaches and drops the pooled connection
    // that held `pg_advisory_lock`, which is what makes the lock available
    // to somebody else. `catalog_a.writer_epoch()` -- the value A's own
    // fenced writes check -- lives in a separate `AtomicI64` on the
    // catalogue, untouched by this drop, which is exactly the bug class the
    // fence exists to catch: A still believes it holds epoch `epoch_a`.
    drop(writer_a);

    // Process B: a fresh catalogue against the same database, claiming the
    // lease now that it is free. This is "a second catalogue claims the
    // writer lease" and it durably bumps the epoch out from under A.
    let catalog_b = Arc::new(connected(&url).await);
    let writer_b = catalog_b
        .claim_writer()
        .await
        .expect("B can claim the lease once A's session is gone");
    assert!(
        writer_b.epoch() > epoch_a,
        "B's claim must durably move the epoch forward, or this test proves nothing"
    );

    // A's flush is fenced against a durable epoch that has moved. Nothing
    // about A's own view (its buffer, its in-memory epoch) told it this
    // happened; the fence is the only thing that catches it.
    let flushed = room_a.log().flush(FlushReason::MaxAge).await;
    match flushed {
        Err(SequencerError::Storage(PgError::Ownership(_))) => {}
        Err(SequencerError::Fenced(_)) => {
            // §5.1's ambiguous-outcome branch would also reach this if a
            // fresh sequence read disagreed with what A expected; either
            // shape of error is the fence doing its job.
        }
        other => panic!("expected the flush to be refused by the epoch fence, got {other:?}"),
    }

    // The refusal must not have written anything: retrying does not create
    // a torn or partial row, and the document's counters have not moved.
    let head_after_failed_flush = catalog_b
        .log_head(room_a.document_id)
        .await
        .expect("read the log head with the new writer's catalogue");
    assert_eq!(
        head_after_failed_flush.update_sequence, head_before.update_sequence,
        "a refused flush must not advance the row counter"
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_updates WHERE document_id=$1")
            .bind(room_a.document_id)
            .fetch_one(catalog_b.pool())
            .await
            .expect("count rows");
    assert_eq!(rows, 0, "no row was written by the refused flush");

    // Retrying does not somehow succeed the second time either: A's fence is
    // durably lost, not merely delayed, until it is re-admitted.
    let retried = room_a.log().flush(FlushReason::MaxAge).await;
    assert!(
        retried.is_err(),
        "a second attempt from the fenced-out process must also be refused"
    );
    let still_buffered = room_a.log().log_state().await;
    assert_eq!(
        still_buffered.buffered, 1,
        "a refused flush leaves the buffer exactly as it was, per §7 step 5's failure case"
    );

    // -- re-admission under the new owner ---------------------------------
    //
    // Process B opens the same document fresh, from storage, with no memory
    // of A's buffer (a different `Registry` has never heard of it). This is
    // "re-admitted from storage under the new owner".
    let registry_b = Registry::new(
        catalog_b.clone(),
        blobs.clone(),
        config.clone(),
        "deployment-b".into(),
    );
    let rooms_b = Rooms::new(
        catalog_b.clone(),
        blobs.clone(),
        config.clone(),
        registry_b.clone(),
    );
    let room_b = rooms_b
        .get(slug)
        .await
        .expect("B can open the same document");

    // No torn row, no gap: the head vector B sees is exactly the head vector
    // from before A ever buffered anything, because nothing A buffered ever
    // became durable. This is not "the relayed text disappeared" (§10 says
    // revocation does not unsay what was relayed, and this test never
    // asserts anything about what A's subscribers received) -- it is that
    // the log itself, the only thing persistence promises, was never torn.
    //
    // Compared as decoded `VersionVector`s, not as raw bytes: a document
    // with neither a base nor a row (this one, before anything is ever
    // flushed) reports its vector as literally empty bytes from
    // `log_head`, which is a "nothing yet" sentinel, not a Loro encoding --
    // whereas the sequencer always holds a decoded `VersionVector` and
    // re-encodes it in Loro's own canonical form, which is not the same
    // bytes for an empty vector even though both mean the same thing.
    let state_b = room_b.log().log_state().await;
    assert_eq!(
        decode_or_default(&state_b.log_vector),
        decode_or_default(&head_before.vector),
        "re-admission must land on exactly the last durable vector, not one row short and not corrupted"
    );
    assert_eq!(state_b.next_sequence, head_before.update_sequence + 1);

    // The document is still readable after the handover, not merely
    // "present": a build under the new owner succeeds.
    room_b
        .projection()
        .await
        .expect("the re-admitted log still projects cleanly");

    // And the new owner is not merely a read replica: it can go on being the
    // log. This is the recovery half of the failure mode, not just its
    // detection.
    let mut outbox_b = Outbox::new();
    let batch_b = outbox_b.edit("written under the new owner");
    assert!(matches!(
        room_b
            .ingest(2, "account:editor", "account:editor", 1, batch_b)
            .await,
        Ingested::Accepted
    ));
    let written = room_b
        .log()
        .flush(FlushReason::Shutdown)
        .await
        .expect("B, the current owner, can flush");
    assert_eq!(written, Some(head_before.update_sequence + 1));
}

// -- 2. idle deployment issues no queries beyond the lease keepalive -------
// (§8.6, §12, §14.2)
//
// §8.6 and §12 delete the job queue, the room sweep, the per-socket tick and
// the egress byte accounting (`CostMeter::socket_bytes` and the socket
// budget's per-state charge) specifically so that a deployment with nothing
// open costs nothing. Searching this codebase for what is left running on a
// clock (`grep -n 'tokio::time::interval' crates/librepaper/src/server/serve.rs`)
// finds exactly two tickers: an hourly retention janitor (not spawned unless
// `--expire-after` is set, and irrelevant here since it is not spawned), and
// the one-second-per-tick §8.6 sweeper, which this test reproduces directly
// (`Rooms::housekeep`) since it is the only piece of the real periodic work
// that matters while idle. `storage/worker.rs`'s own module comment states
// the property under test in so many words: "an idle deployment issues no
// queries beyond the lease connection". There is, in fact, no periodic
// keepalive QUERY in this codebase at all: `WriterLease` holds one
// PostgreSQL session open for the life of the process and never pings it on
// a timer (`grep -rn keepalive crates/librepaper/src` finds nothing but the
// two comments naming this test); `Server::verify_writer` runs the
// lease's `SELECT 1` reactively, from the socket handler, only when a
// `doc-update` frame arrives. So the number this bound is "worked out from"
// is not an interval -- there is none to multiply by a window -- it is the
// count of statements this test's own setup and measurement issue against
// the held connection, which is zero beyond the two samples themselves. The
// idle window below therefore has to reproduce zero real background
// queries, not some small rate of them; if it does not, that is a
// regression to name and locate, not a bound to loosen.
mod idle {
    use super::*;

    /// About a minute by default: long enough to see multiple ticks of the
    /// one-second sweeper and the worker's channel `recv` sitting parked,
    /// short enough for a routine run. The release gate this test stands in
    /// for is the full ten minutes the spec names; set
    /// `LIBREPAPER_TEST_IDLE_SECONDS` to run that before a release.
    fn idle_window() -> Duration {
        let seconds = std::env::var("LIBREPAPER_TEST_IDLE_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(60);
        Duration::from_secs(seconds)
    }

    /// Whether `pg_stat_statements` is installed on this server. When it is,
    /// its `calls` counters are updated synchronously by the executor hook
    /// that runs each statement, so a delta between two samples is exact.
    async fn has_pg_stat_statements(pool: &sqlx::PgPool) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='pg_stat_statements')",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false)
    }

    /// The total call count `pg_stat_statements` has recorded for this
    /// database, excluding the monitoring queries this function issues
    /// itself (matched by a fragment unique to it), so that taking a sample
    /// is not itself "a query the deployment issued".
    async fn statement_calls(pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar(
            "SELECT COALESCE(SUM(calls),0)::bigint FROM pg_stat_statements pss \
             JOIN pg_database d ON d.oid = pss.dbid \
             WHERE d.datname = current_database() \
               AND pss.query NOT ILIKE '%pg_stat_statements%'",
        )
        .fetch_one(pool)
        .await
        .expect("pg_stat_statements is installed; this query only fails if it is not")
    }

    /// One statement another backend ran during the idle window: exactly
    /// what to name when this test fails.
    struct Straggler {
        pid: i32,
        query_start: time::OffsetDateTime,
        query: String,
    }

    impl std::fmt::Display for Straggler {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                formatter,
                "pid {} at {}: {}",
                self.pid,
                self.query_start,
                self.query.trim()
            )
        }
    }

    /// Every statement `pg_stat_activity` has recorded starting at or after
    /// `since`, from a backend other than `conn` itself.
    ///
    /// This is the fallback used when `pg_stat_statements` is not installed
    /// (the case in an ordinary local PostgreSQL, including the one this
    /// test was developed against). The brief for this file suggested
    /// `pg_stat_database.xact_commit`, sampled before and after, as that
    /// fallback; an earlier version of this test used exactly that and
    /// failed under an idle window with PostgreSQL's own statement log
    /// proving byte-for-byte that nothing ran. `xact_commit` is updated by
    /// the same batched shared-memory stats subsystem `pg_stat_statements`
    /// is not: a backend's commit can sit unflushed to that counter for up
    /// to `PGSTAT_MIN_INTERVAL` (10 seconds, unconfigurable) after it
    /// happens, so a counter delta taken across a one-minute idle window
    /// can include a setup-phase commit that simply had not posted yet at
    /// the first sample -- a false positive `pg_stat_statements` does not
    /// have, because its counters are updated synchronously by the executor
    /// hook that runs the statement, not by the deferred stats subsystem.
    /// `pg_stat_activity` is populated the same synchronous way `
    /// pg_stat_statements` is (it is live backend state, not a batched
    /// counter), which is why this is the fallback rather than a
    /// resurrected `xact_commit` reading with a bigger fudge factor: a
    /// bigger fudge factor hides the exact failure this test exists to
    /// catch.
    async fn queries_since(
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        since: time::OffsetDateTime,
    ) -> Vec<Straggler> {
        sqlx::query_as::<_, (i32, time::OffsetDateTime, String)>(
            "SELECT pid, query_start, query FROM pg_stat_activity \
             WHERE datname = current_database() AND pid <> pg_backend_pid() \
               AND query_start >= $1 \
             ORDER BY query_start",
        )
        .bind(since)
        .fetch_all(&mut **conn)
        .await
        .expect("read pg_stat_activity for the test database")
        .into_iter()
        .map(|(pid, query_start, query)| Straggler {
            pid,
            query_start,
            query,
        })
        .collect()
    }

    /// The database server's own clock, from the same connection
    /// `queries_since` will use, so "since" and "query_start" are never
    /// compared across a clock skew between this process and PostgreSQL's.
    async fn server_now(
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
    ) -> time::OffsetDateTime {
        sqlx::query_scalar("SELECT now()")
            .fetch_one(&mut **conn)
            .await
            .expect("read the server clock")
    }

    /// Open a document, ingest into it and flush it, then let it go idle
    /// while a real `Rooms`, `Registry` and `storage::worker::Worker` run
    /// exactly as `server/serve.rs` wires them, for the idle window. Assert
    /// that PostgreSQL recorded no real activity beyond the two samples that
    /// bracket the window.
    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; run with --test-threads=1"]
    async fn idle_deployment_issues_no_queries_beyond_the_lease_keepalive() {
        let Ok(url) = std::env::var("LIBREPAPER_TEST_POSTGRES_URL") else {
            return;
        };

        let blobs = blobs();
        let config = Arc::new(Configuration::default());

        let catalog = Arc::new(connected(&url).await);
        truncate(&catalog).await;
        let _writer = catalog
            .claim_writer()
            .await
            .expect("this process is the only writer for the idle window");

        let owner = seed_account(&catalog, "idle-owner").await;
        let slug = "idle-deployment";
        seed_document(&catalog, owner, slug).await;

        let registry = Registry::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            "deployment-idle".into(),
        );
        // `Rooms` is not `Clone` (it holds a live `Mutex<HashMap<..>>` of open
        // rooms); `server/serve.rs` gets the same sharing by wrapping the
        // whole `Server` in an `Arc` and cloning that, so this does the same
        // to the one piece it needs shared between the setup below and the
        // sweeper task.
        let rooms = Arc::new(Rooms::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            registry.clone(),
        ));

        // The worker's startup scan (§8.6) runs once, immediately, and is
        // real work the deployment is expected to do -- it must happen and
        // settle before the idle window's baseline sample, not during it.
        let (worker, handle) = Worker::new(
            catalog.clone(),
            blobs.clone(),
            registry.clone(),
            config.clone(),
        );
        // Wired exactly as `server/serve.rs` wires it, so the §8.4 flush
        // trigger is live for the idle window rather than a handle nobody
        // holds. This is the gate that stops compaction ever being
        // rescheduled onto a clock: a timer would show up below as queries
        // in a window where the deployment is meant to be silent.
        registry.compacts_through(handle);
        tokio::spawn(worker.run());

        // Open the document, type into it and let it become durable, then
        // drop the handle: a room with no subscribers and an empty buffer is
        // what `Rooms::housekeep`'s idle-retirement (§8.6) looks for, and
        // this is what "let it go idle" means for a document that was just
        // open.
        {
            let room = rooms.get(slug).await.expect("open the document");
            let mut outbox = Outbox::new();
            let batch = outbox.edit("a sentence before the deployment goes idle");
            assert!(matches!(
                room.ingest(1, "account:editor", "account:editor", 1, batch)
                    .await,
                Ingested::Accepted
            ));
            room.log().flush(FlushReason::Shutdown).await.expect(
                "flush the one edit so nothing is left buffered going into the idle window",
            );
        }

        // The real periodic work (§8.6): one tick a second, flush what is
        // due, retire what nobody holds. `server/serve.rs` spawns exactly
        // this loop; reproducing it here (rather than only calling
        // `housekeep` once) is what makes "idle" mean "the real ticking
        // deployment", not "a test that never asked".
        let sweeping = rooms.clone();
        let sweeper = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if !sweeping.registry().all().await.is_empty() {
                    sweeping.housekeep().await;
                }
            }
        });

        // Let the startup scan and the first housekeeping tick settle
        // before the baseline sample, so neither is counted against the
        // idle window.
        tokio::time::sleep(Duration::from_secs(2)).await;

        let has_statements = has_pg_stat_statements(catalog.pool()).await;
        // A dedicated connection for the `pg_stat_activity` fallback, held
        // for the rest of the test so its own backend pid stays constant
        // and `queries_since` can reliably exclude exactly it and nothing
        // else.
        let mut monitor = catalog
            .pool()
            .acquire()
            .await
            .expect("a dedicated monitoring connection");
        // `server_now` is itself a statement, and `pg_stat_statements`
        // counts it like any other, so it has to happen before the baseline
        // sample rather than after it. Taken the other way round, this test
        // measures its own monitoring and reports a call delta of exactly
        // one on a deployment that issued nothing at all.
        let since = server_now(&mut monitor).await;
        let before_calls = if has_statements {
            Some(statement_calls(catalog.pool()).await)
        } else {
            None
        };

        let window = idle_window();
        tokio::time::sleep(window).await;

        let outcome = if let Some(before) = before_calls {
            let after = statement_calls(catalog.pool()).await;
            (after - before, Vec::new())
        } else {
            (0, queries_since(&mut monitor, since).await)
        };

        sweeper.abort();

        let (delta, stragglers) = outcome;
        assert!(
            delta <= 0 && stragglers.is_empty(),
            "idle window of {window:?} recorded activity this deployment should not have \
             issued: pg_stat_statements call delta {delta}, and {} straggler statement(s) in \
             pg_stat_activity:\n{}\n\
             this codebase has no periodic lease-keepalive QUERY (see this module's doc \
             comment) so the correct bound is zero, not a per-minute allowance -- find what \
             issued each one rather than raising this number. Run \
             LIBREPAPER_TEST_IDLE_SECONDS=600 for the full release-gate window.",
            stragglers.len(),
            stragglers
                .iter()
                .map(Straggler::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

// -- 3. authority revoked mid-buffer flushes the buffer and closes ---------
// (§10, §14.2)
//
// §10's row for this failure mode: "Buffered work flushes; socket closed; no
// further ingest." `Server::reauthorize` (server/socket.rs) is where this
// lives: on any socket that is no longer allowed, it flushes with
// `FlushReason::AuthorityRevoked` -- unconditionally, not riding on
// `Room::leave`'s own last-subscriber flush -- and only then closes the
// socket and leaves the room. The comment on that call explains why the
// flush cannot wait for `leave` to decide it is the last one out: the editor
// being cut off here is the one person who can never reconnect to put the
// work back (§6.4's reconciliation on reconnect is not available to someone
// whose authority is gone), so if the flush were conditional on this being
// the last subscriber, a document with another live editor would lose a
// revoked author's buffered typing outright the moment the process died
// before the next ordinary trigger.
//
// Proving that requires the real thing, not a stand-in: two sockets on the
// real `Server`, not one, so that `Room::leave`'s own conditional flush
// (which fires regardless of `AuthorityRevoked` whenever a socket happens to
// be the last one) cannot be the thing making this test pass. If it were,
// deleting the unconditional flush in `reauthorize` -- exactly the change
// this test exists to catch -- would leave the test green. The two sockets
// here are the document's owner (bearer-token session, never revoked by
// anything this test does) and a second, signed-in editor whose only route
// to Editor is an edit link (minted and later revoked through the real
// `handle_share` route in `server/sharing.rs`, the same route a browser's
// share dialog calls) -- a signed-in identity is required to hold anything
// through a link at all, per `Ceiling`'s own doc comment in
// `document/store.rs`: "publishing always asks for a sign-in first, so a
// link never edits" on its own. Revoking the link takes only that second
// editor's authority, so the owner's socket is always there as the second
// subscriber `Room::leave` would otherwise see.
mod revocation {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    type WsStream = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// The one protocol string this server speaks (§6.1); not re-exported
    /// from `server::socket`, which keeps its own copy `pub(super)`, so this
    /// is the wire's own literal rather than a borrowed constant.
    const PROTOCOL: &str = "librepaper.room.v3";

    /// An `Authorization: Bearer` header carrying a device token for the
    /// owner account -- the credential a signed-in browser's fetch and
    /// websocket handshake both send, and the one `Server::viewer` actually
    /// resolves a role from. Mirrors `history_frontier_tests.rs`'s
    /// `owner_bearer`, which cannot be reused directly: that helper is
    /// private to a `#[cfg(test)]` module compiled inside the crate, and
    /// this file is a separate integration-test binary outside it.
    ///
    /// Generalized over the account rather than hard-coded to the owner,
    /// because the second editor in this test needs one too: `Ceiling`'s own
    /// doc comment in `document/store.rs` says why an edit link is not by
    /// itself enough -- "publishing always asks for a sign-in first, so a
    /// link never edits" -- an edit link only raises a caller who is already
    /// signed in and inside `--publishers` up to Editor. A guest who
    /// presents nothing but the link key, signed in to nobody, can never
    /// reach `may_edit` at all, so the second socket here has to be a real
    /// account too, distinct from the owner, holding the link rather than
    /// any grant of its own -- which is exactly what revoking the link is
    /// then a real test of.
    fn bearer_token(key: &[u8], account_id: Uuid, handle: &str, session_generation: i64) -> String {
        let identity = Identity {
            provider: PROVIDER_GITHUB.into(),
            id: account_id.to_string(),
            handle: handle.into(),
            name: handle.into(),
            picture: String::new(),
            session_generation: session_generation.to_string(),
        };
        sign_device(key, &identity, now_unix_here() + 3600)
    }

    /// `crate::util::now_unix` is private to the library; this file has no
    /// access to it, and the device token's expiry only needs to outlive one
    /// short-lived test, so a wall-clock read through `std::time` says the
    /// same thing without reaching for it.
    fn now_unix_here() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the system clock is after 1970")
            .as_secs() as i64
    }

    /// Reads frames off `ws` until one with `"type": kind` arrives, silently
    /// skipping anything else (a `hello`, a `doc-peers` broadcast the other
    /// socket's own join triggered) -- both sockets in this test share one
    /// room, so frames meant for "whoever is listening" land on each of them
    /// and are not what either wait is for. Panics past `budget` frames or
    /// past `timeout`, so a protocol change here fails loudly instead of
    /// hanging the suite.
    async fn wait_for_type(ws: &mut WsStream, kind: &str, timeout: Duration) -> Value {
        let deadline = tokio::time::Instant::now() + timeout;
        for _ in 0..64 {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = tokio::time::timeout(remaining, ws.next())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for a {kind:?} frame"))
                .expect("the socket ended before a {kind:?} frame arrived")
                .expect("a websocket transport error while waiting for {kind:?}");
            let WsMessage::Text(text) = frame else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).expect("a frame is JSON");
            if value["type"] == kind {
                return value;
            }
        }
        panic!("gave up waiting for a {kind:?} frame after 64 unrelated ones");
    }

    /// Whether `ws` was closed by the server -- a real `Outgoing::Close`
    /// reaching this client, or the transport ending outright, either of
    /// which is what §10's "socket closed" means from the outside. Any
    /// ordinary frame in between (a stray broadcast) is not itself the
    /// answer and is skipped exactly as `wait_for_type` skips one.
    async fn wait_for_close(ws: &mut WsStream, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match tokio::time::timeout(remaining, ws.next()).await {
                Ok(Some(Ok(WsMessage::Close(_)))) => return true,
                Ok(None) => return true,
                Ok(Some(Err(_))) => return true,
                Ok(Some(Ok(WsMessage::Text(_)))) => continue,
                Ok(Some(Ok(_))) => continue,
                Err(_) => return false,
            }
        }
    }

    /// Base64, the same encoding `room::encode_update` uses on the wire --
    /// that function is `pub` but not re-exported to this crate's top level,
    /// so this is the same one line rather than another `lib.rs` export for
    /// a helper this test needs exactly twice.
    fn base64_encode(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    async fn document_update_rows(catalog: &PostgresCatalog, document_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM document_updates WHERE document_id=$1")
            .bind(document_id)
            .fetch_one(catalog.pool())
            .await
            .expect("count document_updates rows")
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; run with --test-threads=1"]
    async fn authority_revoked_mid_buffer_flushes_the_buffer_and_closes_only_that_socket() {
        let Ok(url) = std::env::var("LIBREPAPER_TEST_POSTGRES_URL") else {
            return;
        };

        let blobs = blobs();
        let config = Arc::new(Configuration::default());
        let catalog = Arc::new(connected(&url).await);
        truncate(&catalog).await;
        let writer = catalog
            .claim_writer()
            .await
            .expect("this process is the only writer for this test");

        // The owner account this document belongs to, kept as the full
        // catalogue record (not `seed_account`'s bare `Uuid`) because the
        // bearer token below needs its `session_generation` too.
        let owner = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some("subject-revoke-owner".into()),
                handle: "owner".into(),
                display_name: "Owner".into(),
                email: None,
            })
            .await
            .expect("create the owner account");
        // The second editor: a real, signed-in, `--publishers`-allowed
        // account distinct from the owner. It holds no grant of its own --
        // only the edit link, once minted below -- so revoking that link is
        // what takes its authority away, and `bearer_token`'s doc comment is
        // why it needs to be signed in at all rather than a bare link guest.
        let second_editor = catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some("subject-revoke-editor".into()),
                handle: "second-editor".into(),
                display_name: "Second Editor".into(),
                email: None,
            })
            .await
            .expect("create the second editor's account");
        let slug = "authority-revoked-mid-buffer";
        seed_document(&catalog, owner.id, slug).await;
        let document_id = catalog
            .document_by_slug(slug)
            .await
            .expect("read the document row")
            .expect("the document this test just seeded")
            .id;

        let registry = Registry::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            "deployment-revoke".into(),
        );
        let rooms = Rooms::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            registry.clone(),
        );
        let (worker, background) = Worker::new(
            catalog.clone(),
            blobs.clone(),
            registry.clone(),
            config.clone(),
        );
        tokio::spawn(worker.run());
        let store = Store::open_with_catalog(
            blobs.clone(),
            config.clone(),
            catalog.clone(),
            registry.clone(),
        )
        .await
        .expect("open the store against the same catalogue");

        // `provider_configured` (server/mod.rs) needs a non-empty client id
        // before it grants the owner rung to a GitHub-provider bearer;
        // nothing here ever calls out to GitHub, since a device token never
        // goes through `check_token`. Mirrors history_frontier_tests.rs.
        let app = GithubApp {
            client_id: "test-client".into(),
            ..GithubApp::default()
        };
        let key = vec![7u8; 32];
        let mut server = Server::new(
            store,
            rooms,
            background,
            std::collections::HashMap::new(),
            app,
            key.clone(),
            config.clone(),
            Policy::parse_publishers("owner,second-editor").expect("parse the publisher policy"),
            Policy::parse(""),
        );
        server.install_writer(writer);
        let server = Arc::new(server);

        // A real loopback listener and a real router, the same two calls
        // `server/serve.rs` makes, so the socket and the share route under
        // test are reached exactly as a browser reaches them -- not through
        // any private, test-only seam.
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind a loopback port for this test's own server");
        let addr = listener.local_addr().expect("read the bound port");
        let make_service = server
            .clone()
            .router()
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, make_service).await;
        });

        let base = format!("http://{addr}");
        let http = reqwest::Client::new();
        let bearer = bearer_token(&key, owner.id, "owner", owner.session_generation);
        let second_editor_bearer = bearer_token(
            &key,
            second_editor.id,
            "second-editor",
            second_editor.session_generation,
        );

        // -- mint the edit link through the real sharing route ---------------
        let minted = http
            .post(format!("{base}/api/documents/{slug}/share"))
            .header("authorization", format!("Bearer {bearer}"))
            .json(&json!({"link": {"role": "edit"}}))
            .send()
            .await
            .expect("POST the share route to mint an edit link");
        assert_eq!(minted.status(), 200, "minting an edit link must succeed");
        let minted: Value = minted.json().await.expect("the share route answers JSON");
        let edit_key = minted["links"]["editor"]["key"]
            .as_str()
            .expect("a minted edit link carries its own key back to its owner")
            .to_string();
        assert!(!edit_key.is_empty());

        // -- the owner's own socket: never revoked by anything below ---------
        let mut owner_request = format!("ws://{addr}/ws/{slug}")
            .into_client_request()
            .expect("build the owner's websocket handshake request");
        owner_request.headers_mut().insert(
            "authorization",
            format!("Bearer {bearer}").parse().expect("a bearer header"),
        );
        let (mut owner_ws, _) = tokio_tungstenite::connect_async(owner_request)
            .await
            .expect("the owner's socket must be admitted");
        wait_for_type(&mut owner_ws, "hello", Duration::from_secs(5)).await;
        owner_ws
            .send(WsMessage::Text(
                json!({"type": "doc-open", "vector": "", "protocol": PROTOCOL})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send the owner's doc-open");
        let initial = wait_for_type(&mut owner_ws, "doc-state", Duration::from_secs(5)).await;
        assert!(initial["durableVector"].is_string());
        // A returning client that already covers the whole head receives the
        // buffer-only doc-state branch. It still needs durable coverage even
        // when no later flush will occur to announce it.
        owner_ws
            .send(WsMessage::Text(
                json!({"type": "doc-open", "vector": initial["vector"], "protocol": PROTOCOL})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("rejoin from the complete head vector");
        let covered = wait_for_type(&mut owner_ws, "doc-state", Duration::from_secs(5)).await;
        assert!(covered.get("base").is_none() && covered.get("ref").is_none());
        assert_eq!(covered["updates"], json!([]));
        assert_eq!(covered["durableVector"], initial["durableVector"]);
        assert_eq!(covered["durableVector"], covered["vector"]);

        // -- the second editor: signed in (see `bearer_token`'s doc comment
        //    on why that is required), but with no grant of their own,
        //    admitted to Editor only by the edit link about to be revoked.
        //    The query parameter is `k` (sharing.rs's `LINK_PARAM`): a
        //    websocket handshake is the one request a browser cannot attach
        //    a custom header to, so the link key rides in the query string
        //    there and nowhere else.
        let mut editor_request = format!("ws://{addr}/ws/{slug}?k={edit_key}")
            .into_client_request()
            .expect("build the linked editor's websocket handshake request");
        editor_request.headers_mut().insert(
            "authorization",
            format!("Bearer {second_editor_bearer}")
                .parse()
                .expect("a bearer header"),
        );
        let (mut editor_ws, _) = tokio_tungstenite::connect_async(editor_request)
            .await
            .expect("the link must admit a second editor");
        wait_for_type(&mut editor_ws, "hello", Duration::from_secs(5)).await;
        editor_ws
            .send(WsMessage::Text(
                json!({"type": "doc-open", "vector": "", "protocol": PROTOCOL})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send the linked editor's doc-open");
        wait_for_type(&mut editor_ws, "doc-state", Duration::from_secs(5)).await;

        // -- the linked editor types. Nothing here crosses a flush trigger
        //    (30s max age, 5s quiet, 1 MiB of bytes), so this sits in the
        //    sequencer's buffer, relayed to the owner but not yet durable --
        //    exactly what §10 calls "buffered work".
        let mut outbox = Outbox::new();
        let batch = outbox.edit("the sentence the revoked editor never got to save");
        editor_ws
            .send(WsMessage::Text(
                json!({
                    "type": "doc-update",
                    "update": base64_encode(&batch),
                    "seq": 1,
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send the buffered edit");
        // The sequencer relays an accepted update to every other editor
        // (§5 step 5) rather than acking the sender directly; waiting for
        // that relay on the owner's own socket is this test's proof that
        // the ingest has actually happened server-side before it goes on to
        // check what is and is not durable yet, with no sleep to race.
        wait_for_type(&mut owner_ws, "doc-update", Duration::from_secs(5)).await;

        assert_eq!(
            document_update_rows(&catalog, document_id).await,
            0,
            "the edit is buffered, relayed to the other editor, and NOT yet a durable row"
        );

        // -- revoke the edit link through the real sharing route -------------
        // This is `handle_share`'s revoke branch (server/sharing.rs), the
        // same one a browser's share dialog posts to, and it is what calls
        // `Server::reauthorize` at the end of its own write -- the function
        // under test here, not a hand-rolled stand-in for it.
        let revoked = http
            .post(format!("{base}/api/documents/{slug}/share"))
            .header("authorization", format!("Bearer {bearer}"))
            .json(&json!({"revoke": "edit"}))
            .send()
            .await
            .expect("POST the share route to revoke the edit link");
        assert_eq!(revoked.status(), 200, "revoking the edit link must succeed");

        // -- 1: the buffered work is now a durable row ------------------------
        // Polled rather than asserted once: `reauthorize`'s flush runs inside
        // the same request that answered 200 above, but this test does not
        // rely on that ordering being synchronous forever, only on it
        // finishing promptly.
        let mut rows_after_revoke = document_update_rows(&catalog, document_id).await;
        let poll_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while rows_after_revoke == 0 && tokio::time::Instant::now() < poll_deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            rows_after_revoke = document_update_rows(&catalog, document_id).await;
        }
        assert_eq!(
            rows_after_revoke, 1,
            "authority revoked mid-buffer must flush the buffered edit to a durable row \
             (SPEC-server-is-a-log.md §10: 'Buffered work flushes; socket closed; no further \
             ingest') -- if this is 0, `Server::reauthorize`'s unconditional \
             `FlushReason::AuthorityRevoked` flush was made conditional again, exactly the \
             regression this test exists to catch. The owner's socket stayed open the whole \
             time specifically so `Room::leave`'s own last-subscriber flush cannot be what \
             made this pass instead."
        );

        // -- 2: the socket was closed -----------------------------------------
        assert!(
            wait_for_close(&mut editor_ws, Duration::from_secs(5)).await,
            "the revoked editor's socket must receive a close (Outgoing::Close) once its \
             authority is gone"
        );

        // -- 3: the owner's socket, never revoked, is unaffected --------------
        owner_ws
            .send(WsMessage::Text(json!({"type": "ping"}).to_string().into()))
            .await
            .expect("the owner's socket must still take a frame");
        wait_for_type(&mut owner_ws, "pong", Duration::from_secs(5)).await;

        // -- 4: no further ingest from the revoked socket is accepted ---------
        // `run_socket`'s reader loop re-authorizes every incoming frame
        // before acting on it (server/socket.rs), so even a frame that still
        // reaches the transport after the close (the server's own read half
        // is not necessarily torn down the instant its write half queues a
        // close) is refused before it reaches `room.ingest`. The `authorized_at`
        // cache that re-check can otherwise short-circuit on is one second
        // old; this waits past it so the recheck is the real, catalogue-backed
        // one rather than a stale "still fine" answer.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let second_batch = outbox.edit("text a socket with no authority must never get to save");
        let _ = editor_ws
            .send(WsMessage::Text(
                json!({
                    "type": "doc-update",
                    "update": base64_encode(&second_batch),
                    "seq": 2,
                })
                .to_string()
                .into(),
            ))
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            document_update_rows(&catalog, document_id).await,
            1,
            "a revoked socket must not be able to add a second row: its edit must be refused \
             before `room.ingest`, not merely left unflushed"
        );
    }
}
