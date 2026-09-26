//! Measure the exact writer-lease verification shape used by the server:
//! many callers serialize on one Tokio mutex while one dedicated PostgreSQL
//! session executes `SELECT 1`.
//!
//! This is deliberately standalone so it cannot alter or accidentally call
//! production application code. The default query is byte-for-byte the query
//! in `storage/postgres/ownership.rs`; `--server-delay-ms` uses
//! `SELECT 1 FROM pg_sleep($1)` to show the effect of a slow database probe.

use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::Row;
use std::env;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex};

#[derive(Clone, Copy, Debug)]
struct Config {
    warmup: Duration,
    measure: Duration,
    repetitions: usize,
    server_delay: Duration,
    only_concurrency: Option<usize>,
    only_pace_ms: Option<u64>,
}

#[derive(Default)]
struct Samples {
    lock_wait_us: Vec<u64>,
    query_us: Vec<u64>,
    total_us: Vec<u64>,
    scheduled_to_done_us: Vec<u64>,
}

struct RowResult {
    concurrency: usize,
    pace: Option<Duration>,
    delay: Duration,
    repetition: usize,
    samples: Samples,
    measure: Duration,
    elapsed: Duration,
    offered_rps: f64,
}

fn usage() -> ! {
    eprintln!(
        "usage: writer-lease-bench [--url URL] [--warmup-ms N] [--measure-ms N] \
         [--repetitions N] [--server-delay-ms N] [--only-concurrency N] \
         [--only-pace-ms N]"
    );
    std::process::exit(2);
}

fn arg_value(args: &mut impl Iterator<Item = String>, name: &str) -> String {
    args.next().unwrap_or_else(|| {
        eprintln!("missing value for {name}");
        usage();
    })
}

fn parse_u64(value: String, name: &str) -> u64 {
    value.parse().ok().unwrap_or_else(|| {
        eprintln!("invalid value for {name}");
        usage();
    })
}

fn percentile(values: &[u64], p: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[(sorted.len() - 1) * p / 100]
}

fn average(values: &[u64]) -> u64 {
    if values.is_empty() {
        0
    } else {
        values.iter().copied().sum::<u64>() / values.len() as u64
    }
}

fn format_duration(duration: Option<Duration>) -> String {
    duration
        .map(|value| value.as_millis().to_string())
        .unwrap_or_else(|| "closed".into())
}

async fn run_row(
    url: &str,
    concurrency: usize,
    pace: Option<Duration>,
    delay: Duration,
    repetition: usize,
    config: Config,
) -> Result<RowResult, Box<dyn std::error::Error>> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .connect(url)
        .await?;
    let connection = pool.acquire().await?;
    let shared = Arc::new(Mutex::new(connection));

    // Warm the connection and PostgreSQL plan/cache before taking measured
    // samples. A small concurrent warmup also exercises the mutex's queue.
    let warmup_deadline = Instant::now() + config.warmup;
    let mut warmup_tasks = Vec::with_capacity(concurrency);
    for worker_id in 0..concurrency {
        let shared = Arc::clone(&shared);
        let phase = pace.map(|pace| pace.mul_f64(worker_id as f64 / concurrency as f64));
        warmup_tasks.push(tokio::spawn(async move {
            let mut next = Instant::now() + phase.unwrap_or(Duration::ZERO);
            if phase.is_some() {
                tokio::time::sleep_until(next.into()).await;
            }
            while Instant::now() < warmup_deadline {
                let mut connection = shared.lock().await;
                if delay.is_zero() {
                    sqlx::query("SELECT 1")
                        .execute(&mut **connection)
                        .await
                        .expect("warmup SELECT 1");
                } else {
                    sqlx::query("SELECT 1 FROM pg_sleep($1)")
                        .bind(delay.as_secs_f64())
                        .execute(&mut **connection)
                        .await
                        .expect("warmup delayed SELECT");
                }
                drop(connection);
                if let Some(pace) = pace {
                    next += pace;
                    tokio::time::sleep_until(next.into()).await;
                }
            }
        }));
    }
    for task in warmup_tasks {
        task.await?;
    }

    let (start_sender, start_receiver) = watch::channel(None);
    let mut tasks = Vec::with_capacity(concurrency);
    for worker_id in 0..concurrency {
        let shared = Arc::clone(&shared);
        let mut start_receiver = start_receiver.clone();
        let phase = pace.map(|pace| pace.mul_f64(worker_id as f64 / concurrency as f64));
        tasks.push(tokio::spawn(async move {
            let mut samples = Samples::default();
            while start_receiver.borrow().is_none() {
                start_receiver
                    .changed()
                    .await
                    .expect("benchmark start signal");
            }
            let measure_start = start_receiver.borrow().expect("benchmark start instant");
            let deadline = measure_start + config.measure;
            let mut scheduled = measure_start + phase.unwrap_or(Duration::ZERO);
            while match pace {
                Some(_) => scheduled < deadline,
                None => Instant::now() < deadline,
            } {
                if pace.is_some() {
                    let now = Instant::now();
                    if now < scheduled {
                        tokio::time::sleep_until(scheduled.into()).await;
                    }
                } else {
                    scheduled = Instant::now();
                }
                let total_start = Instant::now();
                let mut connection = shared.lock().await;
                let lock_acquired = Instant::now();
                if delay.is_zero() {
                    sqlx::query("SELECT 1")
                        .execute(&mut **connection)
                        .await
                        .expect("measured SELECT 1");
                } else {
                    sqlx::query("SELECT 1 FROM pg_sleep($1)")
                        .bind(delay.as_secs_f64())
                        .execute(&mut **connection)
                        .await
                        .expect("measured delayed SELECT");
                }
                let query_done = Instant::now();
                samples
                    .lock_wait_us
                    .push(lock_acquired.duration_since(total_start).as_micros() as u64);
                samples
                    .query_us
                    .push(query_done.duration_since(lock_acquired).as_micros() as u64);
                samples
                    .total_us
                    .push(query_done.duration_since(total_start).as_micros() as u64);
                samples
                    .scheduled_to_done_us
                    .push(query_done.saturating_duration_since(scheduled).as_micros() as u64);
                drop(connection);
                if let Some(pace) = pace {
                    scheduled += pace;
                }
            }
            samples
        }));
    }
    let measure_start = Instant::now();
    start_sender
        .send(Some(measure_start))
        .expect("benchmark tasks still waiting");
    let mut samples = Samples::default();
    for task in tasks {
        let local = task.await?;
        samples.lock_wait_us.extend(local.lock_wait_us);
        samples.query_us.extend(local.query_us);
        samples.total_us.extend(local.total_us);
        samples
            .scheduled_to_done_us
            .extend(local.scheduled_to_done_us);
    }
    let elapsed = measure_start.elapsed();
    drop(shared);
    pool.close().await;
    Ok(RowResult {
        concurrency,
        pace,
        delay,
        repetition,
        samples,
        measure: config.measure,
        elapsed,
        offered_rps: pace
            .map(|pace| concurrency as f64 / pace.as_secs_f64())
            .unwrap_or(f64::INFINITY),
    })
}

