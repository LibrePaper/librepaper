//! End-to-end writer lease benchmark.
//!
//! This deliberately lives outside the crate's unit tests. It starts the
//! production `Server` behind a loopback Axum listener, authenticates real
//! WebSocket clients, and measures the time from a client's `doc-update` send
//! to an observer on the same document receiving the relay. Each active
//! editor has its own document, so N means N independent room sequencers
//! contending only on the deployment-wide writer check. It is a relay
//! benchmark: a flush is not forced, so these timings must not be described
//! as durable-ack latency.
//!
//! Run against a disposable PostgreSQL database with, for example:
//!
//! ```text
//! export LIBREPAPER_BENCH_POSTGRES_URL='postgresql://postgres:.../lp_socket_bench'
//! WRITER_SOCKET_BENCH_DOCUMENTS=1,10,100 \
//!   cargo test -p librepaper --test writer_socket_bench \
//!   --release -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The benchmark never truncates. Every run creates uniquely named account and
//! document rows, so the database can be reused safely for repeated samples.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{future::join_all, SinkExt, StreamExt};
use librepaper::config::Configuration;
use librepaper::log::Registry;
use librepaper::postgres::{NewAccount, NewDocument, PostgresCatalog, PostgresOptions};
use librepaper::worker::Worker;
use librepaper::{
    sign_device, BlobStore, FsStore, GithubApp, Identity, Policy, Rooms, Server, Store,
    PROVIDER_GITHUB,
};
use loro::{ExportMode, LoroDoc, VersionVector};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const PROTOCOL: &str = "librepaper.room.v3";

fn blobs() -> (tempfile::TempDir, Arc<dyn BlobStore>) {
    let directory = tempfile::tempdir().expect("create an unused blob directory");
    let path = directory.path().to_path_buf();
    (directory, Arc::new(FsStore::new(path, false)))
}

async fn connected(url: &str) -> PostgresCatalog {
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .expect("connect to benchmark PostgreSQL");
    // A benchmark database normally migrates itself. The skip is useful when
    // a concurrent source tree has a migration whose SQL is not accepted by
    // the local PostgreSQL version; the caller must then have initialized the
    // disposable database separately from the same migration set.
    if std::env::var("WRITER_SOCKET_BENCH_SKIP_MIGRATE").as_deref() != Ok("1") {
        catalog.migrate().await.expect("run migrations");
    }
    catalog
}

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
        handle
            .insert_utf16(handle.len_utf16(), text)
            .expect("append benchmark text");
        self.doc.commit();
        let update = self
            .doc
            .export(ExportMode::Updates {
                from: std::borrow::Cow::Borrowed(&self.at),
            })
            .expect("export a benchmark update");
        self.at = self.doc.oplog_vv();
        update
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after 1970")
        .as_secs() as i64
}

fn bearer_token(key: &[u8], account_id: Uuid, handle: &str, generation: i64) -> String {
    sign_device(
        key,
        &Identity {
            provider: PROVIDER_GITHUB.into(),
            id: account_id.to_string(),
            handle: handle.into(),
            name: handle.into(),
            picture: String::new(),
            session_generation: generation.to_string(),
        },
        now_unix() + 3600,
    )
}

async fn wait_for_type(ws: &mut WsStream, wanted: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for {wanted:?}");
        let frame = tokio::time::timeout(remaining, ws.next())
            .await
            .expect("timed out waiting for a websocket frame")
            .expect("the websocket ended while waiting for a frame")
            .expect("websocket read failed");
        let WsMessage::Text(text) = frame else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).expect("websocket frame is JSON");
        if value["type"] == wanted {
            return value;
        }
    }
}

async fn open_socket(base: &str, slug: &str, token: &str) -> WsStream {
    let mut request = format!("ws://{base}/ws/{slug}")
        .into_client_request()
        .expect("build websocket request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("authorization header"),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect to the live server");
    wait_for_type(&mut ws, "hello").await;
    ws.send(WsMessage::Text(
        json!({"type": "doc-open", "vector": "", "protocol": PROTOCOL})
            .to_string()
            .into(),
    ))
    .await
    .expect("send doc-open");
    wait_for_type(&mut ws, "doc-state").await;
    ws
}

async fn select_one_calls(catalog: &PostgresCatalog) -> Option<i64> {
    let installed = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='pg_stat_statements')",
    )
    .fetch_one(catalog.pool())
    .await
    .ok()?;
    if !installed {
        return None;
    }
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(calls),0)::bigint FROM pg_stat_statements pss \
         JOIN pg_database d ON d.oid=pss.dbid \
         WHERE d.datname=current_database() AND pss.query ~* '^\\s*SELECT\\s+\\$?1\\s*;?\\s*$'",
    )
    .fetch_one(catalog.pool())
    .await
    .ok()
}

