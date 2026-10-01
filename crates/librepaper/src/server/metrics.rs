//! Optional, bounded Prometheus metrics for private deployment monitoring.
//!
//! The listener is separate from the user-facing router. All labels are
//! selected from fixed enums, and gauges come from an explicit allowlist of
//! aggregate numeric status fields. No account, document, network, or path
//! identifier can enter the exposition.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde_json::Value;
use tokio::net::TcpListener;

const ROUTES: &[&str] = &[
    "root",
    "health",
    "api_status",
    "api_config",
    "api_auth",
    "api_documents",
    "api_comments",
    "api_assets",
    "api_fonts",
    "api_other",
    "auth",
    "published",
    "raw",
    "pdf",
    "other",
];
const METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "OTHER",
];
const STATUS_CLASSES: &[&str] = &["1xx", "2xx", "3xx", "4xx", "5xx"];
const HISTOGRAM_EDGES_SECONDS: &[f64] = &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0];
const REFUSALS: &[&str] = &[
    "request_memory",
    "work_concurrency",
    "request_budget",
    "transfer_concurrency",
    "socket_budget",
];
pub(super) const METRIC_LISTENER_CONCURRENCY: usize = 2;
const SCRAPE_SAMPLE_TIMEOUT: Duration = Duration::from_secs(1);

