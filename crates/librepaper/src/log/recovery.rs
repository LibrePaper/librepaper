//! SPEC-server-is-a-log §14.1: the recovery spike, run where it can actually
//! reach what it is testing. It also holds one §14.2 test -- "source-
//! producing commands record before and after evidence that match the row's
//! bytes" -- because that claim is about the same `Sequencer::command`
//! internals as §14.1 item 5, needs the same `deployment_with_document`
//! fixture and the same `#[ignore]`d PostgreSQL gate, and an external
//! `tests/*.rs` file could no more reach `Evidence`'s fields or
//! `PostgresCatalog::log_rows` than it could reach anything else this
//! header already explains is crate-private. The retry half of that same
//! bullet ("a retry by `request_id` returns the same label") is a different
//! agent's file.
//!
//! This used to live at `crates/librepaper/tests/recovery_spike.rs`, outside
//! the crate. Every one of §14.1's seven items is a claim about
//! `Sequencer`'s own internals -- the gap check in `ingest`, the buffer, the
//! reply `join` computes, the transaction `command` runs under a held lock
//! -- and an external `tests/*.rs` file sees only what `lib.rs` marks `pub`,
//! which does not include `Registry::new`'s `Arc<dyn BlobStore>` parameter
//! (`storage::blob` is `pub mod` but its parent `storage` is declared
//! privately and not fully re-exported) or `Room`/`Worker`/`Task`. From out
//! there, no `Sequencer` could be built at all, by any means, and most of
//! this file was findings about what could not be reached rather than tests
//! of recovery. In here, `mod room;`, `mod storage;` and everything in them
//! are ordinary crate-internal items: private only blocks another crate,
//! never a sibling module of the same crate, so nothing needed widening to
//! reach them.
//!
//! Reused rather than rebuilt: [`super::tests::Outbox`], the causally-linked
//! batch source `log/tests.rs` already has (three of its methods gained a
//! `pub(super)` and it gained two read-only accessors, `text` and `vector`,
//! and one export variant, `export_from`, so this module could use it
//! without a second copy).
//!
//! One seam was added to `sequencer.rs` for item 1's negative control:
//! `Sequencer::gap_check`, a field `ingest` always consults (`true` in every
//! production sequencer), and `set_gap_check_enforced`, a `#[cfg(test)]`
//! method that is the only thing in this crate that ever calls it with
//! `false`. It is not a `#[cfg(test)]` branch standing in for behaviour
//! production does not run -- the branch in `ingest` is unconditional and
//! is the same branch every deployment executes; only the ability to flip
//! the switch is compiled out of a release build.
//!
//! Everything here needs `LIBREPAPER_TEST_POSTGRES_URL` and is `#[ignore]`d
//! without it, the same convention as `room/comment_anchor_tests.rs` and
//! `storage/postgres/mod.rs`. Point it at a throwaway database, not
//! `librepaper_sqlx` (what the sqlx macros check schemas against) or
//! `librepaper` (a real local deployment):
//! `docker exec librepaper-postgres psql -U postgres -c 'CREATE DATABASE lp_spike'`.
//! Every test truncates the tables it uses and claims the single writer
//! lease, so this file must run with `--test-threads=1`.
//!
//! §6.4's crash contract is held exactly as written and not overstated:
//! "unacknowledged work is recoverable if a surviving client retained it and
//! reconnects." Every test below that models offline or unsynced work keeps
//! that work alive in its own local `Outbox` the whole time -- the
//! `Outbox` standing in for the surviving client -- and never claims
//! recovery for anything this file itself let go of.

use std::sync::Arc;
use std::time::Duration;

use loro::{Frontiers, LoroDoc, VersionVector};
use uuid::Uuid;

use futures_util::future::BoxFuture;

use super::tests::Outbox;
use super::*;
use crate::config::Configuration;
use crate::document::session;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::room::{AddComment, Rooms};
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::postgres::{
    Authority, MutationAuthorization, NewAccount, NewDocument, PostgresCatalog, PostgresOptions,
};
use crate::storage::worker::{Task, Worker};

const TRUNCATE: &str = "TRUNCATE document_updates,document_bases,document_proposal_hunks,\
     document_proposals,replies,annotations,document_labels,document_assets,share_links,grants,\
     documents,accounts CASCADE";