fn print_row(row: &RowResult) {
    let s = &row.samples;
    println!(
        "{:>5} {:>7} {:>7} {:>8} {:>9} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        row.concurrency,
        format_duration(row.pace),
        row.delay.as_millis(),
        row.samples.total_us.len(),
        row.samples.total_us.len() as f64 / row.measure.max(row.elapsed).as_secs_f64(),
        average(&s.lock_wait_us),
        percentile(&s.lock_wait_us, 50),
        percentile(&s.lock_wait_us, 99),
        percentile(&s.query_us, 50),
        percentile(&s.total_us, 99),
        percentile(&s.scheduled_to_done_us, 99),
    );
}

fn csv_row(row: &RowResult) -> String {
    let s = &row.samples;
    let wall = row.measure.max(row.elapsed);
    let actual_rps = s.total_us.len() as f64 / wall.as_secs_f64();
    format!(
        "{},{},{},{},{:.2},{:.2},{:.2},{:.2},{},{},{},{},{},{},{},{},{},{}\n",
        row.concurrency,
        format_duration(row.pace),
        row.delay.as_millis(),
        row.repetition,
        row.offered_rps,
        actual_rps,
        wall.as_secs_f64() * 1000.0,
        row.elapsed.saturating_sub(row.measure).as_secs_f64() * 1000.0,
        s.total_us.len(),
        average(&s.lock_wait_us),
        percentile(&s.lock_wait_us, 50),
        percentile(&s.lock_wait_us, 95),
        percentile(&s.lock_wait_us, 99),
        average(&s.query_us),
        percentile(&s.query_us, 50),
        percentile(&s.query_us, 99),
        percentile(&s.scheduled_to_done_us, 50),
        percentile(&s.scheduled_to_done_us, 99),
    )
}

