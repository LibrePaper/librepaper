//! End-to-end coverage for `storage::worker`'s move from one spawned timer
//! per retry and per scheduled deletion to the bounded `schedule::Deadlines`
//! map. Everything here is about
//! deletion specifically, because deletion is the task whose durable state
//! (`documents.status = 'deleting'`) is the only record of it: unlike
//! compaction, nothing re-triggers it on the next edit, so if the in-memory
//! bound in front of it ever silently ate a wake-up, the document would
//! simply never leave the trash.
//!
//! What this file proves: that the bounded channel overflowing, the same
//! task being asked for over and over, a grace period not yet up, a cold
//! restart with no wake-up at all, a restoration racing the purge, a
//! severed database connection, and a document written behind the scan's
//! own cursor, each still end with the durable row in the state it should
//! be in. What this file does not prove: the exact shape of `Deadlines`
//! itself (its doubling, its dedup, its pruning) -- that is
//! `schedule_tests.rs`, pure and deterministic. This file cannot freeze the
//! clock (it drives a real worker against a real database over real IO), so
//! every wait here is a bounded poll loop with a generous but finite
//! timeout, never a bare sleep for the deletion grace period itself.
//!
//! Every test needs `LIBREPAPER_TEST_POSTGRES_URL` and is `#[ignore]`d
//! without it, the same convention as `log/recovery.rs` and
//! `server/comment_http_tests.rs`. Point it at a throwaway database:
//! `docker exec librepaper-postgres psql -U postgres -c 'CREATE DATABASE lp_worker_spike'`.
//! Every test truncates the tables it uses and claims the single writer
//! lease, so this file must run with `--test-threads=1`.

use std::sync::Arc;
use std::time::Duration;

use sqlx::Connection;
use uuid::Uuid;

use crate::config::Configuration;
use crate::log::Registry;
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::postgres::{
    NewAccount, NewDocument, PendingWorkCursor, PostgresCatalog, PostgresOptions,
};
use crate::storage::worker::{Handle, Task, Worker};

const TRUNCATE: &str = "TRUNCATE documents,accounts CASCADE";

async fn connect(url: String) -> Arc<PostgresCatalog> {
    connect_with(PostgresOptions::new(url)).await
}

async fn connect_with(options: PostgresOptions) -> Arc<PostgresCatalog> {
    let catalog = Arc::new(
        PostgresCatalog::connect(options)
            .await
            .expect("connect to the throwaway database"),
    );
    catalog.migrate().await.expect("apply the current schema");
    sqlx::query(TRUNCATE)
        .execute(catalog.pool())
        .await
        .expect("start each test from an empty database");
    catalog
}

/// One deployment's worth of durable state and blob storage, holding the
/// single writer lease for as long as the test needs it. No `Sequencer` or
/// `Room` here: deletion purges blobs and a catalogue row, and nothing this
/// file tests reads or writes through the log at all.
struct Deployment {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    account_id: Uuid,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn deployment(slug: &str) -> Option<Deployment> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    Some(deployment_over(connect(url).await, slug).await)
}

/// [`deployment`], but over a catalogue whose pool holds at most two
/// physical connections, and with the working one's backend pid handed back
/// so a test can sever it. Two rather than one because `claim_writer` holds
/// a pooled connection for the life of the lease and never returns it, so a
/// pool of one would leave nothing for any other query to run on. That
/// leaves exactly one connection doing every scan, which is what makes
/// severing it deterministic.
async fn deployment_with_one_connection(slug: &str) -> Option<(Deployment, i32)> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let mut options = PostgresOptions::new(url);
    options.max_connections = 2;
    let deployment = deployment_over(connect_with(options).await, slug).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(deployment.catalog.pool())
        .await
        .expect("the sole pooled connection's backend pid");
    Some((deployment, pid))
}

