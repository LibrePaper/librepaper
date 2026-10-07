//! The operator's graphs: a dozen operational numbers, one row a minute for
//! 400 days in one SQLite file under the state directory, drawn on a page
//! served on the admin origin.
//!
//! Nothing here touches PostgreSQL or the Prometheus registry. The sampler
//! hands `observe` the snapshot it already takes; the middleware feeds `Live`
//! for every request. The sampler's connection is the only writer, and each
//! read opens its own read-only connection.

pub mod routes;
mod store;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::Semaphore;

use super::metrics::HISTOGRAM_EDGES_SECONDS;
use store::Store;

/// How often a row is recorded. A constant, not a knob.
pub const HISTORY_INTERVAL: Duration = Duration::from_secs(60);
/// How long a row is kept: a year plus margin, so a year over year view works.
pub const HISTORY_RETENTION: Duration = Duration::from_secs(400 * 24 * 60 * 60);
/// How often old rows are deleted.
const TRIM_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const FILE_NAME: &str = "metrics.sqlite";

/// The request histogram's cells: one per edge and the `+Inf` cell.
const CELLS: usize = 11;
const _: () = assert!(HISTOGRAM_EDGES_SECONDS.len() + 1 == CELLS);

/// How the samples of one series in a wide cell become one value. Rates are
/// already per minute, so both keep their unit at every zoom. Max keeps a
/// one-minute burst visible in a view where a cell is hours wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    Mean,
    Max,
}

/// One line of the catalog: how a series is named, drawn and bucketed.
#[derive(Clone, Copy, Debug)]
pub struct Series {
    pub name: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub bucket: Bucket,
}

const fn entry(
    name: &'static str,
    label: &'static str,
    unit: &'static str,
    bucket: Bucket,
) -> Series {
    Series {
        name,
        label,
        unit,
        bucket,
    }
}

/// The twelve series, in the order the page draws them and `sample` fills
/// them. Adding one is a line here and one in `sample`; its rows start that
/// day.
#[rustfmt::skip]
pub const SERIES: [Series; 12] = [
    entry("requests_per_minute", "Requests per minute", "count", Bucket::Mean),
    entry("client_errors_per_minute", "Client errors per minute", "count", Bucket::Max),
    entry("server_errors_per_minute", "Server errors per minute", "count", Bucket::Max),
    entry("latency_p95_seconds", "Latency p95", "seconds", Bucket::Max),
    entry("documents_resident", "Documents resident", "count", Bucket::Mean),
    entry("sockets_active", "Sockets active", "count", Bucket::Mean),
    entry("storage_bytes", "Storage", "bytes", Bucket::Mean),
    entry("process_rss_bytes", "Process memory", "bytes", Bucket::Mean),
    entry("memory_available_bytes", "Memory available", "bytes", Bucket::Mean),
    entry("disk_available_bytes", "Disk available", "bytes", Bucket::Mean),
    entry("host_cpu_percent", "Host CPU", "percent", Bucket::Mean),
    entry("db_connections_in_use", "Database connections in use", "count", Bucket::Mean),
];

/// Cumulative request counts since the process started, fed by the request
/// middleware. Health checks and the admin origin's own requests are not
/// counted, so the page does not draw itself.
#[derive(Default)]
pub struct Live {
    requests: AtomicU64,
    client_errors: AtomicU64,
    server_errors: AtomicU64,
    /// Not cumulative across cells: a request lands in exactly one.
    cells: [AtomicU64; CELLS],
}

/// A read of `Live`, or the difference of two reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveCounts {
    pub requests: u64,
    pub client_errors: u64,
    pub server_errors: u64,
    pub buckets: [u64; CELLS],
}

impl LiveCounts {
    /// What happened since `earlier`. A counter that went backwards counts as
    /// no change.
    fn since(&self, earlier: &LiveCounts) -> LiveCounts {
        let mut buckets = [0; CELLS];
        for (slot, (now, then)) in buckets
            .iter_mut()
            .zip(self.buckets.iter().zip(earlier.buckets.iter()))
        {
            *slot = now.saturating_sub(*then);
        }
        LiveCounts {
            requests: self.requests.saturating_sub(earlier.requests),
            client_errors: self.client_errors.saturating_sub(earlier.client_errors),
            server_errors: self.server_errors.saturating_sub(earlier.server_errors),
            buckets,
        }
    }
}