async fn connect(url: String) -> Arc<PostgresCatalog> {
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
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

/// One document with nothing written to its log, admitted through nothing
/// but public `PostgresCatalog` methods -- for the tests that exercise
/// `Sequencer` directly (items 1, 2, 3 and 6) and never need a real file
/// layout.
struct Bare {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    document_id: Uuid,
    slug: String,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn bare(slug: &str) -> Option<Bare> {
    bare_with_config(slug, Configuration::default()).await
}

async fn bare_with_config(slug: &str, config: Configuration) -> Option<Bare> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let catalog = connect(url).await;
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
    let document = catalog
        .create_document(NewDocument {
            slug: slug.into(),
            owner_id: account.id,
            ownership_mode: "owned".into(),
            title: "Spike".into(),
            source_format: "markdown".into(),
            main_path: "paper.md".into(),
            settings: serde_json::json!({}),
        })
        .await
        .expect("create the document row");
    let objects = tempfile::tempdir().expect("a temp directory for this test's blob store");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
    Some(Bare {
        catalog,
        blobs,
        config: Arc::new(config),
        document_id: document.id,
        slug: slug.into(),
        _writer: writer,
        _objects: objects,
    })
}

impl Bare {
    /// A fresh `Registry` over the same catalog and the same blob directory
    /// -- what "the server restarts" reduces to: no in-process state
    /// outlives it except the buffer, whose loss is §6.4's contract.
    fn registry(&self) -> Arc<Registry> {
        Registry::new(
            self.catalog.clone(),
            self.blobs.clone(),
            self.config.clone(),
            "deployment".into(),
        )
    }

    async fn sequencer(&self, registry: &Arc<Registry>) -> Arc<Sequencer> {
        registry
            .get(self.document_id, &self.slug)
            .await
            .expect("admit the document")
    }

    /// §8.4, driven the same way `storage::worker` does in production:
    /// a real `Worker` consuming a real `Task::Compact` from its queue,
    /// against the resident sequencer this registry already holds. There is
    /// no shortcut here that skips `Worker::compact`'s own coverage proof.
    async fn force_compaction(&self, registry: &Arc<Registry>) {
        let (worker, handle) = Worker::new(
            self.catalog.clone(),
            self.blobs.clone(),
            registry.clone(),
            self.config.clone(),
        );
        tokio::spawn(worker.run());
        handle.ask(Task::Compact(self.document_id));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if self
                .catalog
                .log_base(self.document_id)
                .await
                .unwrap()
                .is_some()
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "compaction did not land within 10s"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Types and flushes until the document's backlog is over §8.4's row
    /// trigger, one row per flush, which is what an afternoon of editing
    /// does to a log.
    async fn fill_past_the_compaction_threshold(&self, sequencer: &Arc<Sequencer>) {
        let (rows, _bytes) = self.catalog.compaction_thresholds();
        let mut outbox = Outbox::new();
        for seq in 1..=rows {
            assert!(
                matches!(
                    sequencer
                        .ingest(1, "account:writer", "account:writer", seq, outbox.edit())
                        .await,
                    Ingested::Accepted
                ),
                "nothing here is near a quota; every edit is accepted"
            );
            sequencer
                .flush(FlushReason::Quiet)
                .await
                .expect("each edit becomes its own row");
        }
    }

    /// Waits for a base to appear, without ever asking for one.
    async fn base_within(&self, patience: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            if self
                .catalog
                .log_base(self.document_id)
                .await
                .unwrap()
                .is_some()
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

// =========================================================================
// §8.4: compaction is scheduled, not only scanned for.
// =========================================================================

/// The flush that crosses the threshold is what asks for compaction.
///
/// Nothing in this test ever enqueues a task. The worker is started before
/// the document has a single row, so its startup scan (§8.6) finds nothing
/// and then parks on its channel for good; the only thing that can put a
/// base in `document_bases` is the trigger in `Sequencer::flush`. Without
/// it a log grows until §9.1's quota refuses updates with "waiting to be
/// compacted", and then waits for ever, because the next scan is the next
/// process start and the one after that finds the same thing.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_flush_over_the_threshold_compacts_without_waiting_for_a_restart() {
    let Some(bare) = bare("compaction-on-flush").await else {
        return;
    };
    let registry = bare.registry();
    let (worker, handle) = Worker::new(
        bare.catalog.clone(),
        bare.blobs.clone(),
        registry.clone(),
        bare.config.clone(),
    );
    registry.compacts_through(handle);
    tokio::spawn(worker.run());

    let sequencer = bare.sequencer(&registry).await;
    bare.fill_past_the_compaction_threshold(&sequencer).await;

    assert!(
        bare.base_within(Duration::from_secs(30)).await,
        "the flush that crossed §8.4's threshold never scheduled compaction"
    );
    // And the log it folded away is gone, which is the point of compacting:
    // the quota in §9.1 counts the base plus what is left, not the history.
    let head = bare.catalog.log_head(bare.document_id).await.unwrap();
    assert_eq!(
        head.uncompacted_count, 0,
        "every row the base covers was deleted with it"
    );
    bare.catalog.close().await;
}

/// The startup scan compacts a document nothing has opened.
///
/// `pending_background_work` finds documents over the threshold from
/// durable state, and right after a boot none of them is resident -- so a
/// compaction that gave up when it found no sequencer gave up on all of
/// them, which is the only case the scan exists for. This writes the rows
/// through one registry, throws it away (the restart), and lets a fresh
/// deployment's worker find the document on its own.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_startup_scan_compacts_a_document_that_is_not_resident() {
    let Some(bare) = bare("compaction-cold-scan").await else {
        return;
    };
    {
        // No worker is wired to this registry, so nothing here asks for
        // compaction: this is only how the rows get written.
        let registry = bare.registry();
        let sequencer = bare.sequencer(&registry).await;
        bare.fill_past_the_compaction_threshold(&sequencer).await;
        registry.shutdown().await;
    }
    assert!(
        bare.catalog
            .log_base(bare.document_id)
            .await
            .unwrap()
            .is_none(),
        "nothing has compacted this document yet"
    );

    let registry = bare.registry();
    assert!(
        registry.resident(bare.document_id).await.is_none(),
        "a fresh registry has never heard of this document, which is the state a boot is in"
    );
    let (worker, handle) = Worker::new(
        bare.catalog.clone(),
        bare.blobs.clone(),
        registry.clone(),
        bare.config.clone(),
    );
    registry.compacts_through(handle);
    tokio::spawn(worker.run());

    assert!(
        bare.base_within(Duration::from_secs(30)).await,
        "the startup scan found the document and then dropped it for not being resident"
    );
    bare.catalog.close().await;
}

/// The housekeeping sweep re-asks for a compaction nobody else will.
///
/// `Handle::ask` is a `try_send`: a full worker queue drops the task and says
/// nothing, and the worker's own backoff re-asks through that same droppable
/// call, so a dropped retry leaves a document over threshold with no event
/// left to fire from. It then sits there until §9.1's quota refuses edits,
/// recoverable only by another edit or a restart.
///
/// The fixture reproduces the sequencer's side of a dropped ask without
/// racing a real queue: the worker is built and started while the document
/// has no rows, so its startup scan (§8.6) finds nothing and parks, and the
/// registry is left without a handle while the rows are written. Every flush
/// trigger therefore finds an empty `OnceLock` and has nothing to ask --
/// indistinguishable, from the sequencer, from an ask the queue swallowed.
/// The handle is wired only afterwards, and nothing edits the document again.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn housekeeping_re_asks_for_a_compaction_whose_ask_was_lost() {
    let Some(bare) = bare("compaction-lost-ask").await else {
        return;
    };
    let registry = bare.registry();
    let (worker, handle) = Worker::new(
        bare.catalog.clone(),
        bare.blobs.clone(),
        registry.clone(),
        bare.config.clone(),
    );
    tokio::spawn(worker.run());

    let sequencer = bare.sequencer(&registry).await;
    bare.fill_past_the_compaction_threshold(&sequencer).await;
    assert!(
        bare.catalog
            .log_base(bare.document_id)
            .await
            .unwrap()
            .is_none(),
        "no ask reached the worker, so nothing has compacted this document"
    );

    registry.compacts_through(handle);
    registry.housekeep().await;

    assert!(
        bare.base_within(Duration::from_secs(30)).await,
        "a document over the threshold that nobody edits again is never compacted"
    );
    bare.catalog.close().await;
}

fn subscriber() -> crate::room::Sender {
    crate::room::Sender::channel(64, 1 << 20, None, None).0
}

/// A `Joined` reply, reconstructed the way a fresh client would: the base
/// (if any) imported first, then every batch in order.
fn reconstruct(base: &Option<Vec<u8>>, batches: &[Vec<u8>]) -> LoroDoc {
    let doc = LoroDoc::new();
    if let Some(base) = base {
        doc.import(base).expect("a compaction base imports cleanly");
    }
    for batch in batches {
        doc.import(batch)
            .expect("a row this test wrote imports cleanly");
    }
    doc
}

// =========================================================================
// §14.1 item 1: pending dependency across compaction.
// =========================================================================

/// A dependent update sent alone is refused with `doc-gap`; the export from
/// the vector that refusal names is accepted as one batch holding both
/// updates, and that batch survives compaction and a restart. Disabling the
/// gap check and repeating the same send lets the dependent update land
/// alone, which a fresh client cannot reconstruct correctly -- the loss the
/// check exists to prevent.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_dependent_update_is_refused_alone_accepted_together_and_survives_compaction_and_restart()
{
    let Some(deployment) = bare("recovery-14-1-positive").await else {
        return;
    };
    let registry = deployment.registry();
    let sequencer = deployment.sequencer(&registry).await;

    // Client creates update A then B, B depending on A -- B's own start
    // vector is A's end vector, which the server has never seen.
    let mut outbox = Outbox::new();
    let _a = outbox.edit();
    let b_alone = outbox.edit();

    let gapped = sequencer
        .ingest(1, "account:editor", "account:editor", 1, b_alone)
        .await;
    match gapped {
        Ingested::Gap { vector } => {
            assert_eq!(
                vector,
                VersionVector::default().encode(),
                "the head is still empty"
            );
        }
        other => panic!("expected doc-gap, got {other:?}"),
    }
    assert_eq!(
        sequencer.log_state().await.buffered,
        0,
        "a refused gap is not appended"
    );

    // The export from that vector: everything the client has, A and B
    // together, in one batch.
    let combined = outbox.export_from(&VersionVector::default());
    let accepted = sequencer
        .ingest(1, "account:editor", "account:editor", 2, combined)
        .await;
    assert!(matches!(accepted, Ingested::Accepted), "{accepted:?}");
    sequencer
        .flush(FlushReason::Barrier)
        .await
        .unwrap()
        .expect("something was buffered");

    let rows = deployment
        .catalog
        .log_rows(deployment.document_id, 0, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "A and B landed as one row");
    assert_eq!(
        frame::decode(&rows[0].update_bytes).unwrap().len(),
        1,
        "one row, one batch"
    );

    deployment.force_compaction(&registry).await;
    assert!(
        deployment
            .catalog
            .log_rows(deployment.document_id, 0, None)
            .await
            .unwrap()
            .is_empty(),
        "the compacted row is gone"
    );

    // Restart: drop this registry and every sequencer it held, and admit
    // again over the same database and the same blob directory.
    drop(sequencer);
    drop(registry);
    let registry2 = deployment.registry();
    let sequencer2 = deployment.sequencer(&registry2).await;

    let joined = sequencer2
        .join(2, Role::Editor, "account:reader", subscriber(), None)
        .await
        .expect("join succeeds");
    let fresh = reconstruct(&joined.base, &joined.batches);
    assert_eq!(
        fresh.get_text("t").to_string(),
        outbox.text(),
        "a fresh client sees A and B"
    );

    // The writer lease is a session-scoped `pg_try_advisory_lock`: as long
    // as `deployment` (and the connection its `WriterLease` is still
    // holding) is alive, a second `claim_writer` on any other catalog --
    // including the negative control's, below -- fails with "another
    // backend owns document writes". Closing the pool and dropping the
    // deployment releases it before that second claim.
    drop(sequencer2);
    drop(registry2);
    deployment.catalog.close().await;
    drop(deployment);

    // -- the negative control ---------------------------------------------
    let Some(neg) = bare("recovery-14-1-negative").await else {
        return;
    };
    let neg_registry = neg.registry();
    let neg_sequencer = neg.sequencer(&neg_registry).await;
    let mut neg_outbox = Outbox::new();
    let after_a = {
        let _a = neg_outbox.edit();
        neg_outbox.vector()
    };
    let b_only = neg_outbox.edit();

    neg_sequencer.set_gap_check_enforced(false);
    let bypassed = neg_sequencer
        .ingest(1, "account:editor", "account:editor", 1, b_only)
        .await;
    assert!(
        matches!(bypassed, Ingested::Accepted),
        "with the check off, B is accepted alone instead of refused: {bypassed:?}"
    );
    let _ = after_a;
    neg_sequencer.flush(FlushReason::Barrier).await.unwrap();

    let neg_rows = neg
        .catalog
        .log_rows(neg.document_id, 0, None)
        .await
        .unwrap();
    let mut import_failed = false;
    let broken = LoroDoc::new();
    for row in &neg_rows {
        for batch in frame::decode(&row.update_bytes).unwrap() {
            if broken.import(&batch.bytes).is_err() {
                import_failed = true;
            }
        }
    }
    assert!(
        import_failed || broken.get_text("t").to_string() != neg_outbox.text(),
        "the gap check is what prevents this: with it disabled, B landed \
         without A and reconstruction is corrupted rather than reading the \
         same text the client has (import_failed={import_failed}, got={:?}, want={:?})",
        broken.get_text("t").to_string(),
        neg_outbox.text(),
    );

    neg.catalog.close().await;
}

// =========================================================================
// §14.1 item 2: browser and server restart with unsynced work.
// =========================================================================

/// Work an editor typed offline is not reflected in a join reply until it
/// is sent; a server restart in between changes nothing about that; sending
/// it once reconnected is what makes it durable, and a fresh client then
/// sees it.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn offline_work_is_absent_from_a_join_reply_until_sent_and_durable_once_it_is() {
    let Some(deployment) = bare("recovery-14-2").await else {
        return;
    };
    let registry = deployment.registry();
    let sequencer = deployment.sequencer(&registry).await;

    // The editor types offline for a minute. The outbox is the surviving
    // client §6.4 talks about: it keeps this edit the whole time, across
    // the tab closing and the restart below, which is what makes recovery
    // possible at all.
    let mut outbox = Outbox::new();
    let offline_edit = outbox.edit();

    // The tab closes: nothing to do, since nothing was ever sent.

    // The server restarts.
    drop(sequencer);
    drop(registry);
    let registry2 = deployment.registry();
    let sequencer2 = deployment.sequencer(&registry2).await;

    // The editor reopens and joins. A join reply says only what the server
    // already has, which is nothing: the offline edit is not in it.
    let joined = sequencer2
        .join(1, Role::Editor, "account:editor", subscriber(), None)
        .await
        .expect("join succeeds");
    assert!(
        joined.base.is_none() && joined.batches.is_empty(),
        "not synced: nothing was ever sent"
    );

    // The catch-up: now connected, the client sends what it has. `Accepted`
    // is what a catch-up ack is computed from -- the row this produces is
    // exactly what §5.1's acknowledgement names.
    let ack = sequencer2
        .ingest(1, "account:editor", "account:editor", 1, offline_edit)
        .await;
    assert!(matches!(ack, Ingested::Accepted), "{ack:?}");
    sequencer2.flush(FlushReason::Barrier).await.unwrap();

    let rows = deployment
        .catalog
        .log_rows(deployment.document_id, 0, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "the catch-up is durable");

    // A fresh client -- a third admission, over the same database -- sees it.
    drop(sequencer2);
    drop(registry2);
    let registry3 = deployment.registry();
    let sequencer3 = deployment.sequencer(&registry3).await;
    let fresh_join = sequencer3
        .join(2, Role::Editor, "account:fresh", subscriber(), None)
        .await
        .unwrap();
    let fresh = reconstruct(&fresh_join.base, &fresh_join.batches);
    assert_eq!(fresh.get_text("t").to_string(), outbox.text());

    deployment.catalog.close().await;
}