fn report_row(row: &RowResult) -> String {
    let s = &row.samples;
    let wall = row.measure.max(row.elapsed);
    format!(
        "| {} | {} | {} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
        row.concurrency,
        format_duration(row.pace),
        row.delay.as_millis(),
        row.repetition,
        row.offered_rps,
        s.total_us.len() as f64 / wall.as_secs_f64(),
        wall.as_secs_f64() * 1000.0,
        row.elapsed.saturating_sub(row.measure).as_secs_f64() * 1000.0,
        s.total_us.len(),
        percentile(&s.lock_wait_us, 50),
        percentile(&s.lock_wait_us, 95),
        percentile(&s.lock_wait_us, 99),
        percentile(&s.query_us, 50),
        percentile(&s.query_us, 99),
        percentile(&s.scheduled_to_done_us, 50),
        percentile(&s.scheduled_to_done_us, 99),
        percentile(&s.total_us, 50),
        percentile(&s.total_us, 99),
        average(&s.query_us),
    )
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut url = env::var("DATABASE_URL").unwrap_or_default();
    let mut config = Config {
        warmup: Duration::from_millis(500),
        measure: Duration::from_secs(2),
        repetitions: 2,
        server_delay: Duration::ZERO,
        only_concurrency: None,
        only_pace_ms: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => url = arg_value(&mut args, "--url"),
            "--warmup-ms" => {
                config.warmup = Duration::from_millis(parse_u64(
                    arg_value(&mut args, "--warmup-ms"),
                    "--warmup-ms",
                ))
            }
            "--measure-ms" => {
                config.measure = Duration::from_millis(parse_u64(
                    arg_value(&mut args, "--measure-ms"),
                    "--measure-ms",
                ))
            }
            "--repetitions" => {
                config.repetitions =
                    parse_u64(arg_value(&mut args, "--repetitions"), "--repetitions") as usize
            }
            "--server-delay-ms" => {
                config.server_delay = Duration::from_millis(parse_u64(
                    arg_value(&mut args, "--server-delay-ms"),
                    "--server-delay-ms",
                ))
            }
            "--only-concurrency" => {
                config.only_concurrency = Some(parse_u64(
                    arg_value(&mut args, "--only-concurrency"),
                    "--only-concurrency",
                ) as usize)
            }
            "--only-pace-ms" => {
                config.only_pace_ms = Some(parse_u64(
                    arg_value(&mut args, "--only-pace-ms"),
                    "--only-pace-ms",
                ))
            }
            "--help" | "-h" => usage(),
            _ => {
                eprintln!("unknown argument: {arg}");
                usage();
            }
        }
    }
    if url.is_empty()
        || config.repetitions == 0
        || config.measure.is_zero()
        || config.only_concurrency == Some(0)
        || config.only_pace_ms == Some(0)
    {
        eprintln!("DATABASE_URL/--url and positive measurement, repetition, concurrency and pacing values are required");
        usage();
    }

    let metadata_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let (server_version, host) = sqlx::query("SELECT version(), inet_server_addr()::text")
        .fetch_one(&metadata_pool)
        .await
        .map(|row: PgRow| {
            (
                row.try_get::<String, _>(0)
                    .unwrap_or_else(|_| "unknown".into()),
                row.try_get::<Option<String>, _>(1)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "local socket".into()),
            )
        })?;
    metadata_pool.close().await;
    println!("writer-lease benchmark; server={server_version}; addr={host}");
    println!(
        "warmup={}ms measure={}ms repetitions={}",
        config.warmup.as_millis(),
        config.measure.as_millis(),
        config.repetitions
    );
    println!("columns: concurrency pace_ms delay_ms samples actual_rps avg_lock_us p50_lock_us p99_lock_us p50_query_us p99_total_us p99_scheduled_to_done_us");

    let concurrencies = config
        .only_concurrency
        .map(|value| vec![value])
        .unwrap_or_else(|| vec![1, 10, 100, 1000]);
    // The paced cases approximate one and five completed updates per editor
    // per second. They expose queue growth while keeping arrivals realistic.
    let scenarios = config
        .only_pace_ms
        .map(|value| vec![Some(Duration::from_millis(value))])
        .unwrap_or_else(|| {
            vec![
                None,
                Some(Duration::from_secs(1)),
                Some(Duration::from_millis(200)),
            ]
        });
    let mut csv = String::from("concurrency,pace_ms,server_delay_ms,repetition,offered_rps,actual_rps,wall_ms,drain_ms,samples,avg_lock_us,p50_lock_us,p95_lock_us,p99_lock_us,avg_query_us,p50_query_us,p99_query_us,p50_scheduled_to_done_us,p99_scheduled_to_done_us\n");
    let mut report = String::new();
    report.push_str("| concurrency | pace ms | delay ms | repetition | offered/s | actual/s | wall ms | drain ms | samples | p50 lock us | p95 lock us | p99 lock us | p50 query us | p99 query us | p50 scheduled→done us | p99 scheduled→done us | p50 total us | p99 total us | avg query us |\n|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for pace in scenarios {
        for &concurrency in &concurrencies {
            for repetition in 1..=config.repetitions {
                let row = run_row(
                    &url,
                    concurrency,
                    pace,
                    config.server_delay,
                    repetition,
                    config,
                )
                .await?;
                print_row(&row);
                csv.push_str(&csv_row(&row));
                report.push_str(&report_row(&row));
            }
        }
    }
    let out = env::var("WRITER_LEASE_BENCH_OUT").unwrap_or_else(|_| "results.csv".into());
    std::fs::write(&out, csv)?;
    let report_out = format!("{out}.md");
    let mut report_file = String::new();
    writeln!(
        report_file,
        "# Writer lease benchmark\n\nServer: {server_version}; address: {host}.\n\n{report}"
    )
    .expect("format report");
    std::fs::write(report_out, report_file)?;
    Ok(())
}