impl Live {
    /// Count one finished request under its route class, status and duration.
    pub fn record(&self, route: &str, status: u16, elapsed: Duration) {
        if route == "health" || route == "admin" {
            return;
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
        if (400..500).contains(&status) {
            self.client_errors.fetch_add(1, Ordering::Relaxed);
        } else if status >= 500 {
            self.server_errors.fetch_add(1, Ordering::Relaxed);
        }
        let seconds = elapsed.as_secs_f64();
        let cell = HISTOGRAM_EDGES_SECONDS
            .iter()
            .position(|edge| seconds <= *edge)
            .unwrap_or(HISTOGRAM_EDGES_SECONDS.len());
        self.cells[cell].fetch_add(1, Ordering::Relaxed);
    }

    /// The totals so far.
    pub fn counts(&self) -> LiveCounts {
        let mut buckets = [0; CELLS];
        for (slot, cell) in buckets.iter_mut().zip(self.cells.iter()) {
            *slot = cell.load(Ordering::Relaxed);
        }
        LiveCounts {
            requests: self.requests.load(Ordering::Relaxed),
            client_errors: self.client_errors.load(Ordering::Relaxed),
            server_errors: self.server_errors.load(Ordering::Relaxed),
            buckets,
        }
    }
}

/// The 95th percentile of a window's request durations, from the histogram
/// cell counts of that window. It is the first cell whose cumulative count
/// reaches 95 percent of the total, interpolated linearly between the cell's
/// edges; the first cell starts at zero. The `+Inf` cell reports the last
/// finite edge, so a result equal to it means "at least". No requests: `None`.
pub fn p95(deltas: &[u64; CELLS]) -> Option<f64> {
    let total: u64 = deltas.iter().sum();
    if total == 0 {
        return None;
    }
    let target = total as f64 * 0.95;
    let last = HISTOGRAM_EDGES_SECONDS.len();
    let mut before = 0u64;
    for (index, &count) in deltas.iter().enumerate() {
        let cumulative = before + count;
        if cumulative as f64 >= target {
            if index == last {
                return Some(HISTOGRAM_EDGES_SECONDS[last - 1]);
            }
            let lower = if index == 0 {
                0.0
            } else {
                HISTOGRAM_EDGES_SECONDS[index - 1]
            };
            let upper = HISTOGRAM_EDGES_SECONDS[index];
            let fraction = ((target - before as f64) / count as f64).clamp(0.0, 1.0);
            return Some(lower + (upper - lower) * fraction);
        }
        before = cumulative;
    }
    None
}

/// Requests, client errors and server errors per minute, then the p95 in
/// seconds. Counts divide by the real time the window lasted, because a
/// stalled sampler makes a window longer than a minute.
pub fn rates(delta: &LiveCounts, elapsed_seconds: f64) -> [Option<f64>; 4] {
    if !(elapsed_seconds.is_finite() && elapsed_seconds > 0.0) {
        return [None; 4];
    }
    let per_minute = |count: u64| Some(count as f64 / elapsed_seconds * 60.0);
    [
        per_minute(delta.requests),
        per_minute(delta.client_errors),
        per_minute(delta.server_errors),
        p95(&delta.buckets),
    ]
}

/// One series' values over a requested range, one per cell.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesData {
    pub name: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub bucket: Bucket,
    pub values: Vec<Option<f64>>,
}

/// Everything the sampler carries from one record to the next.
struct State {
    store: Store,
    /// When the last row was recorded, or when the process started.
    last_record: Instant,
    /// What `Live` read at `last_record`.
    baseline: LiveCounts,
    /// The host's busy and total CPU ticks at the last record.
    last_ticks: Option<(f64, f64)>,
    last_trim: Instant,
}

/// The history store and its read gate.
pub struct Graphs {
    /// One read at a time: a second `/data` request waits behind the first
    /// rather than being refused.
    pub read_slot: Semaphore,
    path: PathBuf,
    state: Mutex<State>,
}