// =========================================================================
// §14.1 item 3: no cursor to race.
// =========================================================================

/// Two tabs on one document; one is killed before it can import a relayed
/// update and reopens later. What it is sent on reopening is exactly what
/// it missed, no row skipped, and importing it converges with the tab that
/// never dropped anything.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_tab_killed_mid_relay_reconciles_on_reopening_with_no_row_skipped() {
    let Some(deployment) = bare("recovery-14-3").await else {
        return;
    };
    let registry = deployment.registry();
    let sequencer = deployment.sequencer(&registry).await;

    let (_tab_a_tx, _tab_a_rx) = (subscriber(), ());
    let tab_a_sender = subscriber();
    sequencer
        .join(1, Role::Editor, "account:tab-a", tab_a_sender, None)
        .await
        .unwrap();
    let tab_b_sender = subscriber();
    sequencer
        .join(2, Role::Editor, "account:tab-b", tab_b_sender, None)
        .await
        .unwrap();

    let mut outbox = Outbox::new();
    let batch1 = outbox.edit();
    let first = sequencer
        .ingest(1, "account:tab-a", "account:tab-a", 1, batch1.clone())
        .await;
    assert!(matches!(first, Ingested::Accepted));

    // Tab B is killed here, mid-import of the relay this ingest just sent
    // it -- it never applied batch1, and its socket is gone.
    sequencer.unsubscribe(2).await;

    // Tab A keeps going while tab B is dark.
    let batch2 = outbox.edit();
    let second = sequencer
        .ingest(1, "account:tab-a", "account:tab-a", 2, batch2.clone())
        .await;
    assert!(matches!(second, Ingested::Accepted));
    sequencer.flush(FlushReason::Barrier).await.unwrap();

    let rows = deployment
        .catalog
        .log_rows(deployment.document_id, 0, None)
        .await
        .unwrap();
    let sequences: Vec<i64> = rows.iter().map(|row| row.update_sequence).collect();
    let mut sorted = sequences.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        sequences.len(),
        "no row is skipped or duplicated"
    );
    for pair in sequences.windows(2) {
        assert_eq!(
            pair[1],
            pair[0] + 1,
            "the sequence is contiguous: {sequences:?}"
        );
    }

    // Tab B reopens with the vector it still has -- V0, since it never
    // applied anything -- and reconciles.
    let rejoin = sequencer
        .join(
            3,
            Role::Editor,
            "account:tab-b-reopened",
            subscriber(),
            None,
        )
        .await
        .unwrap();
    let reconciled = reconstruct(&rejoin.base, &rejoin.batches);
    assert_eq!(
        reconciled.get_text("t").to_string(),
        outbox.text(),
        "reconciliation converges with tab A"
    );

    deployment.catalog.close().await;
}

