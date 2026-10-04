//! Mixed room, HTTP, reconnect, and authorization-cost benchmark.
//!
//! Starts the production server against a disposable PostgreSQL database.
//! For each requested cohort (default 100 and 1,000), it creates one
//! document per socket, connects one owner/editor per document, and emits
//! one JSON object per phase. Database setup/migration happens before the
//! phase snapshots; the benchmark never truncates existing rows. It uses
//! uniquely named accounts and documents, so repeated runs leave additional
//! disposable data behind.
//!
//! # Prepare PostgreSQL separately
//!
//! Use a database you can discard. `pg_stat_statements` must be preloaded by
//! the PostgreSQL server and enabled in that database. For a local
//! PostgreSQL 17 instance, configure the server before starting/restarting it:
//!
//! ```conf
//! shared_preload_libraries = 'pg_stat_statements'
//! compute_query_id = on
//! ```
//!
//! Then prepare the database and extension before running the measurement:
//!
//! ```sql
//! CREATE DATABASE librepaper_frugal_bench;
//! \connect librepaper_frugal_bench
//! CREATE EXTENSION pg_stat_statements;
//! ```
//!
//! If the test role cannot read `pg_stat_statements`, grant it the needed
//! monitoring role (for example `pg_read_all_stats`) or run with a role that
//! can. The benchmark does not reset global statistics. It snapshots query
//! counts and cumulative execution time around each workload phase and reports
//! deltas, so other concurrent traffic in the same database contaminates the
//! sample. Keep this database quiet during a run. Missing
//! extension/permissions produce empty statement deltas; they do not
//! invalidate the socket and HTTP measurements.
//!
//! Run the focused release benchmark:
//!
//! ```sh
//! export LIBREPAPER_FRUGAL_DATABASE_URL='postgresql://postgres:password@127.0.0.1:55439/librepaper_frugal_bench'
//! FRUGAL_MIXED_SOCKETS=100,1000 \
//! FRUGAL_MIXED_SECONDS=60 \
//! cargo test -p librepaper --test frugal_mixed_bench --release \
//!   -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Migrations run as part of setup unless `FRUGAL_MIXED_SKIP_MIGRATE=1` is
//! set; set that only if the disposable database was migrated separately from
//! this checkout. Socket counts can be run one at a time, for example
//! `FRUGAL_MIXED_SOCKETS=1000`. Keep `FRUGAL_MIXED_SECONDS` at 50 or more for
//! the ping phase to cross multiple browser heartbeat intervals; its default
//! is 60. The mixed editing phase uses the same duration in one-second rounds,
//! then waits (at most 60 seconds) for retained pending bytes to drain.
//!
//! `FRUGAL_MIXED_DB_STALL_SECONDS=45` holds an `EXCLUSIVE` lock on the update
//! table during the mixed phase. The control connection is separate from the
//! application pool. Reads and writer probes remain available while up to nine
//! housekeeping flushes wait for the lock. Its timer releases independently of
//! the workload. HTTP reads include the browser client header and are
//! preflighted before timing. Reconnect accepts either initial state or
//! persisted rows, as required by the room protocol. Edit and HTTP failures
//! fail the benchmark; reconnect admission failures are counted as an observed
//! overload outcome.
//!
//! # Admission and workload details
//!
//! The benchmark deliberately overrides otherwise limiting admission values:
//!
//! | Setting | Default | Benchmark |
//! | --- | ---: | ---: |
//! | Deployment sockets | 4,096 | 4,096 |
//! | Network bucket sockets | 128 | 4,096 |
//! | Principal sockets | 64 | 4,096 |
//! | Per-document sockets | 256 | 4,096 |
//! | Per-document editors | 32 | 4,096 |
//! | Requests per principal per minute | 6,000 | 1,000,000 |
//! | HTTP work concurrency | 64 | 64 (default) |
//!
//! The socket ceilings permit the 1,000-socket cohort from one loopback peer
//! and one shared test principal. The elevated request allowance avoids the
//! benchmark account's request meter ending the HTTP phase early. The
//! production HTTP work concurrency limit remains in effect. Per phase, JSON
//! includes DB pool occupancy and cumulative transaction-begin wait telemetry.
//! Those snapshots do not measure every single-statement read wait.
//!
//! The `ping_authorization` phase sends the application ping once every 25
//! seconds to every established socket and waits for `pong`. This matches the
//! browser heartbeat interval, though all benchmark clients are synchronized,
//! so it creates a burst instead of the smoother average expected from clients
//! with different connection times. Authentication defaults to a signed browser
//! session cookie. Set `FRUGAL_MIXED_AUTH=bearer` for the device-token path.
//! The selected mode is recorded in the configuration event. The first idle
//! ping is sent at 25 seconds, so connection setup cannot supply its cached
//! authorization.
//!
//! The `frequent_authorization` phase sends application pings at five per
//! second for ten seconds. This isolates repeated authorization and its
//! one-second cache from editing, writer verification, and HTTP query costs.
//! SQL execution time and ping round-trip latency are reported separately.
//!
//! The `mixed_edit_http_flush` phase sends one small Loro text update per
//! socket per second, and issues an authenticated project HTTP GET for each
//! document per round. HTTP requests have a concurrency bound of 32. It
//! records edit-plus-ping round-trip latency (which drains incoming error/
//! durability frames), full HTTP response-body latency and failures, then
//! waits for pending writes to drain. The edit round-trip is not a durable-
//! save acknowledgement measurement. Rounds are closed-loop: waiting for
//! responses adds to the one-second interval.
//!
//! The `mass_reconnect` phase closes the cohort and reconnects it in batches
//! of 128. It records end-to-end connection plus room-open latency and counts
//! failed handshakes (including server rejection responses) without retrying.
//! This is a single-process restart approximation: the server remains live,
//! and it does not measure process startup, load-balancer behavior, or client
//! retry backoff.
//!
//! # Reading results
//!
//! Each output line is JSON. `pg_delta` contains per-normalized-statement call
//! and cumulative execution-time deltas along with totals. The
//! `authorization_candidate_*` subset matches statements mentioning
//! `documents`, `share_links`, `grants`, or `accounts` in SELECT queries; it
//! is a query-text classification, not a guaranteed one-to-one authorization
//! attribution. `writer_check_*` counts normalized `SELECT $1` statements as a
//! proxy for the writer liveness check. Compare these only with other traffic
//! held fixed.
//!
//! `pg_stat_statements.total_exec_time` measures time spent executing SQL. It
//! does not include application authorization-cache mutex waits, the writer
//! mutex wait, pool acquisition, or network wait, and it must not be presented
//! as those end-to-end costs. There is no claim that this harness can separate
//! those waits. PostgreSQL statement timings aggregate calls across the phase
//! rather than providing per-request latency.
//!
//! The output is an instrumented local workload, not a capacity claim. Host
//! contention, PostgreSQL configuration, driver pool settings, request sizes,
//! client timing and cache state all affect results. No authorization
//! architecture change should follow from this benchmark alone; use measured
//! cost and repeat under the target deployment conditions first.
//!
//! A project GET is preflighted before any timed phase. The state-transfer
//! endpoint requires a transfer token and is deliberately not used as an
//! ordinary read. HTTP failures retain their status or error category in the
//! output.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::{future::join_all, SinkExt, StreamExt};
use librepaper::{
    config::Configuration,
    log::Registry,
    postgres::{NewAccount, NewDocument, PostgresCatalog, PostgresOptions},
    sign_device,
    worker::Worker,
    BlobStore, FsStore, GithubApp, Identity, Policy, Rooms, Server, Store, PROVIDER_GITHUB,
};
use loro::{ExportMode, LoroDoc, VersionVector};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as WsMessage};
use uuid::Uuid;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
const PROTOCOL: &str = "librepaper.room.v3";

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn token(key: &[u8], account: &librepaper::postgres::AccountRecord) -> String {
    let device = sign_device(
        key,
        &Identity {
            provider: PROVIDER_GITHUB.into(),
            id: account.id.to_string(),
            handle: account.handle.clone(),
            name: account.handle.clone(),
            picture: String::new(),
            session_generation: account.session_generation.to_string(),
        },
        now_unix() + 7200,
    );
    if std::env::var("FRUGAL_MIXED_AUTH").as_deref() == Ok("bearer") {
        return format!("Bearer {device}");
    }
    // Use the production identity envelope, signed with the fixture key in
    // the session domain. This exercises cookie authentication without
    // exporting another internal auth API solely for this benchmark.
    use base64::Engine;
    use hmac::Mac;
    let payload = device
        .split_once("v2.")
        .expect("v2 fixture token")
        .1
        .split('.')
        .next()
        .unwrap();
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("fixture key");
    mac.update(b"session-v2\0");
    mac.update(payload.as_bytes());
    let signature =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("librepaper_session=v2.{payload}.{signature}")
}
fn auth_header(credential: &str) -> &'static str {
    if credential.starts_with("Bearer ") {
        "authorization"
    } else {
        "cookie"
    }
}
async fn wait_type(ws: &mut Ws, wanted: &str) -> Result<Value, String> {
    let until = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let msg = tokio::time::timeout_at(until, ws.next())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("socket closed")?
            .map_err(|e| e.to_string())?;
        if let WsMessage::Text(text) = msg {
            let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if value["type"] == "error" {
                return Err(value.to_string());
            }
            // Persisted history can start with rows when there is no snapshot.
            if value["type"] == wanted || (wanted == "doc-state" && value["type"] == "doc-rows") {
                return Ok(value);
            }
        }
    }
}
async fn open(base: &str, slug: &str, bearer: &str) -> Result<Ws, String> {
    let mut request = format!("ws://{base}/ws/{slug}")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        auth_header(bearer),
        bearer
            .parse()
            .map_err(|e: axum::http::header::InvalidHeaderValue| e.to_string())?,
    );
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(20),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    wait_type(&mut ws, "hello").await?;
    ws.send(WsMessage::Text(
        json!({"type":"doc-open","vector":"","protocol":PROTOCOL})
            .to_string()
            .into(),
    ))
    .await
    .map_err(|e| e.to_string())?;
    wait_type(&mut ws, "doc-state").await?;
    Ok(ws)
}
async fn pg_stats(db: &PostgresCatalog) -> HashMap<String, (i64, f64)> {
    let available = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='pg_stat_statements')",
    )
    .fetch_one(db.pool())
    .await
    .unwrap_or(false);
    if !available {
        return HashMap::new();
    }
    sqlx::query_as::<_, (String, i64, f64)>("SELECT query, calls::bigint, total_exec_time::float8 FROM pg_stat_statements p JOIN pg_database d ON d.oid=p.dbid WHERE d.datname=current_database()")
        .fetch_all(db.pool()).await.unwrap_or_default().into_iter().map(|(q,c,t)|(q,(c,t))).collect()
}
fn stats_delta(before: &HashMap<String, (i64, f64)>, after: &HashMap<String, (i64, f64)>) -> Value {
    let mut rows = Vec::new();
    for (q, (calls, time)) in after {
        let (bc, bt) = before.get(q).copied().unwrap_or_default();
        let dc = calls - bc;
        let dt = time - bt;
        if dc > 0 {
            rows.push(json!({"query":q,"calls":dc,"total_exec_ms":dt,"mean_exec_ms":dt/dc as f64}));
        }
    }
    rows.sort_by(|a, b| a["query"].as_str().cmp(&b["query"].as_str()));
    let total_calls: i64 = rows.iter().map(|r| r["calls"].as_i64().unwrap_or(0)).sum();
    let total_ms: f64 = rows
        .iter()
        .map(|r| r["total_exec_ms"].as_f64().unwrap_or(0.0))
        .sum();
    let auth: Vec<_> = rows
        .iter()
        .filter(|r| {
            let q = r["query"].as_str().unwrap_or("").to_lowercase();
            q.starts_with("select")
                && ["documents", "share_links", "grants", "accounts"]
                    .iter()
                    .any(|x| q.contains(x))
        })
        .collect();
    let auth_calls: i64 = auth.iter().map(|r| r["calls"].as_i64().unwrap_or(0)).sum();
    let auth_ms: f64 = auth
        .iter()
        .map(|r| r["total_exec_ms"].as_f64().unwrap_or(0.0))
        .sum();
    let writer: Vec<_> = rows
        .iter()
        .filter(|r| {
            r["query"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_lowercase()
                .trim_end_matches(';')
                .trim()
                .eq("select $1")
        })
        .collect();
    let writer_calls: i64 = writer
        .iter()
        .map(|r| r["calls"].as_i64().unwrap_or(0))
        .sum();
    let writer_ms: f64 = writer
        .iter()
        .map(|r| r["total_exec_ms"].as_f64().unwrap_or(0.0))
        .sum();
    json!({"all_calls":total_calls,"all_exec_ms":total_ms,"authorization_candidate_calls":auth_calls,"authorization_candidate_exec_ms":auth_ms,"writer_check_calls":writer_calls,"writer_check_exec_ms":writer_ms,"statements":rows})
}
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}
fn pool_telemetry(db: &PostgresCatalog) -> Value {
    db.pool_snapshot()
}

