//! The throughput measurement SPEC-server-is-a-log §14.1 asks for, run
//! against a disposable database.
//!
//! What this used to measure was per-keystroke prepare and commit: validate
//! the whole candidate document, then write a row, for every edit. That is
//! the design the cutover deletes, so the old numbers no longer describe
//! anything the server does. §14.1 names the replacement exactly: "three
//! editors typing continuously into a 1 MiB document for ten minutes with
//! the 30 s flush; record rows, relay latency p50/p99, server CPU and RSS,
//! cache build time after forced eviction, projection latency for a polling
//! reader, and the expansion factor for 9.2."
//!
//! Two of those are honest only if measured rather than modelled, and they
//! are the two the design is most exposed on:
//!
//! * **Rows.** §5.2 claims two rows per document per minute for one
//!   continuously typing editor, and is explicit that this is a best-case
//!   typing figure and not a deployment cost. This measures the real rate
//!   with three editors, which is the number that matters.
//! * **The expansion factor.** §9.2 estimates a resident document as its log
//!   bytes times a factor, and the whole memory budget rests on that factor
//!   being roughly right. The default in `budget::DEFAULT_EXPANSION` is a
//!   guess until this run replaces it. A single build-and-evict RSS delta
//!   cannot measure it, because glibc's allocator keeps freed pages rather
//!   than handing them back to the kernel; this instead builds many distinct
//!   documents and never frees them, so growth cannot hide in reused pages.
//!   See the comment above the measurement in the test body for the numbers
//!   that caught the original approach failing.
//!
//! The spike of §14.1 is a recovery-first list that must pass before any of
//! this is worth reading. This file is the "then measure" half only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde_json::{json, Value};

use super::{Authority, NewAccount, NewDocument, PostgresCatalog, PostgresOptions, StoragePolicy};
use crate::document::session;
use crate::log::{FlushReason, Registry};

fn micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn percentiles(mut values: Vec<u64>) -> Value {
    if values.is_empty() {
        return json!({"samples": 0});
    }
    values.sort_unstable();
    let pick = |percent: usize| values[(values.len() - 1) * percent / 100];
    json!({"samples":values.len(),"p50_us":pick(50),"p95_us":pick(95),
        "p99_us":pick(99),"max_us":values[values.len()-1]})
}

/// Resident set size, in bytes, from the kernel rather than from an estimate.
///
/// `/proc/self/statm`'s second field is resident pages. This is Linux only,
/// which is what the deployment runs on; anywhere else the measurement is
/// absent rather than invented, because a wrong expansion factor would set
/// the memory budget wrong in production.
fn resident_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

async fn document(catalog: &PostgresCatalog, owner: uuid::Uuid, slug: String) -> uuid::Uuid {
    catalog
        .create_document(NewDocument {
            slug,
            owner_id: owner,
            ownership_mode: "owned".into(),
            title: "Throughput benchmark".into(),
            source_format: "markdown".into(),
            main_path: "paper.md".into(),
            settings: json!({"version":1}),
        })
        .await
        .expect("benchmark document")
        .id
}