// =========================================================================
// §14.1 item 4 and item 7 are not repeated here.
// =========================================================================
//
// Item 4 (lost commit responses) and the storage-only half of items 1, 2
// and 5 were already exercised, honestly, against `PostgresCatalog`
// directly before this file moved in-crate -- `flush_log_row`'s own
// fencing and `annotations`' `ON CONFLICT(id) DO NOTHING` do not need a
// `Sequencer` to prove, and nothing about moving this file changes that.
// The instruction that reopened this file asked specifically for the parts
// that could not be reached before -- the gap check, `join`, and
// `Sequencer::command` -- so this file's remaining tests focus there.
// Item 7 stays skipped for the reason already given: there is no migration
// to test, since the cutover this spec describes had none.

// =========================================================================
// §14.1 item 5: buffer differs from durable.
// =========================================================================

/// A real document, for the sub-claims of item 5 that need one: a restore
/// and a comment both go through `Room`, the same façade production code
/// runs, over a real file rather than a bare "t" container.
struct Deployment {
    rooms: Rooms,
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    account_id: Uuid,
}

const MAIN: &str = "paper.md";
const PAPER: &str = "# A paper\n\nOne paragraph a reader can quote.\n";

async fn deployment_with_document(slug: &str) -> Option<Deployment> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let catalog = connect(url).await;
    let _writer = catalog.claim_writer().await.unwrap();
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
        .unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
    // Leaked: the store, the registry and the rooms this test builds all
    // outlive the function, and a short-lived test binary reclaims it at
    // exit regardless.
    std::mem::forget(objects);
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );
    let store = Arc::new(
        Store::open_with_catalog(
            blobs.clone(),
            config.clone(),
            catalog.clone(),
            registry.clone(),
        )
        .await
        .unwrap(),
    );
    let actor = MutationActor {
        account_id: account.id.to_string(),
        owner_key: "owner".into(),
        session_generation: account.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        unowned_publisher: false,
    };
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "A Paper".into(),
                source: PAPER.into(),
                source_format: "markdown".into(),
                main: MAIN.into(),
            },
            Vec::new(),
            actor,
        )
        .await
        .unwrap();
    let document = catalog.document_by_slug(slug).await.unwrap().unwrap();
    Some(Deployment {
        rooms: Rooms::new(catalog.clone(), blobs, config, registry),
        catalog,
        document_id: document.id,
        account_id: account.id,
    })
}