async fn read_project(
    client: &reqwest::Client,
    url: String,
    credential: &str,
) -> Result<f64, String> {
    let started = Instant::now();
    let response = client
        .get(url)
        .header(auth_header(credential), credential)
        .header("x-librepaper-client", "frugal-benchmark")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let project: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if project["schema"] != 1 {
        return Err("expected a project response".into());
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "requires LIBREPAPER_FRUGAL_DATABASE_URL pointing to disposable PostgreSQL"]
async fn frugal_mixed_workload_benchmark() {
    let url = std::env::var("LIBREPAPER_FRUGAL_DATABASE_URL").expect("set disposable database URL");
    let counts: Vec<usize> = std::env::var("FRUGAL_MIXED_SOCKETS")
        .unwrap_or_else(|_| "100,1000".into())
        .split(',')
        .map(|s| s.trim().parse().expect("socket counts are integers"))
        .collect();
    let seconds = std::env::var("FRUGAL_MIXED_SECONDS")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(60usize)
        .max(1);
    let db = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(&url))
            .await
            .expect("connect DB"),
    );
    if std::env::var("FRUGAL_MIXED_SKIP_MIGRATE").as_deref() != Ok("1") {
        db.migrate().await.expect("migrate disposable DB");
    }
    let tmp = tempfile::tempdir().expect("temp blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(tmp.path().to_path_buf(), false));
    let mut config = Configuration::default();
    config.sockets.deployment_max = 4096;
    config.sockets.network_max = 4096;
    config.sockets.principal_max = 4096;
    config.sockets.document_max = 4096;
    config.sockets.document_editors_max = 4096;
    config.cost.requests_per_principal_minute = 1_000_000;
    let config = Arc::new(config);
    let writer = db.claim_writer().await.expect("claim writer");
    let tag = Uuid::now_v7().simple().to_string();
    let owner = db
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(format!("frugal-mixed-{tag}")),
            handle: format!("frugal-{tag}"),
            display_name: "Frugal benchmark".into(),
            email: None,
        })
        .await
        .expect("create account");
    let registry = Registry::new(
        db.clone(),
        blobs.clone(),
        config.clone(),
        format!("frugal-mixed-{tag}"),
    );
    let housekeeper = registry.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if !housekeeper.all().await.is_empty() {
                housekeeper.housekeep().await;
            }
        }
    });
    let rooms = Rooms::new(db.clone(), blobs.clone(), config.clone(), registry.clone());
    let (worker, background) =
        Worker::new(db.clone(), blobs.clone(), registry.clone(), config.clone());
    tokio::spawn(worker.run());
    let store = Store::open_with_catalog(blobs, config.clone(), db.clone(), registry);
    let mut server = Server::new(
        store,
        rooms,
        background,
        HashMap::new(),
        GithubApp {
            client_id: "test".into(),
            ..GithubApp::default()
        },
        vec![9; 32],
        config.clone(),
        Policy::parse_publishers(&owner.handle).expect("publisher policy"),
        Policy::parse(""),
    );
    server.install_writer(writer);
    let server = Arc::new(server);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = server
        .clone()
        .router()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    println!(
        "{}",
        json!({"event":"configuration", "auth":std::env::var("FRUGAL_MIXED_AUTH").unwrap_or_else(|_|"cookie".into()), "http_concurrency":32, "reconnect_concurrency":128, "http_work_limit":config.cost.work_concurrency})
    );
    let base = addr.to_string();
    let bearer = token(&[9; 32], &owner);
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    println!("{{\"event\":\"setup\",\"version\":{},\"sockets\":{:?},\"seconds\":{},\"admission\":{{\"deployment\":4096,\"network\":4096,\"principal\":4096,\"document\":4096,\"editors\":4096}},\"pg_stat_statements\":{}}}",json!(librepaper::VERSION),counts,seconds,!pg_stats(&db).await.is_empty());
    for n in counts {
        assert!(n > 0 && n <= 4000, "choose 1..=4000 sockets");
        let mut slugs = Vec::with_capacity(n);
        for _ in 0..n {
            let slug = format!("fm-{tag}-{}", Uuid::now_v7().simple());
            db.create_document(NewDocument {
                slug: slug.clone(),
                owner_id: owner.id,
                owner_session_generation: Some(owner.session_generation),
                ownership_mode: "owned".into(),
                title: "Mixed benchmark".into(),
                source_format: "markdown".into(),
                main_path: "paper.md".into(),
            })
            .await
            .expect("create doc");
            slugs.push(slug);
        }
        let opening = Instant::now();
        let mut sockets: Vec<(String, Ws, LoroDoc, VersionVector)> =
            futures_util::stream::iter(slugs.iter().cloned().map(|slug| {
                let base = base.clone();
                let bearer = bearer.clone();
                async move {
                    let ws = open(&base, &slug, &bearer).await;
                    (slug, ws)
                }
            }))
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|(slug, result)| {
                let ws = result.expect("initial socket connect");
                let d = LoroDoc::new();
                let _ = d.get_text("t");
                (slug, ws, d, VersionVector::default())
            })
            .collect();
        println!(
            "{{\"event\":\"connected\",\"sockets\":{},\"elapsed_ms\":{},\"pool\":{}}}",
            n,
            opening.elapsed().as_secs_f64() * 1000.0,
            pool_telemetry(&db)
        );
        read_project(
            &http,
            format!("http://{base}/api/documents/{}/project", slugs[0]),
            &bearer,
        )
        .await
        .expect("HTTP project preflight before measurements");

        // Isolated application-ping phase: idle between frames except for the ping itself.
        let before = pg_stats(&db).await;
        let began = Instant::now();
        let mut ping_ms = Vec::new();
        let mut ping_fail = 0usize;
        let ping_cycles = seconds / 25;
        assert!(ping_cycles > 0, "FRUGAL_MIXED_SECONDS must be at least 25");
        for cycle in 0..ping_cycles {
            let due = began + Duration::from_secs(25 * (cycle + 1) as u64);
            tokio::time::sleep_until(due.into()).await;
            let results = join_all(sockets.iter_mut().map(|(_, ws, _, _)| async move {
                let t = Instant::now();
                if ws
                    .send(WsMessage::Text(json!({"type":"ping"}).to_string().into()))
                    .await
                    .is_err()
                {
                    return Err(0.0);
                };
                loop {
                    match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                        Ok(Some(Ok(WsMessage::Text(s))))
                            if serde_json::from_str::<Value>(&s)
                                .map(|v| v["type"] == "pong")
                                .unwrap_or(false) =>
                        {
                            return Ok(t.elapsed().as_secs_f64() * 1000.0)
                        }
                        Ok(Some(Ok(_))) => continue,
                        _ => return Err(t.elapsed().as_secs_f64() * 1000.0),
                    }
                }
            }))
            .await;
            for x in results {
                match x {
                    Ok(t) => ping_ms.push(t),
                    Err(_) => ping_fail += 1,
                }
            }
        }
        tokio::time::sleep_until((began + Duration::from_secs(seconds as u64)).into()).await;
        let ping_elapsed = began.elapsed().as_secs_f64() * 1000.0;
        ping_ms.sort_by(f64::total_cmp);
        let after = pg_stats(&db).await;
        println!("{{\"event\":\"ping_authorization\",\"sockets\":{},\"frames\":{},\"elapsed_ms\":{},\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{},\"failures\":{},\"pool\":{},\"pg_delta\":{}}}",n,ping_ms.len()+ping_fail,ping_elapsed,percentile(&ping_ms,0.5),percentile(&ping_ms,0.95),percentile(&ping_ms,0.99),ping_fail,pool_telemetry(&db),stats_delta(&before,&after));

        // Frequent application pings isolate cache-miss SQL from writer and HTTP work.
        let before = pg_stats(&db).await;
        let began = Instant::now();
        let mut auth_ms = Vec::new();
        for round in 0..50 {
            let samples = join_all(sockets.iter_mut().map(|(_, ws, _, _)| async move {
                let sent = Instant::now();
                ws.send(WsMessage::Text(json!({"type":"ping"}).to_string().into()))
                    .await
                    .expect("send authorization ping");
                wait_type(ws, "pong")
                    .await
                    .expect("authorization ping reply");
                sent.elapsed().as_secs_f64() * 1000.0
            }))
            .await;
            auth_ms.extend(samples);
            tokio::time::sleep_until((began + Duration::from_millis(200 * (round + 1))).into())
                .await;
        }
        auth_ms.sort_by(f64::total_cmp);
        let after = pg_stats(&db).await;
        println!(
            "{}",
            json!({"event":"frequent_authorization", "sockets":n, "frames":auth_ms.len(), "elapsed_ms":began.elapsed().as_secs_f64()*1000.0,
            "p50_ms":percentile(&auth_ms,0.5), "p95_ms":percentile(&auth_ms,0.95), "p99_ms":percentile(&auth_ms,0.99), "pg_delta":stats_delta(&before,&after)})
        );

        // Mixed phase includes edits that invoke the writer fence, state HTTP reads,
        // and the normal age/quiet flush path. Each socket owns a separate document.
        let stall_seconds = std::env::var("FRUGAL_MIXED_DB_STALL_SECONDS")
            .ok()
            .map(|value| value.parse::<u64>().expect("stall seconds"))
            .unwrap_or(0);
        let stall = if stall_seconds > 0 {
            use sqlx::Connection;
            let mut connection = sqlx::PgConnection::connect(&url)
                .await
                .expect("independent stall connection");
            sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
            sqlx::query("LOCK TABLE document_updates IN EXCLUSIVE MODE")
                .execute(&mut connection)
                .await
                .unwrap();
            Some(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(stall_seconds)).await;
                sqlx::query("ROLLBACK")
                    .execute(&mut connection)
                    .await
                    .expect("release mixed write stall");
            }))
        } else {
            None
        };
        println!(
            "{}",
            json!({"event":"mixed_stall", "sockets":n, "seconds":stall_seconds})
        );
        let before = pg_stats(&db).await;
        let began = Instant::now();
        let mut edit_ms = Vec::new();
        let mut edit_fail = 0usize;
        let mut http_ms = Vec::new();
        let mut http_fail = 0usize;
        let mut http_errors = std::collections::BTreeMap::<String, usize>::new();
        for round in 0..seconds {
            let edit_results = join_all(sockets.iter_mut().enumerate().map(
                |(i, (_, ws, doc, vv))| async move {
                    use base64::Engine;
                    let text = format!("{i}-{round} ");
                    let h = doc.get_text("t");
                    if h.insert_utf16(h.len_utf16(), &text).is_err() {
                        return None;
                    }
                    doc.commit();
                    let bytes = doc
                        .export(ExportMode::Updates {
                            from: std::borrow::Cow::Borrowed(vv),
                        })
                        .ok()?;
                    *vv = doc.oplog_vv();
                    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                    let t = Instant::now();
                    if ws
                        .send(WsMessage::Text(
                            json!({"type":"doc-update","update":encoded,"seq":round+1})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .is_err()
                    {
                        return None;
                    }
                    ws.send(WsMessage::Text(json!({"type":"ping"}).to_string().into()))
                        .await
                        .ok()?;
                    wait_type(ws, "pong").await.ok()?;
                    Some(t.elapsed().as_secs_f64() * 1000.0)
                },
            ))
            .await;
            for result in edit_results {
                match result {
                    Some(t) => edit_ms.push(t),
                    None => edit_fail += 1,
                }
            }
            let reqs = sockets.iter().map(|(slug, _, _, _)| {
                read_project(
                    &http,
                    format!("http://{base}/api/documents/{slug}/project"),
                    &bearer,
                )
            });
            for result in futures_util::stream::iter(reqs)
                .buffer_unordered(32)
                .collect::<Vec<_>>()
                .await
            {
                match result {
                    Ok(t) => http_ms.push(t),
                    Err(error) => {
                        http_fail += 1;
                        *http_errors.entry(error).or_default() += 1;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let traffic_ms = began.elapsed().as_secs_f64() * 1000.0;
        if let Some(stall) = stall {
            stall.await.expect("stall release");
        }
        let drain_started = Instant::now();
        loop {
            if server.rooms.registry().pending().retained_used() == 0 {
                break;
            }
            assert!(
                drain_started.elapsed() < Duration::from_secs(60),
                "mixed workload did not drain"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        println!(
            "{}",
            json!({"event":"mixed_drain", "sockets":n, "traffic_ms":traffic_ms,"drain_ms":drain_started.elapsed().as_secs_f64()*1000.0,"pending":server.rooms.registry().pending().snapshot()})
        );
        println!(
            "{}",
            json!({"event":"http_errors", "sockets":n, "errors":http_errors})
        );
        edit_ms.sort_by(f64::total_cmp);
        http_ms.sort_by(f64::total_cmp);
        let after = pg_stats(&db).await;
        println!("{{\"event\":\"mixed_edit_http_flush\",\"sockets\":{},\"elapsed_ms\":{},\"updates_ok\":{},\"update_failures\":{},\"http_ok\":{},\"http_failures\":{},\"edit_roundtrip_p95_ms\":{},\"http_p50_ms\":{},\"http_p95_ms\":{},\"http_p99_ms\":{},\"pool\":{},\"pg_delta\":{}}}",n,began.elapsed().as_secs_f64()*1000.0,edit_ms.len(),edit_fail,http_ms.len(),http_fail,percentile(&edit_ms,0.95),percentile(&http_ms,0.5),percentile(&http_ms,0.95),percentile(&http_ms,0.99),pool_telemetry(&db),stats_delta(&before,&after));
        assert_eq!(edit_fail, 0, "mixed workload lost an edit response");
        assert_eq!(http_fail, 0, "bounded interactive HTTP workload failed");

        // Close and re-establish the same cohort at once to approximate a restart
        // reconnect storm. The listener remains live, so this measures admission,
        // authentication, room join and initial state response.
        for (_, mut ws, _, _) in sockets.drain(..) {
            let _ = ws.close(None).await;
        }
        let before = pg_stats(&db).await;
        let began = Instant::now();
        let reopened = futures_util::stream::iter(slugs.iter().cloned().map(|slug| {
            let base = base.clone();
            let bearer = bearer.clone();
            async move {
                let t = Instant::now();
                let result = open(&base, &slug, &bearer).await;
                (result, t.elapsed().as_secs_f64() * 1000.0)
            }
        }))
        .buffer_unordered(128)
        .collect::<Vec<_>>()
        .await;
        let mut reconnect_ms = Vec::new();
        let mut reconnect_fail = 0usize;
        let mut reconnect_errors = std::collections::BTreeMap::<String, usize>::new();
        let mut restored = Vec::new();
        for (result, ms) in reopened {
            match result {
                Ok(ws) => {
                    reconnect_ms.push(ms);
                    restored.push(ws)
                }
                Err(error) => {
                    reconnect_fail += 1;
                    *reconnect_errors.entry(error).or_default() += 1;
                }
            }
        }
        let after = pg_stats(&db).await;
        reconnect_ms.sort_by(f64::total_cmp);
        println!(
            "{}",
            json!({"event":"reconnect_errors", "sockets":n,"errors":reconnect_errors})
        );
        println!("{{\"event\":\"mass_reconnect\",\"sockets\":{},\"successes\":{},\"failures\":{},\"elapsed_ms\":{},\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{},\"pool\":{},\"pg_delta\":{}}}",n,reconnect_ms.len(),reconnect_fail,began.elapsed().as_secs_f64()*1000.0,percentile(&reconnect_ms,0.5),percentile(&reconnect_ms,0.95),percentile(&reconnect_ms,0.99),pool_telemetry(&db),stats_delta(&before,&after));
        for mut ws in restored {
            let _ = ws.close(None).await;
        }
    }
}