/// §14.1's measurement. The default is the required ten minutes; a shorter
/// duration is a harness smoke check and is recorded in the output so a
/// reader cannot mistake one for the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "release acceptance benchmark; destroys every row in its configured database"]
async fn typing_throughput_release_benchmark() {
    let Ok(url) = std::env::var("LIBREPAPER_BENCHMARK_POSTGRES_URL") else {
        return;
    };
    let seconds = std::env::var("LIBREPAPER_BENCHMARK_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(600_u64);
    let mut options = PostgresOptions::new(url);
    options.max_connections = 32;
    options.policy = StoragePolicy {
        owner_bytes: i64::MAX / 4,
        deployment_bytes: i64::MAX / 4,
        asset_uploads_per_hour: 100_000,
    };
    let catalog = Arc::new(PostgresCatalog::connect(options).await.expect("connect"));
    catalog.migrate().await.expect("migrate");
    sqlx::query("TRUNCATE accounts CASCADE")
        .execute(catalog.pool())
        .await
        .expect("empty disposable benchmark database");
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("benchmark".into()),
            provider_subject: Some("owner".into()),
            handle: "benchmark".into(),
            display_name: "Benchmark".into(),
            email: None,
        })
        .await
        .expect("owner");
    let authority = Authority {
        principal_key: owner.id.to_string(),
        account_id: Some(owner.id),
        link_hash: None,
    };
    let _writer = catalog.claim_writer().await.expect("writer lease");
    let wal_before: String = sqlx::query_scalar("SELECT pg_current_wal_lsn()::text")
        .fetch_one(catalog.pool())
        .await
        .expect("initial WAL position");

    // A scratch directory rather than the real object store: this run
    // writes no base (it never compacts) and reads none, so the store is
    // only here to satisfy the registry's constructor.
    let scratch = tempfile::tempdir().expect("scratch blob directory");
    let blobs: Arc<dyn crate::storage::blob::BlobStore> = Arc::new(
        crate::storage::blob::FsStore::new(scratch.path().to_path_buf(), false),
    );
    // The expansion measurement below keeps many documents resident at once
    // on purpose (see the comment at that block). The default 512 MiB
    // budget would start evicting cold entries partway through that, which
    // would make the RSS delta undercount -- so this run gets a budget wide
    // enough that admission never interferes with the measurement.
    let config = Arc::new(crate::config::Configuration {
        memory_budget_bytes: u64::MAX / 4,
        ..crate::config::Configuration::default()
    });
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "benchmark".to_string(),
    );

    let document_id = document(&catalog, owner.id, "typing".into()).await;
    let sequencer = registry
        .get(document_id, "typing")
        .await
        .expect("admit the document");

    // A 1 MiB document to start from, written as one batch the way a real
    // upload would be. Three editors then type into it concurrently.
    let seed = session::new_doc();
    seed.set_peer_id(1).expect("peer id");
    session::put_text(&seed, "paper.md", &"x".repeat(1024 * 1024));
    seed.commit();
    let opening = session::encode_state(&seed);
    assert!(matches!(
        sequencer
            .ingest(
                0,
                &authority.principal_key,
                &authority.principal_key,
                0,
                opening.clone()
            )
            .await,
        crate::log::Ingested::Accepted
    ));
    sequencer
        .flush(FlushReason::Barrier)
        .await
        .expect("the opening row");

    // Three editors, each with its own Loro peer, all starting from the
    // exact opening state the sequencer holds (imported, not rebuilt), so
    // their updates share a causal history and the gap check in §5 accepts
    // them. Each iteration appends a few characters to the existing text
    // the way a person typing does, rather than replacing the body, so the
    // document stays at or above 1 MiB for the whole run.
    // The 30 s flush §14.1 names is a timed flush, and in a deployment it
    // is `serve`'s one-second housekeeping tick that notices it is due.
    // Without the same tick here the run would measure a server that only
    // ever flushes when a buffer fills, which is not the design being
    // measured.
    let housekeeping = tokio::spawn({
        let registry = registry.clone();
        async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                registry.housekeep().await;
            }
        }
    });

    let started_at = Instant::now();
    let deadline = started_at + Duration::from_secs(seconds);
    let typing = join_all((1..=3_u64).map(|peer| {
        let sequencer = sequencer.clone();
        let principal = authority.principal_key.clone();
        let opening = opening.clone();
        async move {
            let doc = session::new_doc();
            doc.set_peer_id(peer).expect("peer id");
            doc.import(&opening)
                .expect("import the sequencer's opening state");
            let mut vector = session::encode_vector(&doc);
            let mut latencies = Vec::new();
            // Tracked rather than read back from the document: the length
            // is known (the seed plus what this loop has appended), and
            // asking Loro for it every iteration would measure something
            // the ingest path does not pay for.
            let mut len = 1024 * 1024_usize;
            let mut sent = 0_i64;
            let mut accepted = 0_u64;
            let mut retried = 0_u64;
            let mut terminal: Option<String> = None;
            while Instant::now() < deadline {
                let chunk = format!(" edit{sent}from{peer}");
                let applied = session::apply_edits_at(
                    &doc,
                    "paper.md",
                    &[wasm_helpers::text::Edit {
                        at: len,
                        delete: 0,
                        insert: chunk.clone(),
                    }],
                );
                if !applied {
                    terminal = Some("apply_edits_at rejected an append".into());
                    break;
                }
                len += chunk.encode_utf16().count();
                doc.commit();
                let Ok(update) = session::encode_diff(&doc, &vector) else {
                    terminal = Some("encode_diff failed".into());
                    break;
                };
                if update.is_empty() {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    continue;
                }
                sent += 1;
                // §5's back-pressure (`Retryable`) is not a reason to stop:
                // a real client slows down and resends the same update.
                // Only a structural refusal (`Invalid`/`Refused`/`Gap`) ends
                // the run for this editor.
                loop {
                    let at = Instant::now();
                    let outcome = sequencer
                        .ingest(peer, &principal, &principal, sent, update.clone())
                        .await;
                    latencies.push(micros(at));
                    match outcome {
                        crate::log::Ingested::Accepted => {
                            vector = session::encode_vector(&doc);
                            accepted += 1;
                            break;
                        }
                        crate::log::Ingested::Retryable(reason) => {
                            retried += 1;
                            if Instant::now() >= deadline {
                                terminal = Some(format!("retryable at deadline: {reason}"));
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        other => {
                            terminal = Some(format!("{other:?}"));
                            break;
                        }
                    }
                }
                if terminal.is_some() {
                    break;
                }
                // A person types a few characters a second, not a few
                // thousand: a benchmark that spins as fast as it can measures
                // the harness rather than the server.
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
            (latencies, accepted, retried, terminal)
        }
    }))
    .await;
    // Stop ticking before the closing barrier below, so the row count is
    // the periodic flushes plus exactly one closing flush rather than a
    // race with the next tick.
    housekeeping.abort();
    let mut relay: Vec<u64> = Vec::new();
    let mut accepted_total = 0_u64;
    let mut retried_total = 0_u64;
    let mut editor_terminals: Vec<String> = Vec::new();
    for (latencies, accepted, retried, terminal) in typing {
        relay.extend(latencies);
        accepted_total += accepted;
        retried_total += retried;
        if let Some(reason) = terminal {
            editor_terminals.push(reason);
        }
    }
    let elapsed = started_at.elapsed().as_secs_f64().max(1.0);

    // A benchmark that silently reports one relay sample as if it were a
    // measurement is worse than one that fails: three editors sending one
    // update every 120 ms for `seconds` seconds produce, in round numbers,
    // 3 * seconds * 1000 / 120 ingest calls. A run that collapsed to a
    // handful of samples (the two defects this replaced: peers seeded from
    // an unrelated document, or edits that overwrite instead of append)
    // must fail loudly here rather than print a healthy-looking report.
    let expected_samples = 3.0 * (seconds as f64) * 1000.0 / 120.0;
    assert!(
        (relay.len() as f64) >= expected_samples * 0.2,
        "relay sample count {} is implausible for 3 editors over {seconds}s at ~120ms/edit \
         (expected roughly {expected_samples:.0}); accepted={accepted_total} \
         retried={retried_total} editor_terminal_reasons={editor_terminals:?} -- ingest is \
         rejecting updates instead of accepting them, or editors stopped early",
        relay.len(),
    );

    // Rows. §5.2's table claims two per minute for one continuously typing
    // editor, best case. This is the real figure for three.
    sequencer
        .flush(FlushReason::Barrier)
        .await
        .expect("final flush");
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_updates WHERE document_id=$1")
            .bind(document_id)
            .fetch_one(catalog.pool())
            .await
            .expect("row count");
    let state = sequencer.log_state().await;

    // Projection latency for a polling reader. The first call after the
    // typing loop is a cold build (nothing has asked for a projection of
    // this document since the last ingest invalidated the cache), so it is
    // reported on its own rather than folded into the warm distribution as
    // a spurious max; the 50 samples that follow are all against the
    // already-built projection.
    let at = Instant::now();
    sequencer.projection().await.expect("a projection");
    let first_projection_us = micros(at);
    let mut projection = Vec::new();
    for _ in 0..50 {
        let at = Instant::now();
        sequencer.projection().await.expect("a projection");
        projection.push(micros(at));
    }

    // Cache build time after forced eviction. This is a timing measurement
    // only: it used to also drive the expansion factor from the RSS delta
    // across this single evict-and-rebuild, but that delta is not
    // measurable this way. glibc's allocator returns freed pages to its own
    // free list rather than to the kernel, so `drop_cache` does not lower
    // RSS, and the rebuild that follows reuses the very pages it just
    // freed -- the delta is invisible by construction, not small. A run of
    // this benchmark caught it directly: rebuilding a 1276207-byte log
    // moved RSS by exactly one page (4096 bytes), which would have set
    // `measured_expansion_factor` to 0.0032.
    sequencer.drop_cache().await;
    let at = Instant::now();
    sequencer.projection().await.expect("a cold projection");
    let build_us = micros(at);

    // The expansion factor itself, measured the way that survives the
    // problem above: build many distinct documents and never free them.
    // RSS cannot hide growth the allocator never had a chance to reuse, and
    // this measures exactly what §9.2's budget reserves against -- many
    // documents resident in the cache at once -- rather than one document's
    // build-and-evict cycle.
    //
    // `malloc_trim(0)` before each sample was the other candidate: cheap,
    // but it only forces the allocator to hand pages back, which begs the
    // question this benchmark exists to answer (whether the number behind
    // that trim is real). The multi-document approach needs no cooperation
    // from the allocator at all, so it was preferred.
    const EXPANSION_SAMPLE_DOCUMENTS: usize = 20;
    // Below this, a computed factor is a failed measurement, not a small
    // reading. Loro's decoded state carries op-log and version-vector
    // structure that a raw byte count does not, so it cannot come back
    // smaller than the log it was built from; a `null` here must not be
    // read as "close to 1".
    const MIN_PLAUSIBLE_FACTOR: f64 = 1.0;
    // The RSS delta must clear this to be worth dividing. It sits between
    // the two things it has to separate: well above the noise a process
    // moves on its own (thread stacks, allocator arenas, tokio runtime
    // growth), and below the smallest delta a real reading can produce,
    // which is twenty 1 MiB documents at the floor factor of 1. A delta
    // under it means the instrument failed, not that documents are cheap.
    const MIN_PLAUSIBLE_DELTA_BYTES: u64 = 16 * 1024 * 1024;
    /// How many separate edits the typed cohort accumulates per document.
    /// Two thousand is roughly what three editors produce in a
    /// twenty-minute sitting at the rate the typing loop above uses.
    const TYPED_COHORT_EDITS: usize = 2000;

    // Two cohorts, because one number here would be a lie by omission. A
    // document's decoded cost is driven by its OPERATION COUNT at least as
    // much as by its byte count: every op carries an id, a lamport
    // timestamp, a peer and a span in the op log, and none of that is in
    // the byte total the budget estimates from. A 1 MiB document that
    // arrived as a single upload is the cheapest shape Loro can hold; the
    // same 1 MiB typed a few characters at a time is the most expensive.
    // Both are real, so both are measured, and §9.2's factor has to cover
    // the worse of them -- under-reserving is what puts a deployment out of
    // memory, and over-reserving only refuses a read.
    async fn expansion_cohort(
        catalog: &Arc<PostgresCatalog>,
        registry: &Arc<Registry>,
        // The owner is the account this authority already names; passing it
        // separately would be a second way to get one number wrong.
        authority: &Authority,
        name: &str,
        count: usize,
        edits: usize,
        hold: &mut Vec<Arc<crate::log::Sequencer>>,
    ) -> u64 {
        let owner = authority
            .account_id
            .expect("the benchmark owner is a registered account");
        let mut log_bytes_total = 0_u64;
        for index in 0..count {
            let slug = format!("expansion-{name}-{index}");
            let sample_id = document(catalog, owner, slug.clone()).await;
            let sample = registry
                .get(sample_id, &slug)
                .await
                .expect("admit the expansion sample document");
            let doc = session::new_doc();
            doc.set_peer_id(1).expect("peer id");
            // Varied rather than one repeated character: a single run of
            // identical bytes is the best case for any encoder, and a
            // factor measured on it would flatter whatever Loro does with
            // repetition.
            let body: String = (0..1024)
                .map(|line| format!("line {line} of {slug}: the quick brown fox {index}\n"))
                .collect();
            let body = body.repeat(1024 * 1024 / body.len().max(1) + 1);
            let body = &body[..body
                .char_indices()
                .map(|(at, _)| at)
                .find(|at| *at >= 1024 * 1024)
                .unwrap_or(body.len())];
            session::put_text(&doc, "paper.md", body);
            doc.commit();
            let mut vector = session::encode_vector(&doc);
            assert!(matches!(
                sample
                    .ingest(
                        0,
                        &authority.principal_key,
                        &authority.principal_key,
                        0,
                        session::encode_state(&doc)
                    )
                    .await,
                crate::log::Ingested::Accepted
            ));
            // The typed cohort accumulates a real op history on top: the
            // same document, reached by many small edits rather than one.
            let mut len = body.encode_utf16().count();
            for edit in 0..edits {
                let chunk = format!(" e{edit}");
                if !session::apply_edits_at(
                    &doc,
                    "paper.md",
                    &[wasm_helpers::text::Edit {
                        at: len,
                        delete: 0,
                        insert: chunk.clone(),
                    }],
                ) {
                    break;
                }
                len += chunk.encode_utf16().count();
                doc.commit();
                let Ok(update) = session::encode_diff(&doc, &vector) else {
                    break;
                };
                vector = session::encode_vector(&doc);
                if update.is_empty() {
                    continue;
                }
                let _ = sample
                    .ingest(
                        0,
                        &authority.principal_key,
                        &authority.principal_key,
                        edit as i64 + 1,
                        update,
                    )
                    .await;
            }
            sample
                .flush(FlushReason::Barrier)
                .await
                .expect("expansion sample row");
            // Forces the decoded document into the cache. No drop_cache
            // call follows, and the caller holds the handle, so it stays
            // resident for the rest of the run and counts toward the delta.
            sample
                .projection()
                .await
                .expect("expansion sample projection");
            log_bytes_total += sample.log_state().await.log_bytes;
            hold.push(sample);
        }
        log_bytes_total
    }

    let mut held: Vec<Arc<crate::log::Sequencer>> = Vec::new();
    let expansion_rss_before = resident_bytes();
    let uploaded_log_bytes = expansion_cohort(
        &catalog,
        &registry,
        &authority,
        "uploaded",
        EXPANSION_SAMPLE_DOCUMENTS,
        0,
        &mut held,
    )
    .await;
    let uploaded_rss = resident_bytes();
    let typed_log_bytes = expansion_cohort(
        &catalog,
        &registry,
        &authority,
        "typed",
        EXPANSION_SAMPLE_DOCUMENTS,
        TYPED_COHORT_EDITS,
        &mut held,
    )
    .await;
    let expansion_log_bytes_total = uploaded_log_bytes + typed_log_bytes;
    let expansion_rss_after = resident_bytes();
    let uploaded_delta = match (expansion_rss_before, uploaded_rss) {
        (Some(before), Some(after)) if after > before => Some(after - before),
        _ => None,
    };
    let typed_delta = match (uploaded_rss, expansion_rss_after) {
        (Some(before), Some(after)) if after > before => Some(after - before),
        _ => None,
    };
    let uploaded_factor =
        uploaded_delta.map(|delta| delta as f64 / uploaded_log_bytes.max(1) as f64);
    let typed_factor = typed_delta.map(|delta| delta as f64 / typed_log_bytes.max(1) as f64);

    let expansion_rss_delta = match (expansion_rss_before, expansion_rss_after) {
        (Some(before), Some(after)) => after.checked_sub(before),
        _ => None,
    };
    let expansion_raw_factor =
        expansion_rss_delta.map(|delta| delta as f64 / expansion_log_bytes_total.max(1) as f64);
    let (expansion, expansion_measurement_failure): (Option<f64>, Option<&'static str>) = match (
        expansion_rss_before,
        expansion_rss_after,
        expansion_rss_delta,
    ) {
        (None, _, _) | (_, None, _) => (None, Some("resident_bytes unavailable (not Linux)")),
        (Some(_), Some(_), None) => (
            None,
            Some("rss did not increase across the sample documents"),
        ),
        (Some(_), Some(_), Some(delta)) if delta < MIN_PLAUSIBLE_DELTA_BYTES => (
            None,
            Some("rss delta too small to distinguish from measurement noise"),
        ),
        _ => match expansion_raw_factor {
            Some(factor) if factor >= MIN_PLAUSIBLE_FACTOR => (Some(factor), None),
            _ => (
                None,
                Some("computed factor below the sanity floor; treat as a failed measurement"),
            ),
        },
    };

    let wal_bytes: i64 =
        sqlx::query_scalar("SELECT pg_wal_lsn_diff(pg_current_wal_lsn(),$1::pg_lsn)::bigint")
            .bind(&wal_before)
            .fetch_one(catalog.pool())
            .await
            .expect("WAL difference");

    let report = json!({
        "profile": "release",
        "duration_seconds": seconds,
        "is_full_length_run": seconds >= 600,
        "editors": 3,
        "postgres_version": sqlx::query_scalar::<_, String>("SHOW server_version")
            .fetch_one(catalog.pool()).await.expect("version"),
        "rows_written": rows,
        "rows_per_minute": (rows as f64) * 60.0 / elapsed,
        "relay_latency": percentiles(relay),
        "relay_accepted": accepted_total,
        "relay_retried": retried_total,
        "editor_terminal_reasons": editor_terminals,
        // The projection right after the typing loop, still cold at that
        // point; see the comment above the loop that measures it.
        "projection_latency_first_us": first_projection_us,
        "projection_latency_warm": percentiles(projection),
        "cold_build_us": build_us,
        "log_bytes": state.log_bytes,
        // Raw inputs to the expansion measurement, for a human to audit
        // rather than trust the quotient alone.
        "expansion_measurement": {
            "method": "many_documents_never_freed",
            "sample_documents": EXPANSION_SAMPLE_DOCUMENTS,
            "sample_log_bytes_total": expansion_log_bytes_total,
            // Per cohort, because the whole point of running two is that
            // they are expected to differ. A factor set from the uploaded
            // cohort alone would under-reserve for every document anybody
            // has actually typed into.
            "uploaded": {
                "documents": EXPANSION_SAMPLE_DOCUMENTS,
                "edits_each": 0,
                "log_bytes": uploaded_log_bytes,
                "rss_delta_bytes": uploaded_delta,
                "factor": uploaded_factor,
            },
            "typed": {
                "documents": EXPANSION_SAMPLE_DOCUMENTS,
                "edits_each": TYPED_COHORT_EDITS,
                "log_bytes": typed_log_bytes,
                "rss_delta_bytes": typed_delta,
                "factor": typed_factor,
            },
            "rss_before_bytes": expansion_rss_before,
            "rss_after_bytes": expansion_rss_after,
            "rss_delta_bytes": expansion_rss_delta,
            "min_plausible_delta_bytes": MIN_PLAUSIBLE_DELTA_BYTES,
            "min_plausible_factor": MIN_PLAUSIBLE_FACTOR,
            "raw_factor": expansion_raw_factor,
            "failure_reason": expansion_measurement_failure,
        },
        // What §9.2's `DEFAULT_EXPANSION` should be set to. `null` means the
        // measurement could not be taken -- see `expansion_measurement` for
        // why -- and a null here must not be read as a small number.
        "measured_expansion_factor": expansion,
        "configured_expansion_factor": crate::log::budget::DEFAULT_EXPANSION,
        "wal_bytes": wal_bytes,
    });
    let output = serde_json::to_string_pretty(&report).expect("report JSON");
    if let Ok(path) = std::env::var("LIBREPAPER_BENCHMARK_OUTPUT") {
        std::fs::write(path, &output).expect("write benchmark report");
    }
    println!("{output}");
}

/// SPEC-server-is-a-log Priority 2: what compaction costs, stage by stage.
///
/// §8.4 is six distinct pieces of work with six different failure modes, and
/// the remaining Priority 3 items are each conditioned on which of them
/// dominates ("if export stalls editing", "if verification or compression
/// blocks async workers", "if repeated full-history rewrites dominate").
/// A single end-to-end compaction number cannot answer any of those, so this
/// times them separately, on the same call sequence `worker::compact` uses:
///
/// | stage          | what is timed                                          |
/// |----------------|--------------------------------------------------------|
/// | flush          | step 1, the barrier flush that empties the buffer      |
/// | reconstruction | step 2's cold build: base + every row + buffer          |
/// | export         | step 3, `ExportMode::Snapshot` off the built entry      |
/// | verification   | step 4, `prove_coverage`: header, then a scratch load   |
/// | compression    | step 5a's zstd, timed on its own                        |
/// | upload         | step 5a's object-store round trip and length check      |
/// | activation     | step 5b: the catalogue transaction and the gate         |
///
/// Reconstruction and export are measured apart by dropping the cache and
/// building it through `with_head` first, so `snapshot_at_log_vector` finds
/// a warm entry and times only the export. In production those two are one
/// call; the sum of the two numbers here is that call.
///
/// Compression and upload come from the one production call that does both
/// (`write_base`): the zstd is timed separately at the same level, and upload
/// is the remainder. Both the remainder and the raw total are reported, so a
/// reader can see what was subtracted from what.
///
/// Two things are measured around the stages rather than inside them:
///
/// * **Peak memory.** A sampler reads `/proc/self/statm` every 5 ms for the
///   whole compaction and keeps the maximum. Before-and-after RSS cannot see
///   this: the scratch document `prove_coverage` loads, the snapshot buffer
///   and its compressed copy are all freed before compaction returns, and
///   glibc hands those pages to its own arena rather than to the kernel (the
///   same effect documented on the expansion factor above). The peak is the
///   number §9.2 has to reserve against.
/// * **Editing latency.** One editor types at a human rate throughout, and
///   its ingest latencies are split at the instant compaction starts. The
///   comparison is the point: `build` and `snapshot_at_log_vector` both hold
///   the sequencer's `inner` lock across uninterruptible Loro work, and
///   ingest takes that same lock, so whatever that costs shows up here as
///   the difference between the two distributions. This is the measurement
///   Priority 3's first item is conditioned on.
///
/// The document is long-lived by construction: `edits` separate committed
/// edits accumulate on top of a 1 MiB seed, flushed every
/// `EDITS_PER_ROW`, which is roughly what §5.2's 30 s flush produces at the
/// typing rate the throughput benchmark above uses. Several shapes are run
/// because the interesting question is which stages scale with the op count
/// and which only with the byte count.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "release acceptance benchmark; destroys every row in its configured database"]
async fn compaction_cost_release_benchmark() {
    /// Edits per log row, chosen to match §5.2's rate: the throughput
    /// benchmark above types one update per 120 ms, and a 30 s flush window
    /// holds about 250 of them.
    const EDITS_PER_ROW: usize = 250;
    /// How long the typing editor runs before compaction starts, to give the
    /// baseline half of the latency comparison its own samples.
    const BASELINE: Duration = Duration::from_secs(3);
    /// The typing rate, the same one the throughput benchmark uses: a person
    /// types a few characters a second.
    const KEYSTROKE: Duration = Duration::from_millis(120);
    /// How often the RSS sampler looks. Fine enough to catch a peak that
    /// lasts a few tens of milliseconds, cheap enough that the sampler is
    /// not itself part of what is measured.
    const RSS_SAMPLE: Duration = Duration::from_millis(5);
    /// How many times to re-ask for a snapshot that raced a keystroke.
    /// `snapshot_at_log_vector` refuses an entry with anything buffered, so
    /// a compaction that loses the race against the typing editor returns
    /// nothing and is asked again -- exactly what the worker does when the
    /// next flush reports the thresholds are still crossed.
    const SNAPSHOT_ATTEMPTS: usize = 40;

    let Ok(url) = std::env::var("LIBREPAPER_BENCHMARK_POSTGRES_URL") else {
        return;
    };
    // The shapes to run, each `edits:seed_kib`: how many separate edits
    // accumulate, and how large the document was before any of them. The
    // first three span a session, a day and a long-lived document at the
    // 1 MiB §14.1 names. The fourth is a control and not a shape anybody
    // deploys: the same history over a seed small enough to be free, which
    // is the only way to say whether a stage's cost follows the op count or
    // the byte count. Without it the three 1 MiB rows can only be read as
    // "it costs about this much", never as "it costs this much BECAUSE".
    let shapes: Vec<(usize, usize)> = std::env::var("LIBREPAPER_COMPACTION_BENCHMARK_EDITS")
        .unwrap_or_else(|_| "500:1024,2000:1024,8000:1024,2000:16".into())
        .split(',')
        .filter_map(|value| {
            let (edits, seed) = value.trim().split_once(':')?;
            Some((edits.trim().parse().ok()?, seed.trim().parse().ok()?))
        })
        .collect();
    assert!(!shapes.is_empty(), "no shapes to measure");

    let mut options = PostgresOptions::new(url);
    options.max_connections = 32;
    options.policy = StoragePolicy {
        owner_bytes: i64::MAX / 4,
        deployment_bytes: i64::MAX / 4,
        asset_uploads_per_hour: 100_000,
    };
    let catalog = Arc::new(PostgresCatalog::connect(options).await.expect("connect"));
    catalog.migrate().await.expect("migrate");
    sqlx::query("TRUNCATE accounts CASCADE")
        .execute(catalog.pool())
        .await
        .expect("empty disposable benchmark database");
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("benchmark".into()),
            provider_subject: Some("owner".into()),
            handle: "benchmark".into(),
            display_name: "Benchmark".into(),
            email: None,
        })
        .await
        .expect("owner");
    let authority = Authority {
        principal_key: owner.id.to_string(),
        account_id: Some(owner.id),
        link_hash: None,
    };
    let _writer = catalog.claim_writer().await.expect("writer lease");

    // A real object store is not assumed: the upload stage is timed against
    // whatever `LIBREPAPER_BENCHMARK_BLOB_DIR` names, defaulting to a
    // scratch directory. A filesystem store is a floor for that stage and
    // the report says so, because an S3 round trip is not a local write.
    let scratch = tempfile::tempdir().expect("scratch blob directory");
    let blob_root = std::env::var("LIBREPAPER_BENCHMARK_BLOB_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| scratch.path().to_path_buf());
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(crate::storage::blob::FsStore::new(blob_root.clone(), false));
    let config = Arc::new(crate::config::Configuration {
        // The seeding loop below writes a document's whole history as fast
        // as it can, which is not a person and must not be rate-limited
        // into taking an hour. The measured stages are unaffected: nothing
        // in §8.4 consults either of these.
        log_quota_bytes: usize::MAX / 4,
        memory_budget_bytes: u64::MAX / 4,
        session: crate::config::SessionLimit {
            updates_per_minute: 10_000_000,
            ..crate::config::Configuration::default().session
        },
        ..crate::config::Configuration::default()
    });
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "benchmark".to_string(),
    );

    let mut measurements = Vec::new();
    for (edits, seed_kib) in shapes.iter().copied() {
        let slug = format!("compaction-{edits}-{seed_kib}kib");
        let document_id = document(&catalog, owner.id, slug.clone()).await;
        let sequencer = registry
            .get(document_id, &slug)
            .await
            .expect("admit the document");

        // The seed, as one upload the way a real document arrives. Varied
        // text rather than one repeated character for the reason the
        // expansion cohort gives -- a run of identical bytes flatters any
        // encoder.
        let doc = session::new_doc();
        doc.set_peer_id(1).expect("peer id");
        let line: String = (0..1024)
            .map(|index| format!("line {index} of {slug}: the quick brown fox\n"))
            .collect();
        let seed_bytes = seed_kib * 1024;
        let body = line.repeat(seed_bytes / line.len().max(1) + 1);
        let body = &body[..body
            .char_indices()
            .map(|(at, _)| at)
            .find(|at| *at >= seed_bytes)
            .unwrap_or(body.len())];
        session::put_text(&doc, "paper.md", body);
        doc.commit();
        // Where the seed ends, which is where both the seeding loop below
        // and the typing editor further down append.
        let seed_len = body.encode_utf16().count();
        let opening = session::encode_state(&doc);
        let admitted = sequencer
            .ingest(
                0,
                &authority.principal_key,
                &authority.principal_key,
                0,
                opening.clone(),
            )
            .await;
        // A seed ingest can be refused on its own terms: §5 caps one update
        // at max_update_bytes, so a large enough document cannot be
        // uploaded in one batch at all. That is a boundary worth recording
        // rather than a harness fault, and the shapes after it still run.
        if !matches!(admitted, crate::log::Ingested::Accepted) {
            measurements.push(json!({
                "edits": edits,
                "seed_kib": seed_kib,
                "seed_bytes_actual": seed_len,
                "opening_update_bytes": opening.len(),
                "seed_refused": format!("{admitted:?}"),
                "max_update_bytes": crate::log::sequencer::max_update_bytes(config.log_quota_bytes),
            }));
            continue;
        }
        sequencer
            .flush(FlushReason::Barrier)
            .await
            .expect("seed row");

        // The history. Each edit is its own commit and its own ingest, so
        // the op count is real; the periodic flush is what turns them into
        // the rows a cold build has to read back.
        let mut vector = session::encode_vector(&doc);
        let mut len = seed_len;
        let seeding = Instant::now();
        for edit in 0..edits {
            let chunk = format!(" e{edit}");
            assert!(
                session::apply_edits_at(
                    &doc,
                    "paper.md",
                    &[wasm_helpers::text::Edit {
                        at: len,
                        delete: 0,
                        insert: chunk.clone(),
                    }],
                ),
                "seeding edit {edit} was rejected"
            );
            len += chunk.encode_utf16().count();
            doc.commit();
            let update = session::encode_diff(&doc, &vector).expect("seeding diff");
            vector = session::encode_vector(&doc);
            assert!(
                matches!(
                    sequencer
                        .ingest(
                            0,
                            &authority.principal_key,
                            &authority.principal_key,
                            edit as i64 + 1,
                            update
                        )
                        .await,
                    crate::log::Ingested::Accepted
                ),
                "seeding edit {edit} was not accepted"
            );
            if (edit + 1) % EDITS_PER_ROW == 0 {
                sequencer
                    .flush(FlushReason::MaxAge)
                    .await
                    .expect("seeding row");
            }
        }
        sequencer
            .flush(FlushReason::Barrier)
            .await
            .expect("last seeding row");
        let seeding_us = micros(seeding);
        let before = sequencer.log_state().await;
        let rows_before: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_updates WHERE document_id=$1")
                .bind(document_id)
                .fetch_one(catalog.pool())
                .await
                .expect("row count");

        // One editor, typing through the whole thing at a human rate. Its
        // latencies are split at `compaction_started` below.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let typist = tokio::spawn({
            let sequencer = sequencer.clone();
            let principal = authority.principal_key.clone();
            let opening = opening.clone();
            let stop = stop.clone();
            async move {
                let doc = session::new_doc();
                doc.set_peer_id(9).expect("peer id");
                doc.import(&opening).expect("the opening state");
                // This editor joined at the seed and has not seen the
                // history above, which is exactly a client that reconnects:
                // its updates are causally after what it holds, and §5's gap
                // check is satisfied by the seed it shares.
                let mut vector = session::encode_vector(&doc);
                let mut len = seed_len;
                let mut sent = 0_i64;
                // (when the ingest started, how long it took)
                let mut samples: Vec<(Instant, u64)> = Vec::new();
                // Why this editor stopped early, if it did. An empty
                // half of the latency comparison is a failed measurement,
                // and the assertion below can only say which failure it
                // was if the editor says so.
                let mut terminal: Option<String> = None;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let chunk = format!(" t{sent}");
                    if !session::apply_edits_at(
                        &doc,
                        "paper.md",
                        &[wasm_helpers::text::Edit {
                            at: len,
                            delete: 0,
                            insert: chunk.clone(),
                        }],
                    ) {
                        terminal = Some(format!("apply_edits_at rejected an append at {len}"));
                        break;
                    }
                    len += chunk.encode_utf16().count();
                    doc.commit();
                    let Ok(update) = session::encode_diff(&doc, &vector) else {
                        terminal = Some("encode_diff failed".into());
                        break;
                    };
                    if update.is_empty() {
                        tokio::time::sleep(KEYSTROKE).await;
                        continue;
                    }
                    sent += 1;
                    let at = Instant::now();
                    let outcome = sequencer
                        .ingest(9, &principal, &principal, sent, update)
                        .await;
                    samples.push((at, micros(at)));
                    match outcome {
                        crate::log::Ingested::Accepted => vector = session::encode_vector(&doc),
                        // Back-pressure is not a reason to stop: a real
                        // client resends. Anything else is structural.
                        crate::log::Ingested::Retryable(_) => {}
                        other => {
                            terminal = Some(format!("{other:?}"));
                            break;
                        }
                    }
                    tokio::time::sleep(KEYSTROKE).await;
                }
                (samples, terminal)
            }
        });
        tokio::time::sleep(BASELINE).await;

        // Peak RSS across the compaction only. Started here rather than at
        // process start because a lifetime high-water mark would report the
        // seeding loop above, which is not compaction.
        let peak = Arc::new(std::sync::atomic::AtomicU64::new(
            resident_bytes().unwrap_or(0),
        ));
        let sampler = tokio::spawn({
            let peak = peak.clone();
            async move {
                let mut ticker = tokio::time::interval(RSS_SAMPLE);
                loop {
                    ticker.tick().await;
                    if let Some(rss) = resident_bytes() {
                        peak.fetch_max(rss, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        });
        let rss_before = resident_bytes();
        let compaction_started = Instant::now();

        // Step 1.
        let at = Instant::now();
        sequencer
            .flush(FlushReason::Barrier)
            .await
            .expect("the barrier flush");
        let flush_us = micros(at);

        // Steps 2 and 3, measured apart. `with_head` builds and reads
        // nothing, so it times the reconstruction alone; the export then
        // finds a warm entry.
        sequencer.drop_cache().await;
        let at = Instant::now();
        let rebuilt = sequencer.with_head(|_| ()).await;
        let reconstruction_us = micros(at);
        // A cold build that fails is a RESULT, not a harness fault: §9.3
        // marks a document unreadable when its build passes
        // `BUILD_DEADLINE`, and a shape that reaches that is the most
        // important thing this benchmark can report -- compaction can never
        // run on it, so its log can never be trimmed. Recorded and skipped
        // rather than panicked on, so the shapes after it still measure.
        if let Err(error) = rebuilt {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            sampler.abort();
            let (samples, typist_terminal) = typist.await.expect("the typing editor");
            measurements.push(json!({
                "edits": edits,
                "seed_kib": seed_kib,
                "seed_bytes_actual": seed_len,
                "log_bytes_before": before.log_bytes,
                "rows_before": rows_before,
                "reconstruction_us": reconstruction_us,
                "reconstruction_failed": error.to_string(),
                "build_deadline_seconds": crate::log::sequencer::BUILD_DEADLINE.as_secs(),
                "editor_samples": samples.len(),
                "editor_terminal_reason": typist_terminal,
                "memory": {
                    "rss_before_bytes": rss_before,
                    "rss_peak_bytes": peak.load(std::sync::atomic::Ordering::Relaxed),
                },
            }));
            continue;
        }

        let mut attempts = 0_usize;
        let (export_us, snapshot) = loop {
            attempts += 1;
            // A keystroke that landed since the last flush leaves the buffer
            // non-empty, and an entry with anything buffered is refused.
            sequencer
                .flush(FlushReason::Barrier)
                .await
                .expect("a flush before the snapshot");
            let at = Instant::now();
            let taken = sequencer
                .snapshot_at_log_vector()
                .await
                .expect("a snapshot attempt");
            if let Some(taken) = taken {
                break (micros(at), taken);
            }
            assert!(
                attempts < SNAPSHOT_ATTEMPTS,
                "{SNAPSHOT_ATTEMPTS} attempts and the entry was never at the log vector: \
                 compaction cannot win the race against one editor typing every {}ms",
                KEYSTROKE.as_millis(),
            );
        };
        let (through, snapshot, log_vector, changes) = snapshot;

        // Step 4.
        let at = Instant::now();
        crate::storage::worker::prove_coverage(&snapshot, &log_vector, changes)
            .expect("the snapshot covers the log");
        let verification_us = micros(at);

        // Step 5a, in two halves. The compression is the same call at the
        // same level `write_base` makes; `write_base` then makes it again,
        // and the difference is the object-store round trip and the length
        // check that follows it.
        let at = Instant::now();
        let compressed = zstd::stream::encode_all(std::io::Cursor::new(&snapshot[..]), 3)
            .expect("compress the snapshot");
        let compression_us = micros(at);
        let storage = crate::storage::collaboration::CollaborationStorage::new(
            catalog.clone(),
            blobs.clone(),
        );
        let at = Instant::now();
        let written = storage
            .write_base(document_id, &snapshot)
            .await
            .expect("write the base");
        let write_base_us = micros(at);
        let base_bytes = written.bytes;

        // Step 5b.
        let at = Instant::now();
        let mut gate = sequencer.compaction_gate().await;
        let activated = catalog
            .activate_log_base(
                document_id,
                through,
                &log_vector,
                written,
                crate::storage::collaboration::superseded_base_deadline(),
            )
            .await
            .expect("activate the base");
        let activated = activated.expect("a base was activated");
        let base = activated.base;
        gate.note_compacted(
            through,
            &base.vector,
            base.snapshot_bytes.max(0) as u64,
            activated.uncompacted_bytes.max(0) as u64,
            activated.uncompacted_count.max(0),
        );
        drop(gate);
        let activation_us = micros(at);
        let total_us = micros(compaction_started);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        sampler.abort();
        let (samples, typist_terminal) = typist.await.expect("the typing editor");
        let rss_after = resident_bytes();
        let rss_peak = peak.load(std::sync::atomic::Ordering::Relaxed);
        let (baseline, during): (Vec<_>, Vec<_>) = samples
            .into_iter()
            .partition(|(at, _)| *at < compaction_started);
        let baseline: Vec<u64> = baseline.into_iter().map(|(_, took)| took).collect();
        let during: Vec<u64> = during.into_iter().map(|(_, took)| took).collect();
        // Both halves have to have samples or the comparison is not one.
        // The baseline window is three seconds at one keystroke per 120 ms;
        // the compaction window is however long compaction took, and if that
        // is under one keystroke the editor may legitimately have sent
        // nothing, which is itself the answer to "does this stall typing".
        assert!(
            !baseline.is_empty(),
            "the typing editor produced no baseline samples in {BASELINE:?} \
             (terminal reason: {typist_terminal:?})",
        );

        let rows_after: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_updates WHERE document_id=$1")
                .bind(document_id)
                .fetch_one(catalog.pool())
                .await
                .expect("row count after");
        // The cold build a reader pays after compaction, which is the thing
        // compaction exists to shorten: the same measurement as
        // `reconstruction_us` above, now against one base instead of
        // `rows_before` rows.
        sequencer.drop_cache().await;
        let at = Instant::now();
        sequencer
            .with_head(|_| ())
            .await
            .expect("a cold build from the base");
        let rebuild_from_base_us = micros(at);

        measurements.push(json!({
            "edits": edits,
            "seed_kib": seed_kib,
            // What the seed actually came out as, not what was asked for:
            // the attribution of every stage below rests on the seed being
            // the size this row claims.
            "seed_bytes_actual": seed_len,
            "edits_per_row": EDITS_PER_ROW,
            "seeding_us": seeding_us,
            "log_bytes_before": before.log_bytes,
            "rows_before": rows_before,
            "rows_after": rows_after,
            "changes": changes,
            "snapshot_bytes": snapshot.len(),
            "base_bytes": base_bytes,
            "compressed_bytes": compressed.len(),
            "compression_ratio": snapshot.len() as f64 / compressed.len().max(1) as f64,
            "stages_us": {
                "flush": flush_us,
                "reconstruction": reconstruction_us,
                "export": export_us,
                "verification": verification_us,
                "compression": compression_us,
                // `write_base` less the compression above. Negative would
                // mean the two zstd runs differed by more than the round
                // trip, which is a reason not to trust either, so it is
                // saturated and the raw total is reported beside it.
                "upload": write_base_us.saturating_sub(compression_us),
                "write_base_total": write_base_us,
                "activation": activation_us,
                "total": total_us,
            },
            // How many times the snapshot had to be asked for because a
            // keystroke had landed since the flush. One means compaction won
            // on the first try; a large number is §8.4 losing to ordinary
            // typing, which would make compaction on a busy document rare.
            "snapshot_attempts": attempts,
            "rebuild_from_base_us": rebuild_from_base_us,
            "memory": {
                "rss_before_bytes": rss_before,
                "rss_peak_bytes": rss_peak,
                "rss_after_bytes": rss_after,
                "peak_over_before_bytes": rss_before.map(|before| rss_peak.saturating_sub(before)),
                "sample_interval_ms": RSS_SAMPLE.as_millis() as u64,
            },
            "editing_latency": {
                "editor_terminal_reason": typist_terminal,
                "baseline": percentiles(baseline),
                "during_compaction": percentiles(during),
            },
        }));
        // Retired before the next shape, so one shape's resident document
        // does not sit in the budget while the next one is measured.
        sequencer.drop_cache().await;
    }

    let report = json!({
        "profile": "release",
        "measurement": "compaction_stages",
        "postgres_version": sqlx::query_scalar::<_, String>("SHOW server_version")
            .fetch_one(catalog.pool()).await.expect("version"),
        // A filesystem store is a floor for the upload stage: an S3 round
        // trip is not a local write, and a reader comparing stages has to
        // know which one produced these numbers.
        "blob_store": format!("filesystem:{}", blob_root.display()),
        "shapes": shapes,
        "documents": measurements,
    });
    let output = serde_json::to_string_pretty(&report).expect("report JSON");
    if let Ok(path) = std::env::var("LIBREPAPER_COMPACTION_BENCHMARK_OUTPUT") {
        std::fs::write(path, &output).expect("write benchmark report");
    }
    println!("{output}");
}

/// One seeded document for `concurrent_compaction_release_benchmark`: a
/// 1 MiB upload plus `edits` accumulated edits, flushed the same way
/// `compaction_cost_release_benchmark`'s seeding loop does. Returns the
/// sequencer and the seed length, or -- if the opening upload itself was
/// refused, the same boundary the benchmark beside this one records rather
/// than panics on -- the refusal as JSON.
async fn seed_concurrent_document(
    catalog: &Arc<PostgresCatalog>,
    registry: &Arc<Registry>,
    authority: &Authority,
    slug: String,
    seed_kib: usize,
    edits: usize,
    edits_per_row: usize,
) -> Result<(uuid::Uuid, Arc<crate::log::Sequencer>, usize), Value> {
    let owner = authority
        .account_id
        .expect("the benchmark owner is a registered account");
    let document_id = document(catalog, owner, slug.clone()).await;
    let sequencer = registry
        .get(document_id, &slug)
        .await
        .expect("admit the document");

    let doc = session::new_doc();
    doc.set_peer_id(1).expect("peer id");
    let line: String = (0..1024)
        .map(|index| format!("line {index} of {slug}: the quick brown fox\n"))
        .collect();
    let seed_bytes = seed_kib * 1024;
    let body = line.repeat(seed_bytes / line.len().max(1) + 1);
    let body = &body[..body
        .char_indices()
        .map(|(at, _)| at)
        .find(|at| *at >= seed_bytes)
        .unwrap_or(body.len())];
    session::put_text(&doc, "paper.md", body);
    doc.commit();
    let seed_len = body.encode_utf16().count();
    let opening = session::encode_state(&doc);
    let admitted = sequencer
        .ingest(
            0,
            &authority.principal_key,
            &authority.principal_key,
            0,
            opening.clone(),
        )
        .await;
    if !matches!(admitted, crate::log::Ingested::Accepted) {
        return Err(json!({
            "slug": slug,
            "seed_kib": seed_kib,
            "seed_bytes_actual": seed_len,
            "opening_update_bytes": opening.len(),
            "seed_refused": format!("{admitted:?}"),
            "max_update_bytes": crate::log::sequencer::max_update_bytes(crate::config::Configuration::default().log_quota_bytes),
        }));
    }
    sequencer
        .flush(FlushReason::Barrier)
        .await
        .expect("seed row");

    let mut vector = session::encode_vector(&doc);
    let mut len = seed_len;
    for edit in 0..edits {
        let chunk = format!(" e{edit}");
        assert!(
            session::apply_edits_at(
                &doc,
                "paper.md",
                &[wasm_helpers::text::Edit {
                    at: len,
                    delete: 0,
                    insert: chunk.clone(),
                }],
            ),
            "seeding edit {edit} for {slug} was rejected"
        );
        len += chunk.encode_utf16().count();
        doc.commit();
        let update = session::encode_diff(&doc, &vector).expect("seeding diff");
        vector = session::encode_vector(&doc);
        assert!(
            matches!(
                sequencer
                    .ingest(
                        0,
                        &authority.principal_key,
                        &authority.principal_key,
                        edit as i64 + 1,
                        update
                    )
                    .await,
                crate::log::Ingested::Accepted
            ),
            "seeding edit {edit} for {slug} was not accepted"
        );
        if (edit + 1) % edits_per_row == 0 {
            sequencer
                .flush(FlushReason::MaxAge)
                .await
                .expect("seeding row");
        }
    }
    sequencer
        .flush(FlushReason::Barrier)
        .await
        .expect("last seeding row");
    Ok((document_id, sequencer, seed_len))
}

/// Runs the same call sequence `worker::compact` uses -- and
/// `compaction_cost_release_benchmark` measures alone -- against one already
/// seeded document, meant to be awaited concurrently with others. A cold
/// build that fails is `Err` with the reason rather than a panic, the same
/// rule the sibling benchmark follows, so one document in a bad shape does
/// not take the rest of the cohort down with it.
async fn compact_seeded_document(
    document_id: uuid::Uuid,
    sequencer: Arc<crate::log::Sequencer>,
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn crate::storage::blob::BlobStore>,
    snapshot_attempts_max: usize,
    keystroke: Duration,
) -> Result<Value, Value> {
    // Step 1.
    let at = Instant::now();
    sequencer
        .flush(FlushReason::Barrier)
        .await
        .expect("the barrier flush");
    let flush_us = micros(at);

    // Steps 2 and 3, measured apart exactly as the sibling benchmark does.
    sequencer.drop_cache().await;
    let at = Instant::now();
    let rebuilt = sequencer.with_head(|_| ()).await;
    let reconstruction_us = micros(at);
    if let Err(error) = rebuilt {
        return Err(json!({
            "reconstruction_us": reconstruction_us,
            "reconstruction_failed": error.to_string(),
            "build_deadline_seconds": crate::log::sequencer::BUILD_DEADLINE.as_secs(),
        }));
    }

    let mut attempts = 0_usize;
    let (export_us, snapshot) = loop {
        attempts += 1;
        sequencer
            .flush(FlushReason::Barrier)
            .await
            .expect("a flush before the snapshot");
        let at = Instant::now();
        let taken = sequencer
            .snapshot_at_log_vector()
            .await
            .expect("a snapshot attempt");
        if let Some(taken) = taken {
            break (micros(at), taken);
        }
        if attempts >= snapshot_attempts_max {
            return Err(json!({
                "flush_us": flush_us,
                "reconstruction_us": reconstruction_us,
                "snapshot_attempts": attempts,
                "snapshot_never_reached_log_vector": true,
            }));
        }
        tokio::time::sleep(keystroke / 4).await;
    };
    let (through, snapshot, log_vector, changes) = snapshot;

    // Step 4.
    let at = Instant::now();
    crate::storage::worker::prove_coverage(&snapshot, &log_vector, changes)
        .expect("the snapshot covers the log");
    let verification_us = micros(at);

    // Step 5a, in the same two halves the sibling benchmark uses.
    let at = Instant::now();
    let compressed = zstd::stream::encode_all(std::io::Cursor::new(&snapshot[..]), 3)
        .expect("compress the snapshot");
    let compression_us = micros(at);
    let storage = crate::storage::collaboration::CollaborationStorage::new(catalog.clone(), blobs);
    let at = Instant::now();
    let written = storage
        .write_base(document_id, &snapshot)
        .await
        .expect("write the base");
    let write_base_us = micros(at);
    let base_bytes = written.bytes;

    // Step 5b.
    let at = Instant::now();
    let mut gate = sequencer.compaction_gate().await;
    let activated = catalog
        .activate_log_base(
            document_id,
            through,
            &log_vector,
            written,
            crate::storage::collaboration::superseded_base_deadline(),
        )
        .await
        .expect("activate the base");
    let activated = activated.expect("a base was activated");
    let base = activated.base;
    gate.note_compacted(
        through,
        &base.vector,
        base.snapshot_bytes.max(0) as u64,
        activated.uncompacted_bytes.max(0) as u64,
        activated.uncompacted_count.max(0),
    );
    drop(gate);
    let activation_us = micros(at);

    Ok(json!({
        "changes": changes,
        "snapshot_bytes": snapshot.len(),
        "base_bytes": base_bytes,
        "compressed_bytes": compressed.len(),
        "snapshot_attempts": attempts,
        "stages_us": {
            "flush": flush_us,
            "reconstruction": reconstruction_us,
            "export": export_us,
            "verification": verification_us,
            "compression": compression_us,
            "upload": write_base_us.saturating_sub(compression_us),
            "write_base_total": write_base_us,
            "activation": activation_us,
        },
    }))
}

/// SPEC-server-is-a-log Priority 3: what compaction costs when several
/// documents compact at once, not one on an otherwise idle process.
///
/// `compaction_cost_release_benchmark` deliberately runs one document alone
/// so a stage's cost can be attributed to it; that is the right way to
/// attribute a stage and the wrong way to size a deployment, which is why
/// its numbers say nothing about what happens when a busy server has to
/// compact several documents at the same moment. §9.3's admission semaphore
/// (`log::admission`) bounds concurrent heavy work -- a build or an export --
/// to `min(cores, 4)`; this fires N compactions at once, N taken from
/// `LIBREPAPER_CONCURRENT_COMPACTION_N` (default "2,4,8"), so an N above the
/// bound shows queueing rather than contention, and the run records how many
/// cores it saw so a reader can tell the two apart.
///
/// Each of the N documents is a 1 MiB seed plus 2000 accumulated edits, the
/// same shape `compaction_cost_release_benchmark`'s "typed" row uses. The
/// call sequence for each is the one `worker::compact` uses and the sibling
/// benchmark measures alone: flush, `snapshot_at_log_vector`,
/// `worker::prove_coverage`, `write_base`, then `activate_log_base` +
/// `log_head` + `compaction_gate`. A cold build that fails, or a seed
/// refused at ingest, is recorded with its reason and stepped over rather
/// than panicked on.
///
/// Four things are reported per N: per-stage latency distributions across
/// the N compactions (not just a mean -- the whole point of the semaphore is
/// that some of the N wait for others), the wall time for the whole cohort
/// against N times the single-document cost, the peak process RSS across the
/// concurrent window (sampled every 5 ms, against a baseline taken just
/// before), and ingest latency for an editor typing into an UNRELATED
/// document throughout, split into a baseline window before the cohort
/// starts and the window while it runs -- this last one is the number that
/// says whether a busy compacting server still feels responsive to someone
/// who is not being compacted.
#[tokio::test(flavor = "multi_thread", worker_threads = 32)]
#[ignore = "release acceptance benchmark; destroys every row in its configured database"]
async fn concurrent_compaction_release_benchmark() {
    const EDITS_PER_ROW: usize = 250;
    const SEED_KIB: usize = 1024;
    const COHORT_EDITS: usize = 2000;
    const BASELINE: Duration = Duration::from_secs(3);
    const KEYSTROKE: Duration = Duration::from_millis(120);
    const RSS_SAMPLE: Duration = Duration::from_millis(5);
    const SNAPSHOT_ATTEMPTS: usize = 80;

    let Ok(url) = std::env::var("LIBREPAPER_BENCHMARK_POSTGRES_URL") else {
        return;
    };
    let cohort_sizes: Vec<usize> = std::env::var("LIBREPAPER_CONCURRENT_COMPACTION_N")
        .unwrap_or_else(|_| "2,4,8".into())
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect();
    assert!(!cohort_sizes.is_empty(), "no cohort sizes to measure");

    let mut options = PostgresOptions::new(url);
    options.max_connections = 64;
    options.policy = StoragePolicy {
        owner_bytes: i64::MAX / 4,
        deployment_bytes: i64::MAX / 4,
        asset_uploads_per_hour: 100_000,
    };
    let catalog = Arc::new(PostgresCatalog::connect(options).await.expect("connect"));
    catalog.migrate().await.expect("migrate");
    sqlx::query("TRUNCATE accounts CASCADE")
        .execute(catalog.pool())
        .await
        .expect("empty disposable benchmark database");
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("benchmark".into()),
            provider_subject: Some("owner".into()),
            handle: "benchmark".into(),
            display_name: "Benchmark".into(),
            email: None,
        })
        .await
        .expect("owner");
    let authority = Authority {
        principal_key: owner.id.to_string(),
        account_id: Some(owner.id),
        link_hash: None,
    };
    let _writer = catalog.claim_writer().await.expect("writer lease");

    let scratch = tempfile::tempdir().expect("scratch blob directory");
    let blob_root = std::env::var("LIBREPAPER_BENCHMARK_BLOB_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| scratch.path().to_path_buf());
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(crate::storage::blob::FsStore::new(blob_root.clone(), false));
    let config = Arc::new(crate::config::Configuration {
        log_quota_bytes: usize::MAX / 4,
        memory_budget_bytes: u64::MAX / 4,
        session: crate::config::SessionLimit {
            updates_per_minute: 10_000_000,
            ..crate::config::Configuration::default().session
        },
        ..crate::config::Configuration::default()
    });
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "benchmark".to_string(),
    );

    let cores_available = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    let admission_bound = crate::log::admission::concurrency();

    let mut cohorts = Vec::new();
    for n in cohort_sizes.iter().copied() {
        // The N documents to compact together, seeded ahead of the timed
        // window so seeding cost never leaks into the numbers below.
        let mut seeded = Vec::new();
        let mut seed_refusals = Vec::new();
        for index in 0..n {
            let slug = format!("concurrent-{n}-{index}");
            match seed_concurrent_document(
                &catalog,
                &registry,
                &authority,
                slug,
                SEED_KIB,
                COHORT_EDITS,
                EDITS_PER_ROW,
            )
            .await
            {
                Ok(document) => seeded.push(document),
                Err(refusal) => seed_refusals.push(refusal),
            }
        }

        // The unrelated document: a fresh 1 MiB seed, never touched by
        // compaction, that one editor types into for the whole window. This
        // is what answers whether the server stays responsive to someone who
        // is not being compacted while it is busy compacting everyone else.
        let unrelated_slug = format!("concurrent-{n}-unrelated");
        let (_unrelated_id, unrelated_sequencer, unrelated_seed_len) = seed_concurrent_document(
            &catalog,
            &registry,
            &authority,
            unrelated_slug,
            SEED_KIB,
            0,
            EDITS_PER_ROW,
        )
        .await
        .expect("the unrelated document's opening upload");
        let unrelated_opening = unrelated_sequencer
            .with_head(session::encode_state)
            .await
            .expect("the unrelated document's opening state");

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let editor = tokio::spawn({
            let sequencer = unrelated_sequencer.clone();
            let principal = authority.principal_key.clone();
            let stop = stop.clone();
            async move {
                let doc = session::new_doc();
                doc.set_peer_id(9).expect("peer id");
                doc.import(&unrelated_opening)
                    .expect("the unrelated document's opening state");
                let mut vector = session::encode_vector(&doc);
                let mut len = unrelated_seed_len;
                let mut sent = 0_i64;
                let mut samples: Vec<(Instant, u64)> = Vec::new();
                let mut terminal: Option<String> = None;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let chunk = format!(" u{sent}");
                    if !session::apply_edits_at(
                        &doc,
                        "paper.md",
                        &[wasm_helpers::text::Edit {
                            at: len,
                            delete: 0,
                            insert: chunk.clone(),
                        }],
                    ) {
                        terminal = Some(format!("apply_edits_at rejected an append at {len}"));
                        break;
                    }
                    len += chunk.encode_utf16().count();
                    doc.commit();
                    let Ok(update) = session::encode_diff(&doc, &vector) else {
                        terminal = Some("encode_diff failed".into());
                        break;
                    };
                    if update.is_empty() {
                        tokio::time::sleep(KEYSTROKE).await;
                        continue;
                    }
                    sent += 1;
                    let at = Instant::now();
                    let outcome = sequencer
                        .ingest(9, &principal, &principal, sent, update)
                        .await;
                    samples.push((at, micros(at)));
                    match outcome {
                        crate::log::Ingested::Accepted => vector = session::encode_vector(&doc),
                        crate::log::Ingested::Retryable(_) => {}
                        other => {
                            terminal = Some(format!("{other:?}"));
                            break;
                        }
                    }
                    tokio::time::sleep(KEYSTROKE).await;
                }
                (samples, terminal)
            }
        });

        tokio::time::sleep(BASELINE).await;

        let peak = Arc::new(std::sync::atomic::AtomicU64::new(
            resident_bytes().unwrap_or(0),
        ));
        let sampler = tokio::spawn({
            let peak = peak.clone();
            async move {
                let mut ticker = tokio::time::interval(RSS_SAMPLE);
                loop {
                    ticker.tick().await;
                    if let Some(rss) = resident_bytes() {
                        peak.fetch_max(rss, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        });
        let rss_before = resident_bytes();
        let cohort_started = Instant::now();

        let results = join_all(seeded.iter().cloned().map(
            |(document_id, sequencer, _seed_len)| {
                let catalog = catalog.clone();
                let blobs = blobs.clone();
                async move {
                    compact_seeded_document(
                        document_id,
                        sequencer,
                        catalog,
                        blobs,
                        SNAPSHOT_ATTEMPTS,
                        KEYSTROKE,
                    )
                    .await
                }
            },
        ))
        .await;
        let wall_us = micros(cohort_started);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        sampler.abort();
        let (samples, editor_terminal) = editor.await.expect("the unrelated editor");
        let rss_after = resident_bytes();
        let rss_peak = peak.load(std::sync::atomic::Ordering::Relaxed);
        let (baseline, during): (Vec<_>, Vec<_>) = samples
            .into_iter()
            .partition(|(at, _)| *at < cohort_started);
        let baseline: Vec<u64> = baseline.into_iter().map(|(_, took)| took).collect();
        let during: Vec<u64> = during.into_iter().map(|(_, took)| took).collect();
        // A cohort with no baseline samples measured nothing: the editor is
        // the whole point of this run, not decoration on it.
        assert!(
            !baseline.is_empty(),
            "n={n}: the unrelated editor produced no baseline samples in {BASELINE:?} \
             (terminal reason: {editor_terminal:?})",
        );

        let mut activated = 0_usize;
        let mut failed: Vec<Value> = Vec::new();
        let mut stage_samples: std::collections::BTreeMap<&'static str, Vec<u64>> =
            std::collections::BTreeMap::new();
        for stage in [
            "flush",
            "reconstruction",
            "export",
            "verification",
            "compression",
            "upload",
            "write_base_total",
            "activation",
        ] {
            stage_samples.insert(stage, Vec::new());
        }
        for result in &results {
            match result {
                Ok(value) => {
                    activated += 1;
                    let stages = &value["stages_us"];
                    for (stage, samples) in stage_samples.iter_mut() {
                        if let Some(us) = stages[*stage].as_u64() {
                            samples.push(us);
                        }
                    }
                }
                Err(reason) => failed.push(reason.clone()),
            }
        }
        // This is the harness's own promise, restated for a cohort rather
        // than a single document: every N must reach activation or be
        // recorded with the reason it did not. Silence here would be a
        // report full of plausible numbers describing a run that never
        // happened.
        assert_eq!(
            activated + failed.len() + seed_refusals.len(),
            n,
            "n={n}: {} activated, {} failed, {} seed refusals do not add up to n; \
             some document was neither compacted nor accounted for",
            activated,
            failed.len(),
            seed_refusals.len(),
        );

        cohorts.push(json!({
            "n": n,
            "documents_seeded": seeded.len(),
            "seed_refusals": seed_refusals,
            "activated": activated,
            "failed": failed,
            "wall_us": wall_us,
            "stages_us": stage_samples.iter().map(|(stage, samples)| {
                (stage.to_string(), percentiles(samples.clone()))
            }).collect::<serde_json::Map<_, _>>(),
            "memory": {
                "rss_before_bytes": rss_before,
                "rss_peak_bytes": rss_peak,
                "rss_after_bytes": rss_after,
                "peak_over_before_bytes": rss_before.map(|before| rss_peak.saturating_sub(before)),
                "sample_interval_ms": RSS_SAMPLE.as_millis() as u64,
            },
            "unrelated_editor": {
                "terminal_reason": editor_terminal,
                "baseline_latency": percentiles(baseline),
                "during_latency": percentiles(during),
            },
        }));

        // Retired before the next cohort so one N's resident documents do
        // not sit in the budget while the next N is measured.
        for (_, sequencer, _) in &seeded {
            sequencer.drop_cache().await;
        }
        unrelated_sequencer.drop_cache().await;
    }

    let report = json!({
        "profile": "release",
        "measurement": "concurrent_compaction",
        "postgres_version": sqlx::query_scalar::<_, String>("SHOW server_version")
            .fetch_one(catalog.pool()).await.expect("version"),
        "blob_store": format!("filesystem:{}", blob_root.display()),
        "cores_available": cores_available,
        "admission_bound": admission_bound,
        "note": "admission_bound is min(cores_available, 4) per log::admission; \
                 an n above it should show queueing rather than contention",
        "cohort_sizes": cohort_sizes,
        "seed_kib": SEED_KIB,
        "cohort_edits": COHORT_EDITS,
        "cohorts": cohorts,
    });
    let output = serde_json::to_string_pretty(&report).expect("report JSON");
    if let Ok(path) = std::env::var("LIBREPAPER_CONCURRENT_COMPACTION_BENCHMARK_OUTPUT") {
        std::fs::write(path, &output).expect("write benchmark report");
    }
    println!("{output}");
}

/// Where the 3.7 s in `compaction_cost_release_benchmark` actually goes.
///
/// That benchmark says reconstruction of a 1 MiB document costs seconds and
/// hundreds of megabytes while the same document loaded from a snapshot
/// costs milliseconds, and that the edit count barely matters. It cannot say
/// WHY, and the three plausible whys want three different fixes:
///
/// * one large insert op is expensive to replay, whatever carries it;
/// * the `Updates` encoding is expensive to import, however many ops;
/// * `import_batch` over many batches is expensive.
///
/// So each is priced separately here, on the same bytes, with no database
/// and no server: `one_op` is the shape a paste or an upload produces (§5's
/// `encode_state` is `Updates` from an empty vector), `many_ops` is the same
/// text typed in 1 KiB pieces, and `snapshot` is what a compaction base
/// restores from. The sizes double so the exponent is visible rather than
/// inferred: a stage that is linear doubles, and one that is quadratic
/// quadruples.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "diagnostic probe; prints a table and asserts nothing about wall time"]
async fn update_import_cost_probe() {
    fn body(bytes: usize) -> String {
        let line = "line of text: the quick brown fox jumps over the lazy dog\n";
        let mut out = line.repeat(bytes / line.len() + 1);
        out.truncate(bytes);
        out
    }

    let mut rows = Vec::new();
    for kib in [128_usize, 256, 512, 1024] {
        let text = body(kib * 1024);

        // One op: the whole text in a single insert.
        let one = session::new_doc();
        one.set_peer_id(1).expect("peer id");
        session::put_text(&one, "paper.md", &text);
        one.commit();
        let one_updates = session::encode_state(&one);
        let one_snapshot = one.export(loro::ExportMode::Snapshot).expect("snapshot");

        // Many ops: the same text, 1 KiB at a time, in one `Updates` blob.
        let many = session::new_doc();
        many.set_peer_id(2).expect("peer id");
        session::put_text(&many, "paper.md", "");
        many.commit();
        let mut at = 0_usize;
        for chunk in text.as_bytes().chunks(1024) {
            let chunk = std::str::from_utf8(chunk).expect("ascii");
            assert!(session::apply_edits_at(
                &many,
                "paper.md",
                &[wasm_helpers::text::Edit {
                    at,
                    delete: 0,
                    insert: chunk.to_string(),
                }],
            ));
            at += chunk.encode_utf16().count();
            many.commit();
        }
        let many_updates = session::encode_state(&many);

        // Imported into a fresh document each time, which is what a cold
        // build does. Timed on this thread rather than through the
        // sequencer: the question is Loro's cost, not the server's.
        let time = |what: &[u8], batched: bool| -> u64 {
            let doc = session::new_doc();
            let at = Instant::now();
            if batched {
                doc.import_batch(&[what.to_vec()]).expect("import batch");
            } else {
                doc.import(what).expect("import");
            }
            micros(at)
        };

        rows.push(json!({
            "kib": kib,
            "one_op": {
                "update_bytes": one_updates.len(),
                "import_us": time(&one_updates, false),
                // The cold-build path goes through `import_batch`, so both
                // are priced: if they differ, the batch wrapper is the
                // problem rather than the encoding inside it.
                "import_batch_us": time(&one_updates, true),
                "snapshot_bytes": one_snapshot.len(),
                "snapshot_import_us": time(&one_snapshot, false),
            },
            "many_ops": {
                "ops": text.len().div_ceil(1024),
                "update_bytes": many_updates.len(),
                "import_us": time(&many_updates, false),
                "import_batch_us": time(&many_updates, true),
            },
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"probe": "update_import_cost", "sizes": rows}))
            .expect("report JSON")
    );
}

/// The second half of the probe above: what a COLD BUILD does, which is not
/// what a single import does.
///
/// `update_import_cost_probe` prices one blob at a time and finds Loro
/// linear and fast (a 1 MiB `Updates` blob imports in about 1.6 ms). The
/// cold build in `Sequencer::build` never has one blob: it hands
/// `import_batch` the seed and then every batch flushed after it, which is
/// the shape this prices. Instrumenting `build` directly showed 26 batches
/// holding the same 1.05 MB taking 4.56 s, so the cost is in the SHAPE of
/// the call and not in the bytes.
///
/// Each row is the same big document plus `tail` small updates on top,
/// imported the three ways the server could do it: one `import_batch` call
/// (what `build` does today), the same batches imported one at a time, and
/// the big batch alone for reference.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "diagnostic probe; prints a table and asserts nothing about wall time"]
async fn batched_import_cost_probe() {
    let text = {
        let line = "line of text: the quick brown fox jumps over the lazy dog\n";
        let mut out = line.repeat(1024 * 1024 / line.len() + 1);
        out.truncate(1024 * 1024);
        out
    };
    let big = session::new_doc();
    big.set_peer_id(1).expect("peer id");
    session::put_text(&big, "paper.md", &text);
    big.commit();
    let seed = session::encode_state(&big);

    // The tail: separate updates from a second peer, each one keystroke,
    // exactly as the buffer holds them between flushes.
    let typist = session::new_doc();
    typist.set_peer_id(2).expect("peer id");
    typist.import(&seed).expect("the seed");
    let mut vector = session::encode_vector(&typist);
    let mut at = text.encode_utf16().count();
    let mut tail = Vec::new();
    for index in 0..64 {
        let chunk = format!(" t{index}");
        assert!(session::apply_edits_at(
            &typist,
            "paper.md",
            &[wasm_helpers::text::Edit {
                at,
                delete: 0,
                insert: chunk.clone(),
            }],
        ));
        at += chunk.encode_utf16().count();
        typist.commit();
        let update = session::encode_diff(&typist, &vector).expect("a keystroke");
        vector = session::encode_vector(&typist);
        tail.push(update);
    }

    let mut rows = Vec::new();
    for count in [0_usize, 1, 2, 4, 8, 16, 32, 64] {
        let mut batches = vec![seed.clone()];
        batches.extend(tail.iter().take(count).cloned());

        let doc = session::new_doc();
        let at = Instant::now();
        doc.import_batch(&batches).expect("import batch");
        let batched_us = micros(at);

        let doc = session::new_doc();
        let at = Instant::now();
        for batch in &batches {
            doc.import(batch).expect("import");
        }
        let one_at_a_time_us = micros(at);

        rows.push(json!({
            "tail_updates": count,
            "batches": batches.len(),
            "bytes": batches.iter().map(Vec::len).sum::<usize>(),
            "import_batch_us": batched_us,
            "one_at_a_time_us": one_at_a_time_us,
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"probe": "batched_import_cost", "rows": rows}))
            .expect("report JSON")
    );
}

// ---------------------------------------------------------------------------
// Capacity: many active documents at once
// ---------------------------------------------------------------------------
//
// REVIEW-BIG-IDEAS.md §2.1 leaves one thing unverified: "that 20 PostgreSQL
// connections suffice under 64 concurrent HTTP work slots plus background
// work. Measure this under load."
//
// This isolates independent-document editing through the sequencer, periodic
// sweep and pool. It does not answer the mixed HTTP/background-work question:
// HTTP traffic is absent and compaction is disabled. Socket upgrades release
// the HTTP work permit, so sustained edits also need their own measurement.
// Results describe this workload and host, not a deployment capacity limit.

/// What one simulated editor did, so offered load and achieved load can be
/// told apart. An overloaded harness that quietly slows down is the classic
/// way a load test hides saturation.
#[derive(Default)]
struct EditorReport {
    /// Edits the schedule called for by the time the run ended.
    offered: u64,
    /// Scheduled arrivals the bounded producer queue could not deliver.
    queue_dropped: u64,
    /// Edits handed to `ingest`, including resends of the same client_seq.
    attempted: u64,
    accepted: u64,
    retried: u64,
    /// Ingest calls, in microseconds: the relay path only.
    relay: Vec<u64>,
    /// Scheduled arrival to `doc-ack`, including generator and queue delay.
    durable: Vec<u64>,
    /// Accepted edits that never came back acknowledged before the run
    /// ended. Counted rather than dropped: a run whose acknowledgements
    /// stopped arriving must not report a healthy latency for the few that
    /// did.
    unacknowledged: u64,
    /// How far behind its own schedule this editor fell, worst case. A
    /// generator that cannot keep its cadence is measuring itself.
    worst_lateness_us: u64,
    terminal: Option<String>,
}

/// The shared state one editor's acknowledgement reader and its edit loop
/// use to match a `doc-ack {upTo}` back to the submission it covers.
type Pending = Arc<std::sync::Mutex<std::collections::HashMap<i64, Instant>>>;

/// Drains one editor's outbound channel, timing every acknowledged
/// submission. This is the real frame an editor waits for, read off the real
/// `Sender` a socket would hold, rather than a flush return value the
/// server never sends anybody.
fn acknowledgement_reader(
    mut rx: crate::room::outgoing::Receiver,
    pending: Pending,
) -> tokio::task::JoinHandle<Vec<u64>> {
    tokio::spawn(async move {
        let mut durable = Vec::new();
        while let Some(queued) = rx.recv().await {
            let (outgoing, _reservation) = queued.into_parts();
            let text = match &outgoing {
                crate::room::outgoing::Outgoing::Text(text) => text.to_string(),
                crate::room::outgoing::Outgoing::SharedText(text) => text.to_string(),
                _ => continue,
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if value["type"] != "doc-ack" {
                continue;
            }
            let Some(up_to) = value["upTo"].as_i64() else {
                continue;
            };
            let now = Instant::now();
            let mut held = pending.lock().unwrap_or_else(|poison| poison.into_inner());
            let covered: Vec<i64> = held.keys().copied().filter(|seq| *seq <= up_to).collect();
            for seq in covered {
                if let Some(at) = held.remove(&seq) {
                    durable.push(
                        now.duration_since(at)
                            .as_micros()
                            .try_into()
                            .unwrap_or(u64::MAX),
                    );
                }
            }
        }
        durable
    })
}

/// Number of arrivals strictly inside the workload window; independent of
/// how quickly the consumer can submit them. No arrival is scheduled at t=0.
fn scheduled_edits(window: Duration, interval: Duration) -> u64 {
    assert!(!interval.is_zero(), "edit interval must be positive");
    u64::try_from(window.as_nanos().saturating_sub(1) / interval.as_nanos())
        .expect("too many scheduled edits")
}

/// Generate arrival timestamps independently of ingest and retries. The queue
/// is bounded so overload is counted, rather than consuming unlimited memory.
fn capacity_arrivals(
    started_at: Instant,
    deadline: Instant,
    interval: Duration,
    queue_capacity: usize,
) -> (
    tokio::sync::mpsc::Receiver<Instant>,
    tokio::task::JoinHandle<u64>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(queue_capacity);
    let producer = tokio::spawn(async move {
        let mut dropped = 0;
        let mut due = started_at + interval;
        while due < deadline {
            tokio::time::sleep_until(due.into()).await;
            // Check after waking too: runtime starvation is part of overload.
            if Instant::now() >= deadline || tx.try_send(due).is_err() {
                dropped += 1;
            }
            due += interval;
        }
        dropped
    });
    (rx, producer)
}

/// One causally ordered editor consumes independently scheduled arrivals.
/// Retries block this editor, not its producer; queued and dropped demand
/// remains visible, and acknowledgement latency includes the queue delay.
#[allow(clippy::too_many_arguments)]
async fn capacity_editor(
    sequencer: Arc<crate::log::Sequencer>,
    socket: u64,
    peer: u64,
    principal: String,
    opening: Vec<u8>,
    path: String,
    seed_len: usize,
    started_at: Instant,
    deadline: Instant,
    interval: Duration,
) -> EditorReport {
    use crate::room::outgoing::Sender;

    let mut report = EditorReport {
        offered: scheduled_edits(deadline.duration_since(started_at), interval),
        ..EditorReport::default()
    };
    let (mut arrivals, producer) = capacity_arrivals(started_at, deadline, interval, 64);
    let peer_key = format!("peer-{socket}");
    let (tx, rx) = Sender::channel(4096, 64 * 1024 * 1024, None, None);
    let pending: Pending = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let reader = acknowledgement_reader(rx, pending.clone());
    if let Err(error) = sequencer
        .join(socket, crate::log::Role::Editor, &peer_key, tx, None)
        .await
    {
        report.terminal = Some(format!("join failed: {error}"));
        drop(arrivals);
        report.queue_dropped = producer.await.expect("arrival producer");
        reader.await.expect("acknowledgement reader");
        return report;
    }

    let doc = session::new_doc();
    doc.set_peer_id(peer).expect("peer id");
    doc.import(&opening).expect("import the opening state");
    let mut vector = session::encode_vector(&doc);
    let mut len = seed_len;
    let mut sent = 0_i64;
    while let Ok(Some(due)) = tokio::time::timeout_at(deadline.into(), arrivals.recv()).await {
        if Instant::now() >= deadline {
            break;
        }
        report.worst_lateness_us = report.worst_lateness_us.max(micros(due));

        let chunk = format!(" edit{sent}from{peer}");
        if !session::apply_edits_at(
            &doc,
            &path,
            &[wasm_helpers::text::Edit {
                at: len,
                delete: 0,
                insert: chunk.clone(),
            }],
        ) {
            report.terminal = Some("apply_edits_at rejected an append".into());
            break;
        }
        len += chunk.encode_utf16().count();
        doc.commit();
        let Ok(update) = session::encode_diff(&doc, &vector) else {
            report.terminal = Some("encode_diff failed".into());
            break;
        };
        if update.is_empty() {
            continue;
        }
        sent += 1;
        pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(sent, due);
        loop {
            if Instant::now() >= deadline {
                report.terminal = Some("submission still pending at workload deadline".into());
                break;
            }
            let at = Instant::now();
            report.attempted += 1;
            let outcome = tokio::time::timeout_at(
                deadline.into(),
                sequencer.ingest(socket, &peer_key, &principal, sent, update.clone()),
            )
            .await;
            report.relay.push(micros(at));
            let Ok(outcome) = outcome else {
                report.terminal = Some("ingest did not finish before workload deadline".into());
                break;
            };
            match outcome {
                crate::log::Ingested::Accepted => {
                    vector = session::encode_vector(&doc);
                    report.accepted += 1;
                    break;
                }
                crate::log::Ingested::Retryable(reason) => {
                    report.retried += 1;
                    if Instant::now() >= deadline {
                        report.terminal = Some(format!("retryable at deadline: {reason}"));
                        break;
                    }
                    tokio::time::sleep_until(
                        (Instant::now() + Duration::from_millis(50))
                            .min(deadline)
                            .into(),
                    )
                    .await;
                }
                other => {
                    report.terminal = Some(format!("{other:?}"));
                    break;
                }
            }
        }
        if report.terminal.is_some() {
            break;
        }
    }

    // A refused or timed-out submission is not an accepted-but-unacknowledged
    // edit. Sequence numbers advance only once per generated edit.
    if sent as u64 > report.accepted {
        pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&sent);
    }
    drop(arrivals);
    report.queue_dropped = producer.await.expect("arrival producer");
    // Every editor shares the same drain deadline, regardless of its last edit.
    tokio::time::sleep_until((deadline + Duration::from_secs(10)).into()).await;
    sequencer.unsubscribe(socket).await;
    report.durable = reader.await.expect("acknowledgement reader");
    report.unacknowledged = pending
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .len() as u64;
    report
}

fn env_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "capacity benchmark; destroys every row in its configured database"]
async fn active_document_capacity_benchmark() {
    let Ok(url) = std::env::var("LIBREPAPER_BENCHMARK_POSTGRES_URL") else {
        return;
    };
    let documents = env_usize("LIBREPAPER_CAPACITY_DOCUMENTS", 64);
    let hot_editors = env_usize("LIBREPAPER_CAPACITY_HOT_EDITORS", 0);
    let stored = env_usize("LIBREPAPER_CAPACITY_STORED", 0);
    let seconds = env_usize("LIBREPAPER_CAPACITY_SECONDS", 60) as u64;
    let connections = env_usize("LIBREPAPER_CAPACITY_CONNECTIONS", 20) as u32;
    let interval = Duration::from_millis(env_usize("LIBREPAPER_CAPACITY_EDIT_MS", 1000) as u64);
    assert!(seconds > 0 && connections >= 2 && !interval.is_zero());
    assert!(documents + hot_editors > 0);
    let seed_bytes = env_usize("LIBREPAPER_CAPACITY_SEED_BYTES", 4096);
    let label = std::env::var("LIBREPAPER_CAPACITY_LABEL").unwrap_or_else(|_| "run".into());

    let mut options = PostgresOptions::new(url);
    options.max_connections = connections;
    options.policy = StoragePolicy {
        owner_bytes: i64::MAX / 4,
        deployment_bytes: i64::MAX / 4,
        asset_uploads_per_hour: 100_000,
    };
    let catalog = Arc::new(PostgresCatalog::connect(options).await.expect("connect"));
    catalog.migrate().await.expect("migrate");
    sqlx::query("TRUNCATE accounts CASCADE")
        .execute(catalog.pool())
        .await
        .expect("empty disposable benchmark database");
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("benchmark".into()),
            provider_subject: Some("owner".into()),
            handle: "benchmark".into(),
            display_name: "Benchmark".into(),
            email: None,
        })
        .await
        .expect("owner");
    let principal = owner.id.to_string();
    let _writer = catalog.claim_writer().await.expect("writer lease");

    // Stored but inactive documents: rows in the same tables the active
    // documents index into, so index depth and planner choices are those of
    // a deployment that has been running rather than of an empty schema.
    // Inserted in bulk on purpose: these are never edited, so seeding them
    // through the production path would measure nothing and cost minutes.
    if stored > 0 {
        sqlx::query(
            "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,source_format,\
             main_path,settings,status)
             SELECT gen_random_uuid(),'stored-'||n,$1,'owned','Stored','markdown','paper.md',\
             '{\"version\":1}'::jsonb,'active' FROM generate_series(1,$2) AS n",
        )
        .bind(owner.id)
        .bind(stored as i64)
        .execute(catalog.pool())
        .await
        .expect("seed stored documents");
    }

    let scratch = tempfile::tempdir().expect("scratch blob directory");
    let blobs: Arc<dyn crate::storage::blob::BlobStore> = Arc::new(
        crate::storage::blob::FsStore::new(scratch.path().to_path_buf(), false),
    );
    let config = Arc::new(crate::config::Configuration {
        memory_budget_bytes: u64::MAX / 4,
        ..crate::config::Configuration::default()
    });
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "benchmark".to_string(),
    );

    // One opening row per document, written through the production ingest
    // and flush path so every document starts from a state a real one could
    // be in. This is setup, and its cost is reported separately so it cannot
    // be confused with the measurement.
    let seeding = Instant::now();
    let body = "x".repeat(seed_bytes);
    let mut active = Vec::with_capacity(documents + usize::from(hot_editors > 0));
    for index in 0..documents + usize::from(hot_editors > 0) {
        let slug = format!("active-{index}");
        let id = document(&catalog, owner.id, slug.clone()).await;
        let sequencer = registry.get(id, &slug).await.expect("admit");
        let seed = session::new_doc();
        seed.set_peer_id(1).expect("peer id");
        session::put_text(&seed, "paper.md", &body);
        seed.commit();
        let opening = session::encode_state(&seed);
        assert!(matches!(
            sequencer
                .ingest(0, "seed", &principal, 0, opening.clone())
                .await,
            crate::log::Ingested::Accepted
        ));
        sequencer
            .flush(FlushReason::Barrier)
            .await
            .expect("opening row");
        active.push((sequencer, opening));
    }
    let seeding_seconds = seeding.elapsed().as_secs_f64();
    let rows_before: i64 = sqlx::query_scalar("SELECT count(*) FROM document_updates")
        .fetch_one(catalog.pool())
        .await
        .expect("setup row baseline");

    // The production sweep, at the production cadence, timed. Whether this
    // pass keeps up is the measurement: it is the only thing in a deployment
    // that flushes a document nobody has filled a buffer for.
    let sweep_times: Arc<std::sync::Mutex<Vec<u64>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (stop_sweep, mut stopped) = tokio::sync::oneshot::channel::<()>();
    let housekeeping = tokio::spawn({
        let registry = registry.clone();
        let sweep_times = sweep_times.clone();
        async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = ticker.tick() => {}
                }
                let at = Instant::now();
                registry.housekeep().await;
                sweep_times
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(micros(at));
            }
        }
    });

    let multixact_before = control_multixact(&catalog).await;
    let started_at = Instant::now();
    let deadline = started_at + Duration::from_secs(seconds);
    let mut socket = 1_u64;
    let mut editors = Vec::new();
    for (index, (sequencer, opening)) in active.iter().take(documents).enumerate() {
        let handle = tokio::spawn(capacity_editor(
            sequencer.clone(),
            socket,
            (index as u64) + 2,
            principal.clone(),
            opening.clone(),
            "paper.md".to_string(),
            seed_bytes,
            started_at,
            deadline,
            interval,
        ));
        socket += 1;
        editors.push(handle);
    }
    // The shared document, if this run has one: the same cadence, but every
    // editor's batches land in one buffer, one sequencer lock and one row.
    if hot_editors > 0 {
        let (sequencer, opening) = active.last().expect("hot document").clone();
        for peer in 0..hot_editors {
            let handle = tokio::spawn(capacity_editor(
                sequencer.clone(),
                socket,
                (peer as u64) + 10_000,
                principal.clone(),
                opening.clone(),
                "paper.md".to_string(),
                seed_bytes,
                started_at,
                deadline,
                interval,
            ));
            socket += 1;
            editors.push(handle);
        }
    }

    tokio::time::sleep_until(deadline.into()).await;
    let rows_at_deadline: i64 = sqlx::query_scalar("SELECT count(*) FROM document_updates")
        .fetch_one(catalog.pool())
        .await
        .expect("workload row count");
    let workload_snapshot_seconds = started_at.elapsed().as_secs_f64();
    let reports = join_all(editors).await;
    let _ = stop_sweep.send(());
    // Do not cancel a flush between COMMIT and acknowledging its batches.
    housekeeping
        .await
        .expect("housekeeping stopped between passes");
    let pool = catalog.pool_snapshot();
    let elapsed = started_at.elapsed().as_secs_f64();
    let multixact_after = control_multixact(&catalog).await;

    let mut relay = Vec::new();
    let mut durable = Vec::new();
    let (mut offered, mut attempted, mut accepted, mut retried, mut unacknowledged) =
        (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    let mut queue_dropped = 0_u64;
    let mut worst_lateness = 0_u64;
    let mut terminals: Vec<String> = Vec::new();
    for report in reports {
        let report = report.expect("editor task");
        relay.extend(report.relay);
        durable.extend(report.durable);
        offered += report.offered;
        queue_dropped += report.queue_dropped;
        attempted += report.attempted;
        accepted += report.accepted;
        retried += report.retried;
        unacknowledged += report.unacknowledged;
        worst_lateness = worst_lateness.max(report.worst_lateness_us);
        if let Some(reason) = report.terminal {
            terminals.push(reason);
        }
    }
    let durable_samples = durable.len();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM document_updates")
        .fetch_one(catalog.pool())
        .await
        .expect("rows");
    let sweeps = {
        let held = sweep_times
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        held.clone()
    };

    let report = json!({
        "label": label,
        "workload": {
            "active_documents": documents,
            "hot_document_editors": hot_editors,
            "stored_inactive_documents": stored,
            "editor_interval_ms": interval.as_millis() as u64,
            "seed_bytes": seed_bytes,
            "seconds": seconds,
        },
        "timing": {
            "workload_seconds": seconds,
            "workload_row_snapshot_seconds": workload_snapshot_seconds,
            "acknowledgement_drain_seconds": 10,
            "elapsed_through_sweep_stop_seconds": elapsed,
        },
        "configuration": {
            "database_connections": connections,
            "flush_concurrency": catalog.flush_concurrency(),
            "seeding_seconds": (seeding_seconds * 100.0).round() / 100.0,
            "setup_rows_excluded": rows_before,
        },
        "offered": {
            "edits": offered,
            "per_second": (offered as f64 / seconds as f64 * 100.0).round() / 100.0,
            "worst_editor_lateness_us": worst_lateness,
        },
        "achieved": {
            "ingest_attempts": attempted,
            "accepted": accepted,
            "retried": retried,
            "accepted_per_second": (accepted as f64 / seconds as f64 * 100.0).round() / 100.0,
            "not_accepted": offered - accepted,
            "arrival_queue_dropped": queue_dropped,
            "durable_rows_at_workload_snapshot": rows_at_deadline - rows_before,
            "rows_per_second_at_workload_snapshot": (rows_at_deadline - rows_before) as f64 / workload_snapshot_seconds,
            "durable_rows_after_drain": rows - rows_before,
            "rows_per_second_including_drain": (rows - rows_before) as f64 / elapsed,
            "unacknowledged_at_end": unacknowledged,
            "editor_terminals": terminals,
        },
        "latency": {
            "relay_us": percentiles(relay),
            "scheduled_to_durable_ack_us": percentiles(durable),
            "housekeeping_pass_us": percentiles(sweeps),
        },
        "database": pool,
        "multixact_ids_consumed": multixact_after.saturating_sub(multixact_before),
        "host": {
            "resident_bytes": resident_bytes(),
            "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        },
    });
    println!(
        "active_document_capacity_benchmark {}",
        serde_json::to_string_pretty(&report).expect("report")
    );

    // The only thing asserted is that the run measured something. A capacity
    // envelope is a report, not a gate, but a run whose editors all died in
    // the first second must not print a healthy-looking one.
    assert!(
        accepted > 0 && durable_samples > 0,
        "no edit was accepted and acknowledged; the run measured nothing: {report}"
    );
    drop(_writer);
    catalog.close().await;
}

/// How many MultiXactIds the deployment consumed during the run.
///
/// This is the direct test of one specific suspicion: every writer
/// transaction takes `SELECT ... FROM deployment_writer FOR SHARE` on one
/// singleton row, and concurrent share locks on one row are what PostgreSQL
/// allocates MultiXactIds for. If this number tracks the transaction count
/// as concurrency rises, that one row is a deployment-wide cost paid by
/// documents that share nothing.
/// `pg_control_checkpoint()` reports what the last checkpoint wrote, not
/// what the server has reached, so this forces one first. Without that the
/// reading is stale at both ends and the delta is a flat zero, which is what
/// it reported before the checkpoint was added -- a metric that always says
/// "no" is worse than no metric.
async fn control_multixact(catalog: &PostgresCatalog) -> i64 {
    let _ = sqlx::query("CHECKPOINT").execute(catalog.pool()).await;
    sqlx::query_scalar::<_, i64>(
        "SELECT next_multixact_id::text::bigint FROM pg_control_checkpoint()",
    )
    .fetch_one(catalog.pool())
    .await
    .unwrap_or(0)
}

#[test]
fn capacity_demand_counts_arrivals_independently_of_delivery() {
    assert_eq!(
        scheduled_edits(Duration::from_secs(60), Duration::from_secs(8)),
        7
    );
    assert_eq!(
        scheduled_edits(Duration::from_secs(16), Duration::from_secs(8)),
        1
    );
    assert_eq!(
        scheduled_edits(Duration::from_secs(1), Duration::from_secs(8)),
        0
    );
}

#[tokio::test]
async fn capacity_arrivals_keep_scheduling_when_the_consumer_stalls() {
    let start = Instant::now();
    let interval = Duration::from_millis(10);
    let window = Duration::from_millis(60);
    let (mut arrivals, producer) = capacity_arrivals(start, start + window, interval, 1);
    // Never receive until the producer finishes: a blocking send would hang.
    let dropped = tokio::time::timeout(Duration::from_secs(2), producer)
        .await
        .expect("producer must not wait for consumer")
        .unwrap();
    let mut received = 0;
    while let Some(due) = arrivals.recv().await {
        assert!(due < start + window);
        assert!(due.elapsed() >= interval);
        received += 1;
    }
    assert_eq!(received + dropped, scheduled_edits(window, interval));
    assert!(dropped >= 4, "queue overload must be counted");
}