/// Optional exporter state shared with the request admission middleware.
pub struct Metrics {
    started: Instant,
    requests: Vec<AtomicU64>,
    histogram_buckets: Vec<AtomicU64>,
    histogram_sum_micros: Vec<AtomicU64>,
    refusals: Vec<AtomicU64>,
    gauges: RwLock<Vec<(&'static str, f64)>>,
    snapshot_success: AtomicU64,
    snapshot_timestamp: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        let route_method_count = ROUTES.len() * METHODS.len();
        Self {
            started: Instant::now(),
            requests: (0..route_method_count * STATUS_CLASSES.len())
                .map(|_| AtomicU64::new(0))
                .collect(),
            histogram_buckets: (0..route_method_count * (HISTOGRAM_EDGES_SECONDS.len() + 1))
                .map(|_| AtomicU64::new(0))
                .collect(),
            histogram_sum_micros: (0..route_method_count).map(|_| AtomicU64::new(0)).collect(),
            refusals: (0..REFUSALS.len()).map(|_| AtomicU64::new(0)).collect(),
            gauges: RwLock::new(Vec::new()),
            snapshot_success: AtomicU64::new(0),
            snapshot_timestamp: AtomicU64::new(0),
        }
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(&self, route: &str, method: &str, status: u16, elapsed: Duration) {
        let route = route_index(route);
        let method = method_index(method);
        let route_method = route * METHODS.len() + method;
        let status_class = usize::from(status / 100).saturating_sub(1).min(4);
        self.requests[route_method * STATUS_CLASSES.len() + status_class]
            .fetch_add(1, Ordering::Relaxed);

        let seconds = elapsed.as_secs_f64();
        let bucket = HISTOGRAM_EDGES_SECONDS
            .iter()
            .position(|edge| seconds <= *edge)
            .unwrap_or(HISTOGRAM_EDGES_SECONDS.len());
        self.histogram_buckets[route_method * (HISTOGRAM_EDGES_SECONDS.len() + 1) + bucket]
            .fetch_add(1, Ordering::Relaxed);
        self.histogram_sum_micros[route_method].fetch_add(
            elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
    }

    pub(super) fn record_middleware_result(
        &self,
        route: &str,
        method: &str,
        status: u16,
        started: Instant,
    ) {
        self.record_request(route, method, status, started.elapsed());
    }

    pub fn record_refusal(&self, reason: &str) {
        if let Some(index) = REFUSALS.iter().position(|known| *known == reason) {
            self.refusals[index].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Replace the cached gauges after a timed background sample. The caller
    /// supplies only values selected by `GAUGE_FIELDS` below.
    fn update_gauges(&self, snapshot: &Value, config: &crate::config::Configuration) {
        let mut gauges = Vec::with_capacity(GAUGE_FIELDS.len() + 8);
        for (family, section, field) in GAUGE_FIELDS {
            if let Some(value) = snapshot.get(section).and_then(|v| v.get(field)) {
                if let Some(number) = json_number(value) {
                    let value = match *family {
                        "librepaper_database_begin_wait_mean_seconds"
                        | "librepaper_database_begin_wait_max_seconds" => number / 1_000_000.0,
                        _ => number,
                    };
                    gauges.push((*family, value));
                }
            }
        }

        let config_gauges = [
            (
                "librepaper_http_work_limit",
                config.cost.work_concurrency as f64,
            ),
            ("librepaper_http_control_work_limit", 16.0),
            (
                "librepaper_http_artifact_transfer_limit",
                config.cost.artifact_transfers as f64,
            ),
            (
                "librepaper_http_requests_per_principal_minute_limit",
                config.cost.requests_per_principal_minute as f64,
            ),
            (
                "librepaper_memory_budget_configured_bytes",
                config.memory_budget_bytes as f64,
            ),
            (
                "librepaper_pending_budget_configured_bytes",
                config.pending_bytes as f64,
            ),
            (
                "librepaper_pending_scratch_configured_bytes",
                config.pending_scratch_bytes as f64,
            ),
            (
                "librepaper_sockets_deployment_limit",
                config.sockets.deployment_max as f64,
            ),
            (
                "librepaper_sockets_network_limit",
                config.sockets.network_max as f64,
            ),
            (
                "librepaper_sockets_principal_limit",
                config.sockets.principal_max as f64,
            ),
            (
                "librepaper_sockets_document_limit",
                config.sockets.document_max as f64,
            ),
            (
                "librepaper_sockets_document_readers_limit",
                config.sockets.document_readers_max as f64,
            ),
            (
                "librepaper_sockets_document_commenters_limit",
                config.sockets.document_commenters_max as f64,
            ),
            (
                "librepaper_sockets_document_editors_limit",
                config.sockets.document_editors_max as f64,
            ),
            (
                "librepaper_sockets_queue_bytes_limit",
                config.sockets.queue_bytes_max as f64,
            ),
            (
                "librepaper_storage_deployment_limit_bytes",
                config.storage.total as f64,
            ),
            (
                "librepaper_storage_owner_limit_bytes",
                config.storage.per_owner as f64,
            ),
        ];
        gauges.extend(config_gauges);

        if let (Some(used), Some(limit)) = (
            snapshot
                .pointer("/memory_budget/reserved_bytes")
                .and_then(json_number),
            snapshot
                .pointer("/memory_budget/limit_bytes")
                .and_then(json_number),
        ) {
            gauges.push((
                "librepaper_memory_budget_utilization_ratio",
                ratio(used, limit),
            ));
        }
        if let (Some(used), Some(limit)) = (
            snapshot
                .pointer("/pending_budget/retained_reserved_bytes")
                .and_then(json_number),
            snapshot
                .pointer("/pending_budget/retained_limit_bytes")
                .and_then(json_number),
        ) {
            gauges.push((
                "librepaper_pending_retained_utilization_ratio",
                ratio(used, limit),
            ));
        }
        if let (Some(used), Some(limit)) = (
            snapshot
                .pointer("/pending_budget/scratch_reserved_bytes")
                .and_then(json_number),
            snapshot
                .pointer("/pending_budget/scratch_limit_bytes")
                .and_then(json_number),
        ) {
            gauges.push((
                "librepaper_pending_scratch_utilization_ratio",
                ratio(used, limit),
            ));
        }
        if let Some(active) = snapshot.pointer("/sockets/active").and_then(json_number) {
            gauges.push((
                "librepaper_sockets_deployment_utilization_ratio",
                ratio(active, config.sockets.deployment_max as f64),
            ));
        }
        if let Some(queue_bytes) = snapshot
            .pointer("/sockets/queue_bytes")
            .and_then(json_number)
        {
            gauges.push((
                "librepaper_sockets_queue_bytes_utilization_ratio",
                ratio(queue_bytes, config.sockets.queue_bytes_max as f64),
            ));
        }
        if let Some(used) = snapshot
            .pointer("/storage_ledger/deployment_bytes")
            .and_then(json_number)
        {
            gauges.push((
                "librepaper_storage_deployment_utilization_ratio",
                ratio(used, config.storage.total as f64),
            ));
        }
        if let (Some(checked_out), Some(max_connections)) = (
            snapshot
                .pointer("/database/checked_out")
                .and_then(json_number),
            snapshot
                .pointer("/database/max_connections")
                .and_then(json_number),
        ) {
            gauges.push((
                "librepaper_database_connection_utilization_ratio",
                ratio(checked_out, max_connections),
            ));
        }

        self.snapshot_success.store(1, Ordering::Relaxed);
        self.snapshot_timestamp.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );
        *self
            .gauges
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = gauges;
    }

    fn note_snapshot_failure(&self) {
        self.snapshot_success.store(0, Ordering::Relaxed);
    }

    fn render(&self) -> String {
        let mut out = String::with_capacity(32 * 1024);
        out.push_str("# HELP librepaper_build_info Build version information.\n# TYPE librepaper_build_info gauge\n");
        out.push_str(&format!(
            "librepaper_build_info{{version=\"{}\"}} 1\n",
            env!("CARGO_PKG_VERSION")
        ));
        out.push_str("# HELP librepaper_process_uptime_seconds Time since this process started.\n# TYPE librepaper_process_uptime_seconds gauge\n");
        out.push_str(&format!(
            "librepaper_process_uptime_seconds {:.3}\n",
            self.started.elapsed().as_secs_f64()
        ));
        out.push_str("# HELP librepaper_metrics_snapshot_success Whether the latest aggregate sample completed.\n# TYPE librepaper_metrics_snapshot_success gauge\n");
        out.push_str(&format!(
            "librepaper_metrics_snapshot_success {}\n",
            self.snapshot_success.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP librepaper_metrics_snapshot_timestamp_seconds Unix timestamp of the last successful aggregate sample.\n# TYPE librepaper_metrics_snapshot_timestamp_seconds gauge\n");
        out.push_str(&format!(
            "librepaper_metrics_snapshot_timestamp_seconds {}\n",
            self.snapshot_timestamp.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP librepaper_http_requests_total Completed HTTP requests by bounded route, method, and response class.\n# TYPE librepaper_http_requests_total counter\n");
        for (route_index, route) in ROUTES.iter().enumerate() {
            for (method_index, method) in METHODS.iter().enumerate() {
                for (status_index, status_class) in STATUS_CLASSES.iter().enumerate() {
                    let index = ((route_index * METHODS.len() + method_index)
                        * STATUS_CLASSES.len())
                        + status_index;
                    let count = self.requests[index].load(Ordering::Relaxed);
                    out.push_str(&format!("librepaper_http_requests_total{{route=\"{route}\",method=\"{method}\",status_class=\"{status_class}\"}} {count}\n"));
                }
            }
        }

        out.push_str("# HELP librepaper_http_request_duration_seconds HTTP request handling time through response headers.\n# TYPE librepaper_http_request_duration_seconds histogram\n");
        for (route_index, route) in ROUTES.iter().enumerate() {
            for (method_index, method) in METHODS.iter().enumerate() {
                let index = route_index * METHODS.len() + method_index;
                let bucket_counts: [u64; HISTOGRAM_EDGES_SECONDS.len() + 1] =
                    std::array::from_fn(|bucket| {
                        self.histogram_buckets[index * (HISTOGRAM_EDGES_SECONDS.len() + 1) + bucket]
                            .load(Ordering::Relaxed)
                    });
                let mut cumulative = 0;
                for (edge, bucket_count) in HISTOGRAM_EDGES_SECONDS.iter().zip(bucket_counts.iter())
                {
                    cumulative += *bucket_count;
                    out.push_str(&format!("librepaper_http_request_duration_seconds_bucket{{route=\"{route}\",method=\"{method}\",le=\"{edge}\"}} {cumulative}\n"));
                }
                let count: u64 = bucket_counts.iter().sum();
                out.push_str(&format!("librepaper_http_request_duration_seconds_bucket{{route=\"{route}\",method=\"{method}\",le=\"+Inf\"}} {count}\n"));
                let sum =
                    self.histogram_sum_micros[index].load(Ordering::Relaxed) as f64 / 1_000_000.0;
                out.push_str(&format!("librepaper_http_request_duration_seconds_sum{{route=\"{route}\",method=\"{method}\"}} {sum:.6}\n"));
                out.push_str(&format!("librepaper_http_request_duration_seconds_count{{route=\"{route}\",method=\"{method}\"}} {count}\n"));
            }
        }

        out.push_str("# HELP librepaper_resource_refusals_total Requests refused by bounded deployment resource reason.\n# TYPE librepaper_resource_refusals_total counter\n");
        for (index, reason) in REFUSALS.iter().enumerate() {
            let count = self.refusals[index].load(Ordering::Relaxed);
            out.push_str(&format!(
                "librepaper_resource_refusals_total{{reason=\"{reason}\"}} {count}\n"
            ));
        }

        let gauges = self
            .gauges
            .read()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut described = HashSet::new();
        for (family, value) in gauges.iter() {
            if described.insert(*family) {
                let kind = if family.ends_with("_total") {
                    "counter"
                } else {
                    "gauge"
                };
                out.push_str(&format!("# HELP {family} Aggregate LibrePaper operational measurement.\n# TYPE {family} {kind}\n"));
            }
            out.push_str(&format!("{family} {value}\n"));
        }
        out
    }
}

fn json_number(value: &Value) -> Option<f64> {
    value
        .as_u64()
        .map(|n| n as f64)
        .or_else(|| value.as_i64().map(|n| n as f64))
        .or_else(|| value.as_f64())
}

fn ratio(used: f64, limit: f64) -> f64 {
    if limit > 0.0 {
        used / limit
    } else {
        0.0
    }
}

fn route_index(route: &str) -> usize {
    ROUTES
        .iter()
        .position(|known| *known == route)
        .unwrap_or(ROUTES.len() - 1)
}

fn method_index(method: &str) -> usize {
    METHODS
        .iter()
        .position(|known| *known == method)
        .unwrap_or(METHODS.len() - 1)
}

/// Map untrusted request paths to a small fixed set of route classes.
pub fn route_class(path: &str) -> &'static str {
    if path == "/" {
        "root"
    } else if path == "/health" {
        "health"
    } else if path == "/api/status" {
        "api_status"
    } else if path == "/api/config" {
        "api_config"
    } else if path.starts_with("/api/auth/") {
        "api_auth"
    } else if path.starts_with("/api/documents") {
        "api_documents"
    } else if path.starts_with("/api/comments") {
        "api_comments"
    } else if path.starts_with("/api/assets/") {
        "api_assets"
    } else if path.starts_with("/api/fonts/") {
        "api_fonts"
    } else if path.starts_with("/api/") {
        "api_other"
    } else if path.starts_with("/auth/") {
        "auth"
    } else if path.starts_with("/published/") {
        "published"
    } else if path.starts_with("/raw/") {
        "raw"
    } else if path.starts_with("/pdf/") {
        "pdf"
    } else {
        "other"
    }
}

/// Bind errors are returned to startup, where they are fatal when explicitly
/// configured. Empty or absent addresses leave metrics disabled.
pub async fn bind_from_environment() -> Result<Option<TcpListener>, String> {
    let Some(value) = std::env::var_os("LIBREPAPER_METRICS_ADDR") else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| "LIBREPAPER_METRICS_ADDR must be valid UTF-8".to_string())?;
    bind_address((!value.trim().is_empty()).then_some(value.trim())).await
}

async fn bind_address(value: Option<&str>) -> Result<Option<TcpListener>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let addr: SocketAddr = value.parse().map_err(|_| format!("LIBREPAPER_METRICS_ADDR must be an IP socket address such as 0.0.0.0:9091 (got {value:?})"))?;
    TcpListener::bind(addr)
        .await
        .map(Some)
        .map_err(|error| format!("could not bind metrics listener at {addr}: {error}"))
}

/// Run the listener separately from the user router and its origin/admission
/// middleware. Scrapes return cached gauges and are capped at two at a time.
pub async fn serve(listener: TcpListener, server: Arc<super::Server>) -> std::io::Result<()> {
    let app = Router::new()
        .route("/metrics", get(handle_metrics))
        .with_state(server);
    axum::serve(listener, app).await
}

async fn handle_metrics(State(server): State<Arc<super::Server>>) -> Response<Body> {
    let permit = match server.metrics_listener_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body(Body::empty())
                .unwrap()
        }
    };
    let metrics = server.metrics.clone();
    let body = tokio::time::timeout(SCRAPE_SAMPLE_TIMEOUT, async move {
        let _permit = permit;
        metrics.render()
    })
    .await;
    match body {
        Ok(body) => Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )
            .body(Body::from(body))
            .unwrap(),
        Err(_) => Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::empty())
            .unwrap(),
    }
}

