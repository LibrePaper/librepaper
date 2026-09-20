//! Real Loro commit release gate. Run only against a disposable database.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde_json::{json, Value};

use super::{
    Authority, NewAccount, NewDocument, PostgresCatalog, PostgresOptions, PreparedSource,
    StoragePolicy,
};
use crate::document::session;

fn micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn percentiles(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    let pick = |percent: usize| values[(values.len() - 1) * percent / 100];
    json!({"samples":values.len(),"p50_us":pick(50),"p95_us":pick(95),
        "p99_us":pick(99),"max_us":values[values.len()-1]})
}

async fn document(catalog: &PostgresCatalog, owner: uuid::Uuid, slug: String) -> uuid::Uuid {
    catalog
        .create_document(NewDocument {
            slug,
            owner_id: owner,
            ownership_mode: "owned".into(),
            title: "Commit benchmark".into(),
            source_format: "markdown".into(),
            main_path: "paper.md".into(),
            settings: json!({"version":1}),
        })
        .await
        .expect("benchmark document")
        .id
}

/// Uses valid Loro deltas and the production fenced transaction. The default
/// is the required ten minutes per size; a shorter duration is only a harness
/// smoke check and is recorded in the output.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "release acceptance benchmark; destroys every row in its configured database"]
async fn document_commit_release_benchmark() {
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
        versions_per_hour: 100_000,
        max_uncompacted_updates: 1_000_000,
        max_uncompacted_bytes: i64::MAX / 4,
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

    let mut cases = Vec::new();
    for bytes in [100 * 1024_usize, 1024 * 1024, 4 * 1024 * 1024] {
        let document_id = document(&catalog, owner.id, format!("size-{bytes}")).await;
        let doc = session::new_doc();
        let mut vector = session::encode_vector(&doc);
        let mut text = "x".repeat(bytes.saturating_sub(1));
        text.push('\n');
        session::replace_text(&doc, &text, "paper.md");
        let mut sequence = 0_i64;
        let mut prepare = Vec::new();
        let mut commit = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < deadline || commit.is_empty() {
            let started = Instant::now();
            text.push('x');
            session::replace_text(&doc, &text, "paper.md");
            let update = session::encode_diff(&doc, &vector).expect("valid Loro delta");
            vector = session::encode_vector(&doc);
            let frontier = doc.state_frontiers().encode();
            let encoded = session::encode_state(&doc).len() as i64;
            librepaper_document_core::validate(
                &doc,
                librepaper_document_core::Limits {
                    source_bytes: 8 * 1024 * 1024,
                    encoded_history_bytes: 64 * 1024 * 1024,
                    files: 10_000,
                },
            )
            .expect("candidate validates");
            prepare.push(micros(started));
            let started = Instant::now();
            let result = catalog
                .commit_source(
                    document_id,
                    &authority,
                    PreparedSource {
                        expected_update_sequence: sequence,
                        update: &update,
                        frontier: &frontier,
                        encoded_history_bytes: encoded,
                        actor_key: &authority.principal_key,
                        source_format: None,
                        main_path: None,
                        resolve_annotation_id: None,
                        supersede_proposals: false,
                    },
                    None,
                )
                .await
                .expect("fenced source commit");
            sequence = result.update_sequence.parse().expect("decimal sequence");
            commit.push(micros(started));
        }
        cases.push(json!({"initial_source_bytes":bytes,"prepare":percentiles(prepare),
            "commit":percentiles(commit),"final_encoded_history_bytes":session::encode_state(&doc).len()}));
    }

    let simultaneous = join_all((0..100).map(|index| {
        let catalog = catalog.clone();
        let authority = authority.clone();
        async move {
            let id = document(&catalog, owner.id, format!("active-{index}")).await;
            let doc = session::new_doc();
            session::replace_text(&doc, &"y".repeat(100 * 1024), "paper.md");
            let update = session::encode_state(&doc);
            let frontier = doc.state_frontiers().encode();
            let started = Instant::now();
            catalog
                .commit_source(
                    id,
                    &authority,
                    PreparedSource {
                        expected_update_sequence: 0,
                        update: &update,
                        frontier: &frontier,
                        encoded_history_bytes: update.len() as i64,
                        actor_key: &authority.principal_key,
                        source_format: None,
                        main_path: None,
                        resolve_annotation_id: None,
                        supersede_proposals: false,
                    },
                    None,
                )
                .await
                .expect("simultaneous commit");
            micros(started)
        }
    }))
    .await;
    let wal_bytes: i64 =
        sqlx::query_scalar("SELECT pg_wal_lsn_diff(pg_current_wal_lsn(),$1::pg_lsn)::bigint")
            .bind(&wal_before)
            .fetch_one(catalog.pool())
            .await
            .expect("WAL difference");
    let report = json!({"profile":"release","duration_seconds_per_size":seconds,
        "postgres_version":sqlx::query_scalar::<_, String>("SHOW server_version")
            .fetch_one(catalog.pool()).await.expect("version"),
        "real_loro_commit_cases":cases,"one_hundred_active_documents":percentiles(simultaneous),
        "wal_bytes":wal_bytes});
    let output = serde_json::to_string_pretty(&report).expect("report JSON");
    if let Ok(path) = std::env::var("LIBREPAPER_BENCHMARK_OUTPUT") {
        std::fs::write(path, &output).expect("write benchmark report");
    }
    println!("{output}");
}