impl Deployment {
    fn authority(&self) -> Authority {
        Authority {
            principal_key: self.account_id.to_string(),
            account_id: Some(self.account_id),
            link_hash: None,
        }
    }

    fn mutation_authorization(&self) -> MutationAuthorization {
        MutationAuthorization {
            principal_key: self.account_id.to_string(),
            account_id: Some(self.account_id),
            session_generation: None,
            token_hash: None,
            policy_editor: true,
        }
    }
}

/// A reader's projection is stable while nothing changes; a restore refused
/// on a stale `expected_frontier` writes nothing; a rendered comment with a
/// digest that is not the head digest is refused `StaleSelection` naming
/// the current one; a comment made while typing sits unflushed in the
/// buffer flushes that buffer and commits both in the one transaction
/// `Sequencer::command` opens; and closing this test's own connection to
/// PostgreSQL between `evaluate` (which never touches storage) and the
/// `transact` that would have run leaves nothing written and the buffer
/// exactly as it was.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_durable_projection_is_stable_and_command_bundles_or_writes_nothing() {
    let Some(deployment) = deployment_with_document("recovery-14-5").await else {
        return;
    };
    let room = deployment
        .rooms
        .get("recovery-14-5")
        .await
        .expect("the room for the document just created");

    // -- (a) a reader projection is stable ---------------------------------
    let first = room.projection().await.unwrap().projection.digest();
    let second = room.projection().await.unwrap().projection.digest();
    assert_eq!(
        first, second,
        "unchanged durable state projects to the same digest"
    );

    // -- (b) refused on a stale expected_frontier ---------------------------
    //
    // Restore itself moved to `server::history::Restore`, which is private
    // to that module and rebuilds from a label's archived projection rather
    // than forking the live document -- not reachable from here, and not
    // what this sub-claim is actually about. The claim is §7.1's
    // precondition, checked by `Sequencer::command` the same way for every
    // command: "`expected_frontier` equals the head frontier, or refused as
    // a conflict". `StaleFrontierProbe` below carries exactly that
    // precondition and nothing else, so what runs it -- the locking, the
    // evidence, the refusal path -- is the real `Sequencer::command`, not a
    // stand-in for it.
    struct StaleFrontierProbe {
        expected_frontier: Vec<u8>,
    }
    impl Command for StaleFrontierProbe {
        type Output = ();
        fn name(&self) -> &'static str {
            "stale-frontier-probe"
        }
        fn evaluate(
            &mut self,
            head: &Head<'_>,
        ) -> std::result::Result<Option<PreparedSource>, CommandError> {
            if head.frontier.encode() != self.expected_frontier {
                return Err(CommandError::Conflict(
                    "the document moved since this was requested".into(),
                ));
            }
            Ok(None)
        }
        fn transact<'a>(
            &'a mut self,
            _tx: &'a mut sqlx::Transaction<'static, sqlx::Postgres>,
            _evidence: &'a Evidence,
        ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
            Box::pin(async move { Ok(()) })
        }
    }
    // `put_directory_as_actor` (document setup, above) already writes one
    // label of its own -- the command that lands the document's initial
    // content records a label the same way any source-producing command
    // does -- so the claim under test is "the refusal adds none", not "none
    // exist", the same distinction one of `comment_anchor_tests` already
    // had to draw for the same reason.
    let labels_before_refusal = deployment
        .catalog
        .label_page(deployment.document_id, None, 10)
        .await
        .unwrap()
        .len();
    let mut stale_probe = StaleFrontierProbe {
        expected_frontier: Frontiers::default().encode(), // never the head once anything has been written
    };
    let probe_result = room
        .command(&deployment.authority(), &mut stale_probe)
        .await;
    match probe_result {
        Err(CommandError::Conflict(message)) => {
            assert!(message.contains("moved"), "{message}");
        }
        other => panic!("expected a conflict on a stale expected_frontier, got {other:?}"),
    }
    let labels_after_refusal = deployment
        .catalog
        .label_page(deployment.document_id, None, 10)
        .await
        .unwrap()
        .len();
    assert_eq!(
        labels_after_refusal, labels_before_refusal,
        "a refused precondition writes no label of its own"
    );

    // -- (c) a rendered comment with a stale digest ------------------------
    let wrong_digest: [u8; 32] = *blake3_or_zero(&first);
    let config = Configuration::default();
    let mut stale_comment = AddComment::new(
        deployment.catalog.clone(),
        deployment.document_id,
        Uuid::new_v4(),
        &config,
        "commenting",
        "A remark.",
        "Reviewer",
        Some(deployment.account_id),
        format!("account:{}", deployment.account_id),
        deployment.mutation_authorization(),
        "paragraph",
        "One ",
        " a reader can quote.",
        None,
        false,
        None,
        None,
        Some(wrong_digest),
    )
    .expect("a well-formed comment");
    let stale_result = room
        .command(&deployment.authority(), &mut stale_comment)
        .await;
    match stale_result {
        Err(CommandError::StaleSelection { digest }) => {
            assert_eq!(
                digest, first,
                "the current digest comes back with the refusal"
            );
        }
        other => panic!("expected StaleSelection, got {other:?}"),
    }

    // -- (d) a comment bundles the buffer into one transaction -------------
    let (vector, edited) = room
        .log()
        .with_head(|doc| (session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    session::put_text(&edited, MAIN, &format!("{PAPER}\nA second paragraph.\n"));
    let update = session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(matches!(ingested, Ingested::Accepted), "{ingested:?}");
    let buffered_before = room.log().log_state().await.buffered;
    assert!(
        buffered_before > 0,
        "the typing above sat in the buffer, unflushed"
    );
    let sequence_before = deployment
        .catalog
        .log_sequence(deployment.document_id)
        .await
        .unwrap();

    let mut bundled_comment = AddComment::new(
        deployment.catalog.clone(),
        deployment.document_id,
        Uuid::new_v4(),
        &config,
        "commenting",
        "A remark on the original paragraph.",
        "Reviewer",
        Some(deployment.account_id),
        format!("account:{}", deployment.account_id),
        deployment.mutation_authorization(),
        "paragraph",
        "One ",
        " a reader can quote.",
        None,
        false,
        None,
        None,
        None,
    )
    .expect("a well-formed comment");
    let comment = room
        .command(&deployment.authority(), &mut bundled_comment)
        .await
        .unwrap();
    assert_eq!(
        room.log().log_state().await.buffered,
        0,
        "the buffer flushed as part of the command"
    );
    let sequence_after = deployment
        .catalog
        .log_sequence(deployment.document_id)
        .await
        .unwrap();
    assert_eq!(
        sequence_after,
        sequence_before + 1,
        "one row, not two: the buffer and the comment share it"
    );
    let anchor = comment.original_anchor.as_ref().expect("a source anchor");
    assert_eq!(
        anchor.source_sequence, sequence_after,
        "the comment's own evidence names the row that made its head durable, \
         which is the same row the buffered typing was flushed into"
    );

    // -- (e) PostgreSQL unreachable between evaluate and commit -------------
    let (_vector2, edited2) = room
        .log()
        .with_head(|doc| (session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    session::put_text(
        &edited2,
        MAIN,
        &format!("{PAPER}\nA third paragraph, typed just now.\n"),
    );
    let update2 = session::encode_diff(&edited2, &_vector2).unwrap();
    let ingested2 = room
        .ingest(999, "editor-999", "editor-999", 2, update2)
        .await;
    assert!(matches!(ingested2, Ingested::Accepted), "{ingested2:?}");
    let buffered_before_kill = room.log().log_state().await.buffered;
    assert!(buffered_before_kill > 0);
    let sequence_before_kill = deployment
        .catalog
        .log_sequence(deployment.document_id)
        .await
        .unwrap();

    // `evaluate` never touches storage (it is synchronous, against the head
    // already resident); every write happens in `transact`, after
    // `begin_document_command` opens a transaction on this pool. Closing
    // the pool here, before calling `command`, is indistinguishable from
    // PostgreSQL becoming unreachable at any point from here through
    // `transact` -- this test's own connection is what goes away, not the
    // shared database other tests use.
    deployment.catalog.close().await;

    let mut doomed_comment = AddComment::new(
        deployment.catalog.clone(),
        deployment.document_id,
        Uuid::new_v4(),
        &config,
        "commenting",
        "Never lands.",
        "Reviewer",
        Some(deployment.account_id),
        format!("account:{}", deployment.account_id),
        deployment.mutation_authorization(),
        "paragraph",
        "One ",
        " a reader can quote.",
        None,
        false,
        None,
        None,
        None,
    )
    .expect("a well-formed comment");
    let doomed_result = room
        .command(&deployment.authority(), &mut doomed_comment)
        .await;
    assert!(
        matches!(
            doomed_result,
            Err(CommandError::Storage(_)) | Err(CommandError::Sequencer(_))
        ),
        "expected a storage failure once the connection is gone, got {doomed_result:?}"
    );
    assert_eq!(
        room.log().log_state().await.buffered,
        buffered_before_kill,
        "the buffer is exactly as it was: the failed command changed nothing about it"
    );

    // Verify "nothing written" from a second, fresh connection -- standing
    // in for PostgreSQL being reachable again, or an operator checking from
    // elsewhere, since this test's own connection is the one that is gone.
    let verify = PostgresCatalog::connect(PostgresOptions::new(
        std::env::var("LIBREPAPER_TEST_POSTGRES_URL").unwrap(),
    ))
    .await
    .unwrap();
    let sequence_after_kill = verify.log_sequence(deployment.document_id).await.unwrap();
    assert_eq!(
        sequence_after_kill, sequence_before_kill,
        "the doomed row was never written"
    );
    let annotations = verify
        .annotations(deployment.document_id, None, 10)
        .await
        .unwrap();
    assert_eq!(
        annotations.len(),
        1,
        "still just the one comment from step (d)"
    );
    verify.close().await;
}

/// A trivial stand-in digest guaranteed to differ from `head`: flips every
/// byte of the head digest's own hex decoding, so it is never accidentally
/// equal to a real projection digest, without depending on any hashing this
/// crate does not already expose.
fn blake3_or_zero(head_hex: &str) -> Box<[u8; 32]> {
    let mut bytes = [0u8; 32];
    if let Ok(decoded) = hex::decode(head_hex) {
        for (slot, byte) in bytes.iter_mut().zip(decoded.iter()) {
            *slot = !byte;
        }
    }
    Box::new(bytes)
}

// =========================================================================
// §14.1 item 6: memory pressure with every active document subscribed.
// =========================================================================

/// More documents are opened than the memory budget holds, every one
/// subscribed. Opening the one that does not fit evicts a cold entry
/// first, per §9.2; typing continues on every document regardless of
/// whether its cache is warm; and a projection on an evicted document
/// rebuilds it on demand.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn opening_more_documents_than_the_budget_holds_evicts_a_subscribed_entry_and_rebuilds_on_demand(
) {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok();
    let Some(url) = url else { return };
    let catalog = connect(url).await;
    let _writer = catalog.claim_writer().await.unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
    // Three floors' worth of budget (the budget's floor per resident cache
    // is 64 KiB, from `log::budget::estimate`) admits three warm entries
    // comfortably; a fourth does not fit and must evict.
    let config = Arc::new(Configuration {
        memory_budget_bytes: 3 * 64 * 1024 + 32 * 1024,
        cache_expansion: 1,
        ..Configuration::default()
    });
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );

    let account = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("recovery-14-6".into()),
            handle: "recovery-14-6".into(),
            display_name: "Spike".into(),
            email: None,
        })
        .await
        .unwrap();

    let mut sequencers = Vec::new();
    let mut outboxes = Vec::new();
    for index in 0..5 {
        let slug = format!("recovery-14-6-{index}");
        let document = catalog
            .create_document(NewDocument {
                slug: slug.clone(),
                owner_id: account.id,
                ownership_mode: "owned".into(),
                title: "Spike".into(),
                source_format: "markdown".into(),
                main_path: "paper.md".into(),
                settings: serde_json::json!({}),
            })
            .await
            .unwrap();
        let sequencer = registry.get(document.id, &slug).await.unwrap();
        sequencer
            .join(
                index as u64,
                Role::Editor,
                "account:editor",
                subscriber(),
                None,
            )
            .await
            .unwrap();
        let mut outbox = Outbox::new();
        let batch = outbox.edit();
        assert!(matches!(
            sequencer
                .ingest(index as u64, "account:editor", "account:editor", 1, batch)
                .await,
            Ingested::Accepted
        ));
        sequencer.flush(FlushReason::Barrier).await.unwrap();
        // Warm the cache: this is the reservation that, on documents past
        // the third, does not fit and must evict before it can proceed.
        sequencer
            .projection()
            .await
            .expect("busy would mean eviction never ran");
        sequencers.push(sequencer);
        outboxes.push((document.id, slug, outbox));
    }

    let warm: Vec<bool> = {
        let mut states = Vec::new();
        for sequencer in &sequencers {
            states.push(sequencer.log_state().await.warm);
        }
        states
    };
    assert!(
        warm.iter().filter(|w| !**w).count() >= 2,
        "opening the fourth and fifth documents evicted earlier entries \
         rather than being refused: warm = {warm:?}"
    );

    // Typing continues on every document, warm or not: ingest does not
    // need a decoded cache (§5).
    for (index, sequencer) in sequencers.iter().enumerate() {
        let (_, _, outbox) = &mut outboxes[index];
        let batch = outbox.edit();
        let outcome = sequencer
            .ingest(index as u64, "account:editor", "account:editor", 2, batch)
            .await;
        assert!(
            matches!(outcome, Ingested::Accepted),
            "document {index}: {outcome:?}"
        );
    }

    // A projection on an evicted document rebuilds it rather than failing.
    // Read the rebuilt cache's own root text, not `Projected::texts`: the
    // projection only ever reads the four schema maps (§4.4 step 1), so a
    // root container named "t" -- what `Outbox` writes to, and not a file
    // under `files` -- is correctly never in it. `Projected::texts` would
    // be `None` for this document no matter what the rebuild held, which
    // tests the projection rather than the claim under test: that the cache
    // itself, once rebuilt, holds the same content the buffer and the log
    // held before eviction.
    for (index, sequencer) in sequencers.iter().enumerate() {
        if !warm[index] {
            sequencer.flush(FlushReason::Barrier).await.unwrap();
            let rebuilt = sequencer
                .with_head(|doc| doc.get_text("t").to_string())
                .await
                .expect("rebuilds on demand");
            let (_, _, outbox) = &outboxes[index];
            assert_eq!(
                rebuilt,
                outbox.text(),
                "the rebuild reads the same content the buffer and the log hold"
            );
        }
    }

    catalog.close().await;
}