async fn deployment_over(catalog: Arc<PostgresCatalog>, slug: &str) -> Deployment {
    let writer = catalog
        .claim_writer()
        .await
        .expect("claim the single writer lease");
    let account = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(slug.into()),
            handle: slug.into(),
            display_name: "Spike".into(),
            email: None,
        })
        .await
        .expect("create the owner account");
    let objects = tempfile::tempdir().expect("a temp directory for this test's blob store");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
    Deployment {
        catalog,
        blobs,
        config: Arc::new(Configuration::default()),
        account_id: account.id,
        _writer: writer,
        _objects: objects,
    }
}

impl Deployment {
    /// A document already in the trash, backdated so its grace period ended
    /// `age` ago (a negative-looking `age` smaller than
    /// `storage::worker::DELETION_GRACE` leaves it still inside the grace).
    /// Backdating `deleted_at` directly, rather than waiting for real time
    /// to pass, is what lets this file test the grace boundary without a
    /// seven-day sleep.
    async fn document_deleted(&self, slug: &str, age: Duration) -> Uuid {
        let document = self
            .catalog
            .create_document(NewDocument {
                slug: slug.into(),
                owner_id: self.account_id,
                ownership_mode: "owned".into(),
                title: "Trash".into(),
                source_format: "markdown".into(),
                main_path: "paper.md".into(),
                settings: serde_json::json!({}),
            })
            .await
            .expect("create the document row");
        assert!(
            self.catalog
                .mark_document_deleting(document.id)
                .await
                .expect("mark the document deleting"),
            "a freshly created document is active, so marking it deleting must succeed"
        );
        sqlx::query("UPDATE documents SET deleted_at = now() - $2::interval WHERE id = $1")
            .bind(document.id)
            .bind(
                sqlx::postgres::types::PgInterval::try_from(age)
                    .expect("a test-sized age fits in an interval"),
            )
            .execute(self.catalog.pool())
            .await
            .expect("backdate the deletion so the grace boundary can be tested precisely");
        document.id
    }

    /// Teardown. The writer lease holds a pooled connection for as long as
    /// the deployment is alive and never returns it, while `close` waits for
    /// every connection to come back, so the lease goes first and the pool
    /// second. On a deployment sized for one working connection that
    /// ordering is the difference between closing and waiting forever.
    async fn close(self) {
        let catalog = self.catalog.clone();
        drop(self);
        catalog.close().await;
    }

    /// A fresh `Worker` and `Handle` over this deployment's catalogue and
    /// blob store, the same construction `serve` uses, with a `Registry` of
    /// its own since nothing here needs one document's sequencer to be
    /// shared across workers.
    fn worker(&self) -> (Worker, Handle) {
        let registry = Registry::new(
            self.catalog.clone(),
            self.blobs.clone(),
            self.config.clone(),
            "deployment".into(),
        );
        Worker::new(
            self.catalog.clone(),
            self.blobs.clone(),
            registry,
            self.config.clone(),
        )
    }

    async fn status(&self, document_id: Uuid) -> Option<String> {
        self.catalog
            .document(document_id)
            .await
            .expect("read the document row")
            .map(|document| document.status)
    }
}