/// Refresh aggregate gauges at a low fixed rate. The one-second timeout makes
/// a stuck status query harmless to the user-facing request path.
pub fn spawn_sampler(server: Arc<super::Server>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let snapshot =
                tokio::time::timeout(SCRAPE_SAMPLE_TIMEOUT, server.metrics_snapshot()).await;
            if let Ok(snapshot) = snapshot {
                server.metrics.update_gauges(&snapshot, &server.config);
            } else {
                server.metrics.note_snapshot_failure();
            }
        }
    });
}

/// Fixed allowlist from the aggregate cost snapshot. `field` may be a JSON
/// number only; nested/dynamic status data and identifiers are never exported.
const GAUGE_FIELDS: &[(&str, &str, &str)] = &[
    ("librepaper_rooms_documents", "rooms", "documents"),
    (
        "librepaper_rooms_buffered_batches",
        "rooms",
        "buffered_batches",
    ),
    ("librepaper_rooms_subscribers", "rooms", "subscribers"),
    ("librepaper_rooms_warm_caches", "rooms", "warm_caches"),
    ("librepaper_rooms_log_bytes", "rooms", "log_bytes"),
    ("librepaper_rooms_memory_used_bytes", "rooms", "memory_used"),
    (
        "librepaper_rooms_memory_limit_bytes",
        "rooms",
        "memory_limit",
    ),
    ("librepaper_sockets_active", "sockets", "active"),
    ("librepaper_sockets_queue_frames", "sockets", "queue_frames"),
    ("librepaper_sockets_queue_bytes", "sockets", "queue_bytes"),
    (
        "librepaper_sockets_max_network_sockets",
        "sockets",
        "max_network_sockets",
    ),
    (
        "librepaper_sockets_max_principal_sockets",
        "sockets",
        "max_principal_sockets",
    ),
    (
        "librepaper_sockets_max_document_sockets",
        "sockets",
        "max_document_sockets",
    ),
    (
        "librepaper_sockets_max_document_reader_sockets",
        "sockets",
        "max_document_reader_sockets",
    ),
    (
        "librepaper_sockets_max_document_commenter_sockets",
        "sockets",
        "max_document_commenter_sockets",
    ),
    (
        "librepaper_sockets_max_document_editor_sockets",
        "sockets",
        "max_document_editor_sockets",
    ),
    (
        "librepaper_memory_budget_limit_bytes",
        "memory_budget",
        "limit_bytes",
    ),
    (
        "librepaper_memory_budget_reserved_bytes",
        "memory_budget",
        "reserved_bytes",
    ),
    (
        "librepaper_memory_budget_evicted_caches_total",
        "memory_budget",
        "evicted_caches",
    ),
    (
        "librepaper_memory_budget_refused_busy_total",
        "memory_budget",
        "refused_busy",
    ),
    (
        "librepaper_pending_retained_limit_bytes",
        "pending_budget",
        "retained_limit_bytes",
    ),
    (
        "librepaper_pending_retained_reserved_bytes",
        "pending_budget",
        "retained_reserved_bytes",
    ),
    (
        "librepaper_pending_scratch_limit_bytes",
        "pending_budget",
        "scratch_limit_bytes",
    ),
    (
        "librepaper_pending_scratch_reserved_bytes",
        "pending_budget",
        "scratch_reserved_bytes",
    ),
    (
        "librepaper_pending_refused_retained_total",
        "pending_budget",
        "refused_retained",
    ),
    (
        "librepaper_pending_refused_scratch_total",
        "pending_budget",
        "refused_scratch",
    ),
    ("librepaper_http_work_active", "http_work", "active"),
    (
        "librepaper_http_control_work_active",
        "http_work",
        "control_active",
    ),
    (
        "librepaper_http_artifact_transfers_active",
        "http_work",
        "active_artifact_transfers",
    ),
    ("librepaper_background_queued", "background", "queued"),
    (
        "librepaper_background_queue_capacity",
        "background",
        "queue",
    ),
    (
        "librepaper_background_refused_wake_ups_total",
        "background",
        "refused_wake_ups",
    ),
    ("librepaper_background_deadlines", "background", "deadlines"),
    (
        "librepaper_background_deadline_capacity",
        "background",
        "deadline_capacity",
    ),
    (
        "librepaper_background_refused_deadlines_total",
        "background",
        "refused_deadlines",
    ),
    (
        "librepaper_database_max_connections",
        "database",
        "max_connections",
    ),
    (
        "librepaper_database_open_connections",
        "database",
        "open_connections",
    ),
    (
        "librepaper_database_idle_connections",
        "database",
        "idle_connections",
    ),
    (
        "librepaper_database_checked_out_connections",
        "database",
        "checked_out",
    ),
    (
        "librepaper_database_transactions_begun_total",
        "database",
        "transactions_begun",
    ),
    (
        "librepaper_database_transactions_failed_total",
        "database",
        "transactions_failed_to_begin",
    ),
    (
        "librepaper_database_begin_wait_mean_seconds",
        "database",
        "begin_wait_mean_us",
    ),
    (
        "librepaper_database_begin_wait_max_seconds",
        "database",
        "begin_wait_max_us",
    ),
    (
        "librepaper_storage_ledger_deployment_bytes",
        "storage_ledger",
        "deployment_bytes",
    ),
    ("librepaper_host_process_rss_bytes", "host", "rss_bytes"),
    (
        "librepaper_host_process_peak_rss_bytes",
        "host",
        "peak_rss_bytes",
    ),
    ("librepaper_host_memory_bytes", "host", "host_memory_bytes"),
    (
        "librepaper_host_available_memory_bytes",
        "host",
        "host_available_memory_bytes",
    ),
    (
        "librepaper_cgroup_memory_limit_bytes",
        "host",
        "cgroup_memory_limit_bytes",
    ),
    (
        "librepaper_cgroup_memory_used_bytes",
        "host",
        "cgroup_memory_used_bytes",
    ),
    (
        "librepaper_host_disk_read_bytes_total",
        "host",
        "disk_read_bytes",
    ),
    (
        "librepaper_host_disk_write_bytes_total",
        "host",
        "disk_write_bytes",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_classes_never_retain_dynamic_path_segments() {
        assert_eq!(
            route_class("/api/documents/private-slug-123"),
            "api_documents"
        );
        assert_eq!(route_class("/raw/secret-document/abc"), "raw");
        assert_eq!(route_class("/unknown/private"), "other");
    }

    #[test]
    fn exposition_uses_bounded_labels_and_prometheus_histogram_shape() {
        let metrics = Metrics::new();
        metrics.record_request(
            route_class("/api/documents/sensitive-id"),
            "GET",
            503,
            Duration::from_millis(12),
        );
        metrics.record_refusal("work_concurrency");
        metrics.update_gauges(
            &serde_json::json!({"rooms":{"documents":2,"private_id":8}}),
            &crate::config::Configuration::default(),
        );
        let text = metrics.render();
        assert!(text.contains("route=\"api_documents\",method=\"GET\",status_class=\"5xx\""));
        assert!(text.contains("le=\"0.005\"} 0\n"));
        assert!(text.contains("le=\"0.01\"} 0\n"));
        assert!(text.contains("le=\"0.025\"} 1\n"));
        assert!(text.contains("le=\"+Inf\"} 1\n"));
        assert!(text.contains("librepaper_http_request_duration_seconds_count{route=\"api_documents\",method=\"GET\"} 1"));
        assert!(text.contains("librepaper_resource_refusals_total{reason=\"work_concurrency\"} 1"));
        for reason in REFUSALS {
            assert!(text.contains(&format!(
                "librepaper_resource_refusals_total{{reason=\"{reason}\"}} "
            )));
        }
        assert!(!text.contains("sensitive-id"));
        assert!(!text.contains("private_id"));
        assert!(!text.contains("/api/documents"));
    }

    #[test]
    fn zero_series_are_exposed_at_startup_and_first_5xx_advances_them() {
        let metrics = Metrics::new();
        let before = metrics.render();
        let request_series = before
            .lines()
            .filter(|line| line.starts_with("librepaper_http_requests_total{"))
            .count();
        assert_eq!(
            request_series,
            ROUTES.len() * METHODS.len() * STATUS_CLASSES.len()
        );
        let histogram_bucket_series = before
            .lines()
            .filter(|line| line.starts_with("librepaper_http_request_duration_seconds_bucket{"))
            .count();
        assert_eq!(
            histogram_bucket_series,
            ROUTES.len() * METHODS.len() * (HISTOGRAM_EDGES_SECONDS.len() + 1)
        );
        assert!(before.contains(
            "librepaper_http_requests_total{route=\"health\",method=\"GET\",status_class=\"5xx\"} 0\n"
        ));
        assert!(before.contains(
            "librepaper_http_request_duration_seconds_count{route=\"health\",method=\"GET\"} 0\n"
        ));
        assert!(before.contains(
            "librepaper_http_request_duration_seconds_bucket{route=\"health\",method=\"GET\",le=\"+Inf\"} 0\n"
        ));

        metrics.record_request("health", "GET", 503, Duration::from_millis(12));
        let after = metrics.render();
        assert!(after.contains(
            "librepaper_http_requests_total{route=\"health\",method=\"GET\",status_class=\"5xx\"} 1\n"
        ));
        assert!(after.contains(
            "librepaper_http_request_duration_seconds_bucket{route=\"health\",method=\"GET\",le=\"0.025\"} 1\n"
        ));
        assert!(after.contains(
            "librepaper_http_request_duration_seconds_bucket{route=\"health\",method=\"GET\",le=\"+Inf\"} 1\n"
        ));
        assert!(after.contains(
            "librepaper_http_request_duration_seconds_count{route=\"health\",method=\"GET\"} 1\n"
        ));
    }

    #[test]
    fn middleware_result_records_an_early_rejection_and_exposition_has_unique_series() {
        let metrics = Metrics::new();
        let started = Instant::now();
        metrics.record_middleware_result("api_status", "GET", 421, started);
        metrics.update_gauges(
            &serde_json::json!({"rooms":{"documents":1}}),
            &crate::config::Configuration::default(),
        );
        let text = metrics.render();
        assert!(text.contains("route=\"api_status\",method=\"GET\",status_class=\"4xx\"} 1"));

        let mut series = HashSet::new();
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let identity = line.split_whitespace().next().unwrap();
            assert!(
                series.insert(identity),
                "duplicate metric sample: {identity}"
            );
        }
    }

    #[test]
    fn failed_sample_marks_export_stale_without_erasing_last_success_timestamp() {
        let metrics = Metrics::new();
        metrics.update_gauges(
            &serde_json::json!({"rooms":{"documents":1}}),
            &crate::config::Configuration::default(),
        );
        let timestamp = metrics.snapshot_timestamp.load(Ordering::Relaxed);
        assert_ne!(timestamp, 0);
        metrics.note_snapshot_failure();
        let text = metrics.render();
        assert!(text.contains("librepaper_metrics_snapshot_success 0\n"));
        assert!(text.contains(&format!(
            "librepaper_metrics_snapshot_timestamp_seconds {timestamp}\n"
        )));
    }

    #[test]
    fn concurrent_histogram_render_keeps_buckets_and_count_consistent() {
        let metrics = Arc::new(Metrics::new());
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let metrics = metrics.clone();
                std::thread::spawn(move || {
                    for sample in 0..2_000 {
                        let millis = ((sample + worker) % 30 + 1) as u64;
                        metrics.record_request("health", "GET", 200, Duration::from_millis(millis));
                    }
                })
            })
            .collect();

        for _ in 0..100 {
            let text = metrics.render();
            let mut cumulative = Vec::new();
            let mut count = None;
            let mut total = None;
            for line in text.lines() {
                if line.starts_with("librepaper_http_request_duration_seconds_bucket{route=\"health\",method=\"GET\",") {
                    let value = line.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap();
                    if line.contains("le=\"+Inf\"") {
                        total = Some(value);
                    } else {
                        cumulative.push(value);
                    }
                } else if line.starts_with("librepaper_http_request_duration_seconds_count{route=\"health\",method=\"GET\"}") {
                    count = Some(line.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap());
                }
            }
            assert!(cumulative.windows(2).all(|pair| pair[0] <= pair[1]));
            if let (Some(total), Some(count)) = (total, count) {
                assert_eq!(cumulative.last().copied(), Some(total));
                assert_eq!(total, count);
            }
        }
        for worker in workers {
            worker.join().unwrap();
        }
        let text = metrics.render();
        assert!(text.contains(
            "librepaper_http_request_duration_seconds_count{route=\"health\",method=\"GET\"} 8000"
        ));
    }

    #[test]
    fn only_allowlisted_snapshot_numbers_become_gauges() {
        let metrics = Metrics::new();
        metrics.update_gauges(
            &serde_json::json!({"rooms":{"documents":7,"account_id":"private","email":12}}),
            &crate::config::Configuration::default(),
        );
        let rendered = metrics.render();
        assert!(rendered.contains("librepaper_rooms_documents 7"));
        assert!(!rendered.contains("account_id"));
        assert!(!rendered.contains("email"));
    }

    #[test]
    fn database_waits_export_in_seconds_while_status_values_remain_microseconds() {
        let metrics = Metrics::new();
        let snapshot = serde_json::json!({
            "database": {
                "begin_wait_mean_us": 1_250_000,
                "begin_wait_max_us": 2_500_000,
            }
        });
        metrics.update_gauges(&snapshot, &crate::config::Configuration::default());
        let rendered = metrics.render();

        assert!(rendered.contains("librepaper_database_begin_wait_mean_seconds 1.25\n"));
        assert!(rendered.contains("librepaper_database_begin_wait_max_seconds 2.5\n"));
        assert!(!rendered.contains("librepaper_database_begin_wait_mean_microseconds"));
        assert!(!rendered.contains("librepaper_database_begin_wait_max_microseconds"));
        assert_eq!(snapshot["database"]["begin_wait_mean_us"], 1_250_000);
        assert_eq!(snapshot["database"]["begin_wait_max_us"], 2_500_000);
    }

    #[tokio::test]
    async fn metrics_listener_can_be_disabled_and_rejects_invalid_or_occupied_addresses() {
        assert!(bind_address(None).await.unwrap().is_none());
        assert!(bind_address(Some("not-an-ip:9091")).await.is_err());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        assert!(bind_address(Some(&address)).await.is_err());
    }
}