// =========================================================================
// §14.2: source-producing commands record before and after evidence that
// match the row's bytes.
// =========================================================================

/// A minimal `Command` whose only job is to hand back the `Evidence`
/// `Sequencer::command` built for it (§7 step 4), unmodified, so this file
/// can check that evidence against the row the command's own flush wrote
/// rather than trusting the two agree by construction. `edit` makes it a
/// source-producing command, the same shape as restore or an agent patch
/// (§7.3): it calls `Head::prepare` and returns `Some`. Without an edit it
/// never calls `prepare` at all, the same shape as a comment that quotes
/// text without changing it.
struct EvidenceProbe {
    edit: Option<String>,
}

impl Command for EvidenceProbe {
    type Output = Evidence;

    fn name(&self) -> &'static str {
        "evidence-probe"
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        match self.edit.clone() {
            Some(text) => head
                .prepare(1, move |draft| {
                    session::put_text(draft, MAIN, &text);
                    Ok(())
                })
                .map_err(CommandError::Conflict),
            None => Ok(None),
        }
    }

    fn transact<'a>(
        &'a mut self,
        _tx: &'a mut sqlx::Transaction<'static, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        let evidence = evidence.clone();
        Box::pin(async move { Ok(evidence) })
    }
}

/// Reconstructs the document from the raw log rows through `through`,
/// decoded the same way a fresh client or a restore would decode them
/// (`frame::decode` then `LoroDoc::import` per batch) -- not the sequencer's
/// resident cache, so this owes the code under test nothing. No test in
/// this function runs compaction, so there is never a base to import first;
/// `log_rows` is asked for everything from the start.
async fn reconstruct_through(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    through: i64,
) -> LoroDoc {
    let rows = catalog
        .log_rows(document_id, 0, Some(through))
        .await
        .expect("read the rows this test's own command wrote");
    let doc = LoroDoc::new();
    for row in rows {
        for batch in frame::decode(&row.update_bytes).expect("a row this test wrote decodes") {
            doc.import(&batch.bytes)
                .expect("a batch this test wrote imports cleanly");
        }
    }
    doc
}