/// Polls until `document_id`'s row is gone, or gives up after `patience` and
/// returns false. Never a bare sleep: the worker's own timing (backoff,
/// scan pagination) is exactly what this file must not assume anything
/// about.
async fn purged_within(catalog: &PostgresCatalog, document_id: Uuid, patience: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        if catalog
            .document(document_id)
            .await
            .expect("read the document row")
            .is_none()
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Grace period the fixtures back-date against. Kept local rather than
/// imported from `storage::worker`, since that constant's own value is not
/// what this file is testing -- only that the worker's behavior straddles
/// whatever it is correctly.
const DELETION_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// More wake-ups than the channel holds, asked for before the worker ever
/// starts consuming, so most of them are refused and only the coalesced
/// rescan bit survives. Every one of those documents must still be purged:
/// durable state (`status = 'deleting'`) is what the rescan re-discovers,
/// so an in-memory bound being reached must never mean a document is
/// silently abandoned in the trash.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_saturated_channel_still_purges_every_document_once_the_worker_runs() {
    let Some(deployment) = deployment("worker-saturation").await else {
        return;
    };

    // QUEUE is 256; comfortably more than that overflows it.
    const COUNT: usize = 300;
    let mut ids = Vec::with_capacity(COUNT);
    for index in 0..COUNT {
        let id = deployment
            .document_deleted(
                &format!("worker-saturation-{index}"),
                DELETION_GRACE + Duration::from_secs(60),
            )
            .await;
        ids.push(id);
    }

    let (worker, handle) = deployment.worker();
    // Every one of these is asked for before the worker has consumed a
    // single task, which is what saturates the channel and forces most of
    // them onto the single coalesced rescan bit instead of being queued.
    for &id in &ids {
        handle.ask(Task::Delete(id));
    }

    tokio::spawn(worker.run());

    for &id in &ids {
        assert!(
            purged_within(&deployment.catalog, id, Duration::from_secs(60)).await,
            "document {id} was never purged after the channel saturated"
        );
    }

    deployment.close().await;
}

/// Asking for the same document's deletion many times over purges it once
/// and leaves nothing behind to keep firing: a second wave of wake-ups
/// after the purge must not resurrect the row or error.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn duplicate_wake_ups_for_one_document_purge_it_once_and_leave_the_deployment_idle() {
    let Some(deployment) = deployment("worker-duplicate-wakeups").await else {
        return;
    };
    let id = deployment
        .document_deleted(
            "worker-duplicate-wakeups",
            DELETION_GRACE + Duration::from_secs(60),
        )
        .await;

    let (worker, handle) = deployment.worker();
    for _ in 0..50 {
        handle.ask(Task::Delete(id));
    }
    tokio::spawn(worker.run());

    assert!(
        purged_within(&deployment.catalog, id, Duration::from_secs(30)).await,
        "the repeatedly-asked-for document was never purged"
    );

    // Nothing left to do: asking again for a document that no longer exists
    // must be a harmless no-op, not a panic or a resurrected row.
    for _ in 0..10 {
        handle.ask(Task::Delete(id));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        deployment
            .catalog
            .document(id)
            .await
            .expect("read the document row")
            .is_none(),
        "asking again after the purge must not bring the row back"
    );

    deployment.close().await;
}

/// A document still inside its seven-day grace is not purged no matter how
/// often it is asked for, and repeated wake-ups for it do not accumulate
/// work the way one timer per observation used to. A document whose grace
/// really is up, asked for the same way, is purged.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_document_inside_its_grace_is_not_purged_and_one_past_it_is() {
    let Some(deployment) = deployment("worker-grace").await else {
        return;
    };
    let recent = deployment
        .document_deleted("worker-grace-recent", Duration::from_secs(60))
        .await;
    let overdue = deployment
        .document_deleted(
            "worker-grace-overdue",
            DELETION_GRACE + Duration::from_secs(60),
        )
        .await;

    let (worker, handle) = deployment.worker();
    // Ask for the still-fresh document many times over: if repeated
    // observations accumulated one timer apiece the way they used to, this
    // is exactly the pattern that would pile them up.
    for _ in 0..20 {
        handle.ask(Task::Delete(recent));
    }
    handle.ask(Task::Delete(overdue));
    tokio::spawn(worker.run());

    assert!(
        purged_within(&deployment.catalog, overdue, Duration::from_secs(30)).await,
        "the document whose grace period is up was never purged"
    );

    // The still-fresh document must not have been purged in the meantime.
    assert_eq!(
        deployment.status(recent).await.as_deref(),
        Some("deleting"),
        "a document inside its grace period must not be purged"
    );

    // Give any further scheduled retry a moment to run, then check again:
    // nothing should have changed, because the scheduled deadline for
    // `recent` is still days away.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        deployment.status(recent).await.as_deref(),
        Some("deleting"),
        "the document inside its grace period is still exactly where it was"
    );

    deployment.close().await;
}