impl Graphs {
    /// Open `<state_dir>/metrics.sqlite`, or set it aside and start a new one
    /// when it will not open or fails `quick_check`. History is disposable, so
    /// only a failure to create the fresh file is an error. Blocking, and
    /// `quick_check` reads the whole file: call it off the listener path.
    pub fn open(state_dir: &Path) -> Result<Graphs, String> {
        let path = state_dir.join(FILE_NAME);
        let store = match open_checked(&path) {
            Ok(store) => store,
            Err(error) => {
                tracing::warn!(
                    "the graphs history {} is unusable and was set aside: {error}",
                    path.display()
                );
                set_aside(&path);
                Store::open(&path)
                    .map_err(|error| format!("could not create {}: {error}", path.display()))?
            }
        };
        let started = Instant::now();
        let mut state = State {
            store,
            last_record: started,
            baseline: LiveCounts::default(),
            last_ticks: None,
            last_trim: started,
        };
        trim(&mut state.store, unix_seconds());
        Ok(Graphs {
            read_slot: Semaphore::new(1),
            path,
            state: Mutex::new(state),
        })
    }

    /// Called by the sampler on every tick with the snapshot it just took.
    /// Writes one row per series when a minute has passed since the last, and
    /// deletes old rows once a day. Failures are logged and lose that minute.
    /// Blocking.
    pub fn observe(&self, snapshot: &Value, live: &Live) {
        self.observe_at(Instant::now(), unix_seconds(), snapshot, live);
    }

    fn observe_at(&self, now: Instant, unix: i64, snapshot: &Value, live: &Live) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let elapsed = now.saturating_duration_since(state.last_record);
        if elapsed < HISTORY_INTERVAL {
            return;
        }
        let counts = live.counts();
        let window = rates(&counts.since(&state.baseline), elapsed.as_secs_f64());
        let ticks = cpu_ticks(snapshot);
        let values = sample(snapshot, window, cpu_percent(state.last_ticks, ticks));
        if let Err(error) = state.store.insert(unix, &values) {
            tracing::warn!("the graphs history could not record a minute: {error}");
        }
        state.last_record = now;
        state.baseline = counts;
        state.last_ticks = ticks;
        if now.saturating_duration_since(state.last_trim) >= TRIM_INTERVAL {
            trim(&mut state.store, unix);
            state.last_trim = now;
        }
    }

    /// Every series over `[from, to)` in `points` cells, in catalog order.
    /// Opens a read-only connection for the call. Blocking.
    pub fn read(&self, from: i64, to: i64, points: u32) -> Result<Vec<SeriesData>, String> {
        if from >= to || points == 0 {
            return Err("the range is empty".to_string());
        }
        let columns = store::read_range(&self.path, from, to, points)
            .map_err(|error| format!("could not read the graphs history: {error}"))?;
        Ok(SERIES
            .iter()
            .zip(columns)
            .map(|(series, values)| SeriesData {
                name: series.name,
                label: series.label,
                unit: series.unit,
                bucket: series.bucket,
                values,
            })
            .collect())
    }
}