#[derive(Debug, serde::Serialize)]
struct ResultRow {
    documents: usize,
    rounds: usize,
    updates: usize,
    elapsed_ms: f64,
    updates_per_second: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    scheduled_p95_ms: f64,
    scheduled_p99_ms: f64,
    scheduled_max_ms: f64,
    select_one_delta: Option<i64>,
}

struct DocumentPeer {
    editor: WsStream,
    observer: WsStream,
    outbox: Outbox,
}

async fn drain_close(mut ws: WsStream) {
    let _ = ws.close(None).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => break,
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn scenario(
    catalog: &PostgresCatalog,
    base: &str,
    key: &[u8],
    owner: &librepaper::postgres::AccountRecord,
    documents: usize,
    rounds: usize,
    rate: usize,
) -> ResultRow {
    let token = bearer_token(key, owner.id, &owner.handle, owner.session_generation);
    let mut slugs = Vec::with_capacity(documents);
    for _ in 0..documents {
        let slug = format!(
            "writer-socket-bench-{}-{}",
            documents,
            Uuid::now_v7().simple()
        );
        catalog
            .create_document(NewDocument {
                slug: slug.clone(),
                owner_id: owner.id,
                ownership_mode: "owned".into(),
                title: format!("Writer socket benchmark ({documents})"),
                source_format: "markdown".into(),
                main_path: "paper.md".into(),
                settings: json!({"version": 1}),
            })
            .await
            .expect("create benchmark document");
        slugs.push(slug);
    }

    // Each active editor has one independent document and one observer on that
    // document. This keeps room-level serialization and quadratic fanout out
    // of the deployment-wide writer-lease measurement.
    let mut peers = futures_util::stream::iter(slugs.into_iter().map(|slug| {
        let token = token.clone();
        async move {
            let (observer, editor) = tokio::join!(
                open_socket(base, &slug, &token),
                open_socket(base, &slug, &token),
            );
            DocumentPeer {
                editor,
                observer,
                outbox: Outbox::new(),
            }
        }
    }))
    .buffer_unordered(16)
    .collect::<Vec<_>>()
    .await;
    let before = select_one_calls(catalog).await;

    use base64::Engine;
    let mut latencies = Vec::with_capacity(documents * rounds);
    let mut scheduled_latencies = Vec::with_capacity(documents * rounds);
    let interval = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
    let wall_start = Instant::now();
    for round in 0..rounds {
        let updates: Vec<String> = peers
            .iter_mut()
            .enumerate()
            .map(|(index, peer)| {
                Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    peer.outbox.edit(&format!("editor-{index}-round-{round}")),
                )
            })
            .collect();
        let sent: Vec<(String, Instant)> = join_all(peers.iter_mut().zip(updates.iter()).map(
            |(peer, update)| async move {
                let start = Instant::now();
                peer.editor
                    .send(WsMessage::Text(
                        json!({"type": "doc-update", "update": update, "seq": round + 1})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .expect("send concurrent doc-update");
                (update.clone(), start)
            },
        ))
        .await;
        let starts: HashMap<String, Instant> = sent.into_iter().collect();
        let planned_start = wall_start + interval.mul_f64(round as f64);
        let received: Vec<(f64, f64)> = join_all(peers.iter_mut().zip(updates.iter()).map(
            |(peer, update)| async {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                loop {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    assert!(!remaining.is_zero(), "timed out waiting for relay");
                    let frame = tokio::time::timeout(remaining, peer.observer.next())
                        .await
                        .expect("timed out reading relay")
                        .expect("observer closed before relay")
                        .expect("observer read failed");
                    let WsMessage::Text(text) = frame else {
                        continue;
                    };
                    let value: Value = serde_json::from_str(&text).expect("relay is JSON");
                    if value["type"] != "doc-update" || value["update"] != *update {
                        continue;
                    }
                    let now = Instant::now();
                    return (
                        starts[update].elapsed().as_secs_f64() * 1000.0,
                        now.saturating_duration_since(planned_start).as_secs_f64() * 1000.0,
                    );
                }
            },
        ))
        .await;
        for (latency, scheduled_latency) in received {
            latencies.push(latency);
            scheduled_latencies.push(scheduled_latency);
        }
        let planned = wall_start + interval.mul_f64((round + 1) as f64);
        tokio::time::sleep_until(planned.into()).await;
    }
    let after = select_one_calls(catalog).await;
    let elapsed_ms = wall_start.elapsed().as_secs_f64() * 1000.0;
    latencies.sort_by(f64::total_cmp);
    scheduled_latencies.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        let index = ((latencies.len() - 1) as f64 * fraction).round() as usize;
        latencies[index]
    };
    let draining = peers.drain(..).map(|peer| async move {
        tokio::join!(drain_close(peer.editor), drain_close(peer.observer));
    });
    join_all(draining).await;
    ResultRow {
        documents,
        rounds,
        updates: latencies.len(),
        elapsed_ms,
        updates_per_second: latencies.len() as f64 / (elapsed_ms / 1000.0),
        p50_ms: percentile(0.50),
        p95_ms: percentile(0.95),
        p99_ms: percentile(0.99),
        max_ms: *latencies.last().expect("at least one relay"),
        scheduled_p95_ms: {
            let index = ((scheduled_latencies.len() - 1) as f64 * 0.95).round() as usize;
            scheduled_latencies[index]
        },
        scheduled_max_ms: *scheduled_latencies
            .last()
            .expect("at least one scheduled relay"),
        scheduled_p99_ms: {
            let index = ((scheduled_latencies.len() - 1) as f64 * 0.99).round() as usize;
            scheduled_latencies[index]
        },
        select_one_delta: before.zip(after).map(|(start, end)| end - start),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "requires LIBREPAPER_BENCH_POSTGRES_URL and a disposable database"]
async fn writer_socket_end_to_end_benchmark() {
    let url = std::env::var("LIBREPAPER_BENCH_POSTGRES_URL")
        .expect("set LIBREPAPER_BENCH_POSTGRES_URL to a disposable database");
    let documents: Vec<usize> = std::env::var("WRITER_SOCKET_BENCH_DOCUMENTS")
        .unwrap_or_else(|_| "1,10,100".into())
        .split(',')
        .map(|value| value.trim().parse().expect("document counts are integers"))
        .collect();
    assert!(!documents.is_empty() && documents.iter().all(|count| *count > 0));
    let rounds_per_second = std::env::var("WRITER_SOCKET_BENCH_RATE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(5)
        .max(1);
    let duration_seconds = std::env::var("WRITER_SOCKET_BENCH_SECONDS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(5)
        .max(1);
    let repetitions = std::env::var("WRITER_SOCKET_BENCH_REPS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2)
        .max(1);
    let rounds = rounds_per_second * duration_seconds;

    let catalog = Arc::new(connected(&url).await);
    let (_blob_directory, blobs) = blobs();
    let mut config = Configuration::default();
    // One shared bearer is intentional: this isolates the deployment-wide
    // writer mutex from account lookup differences. Raise only socket caps so
    // the requested 100-document scenario is admitted concurrently.
    config.sockets.deployment_max = 2048;
    config.sockets.network_max = 2048;
    config.sockets.principal_max = 2048;
    config.sockets.document_max = 2048;
    config.sockets.document_editors_max = 2048;
    let config = Arc::new(config);
    let writer = catalog
        .claim_writer()
        .await
        .expect("claim benchmark writer");
    let tag = Uuid::now_v7().simple().to_string();
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(format!("writer-socket-bench-{tag}")),
            handle: format!("writerbench-{tag}"),
            display_name: "Writer socket benchmark".into(),
            email: None,
        })
        .await
        .expect("create benchmark account");
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        format!("writer-socket-bench-{tag}"),
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
    let store = Store::open_with_catalog(blobs, config.clone(), catalog.clone(), registry)
        .await
        .expect("open benchmark store");
    let mut server = Server::new(
        store,
        rooms,
        background,
        HashMap::new(),
        GithubApp {
            client_id: "test-client".into(),
            ..GithubApp::default()
        },
        vec![7_u8; 32],
        config,
        Policy::parse_publishers(&owner.handle).expect("benchmark publisher policy"),
        Policy::parse(""),
    );
    let key = vec![7_u8; 32];
    server.install_writer(writer);
    let server = Arc::new(server);
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind benchmark listener");
    let addr = listener.local_addr().expect("read benchmark address");
    let service = server
        .clone()
        .router()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, service).await;
    });
    let base = addr.to_string();

    println!(
        "writer_socket_bench version={} base=ws://{base}",
        librepaper::VERSION
    );
    for repetition in 0..repetitions {
        for count in &documents {
            let row = scenario(
                &catalog,
                &base,
                &key,
                &owner,
                *count,
                rounds,
                rounds_per_second,
            )
            .await;
            let mut result = serde_json::to_value(&row).expect("serialize measured row");
            result["rep"] = json!(repetition + 1);
            println!("{result}");
        }
    }
}