/// The other half of the grace period, and the claim the unit tests cannot
/// make on their own: a deletion the worker deferred because its grace was
/// not up runs by itself when the grace ends, in the same process, with
/// nobody asking again and nothing restarting. `Worker::delete` records that
/// deadline by succeeding, and the worker's own `select!` sleeps on it.
///
/// Backdating `deleted_at` to two seconds short of the grace is what makes
/// this testable: the deadline the worker computes is two seconds away
/// rather than seven days, but it is computed by exactly the same code path
/// from exactly the same durable row.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_deferred_deletion_runs_when_its_grace_ends_without_being_asked_again() {
    let Some(deployment) = deployment("worker-grace-fires").await else {
        return;
    };
    let id = deployment
        .document_deleted(
            "worker-grace-fires",
            DELETION_GRACE - Duration::from_secs(2),
        )
        .await;

    let (worker, handle) = deployment.worker();
    handle.ask(Task::Delete(id));
    tokio::spawn(worker.run());

    // One ask, and it lands while the document is still inside its grace.
    // Anything that purges this document from here on is the deadline the
    // worker kept for itself.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        deployment.status(id).await.as_deref(),
        Some("deleting"),
        "the document must still be waiting out the last of its grace"
    );

    assert!(
        purged_within(&deployment.catalog, id, Duration::from_secs(30)).await,
        "the deferred deletion never ran; its grace deadline was lost"
    );

    deployment.close().await;
}

/// An owner can choose "delete forever" while the worker is holding the
/// ordinary grace deadline. The explicit wake-up must re-read durable state,
/// purge immediately, and remove the old deadline instead of being discarded
/// as a duplicate.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_hastened_deletion_overrides_grace_and_clears_its_deadline() {
    hastened_deletion_overrides_grace(false).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_durable_rescan_finds_hastened_deletion_behind_a_grace_deadline() {
    hastened_deletion_overrides_grace(true).await;
}

async fn hastened_deletion_overrides_grace(rescan: bool) {
    let Some(deployment) = deployment("worker-hasten").await else {
        return;
    };
    let id = deployment
        .document_deleted("worker-hasten", Duration::from_secs(1))
        .await;
    let (worker, handle) = deployment.worker();
    let running = tokio::spawn(worker.run());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.snapshot()["deadlines"].as_u64() == Some(0) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the worker never armed the document's grace deadline"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(deployment.catalog.hasten_deletion(id).await.unwrap());
    // A durable rescan is what remains when the bounded queue cannot carry
    // the original Delete task. Both paths must notice the changed timestamp.
    handle.ask(if rescan {
        Task::Scan(Default::default())
    } else {
        Task::Delete(id)
    });
    assert!(
        purged_within(&deployment.catalog, id, Duration::from_secs(2)).await,
        "Delete forever was ignored behind the existing grace deadline"
    );

    let until = tokio::time::Instant::now() + Duration::from_secs(2);
    while handle.snapshot()["deadlines"].as_u64() != Some(0) && tokio::time::Instant::now() < until
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        handle.snapshot()["deadlines"].as_u64(),
        Some(0),
        "successful early purge must remove its stale grace deadline"
    );

    running.abort();
    deployment.close().await;
}

/// Durable rows with no wake-up asked for at all -- the state after a crash
/// that lost every in-memory `Deadlines` entry -- are found and completed by
/// a freshly constructed worker's own startup scan.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_fresh_worker_finds_and_purges_durable_rows_with_no_wake_up_at_all() {
    let Some(deployment) = deployment("worker-restart-recovery").await else {
        return;
    };
    let id = deployment
        .document_deleted(
            "worker-restart-recovery",
            DELETION_GRACE + Duration::from_secs(60),
        )
        .await;

    // Nothing asks for this document at all; only `Worker::run`'s own
    // startup scan (§8.6) can find it.
    let (worker, _handle) = deployment.worker();
    tokio::spawn(worker.run());

    assert!(
        purged_within(&deployment.catalog, id, Duration::from_secs(30)).await,
        "the startup scan never found a document with an overdue deletion and no wake-up"
    );

    deployment.close().await;
}