/// Open the file and refuse it unless `quick_check` passes.
fn open_checked(path: &Path) -> Result<Store, String> {
    let store = Store::open(path).map_err(|error| error.to_string())?;
    match store.quick_check() {
        Ok(true) => Ok(store),
        Ok(false) => Err("quick_check found damage".to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Rename the file and its `-wal` and `-shm` siblings out of the way, each to
/// its own name plus `.corrupt-<unix seconds>`.
fn set_aside(path: &Path) {
    let stamp = unix_seconds();
    for suffix in ["", "-wal", "-shm"] {
        let mut from = path.as_os_str().to_owned();
        from.push(suffix);
        let mut to = from.clone();
        to.push(format!(".corrupt-{stamp}"));
        match std::fs::rename(&from, &to) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                "could not set {} aside: {error}",
                Path::new(&from).display()
            ),
        }
    }
}

fn trim(store: &mut Store, now: i64) {
    let cutoff = now.saturating_sub(HISTORY_RETENTION.as_secs() as i64);
    if let Err(error) = store.trim(cutoff) {
        tracing::warn!("the graphs history could not delete old rows: {error}");
    }
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// A finite number at a JSON pointer in the snapshot, or `None`.
fn gauge(snapshot: &Value, pointer: &str) -> Option<f64> {
    snapshot
        .pointer(pointer)?
        .as_f64()
        .filter(|value| value.is_finite())
}

fn cpu_ticks(snapshot: &Value) -> Option<(f64, f64)> {
    Some((
        gauge(snapshot, "/host/host_cpu_busy_ticks")?,
        gauge(snapshot, "/host/host_cpu_total_ticks")?,
    ))
}

/// Host busy time as a percentage of all time since the previous record.
/// `None` on the first record, when a reading is missing, or when no ticks
/// passed.
fn cpu_percent(previous: Option<(f64, f64)>, current: Option<(f64, f64)>) -> Option<f64> {
    let ((busy_before, total_before), (busy, total)) = previous.zip(current)?;
    let total_delta = total - total_before;
    if total_delta <= 0.0 {
        return None;
    }
    Some((100.0 * (busy - busy_before) / total_delta).clamp(0.0, 100.0))
}

/// The twelve values for one record, in `SERIES` order.
fn sample(snapshot: &Value, per_minute: [Option<f64>; 4], cpu: Option<f64>) -> [Option<f64>; 12] {
    [
        per_minute[0],
        per_minute[1],
        per_minute[2],
        per_minute[3],
        gauge(snapshot, "/rooms/documents"),
        gauge(snapshot, "/sockets/active"),
        gauge(snapshot, "/storage_ledger/deployment_bytes"),
        gauge(snapshot, "/host/rss_bytes"),
        gauge(snapshot, "/host/host_available_memory_bytes"),
        gauge(snapshot, "/host/disk_available_bytes"),
        cpu,
        gauge(snapshot, "/database/checked_out"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn near(left: Option<f64>, right: f64) {
        let left = left.expect("a value");
        assert!((left - right).abs() < 1e-9, "{left} is not {right}");
    }

    fn snapshot(busy: u64, total: u64) -> Value {
        json!({
            "rooms": {"documents": 17},
            "sockets": {"active": 1},
            "storage_ledger": {"deployment_bytes": 840_000},
            "host": {
                "rss_bytes": 23_000_000,
                "host_available_memory_bytes": 4_000_000_000u64,
                "disk_available_bytes": 50_000_000_000u64,
                "host_cpu_busy_ticks": busy,
                "host_cpu_total_ticks": total,
            },
            "database": {"checked_out": 2},
        })
    }

    fn column(graphs: &Graphs, name: &str, from: i64, to: i64, points: u32) -> Vec<Option<f64>> {
        graphs
            .read(from, to, points)
            .unwrap()
            .into_iter()
            .find(|series| series.name == name)
            .unwrap()
            .values
    }

    #[test]
    fn the_catalog_is_twelve_names_in_the_documented_order() {
        let names: Vec<_> = SERIES.iter().map(|series| series.name).collect();
        assert_eq!(
            names,
            [
                "requests_per_minute",
                "client_errors_per_minute",
                "server_errors_per_minute",
                "latency_p95_seconds",
                "documents_resident",
                "sockets_active",
                "storage_bytes",
                "process_rss_bytes",
                "memory_available_bytes",
                "disk_available_bytes",
                "host_cpu_percent",
                "db_connections_in_use",
            ]
        );
        let buckets: Vec<_> = SERIES.iter().map(|series| series.bucket).collect();
        assert_eq!(buckets[0], Bucket::Mean);
        assert!(buckets[1..4].iter().all(|bucket| *bucket == Bucket::Max));
        assert!(buckets[4..].iter().all(|bucket| *bucket == Bucket::Mean));
        let units: Vec<_> = SERIES.iter().map(|series| series.unit).collect();
        assert_eq!(
            units,
            [
                "count", "count", "count", "seconds", "count", "count", "bytes", "bytes",
                "bytes", "bytes", "percent", "count",
            ]
        );
    }

    #[test]
    fn p95_interpolates_inside_the_cell_that_reaches_95_percent() {
        // 50 requests under 5 ms, 40 in (25 ms, 50 ms], 10 in (250 ms, 500 ms]:
        // 95 of 100 falls halfway through the last populated cell.
        let mut deltas = [0u64; CELLS];
        deltas[0] = 50;
        deltas[3] = 40;
        deltas[6] = 10;
        near(p95(&deltas), 0.375);
    }

    #[test]
    fn p95_of_the_first_cell_starts_at_zero() {
        let mut deltas = [0u64; CELLS];
        deltas[0] = 100;
        near(p95(&deltas), 0.005 * 0.95);
    }

    #[test]
    fn p95_with_no_requests_is_none() {
        assert_eq!(p95(&[0; CELLS]), None);
    }

    #[test]
    fn p95_of_everything_in_the_last_cell_is_the_last_edge() {
        let mut deltas = [0u64; CELLS];
        deltas[CELLS - 1] = 7;
        assert_eq!(p95(&deltas), Some(5.0));
    }

    #[test]
    fn ninety_requests_over_ninety_seconds_is_sixty_a_minute() {
        let delta = LiveCounts {
            requests: 90,
            client_errors: 9,
            server_errors: 3,
            ..LiveCounts::default()
        };
        let per_minute = rates(&delta, 90.0);
        near(per_minute[0], 60.0);
        near(per_minute[1], 6.0);
        near(per_minute[2], 2.0);
        assert_eq!(per_minute[3], None);
    }

    #[test]
    fn rates_need_a_positive_elapsed_time() {
        let delta = LiveCounts {
            requests: 10,
            ..LiveCounts::default()
        };
        assert_eq!(rates(&delta, 0.0), [None; 4]);
        assert_eq!(rates(&delta, f64::NAN), [None; 4]);
    }

    #[test]
    fn live_ignores_health_and_admin_and_files_the_rest() {
        let live = Live::default();
        live.record("health", 200, Duration::from_millis(1));
        live.record("admin", 200, Duration::from_millis(1));
        assert_eq!(live.counts(), LiveCounts::default());

        live.record("api_documents", 200, Duration::from_millis(3));
        live.record("api_documents", 404, Duration::from_millis(5));
        live.record("root", 503, Duration::from_millis(30));
        live.record("root", 500, Duration::from_secs(9));
        let counts = live.counts();
        assert_eq!(counts.requests, 4);
        assert_eq!(counts.client_errors, 1);
        assert_eq!(counts.server_errors, 2);
        // 3 ms and 5 ms are both at or under the first edge, 30 ms is under
        // the fourth (50 ms), and 9 s is past the last edge.
        assert_eq!(counts.buckets[0], 2);
        assert_eq!(counts.buckets[3], 1);
        assert_eq!(counts.buckets[CELLS - 1], 1);
        assert_eq!(counts.buckets.iter().sum::<u64>(), 4);
    }

    #[test]
    fn a_duration_on_an_edge_belongs_to_that_edges_cell() {
        let live = Live::default();
        live.record("root", 200, Duration::from_secs_f64(HISTOGRAM_EDGES_SECONDS[2]));
        assert_eq!(live.counts().buckets[2], 1);
    }

    #[test]
    fn counts_since_never_go_negative() {
        let earlier = LiveCounts {
            requests: 5,
            ..LiveCounts::default()
        };
        assert_eq!(LiveCounts::default().since(&earlier).requests, 0);
    }

    #[test]
    fn cpu_percent_is_busy_over_total_between_two_readings() {
        assert_eq!(cpu_percent(None, Some((10.0, 100.0))), None);
        assert_eq!(cpu_percent(Some((10.0, 100.0)), None), None);
        assert_eq!(cpu_percent(Some((10.0, 100.0)), Some((10.0, 100.0))), None);
        near(cpu_percent(Some((10.0, 100.0)), Some((60.0, 300.0))), 25.0);
    }

    #[test]
    fn a_missing_or_odd_field_is_no_value() {
        let values = sample(
            &json!({"rooms": {"documents": "many"}, "sockets": {}}),
            [None; 4],
            None,
        );
        assert_eq!(values, [None; 12]);
    }

    #[test]
    fn open_creates_the_file_and_reads_an_empty_range_as_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let graphs = Graphs::open(dir.path()).unwrap();
        assert!(dir.path().join("metrics.sqlite").exists());
        let read = graphs.read(0, 600, 10).unwrap();
        assert_eq!(read.len(), 12);
        assert!(read.iter().all(|series| series.values == vec![None; 10]));
        assert!(graphs.read(5, 5, 10).is_err());
        assert!(graphs.read(0, 5, 0).is_err());
    }

    #[test]
    fn observe_waits_for_the_interval_then_writes_one_row_per_series() {
        let dir = tempfile::tempdir().unwrap();
        let graphs = Graphs::open(dir.path()).unwrap();
        let live = Live::default();
        let started = graphs.state.lock().unwrap().last_record;
        let now = unix_seconds();

        graphs.observe_at(started + Duration::from_secs(30), now, &snapshot(0, 0), &live);
        assert!(column(&graphs, "sockets_active", now - 10, now + 10, 20)
            .iter()
            .all(Option::is_none));

        for _ in 0..90 {
            live.record("api_documents", 200, Duration::from_millis(3));
        }
        graphs.observe_at(
            started + Duration::from_secs(90),
            now,
            &snapshot(100, 1000),
            &live,
        );
        let at = |name: &str| column(&graphs, name, now, now + 1, 1)[0];
        near(at("requests_per_minute"), 60.0);
        near(at("server_errors_per_minute"), 0.0);
        assert_eq!(at("documents_resident"), Some(17.0));
        assert_eq!(at("sockets_active"), Some(1.0));
        assert_eq!(at("storage_bytes"), Some(840_000.0));
        assert_eq!(at("process_rss_bytes"), Some(23_000_000.0));
        assert_eq!(at("disk_available_bytes"), Some(50_000_000_000.0));
        assert_eq!(at("db_connections_in_use"), Some(2.0));
        near(at("latency_p95_seconds"), 0.005 * 0.95);
        // The first record has no earlier ticks to compare with.
        assert_eq!(at("host_cpu_percent"), None);

        // The next record diffs against the first: no requests, 25 percent busy.
        graphs.observe_at(
            started + Duration::from_secs(150),
            now + 60,
            &snapshot(200, 1400),
            &live,
        );
        let later = |name: &str| column(&graphs, name, now + 60, now + 61, 1)[0];
        near(later("requests_per_minute"), 0.0);
        assert_eq!(later("latency_p95_seconds"), None);
        near(later("host_cpu_percent"), 25.0);
    }

    #[test]
    fn a_garbage_file_is_set_aside_and_a_fresh_one_created() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("metrics.sqlite"), vec![0x5a; 4096]).unwrap();
        std::fs::write(dir.path().join("metrics.sqlite-wal"), b"stale").unwrap();

        let graphs = Graphs::open(dir.path()).unwrap();
        assert_eq!(graphs.read(0, 600, 10).unwrap().len(), 12);

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let aside = |prefix: &str| names.iter().find(|name| name.starts_with(prefix)).cloned();
        let corrupt = aside("metrics.sqlite.corrupt-").expect("the garbage file is kept");
        assert!(aside("metrics.sqlite-wal.corrupt-").is_some());
        assert_eq!(
            std::fs::read(dir.path().join(corrupt)).unwrap(),
            vec![0x5a; 4096]
        );
    }

    #[test]
    fn old_rows_are_trimmed_when_a_day_has_passed() {
        let dir = tempfile::tempdir().unwrap();
        let graphs = Graphs::open(dir.path()).unwrap();
        let live = Live::default();
        let started = graphs.state.lock().unwrap().last_record;
        let now = unix_seconds();
        let old = now - HISTORY_RETENTION.as_secs() as i64 - 100;
        graphs.observe_at(started + Duration::from_secs(60), old, &snapshot(0, 0), &live);
        assert_eq!(column(&graphs, "sockets_active", old, old + 1, 1)[0], Some(1.0));

        graphs.observe_at(
            started + Duration::from_secs(60) + TRIM_INTERVAL,
            now,
            &snapshot(0, 0),
            &live,
        );
        assert_eq!(column(&graphs, "sockets_active", old, old + 1, 1)[0], None);
        assert_eq!(column(&graphs, "sockets_active", now, now + 1, 1)[0], Some(1.0));
    }
}
