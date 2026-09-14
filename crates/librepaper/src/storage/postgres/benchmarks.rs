//! Reproducible release-gate measurements. Run only against a disposable DB:
//! `LIBREPAPER_BENCHMARK_POSTGRES_URL=... cargo test --release -p librepaper
//! catalog_v3_release_benchmark -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Instant;

use futures_util::future::join_all;
use serde_json::{json, Value};
use time::OffsetDateTime;

use super::{NewAccount, NewDocument, NewJob, PostgresCatalog, PostgresOptions, StoragePolicy};
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::collaboration::CollaborationStorage;
use crate::storage::publication::{PublicationFile, PublicationStorage, Publish};
use crate::storage::source::{CommitProject, ProjectFile, SourceStorage};
use crate::storage::source_archive::ArchiveLimits;

fn micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn percentiles(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    if values.is_empty() {
        return json!({"samples":0,"p50_us":null,"p95_us":null,"p99_us":null});
    }
    let pick = |percent: usize| values[(values.len() - 1) * percent / 100];
    json!({"samples":values.len(),"p50_us":pick(50),"p95_us":pick(95),"p99_us":pick(99)})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "release acceptance benchmark; destroys every row in its configured database"]
async fn catalog_v3_release_benchmark() {
    let url = std::env::var("LIBREPAPER_BENCHMARK_POSTGRES_URL")
        .expect("set LIBREPAPER_BENCHMARK_POSTGRES_URL to a disposable PostgreSQL database");
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
    sqlx::query("TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,publication_files,publications,document_versions,document_assets,replies,annotations,share_links,grants,documents,accounts CASCADE")
        .execute(catalog.pool()).await.expect("empty benchmark database");
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
    let root = tempfile::tempdir().expect("object directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(root.path(), false));
    let mut documents = Vec::new();
    for index in 0..100 {
        documents.push(
            catalog
                .create_document(NewDocument {
                    slug: format!("room-{index}"),
                    owner_id: owner.id,
                    ownership_mode: "owned".into(),
                    title: format!("Room {index}"),
                    source_format: "markdown".into(),
                    main_path: "paper.md".into(),
                    settings: json!({"version":1}),
                })
                .await
                .expect("room document"),
        );
    }

    let mut room_results = Vec::new();
    for count in [1_usize, 20, 100] {
        let storage = CollaborationStorage::new(catalog.clone(), blobs.clone());
        let started = Instant::now();
        let mut append_latencies = Vec::new();
        for _ in 0..30 {
            append_latencies.extend(
                join_all(documents[..count].iter().map(|document| {
                    let storage = &storage;
                    async move {
                        let started = Instant::now();
                        storage
                            .append(document.id, b"benchmark-update")
                            .await
                            .expect("append");
                        micros(started)
                    }
                }))
                .await,
            );
        }
        let elapsed = micros(started);
        let mut reconstruction = Vec::new();
        for document in &documents[..count] {
            let started = Instant::now();
            let recovered = storage.recover(document.id).await.expect("recover");
            assert!(!recovered.updates.is_empty());
            reconstruction.push(micros(started));
        }
        room_results.push(json!({
            "active_rooms":count,
            "append_batch_us":elapsed,
            "append":percentiles(append_latencies),
            "reconstruction":percentiles(reconstruction)
        }));
    }

    let source = SourceStorage::new(catalog.clone(), blobs.clone(), ArchiveLimits::default())
        .with_retained_asset_limit(i64::MAX / 4);
    let mut source_results = Vec::new();
    for bytes in [100 * 1024, 1024 * 1024, 4 * 1024 * 1024] {
        let started = Instant::now();
        let stored = source
            .commit_project(CommitProject {
                document_id: documents[0].id,
                files: vec![ProjectFile {
                    path: "paper.md".into(),
                    bytes: vec![b'x'; bytes],
                    media_type: "text/markdown".into(),
                }],
                through_update_sequence: 1,
                project_generation: 0,
                reason: "benchmark".into(),
                label: None,
                author_account_id: Some(owner.id),
                author_label: "Benchmark".into(),
                make_current: true,
            })
            .await
            .expect("source version");
        source_results.push(json!({"source_bytes":bytes,"elapsed_us":micros(started),"archive_bytes":stored.version.archive_bytes}));
    }

    let mut asset_results = Vec::new();
    for count in [1_usize, 100, 512] {
        let files = (0..count)
            .map(|index| ProjectFile {
                path: format!("assets/{index}.bin"),
                bytes: vec![0xff, (index % 251) as u8, (index / 251) as u8],
                media_type: "application/octet-stream".into(),
            })
            .chain(std::iter::once(ProjectFile {
                path: "paper.md".into(),
                bytes: b"# Assets".to_vec(),
                media_type: "text/markdown".into(),
            }))
            .collect();
        let started = Instant::now();
        let stored = source
            .commit_project(CommitProject {
                document_id: documents[1].id,
                files,
                through_update_sequence: 1,
                project_generation: 0,
                reason: "asset benchmark".into(),
                label: None,
                author_account_id: Some(owner.id),
                author_label: "Benchmark".into(),
                make_current: true,
            })
            .await
            .expect("asset version");
        asset_results.push(json!({"asset_references":count,"elapsed_us":micros(started),"archive_bytes":stored.version.archive_bytes}));
    }

    let mut job_results = Vec::new();
    for workers in [1_usize, 4, 16] {
        sqlx::query("TRUNCATE jobs")
            .execute(catalog.pool())
            .await
            .expect("clear jobs");
        for sequence in 0..1600 {
            catalog.enqueue_job(NewJob {
                kind: "maintenance".into(), document_id: None, account_id: None,
                scope_key: "benchmark".into(), dedupe_key: Some(format!("{workers}:{sequence}")),
                payload: json!({"base_cleanup_batch":100,"completed_job_retention_days":7,"completed_job_batch":1000,"orphan_scan_batch":500,"orphan_grace_hours":168}),
                priority: 0, max_attempts: 3, run_after: OffsetDateTime::now_utc(),
            }).await.expect("enqueue");
        }
        let started = Instant::now();
        let claims = join_all((0..workers).map(|index| {
            let catalog = catalog.clone();
            async move {
                catalog
                    .claim_jobs(&format!("benchmark-{index}"), 100)
                    .await
                    .expect("claim")
            }
        }))
        .await;
        let claimed: usize = claims.iter().map(Vec::len).sum();
        job_results.push(json!({"workers":workers,"claimed":claimed,"elapsed_us":micros(started)}));
    }

    let publication_storage = PublicationStorage::new(catalog.clone(), blobs.clone());
    let publication_files = (0..4096)
        .map(|index| PublicationFile {
            path: format!("files/{index}.txt"),
            bytes: b"x".to_vec(),
            media_type: "text/plain".into(),
        })
        .collect();
    let publication_started = Instant::now();
    let publication = publication_storage
        .publish(Publish {
            document_id: documents[2].id,
            source_version_id: None,
            request_key: "benchmark-files-limit".into(),
            expected_current_id: None,
            publisher_account_id: Some(owner.id),
            publisher_label: "Benchmark".into(),
            files: publication_files,
        })
        .await
        .expect("publication at file limit");
    let publication_result = json!({
        "files":4096,
        "logical_bytes":4096,
        "elapsed_us":micros(publication_started),
        "stored_files":catalog.publication_files(publication.id).await.expect("publication files").len()
    });
    let publication_byte_started = Instant::now();
    let publication_at_byte_limit = publication_storage
        .publish(Publish {
            document_id: documents[4].id,
            source_version_id: None,
            request_key: "benchmark-byte-limit".into(),
            expected_current_id: None,
            publisher_account_id: Some(owner.id),
            publisher_label: "Benchmark".into(),
            files: vec![PublicationFile {
                path: "maximum.bin".into(),
                bytes: vec![0x5a; 256 * 1024 * 1024],
                media_type: "application/octet-stream".into(),
            }],
        })
        .await
        .expect("publication at byte limit");
    let publication_byte_result = json!({
        "files":1,
        "logical_bytes":256 * 1024 * 1024_u64,
        "elapsed_us":micros(publication_byte_started),
        "stored_files":catalog.publication_files(publication_at_byte_limit.id).await.expect("publication files").len()
    });

    sqlx::query("INSERT INTO annotations(id,document_id,kind,body,author_account_id,author_key,author_label,selector,context)
        SELECT gen_random_uuid(),$1,'comment','benchmark',$2,'benchmark','Benchmark','{}'::jsonb,'{\"version\":1}'::jsonb
        FROM generate_series(1,500)")
        .bind(documents[3].id).bind(owner.id).execute(catalog.pool()).await.expect("timeline fixtures");
    let timeline_started = Instant::now();
    let timeline = catalog
        .annotations(documents[3].id, None, None, 500)
        .await
        .expect("annotation timeline");
    let timeline_us = micros(timeline_started);

    let scale_started = Instant::now();
    sqlx::query("UPDATE documents SET current_version_id=NULL,current_publication_id=NULL")
        .execute(catalog.pool())
        .await
        .expect("clear scale heads");
    for table in [
        "publication_files",
        "publications",
        "document_versions",
        "document_assets",
    ] {
        sqlx::query(&format!("DELETE FROM {table}"))
            .execute(catalog.pool())
            .await
            .expect("clear scale content");
    }
    sqlx::query("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,source_format,main_path)
        SELECT gen_random_uuid(),'scale-'||n,$1,'owned','Scale '||n,'scale '||n,'active','markdown','paper.md'
        FROM generate_series(1,9900) n")
        .bind(owner.id).execute(catalog.pool()).await.expect("scale documents");
    sqlx::query("WITH generated AS (
          SELECT gen_random_uuid() id,d.id document_id,n sequence
          FROM documents d CROSS JOIN generate_series(1,50) n
        ) INSERT INTO document_versions(id,document_id,sequence,through_update_sequence,project_generation,
          archive_key,archive_encoding_version,archive_digest,archive_bytes,logical_bytes,reason,author_label)
        SELECT id,document_id,sequence,0,0,'benchmark/versions/'||id,1,decode(repeat('00',32),'hex'),1024,2048,'scale','Benchmark'
        FROM generated")
        .execute(catalog.pool()).await.expect("scale versions");
    let database_bytes: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(catalog.pool())
        .await
        .expect("database size");
    let listing_started = Instant::now();
    let listing = catalog
        .list_documents(None, 200)
        .await
        .expect("document listing");
    let listing_us = micros(listing_started);
    let report = json!({
        "measured_at":OffsetDateTime::now_utc().to_string(),
        "profile":"release",
        "postgres_version":sqlx::query_scalar::<_,String>("SHOW server_version").fetch_one(catalog.pool()).await.expect("version"),
        "rooms":room_results,
        "source_versions":source_results,
        "asset_versions":asset_results,
        "job_claims":job_results,
        "publications":{"file_limit":publication_result,"byte_limit":publication_byte_result},
        "bounded_reads":{
            "document_listing":{"rows":listing.len(),"elapsed_us":listing_us},
            "annotation_timeline":{"rows":timeline.len(),"elapsed_us":timeline_us}
        },
        "scale":{"documents":10_000,"versions":500_000,"load_us":micros(scale_started),"database_bytes":database_bytes}
    });
    let output = serde_json::to_string_pretty(&report).expect("report json");
    if let Ok(path) = std::env::var("LIBREPAPER_BENCHMARK_OUTPUT") {
        let path = std::path::Path::new(&path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).expect("create report directory");
        }
        std::fs::write(path, &output).expect("write report");
    }
    println!("{output}");
}