/// A document restored to `active` before the worker gets to it must not be
/// purged. This is `claim_document_purge`'s guard: the row is restored
/// while the worker still has an outstanding ask for it, so the worker's
/// own claim on the purge must lose to the restoration rather than deleting
/// blobs out from under a document somebody just got back.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_document_restored_before_the_worker_gets_to_it_is_not_purged() {
    let Some(deployment) = deployment("worker-restoration-wins").await else {
        return;
    };
    let id = deployment
        .document_deleted(
            "worker-restoration-wins",
            DELETION_GRACE + Duration::from_secs(60),
        )
        .await;

    assert!(
        deployment
            .catalog
            .restore_document(id)
            .await
            .expect("restore the document"),
        "restoring a document still within reach of the worker must succeed"
    );

    let (worker, handle) = deployment.worker();
    // The worker is still asked for this document, exactly as if its
    // deletion wake-up had arrived before the restore did; the guard inside
    // `delete` is what has to refuse it, not the absence of a wake-up.
    handle.ask(Task::Delete(id));
    tokio::spawn(worker.run());

    // There is no event to wait for here beyond "nothing bad happened", so
    // this gives the worker a real window to have acted wrongly before
    // checking.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        deployment.status(id).await.as_deref(),
        Some("active"),
        "a restored document must not be purged, however it was asked for"
    );

    deployment.close().await;
}

/// A background scan's own connection can be severed underneath it -- the
/// backend restarted, a firewall reset the socket, an operator terminated
/// it by hand -- without the scan it was running being lost. This
/// deployment's pool holds exactly one physical connection (`max_connections
/// = 1`), so terminating that connection's backend pid before the worker
/// ever touches it forces every query the startup scan makes to go through
/// a connection whose old backend is already gone.
///
/// What happens next is not pinned down on purpose: `sqlx`'s pool validates
/// an idle connection before handing it out, so it may reopen a fresh one
/// silently and the scan simply succeeds; or the scan's own query may be the
/// one to discover the severed connection and fail, in which case
/// `Worker::failed` schedules the ordinary backoff retry and a later attempt
/// succeeds instead. Either way is a correct recovery, and this test proves
/// the property both paths share: the worker does not wedge, does not
/// silently drop the scan, and the document it was scanning for is still
/// found and purged. The pid comparison at the end additionally proves a
/// real severance happened, not a connection that coincidentally survived.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_scan_recovers_after_its_pooled_connection_is_severed() {
    let Some((deployment, pid)) = deployment_with_one_connection("worker-connection-loss").await
    else {
        return;
    };
    let id = deployment
        .document_deleted(
            "worker-connection-loss",
            DELETION_GRACE + Duration::from_secs(60),
        )
        .await;

    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").expect("test database URL");
    let mut admin = sqlx::PgConnection::connect(&url)
        .await
        .expect("a second connection to terminate the first from");
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&mut admin)
        .await
        .expect("terminate the deployment's only connection");
    assert!(
        terminated,
        "the connection to sever must still have existed"
    );

    let (worker, _handle) = deployment.worker();
    tokio::spawn(worker.run());

    // A generous patience: if the severed connection surfaces as a genuine
    // failure rather than being healed silently, the worker's ordinary
    // backoff (60s on a first failure) is what stands between here and the
    // retry that succeeds.
    assert!(
        purged_within(&deployment.catalog, id, Duration::from_secs(90)).await,
        "the worker never recovered from its severed connection"
    );

    let replacement: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(deployment.catalog.pool())
        .await
        .expect("a working connection after recovery");
    assert_ne!(
        pid, replacement,
        "the severed backend must actually have been replaced, not merely alive by chance"
    );

    admin.close().await.expect("close the admin connection");
    deployment.close().await;
}