/// The core claim of the whole design (SPEC-server-is-a-log §2.1): a label,
/// a comment anchor or a restore names a moment by `Evidence`'s fields and
/// nothing else, so those fields had better describe the row that was
/// actually written. This reconstructs the document from the stored log
/// bytes independently of `Sequencer::command` and checks every field of
/// the `Evidence` a source-producing command received against that
/// reconstruction, rather than against a second copy of the same
/// arithmetic `Sequencer::command` used to produce it.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_producing_evidence_matches_the_row_it_names() {
    let Some(deployment) = deployment_with_document("recovery-14-2-evidence").await else {
        return;
    };
    let room = deployment
        .rooms
        .get("recovery-14-2-evidence")
        .await
        .expect("the room for the document just created");
    let config = Configuration::default();

    let sequence_before = deployment
        .catalog
        .log_sequence(deployment.document_id)
        .await
        .unwrap();

    let added = "\nA sentence only this command's edit adds.\n";
    let edited_source = format!("{PAPER}{added}");
    let mut probe = EvidenceProbe {
        edit: Some(edited_source.clone()),
    };
    let evidence = room
        .command(&deployment.authority(), &mut probe)
        .await
        .expect("a well-formed source-producing command");

    // -- (1) source_sequence names the row this command's own flush wrote --
    let sequence_after = deployment
        .catalog
        .log_sequence(deployment.document_id)
        .await
        .unwrap();
    assert!(
        sequence_after > sequence_before,
        "a source-producing command appends a row"
    );
    assert_eq!(
        evidence.source_sequence, sequence_after,
        "the evidence names the row the command's own flush wrote"
    );
    let rows = deployment
        .catalog
        .log_rows(
            deployment.document_id,
            sequence_after - 1,
            Some(sequence_after),
        )
        .await
        .unwrap();
    let row = rows.into_iter().next().expect("the row the flush wrote");
    assert_eq!(row.update_sequence, evidence.source_sequence);

    // -- (2) vector equals the log vector that row carries ------------------
    assert_eq!(
        evidence.vector, row.vector,
        "the evidence's vector is the row's own vector, not a value computed \
         alongside it that happens to agree"
    );

    // -- (3) after_frontier and after_digest describe the state the row
    //        actually produced, checked by rebuilding the document from the
    //        stored log bytes and forking at after_frontier -------------
    let reconstructed = reconstruct_through(
        &deployment.catalog,
        deployment.document_id,
        evidence.source_sequence,
    )
    .await;
    let after_frontier = Frontiers::decode(
        evidence
            .after_frontier
            .as_ref()
            .expect("a source-producing command records an after frontier"),
    )
    .unwrap();
    let after_fork = reconstructed
        .fork_at(&after_frontier)
        .expect("after_frontier is reachable in the reconstructed log");
    let after_projected = librepaper_document_core::project(&after_fork, &config.paths());
    assert_eq!(
        after_projected.projection.digest(),
        evidence.after_digest.as_deref().unwrap(),
        "after_frontier really does lead to the state after_digest claims -- \
         reconstructed independently, not recomputed the way production did it"
    );
    assert_eq!(
        after_projected.texts.get(MAIN).map(String::as_str),
        Some(edited_source.as_str()),
        "the reconstructed after-state actually contains this command's edit"
    );

    // -- (4) before_frontier and before_digest describe the state this
    //        command was evaluated against, and that state does not already
    //        contain the edit ------------------------------------------
    let before_frontier = Frontiers::decode(&evidence.before_frontier).unwrap();
    let before_fork = reconstructed
        .fork_at(&before_frontier)
        .expect("before_frontier is reachable in the reconstructed log");
    let before_projected = librepaper_document_core::project(&before_fork, &config.paths());
    assert_eq!(
        before_projected.projection.digest(),
        evidence.before_digest,
        "before_frontier really does lead to the state before_digest claims"
    );
    assert_eq!(
        before_projected.texts.get(MAIN).map(String::as_str),
        Some(PAPER),
        "the before state is the document as it stood before this command's \
         edit, not after it"
    );

    deployment.catalog.close().await;
}