/// The startup scan pages through `documents` (and the other three
/// categories) with a keyset cursor over `id`, and `id` is a random uuid --
/// nothing about it tracks insertion order. So once a page returns, its
/// cursor only ever moves forward, and a row written afterward whose id
/// sorts below that cursor can never be seen by the rest of that pass: the
/// query is `id > cursor`.
///
/// In this deployment that is normally harmless, because everything that
/// writes this kind of durable state also asks the worker for it directly
/// (`Handle::ask`, independent of any cursor -- see `delete_document` and
/// its siblings). The one case with no such ask standing behind it is the
/// very first scan after a restart, racing whatever else touches durable
/// state at that moment. `Worker::run`'s one-shot follow-up pass (armed by
/// its `startup` flag, cleared the first time a full scan completes) is
/// what closes that window: once the very first scan finishes, it always
/// runs one more, from a fresh cursor, before settling into pure wake-up
/// driven idling.
///
/// This proves the underlying phenomenon directly against PostgreSQL rather
/// than through `Worker::run`'s own timing: making a real insert land
/// between two of its page fetches is not something a test can force
/// without slowing the scan down for production too.
/// `pending_background_work` is the same query the scan runs, and a
/// hand-built cursor is exactly the state pagination leaves behind after a
/// page returns, so calling it directly proves the same claim a slower,
/// flakier race would -- a document behind an already-advanced cursor is
/// invisible to it, and a fresh cursor, what the follow-up pass always
/// uses, finds it anyway. The last section then confirms a real, running
/// `Worker`, over its own ordinary startup scan and with no wake-up at all
/// for this document, still finds and purges it.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn work_created_behind_an_advanced_cursor_is_found_by_a_fresh_pass() {
    let Some(deployment) = deployment("worker-behind-cursor").await else {
        return;
    };

    // The document that must not be missed is created *first*, so its id
    // sorts below the anchor's. Document ids are uuid v7
    // (`postgres::new_id`), which is time ordered, so a row written after a
    // page cannot hide behind that page's cursor. What can is a row written
    // long before it that only *becomes* work later: an old document whose
    // `deleted_at` is set while the pass is already past its id. That is the
    // case here, and it is why a single pass is not enough on its own.
    let hidden = deployment
        .document_deleted(
            "worker-behind-cursor-hidden",
            DELETION_GRACE + Duration::from_secs(60),
        )
        .await;
    let anchor = deployment
        .document_deleted(
            "worker-behind-cursor-anchor",
            DELETION_GRACE + Duration::from_secs(60),
        )
        .await;
    assert!(
        hidden < anchor,
        "uuid v7 orders ids by creation time, so the first document sorts first",
    );

    // A cursor already sitting at the anchor -- exactly what `next.deleting`
    // holds once a real page's last row was the anchor itself -- cannot see
    // a document whose id sorts below it, no matter when that document was
    // written.
    let advanced = deployment
        .catalog
        .pending_background_work(
            PendingWorkCursor {
                deleting: anchor,
                compaction_done: true,
                archives_done: true,
                erasing_done: true,
                ..Default::default()
            },
            64,
        )
        .await
        .expect("scan with an advanced cursor");
    assert!(
        !advanced.deleting.contains(&hidden),
        "a document behind an already-advanced cursor must not be visible to it"
    );

    // The same query, from a fresh cursor, finds it: this is what makes
    // `Worker::run`'s follow-up pass a real fix rather than a no-op.
    let fresh = deployment
        .catalog
        .pending_background_work(PendingWorkCursor::default(), 64)
        .await
        .expect("scan with a fresh cursor");
    assert!(
        fresh.deleting.contains(&hidden),
        "a fresh cursor must find a document an advanced one could not"
    );

    // And the worker itself, over its own real startup scan, still finds
    // and purges it -- there is no wake-up for `hidden` here at all, only
    // durable state.
    let (worker, _handle) = deployment.worker();
    tokio::spawn(worker.run());
    assert!(
        purged_within(&deployment.catalog, hidden, Duration::from_secs(30)).await,
        "the worker never purged a document reachable only through its scan"
    );

    deployment.close().await;
}