/// The other half of §7.3 step 4: a command that produces no source writes
/// no row of its own and names the row that was already there.
/// `EvidenceProbe { edit: None }` never calls `Head::prepare`, the same
/// shape as a comment on text it does not change, so the flush in step 3 of
/// §7's algorithm sees an empty buffer and an empty prepared batch --
/// nothing to insert.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_command_that_produces_no_source_names_the_last_row_and_records_no_after_state() {
    let Some(deployment) = deployment_with_document("recovery-14-2-no-source").await else {
        return;
    };
    let room = deployment
        .rooms
        .get("recovery-14-2-no-source")
        .await
        .expect("the room for the document just created");

    let sequence_before = deployment
        .catalog
        .log_sequence(deployment.document_id)
        .await
        .unwrap();
    assert!(
        sequence_before > 0,
        "the document's own creation already wrote a row for this command to name"
    );

    let mut probe = EvidenceProbe { edit: None };
    let evidence = room
        .command(&deployment.authority(), &mut probe)
        .await
        .expect("a command that touches nothing still commits");

    let sequence_after = deployment
        .catalog
        .log_sequence(deployment.document_id)
        .await
        .unwrap();
    assert_eq!(
        sequence_after, sequence_before,
        "a command that produces no source and finds nothing buffered writes no row"
    );
    assert_eq!(
        evidence.source_sequence, sequence_before,
        "source_sequence names the last existing row, not a new one"
    );
    assert!(
        evidence.after_frontier.is_none(),
        "no source was produced, so there is no after_frontier"
    );
    assert!(
        evidence.after_digest.is_none(),
        "no source was produced, so there is no after_digest"
    );

    deployment.catalog.close().await;
}
