//! Optional, bounded Prometheus metrics for private deployment monitoring.
//!
//! The listener is separate from the user-facing router. All labels are
//! selected from fixed enums, and gauges come from an explicit allowlist of
//! aggregate numeric status fields. No account, document, network, or path
//! identifier can enter the exposition.
//!
//! The body is OpenMetrics text produced by `prometheus-client`.

use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use prometheus_client::collector::Collector;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::{DescriptorEncoder, EncodeLabelSet, EncodeMetric};
use prometheus_client::metrics::counter::{ConstCounter, Counter};
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::{ConstGauge, Gauge};
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
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

const OPENMETRICS_CONTENT_TYPE: &str =
    "application/openmetrics-text; version=1.0.0; charset=utf-8";

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RequestLabels {
    route: &'static str,
    method: &'static str,
    status_class: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct DurationLabels {
    route: &'static str,
    method: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RefusalLabels {
    reason: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct BuildLabels {
    version: &'static str,
}

type DurationFamily = Family<DurationLabels, Histogram, fn() -> Histogram>;

fn new_duration_histogram() -> Histogram {
    Histogram::new(HISTOGRAM_EDGES_SECONDS.iter().copied())
}

/// The sampled gauges, exported only when present in the last good sample.
/// Names ending in `_total` are exported as counters, as before.
#[derive(Debug, Default)]
struct SampledGauges {
    values: RwLock<Vec<(&'static str, f64)>>,
}

impl Collector for SampledGauges {
    fn encode(&self, mut encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        let values = self
            .values
            .read()
            .unwrap_or_else(|poison| poison.into_inner());
        for (family, value) in values.iter() {
            const HELP: &str = "Aggregate LibrePaper operational measurement.";
            if let Some(name) = family.strip_suffix("_total") {
                let counter = ConstCounter::new(*value);
                let metric = encoder.encode_descriptor(name, HELP, None, counter.metric_type())?;
                counter.encode(metric)?;
            } else {
                let gauge = ConstGauge::new(*value);
                let metric =
                    encoder.encode_descriptor(family, HELP, None, gauge.metric_type())?;
                gauge.encode(metric)?;
            }
        }
        Ok(())
    }
}

/// Optional exporter state shared with the request admission middleware.
pub struct Metrics {
    started: Instant,
    registry: Registry,
    requests: Family<RequestLabels, Counter>,
    durations: DurationFamily,
    refusals: Family<RefusalLabels, Counter>,
    uptime: Gauge<f64, AtomicU64>,
    snapshot_success: Gauge<u64, AtomicU64>,
    snapshot_timestamp: Gauge<u64, AtomicU64>,
    samples: Arc<SampledGauges>,
}

impl Default for Metrics {
    fn default() -> Self {
        let mut registry = Registry::default();

        let build_info: Family<BuildLabels, Gauge> = Family::default();
        build_info
            .get_or_create(&BuildLabels {
                version: env!("LIBREPAPER_PKG_VERSION"),
            })
            .set(1);
        registry.register(
            "librepaper_build_info",
            "Build version information.",
            build_info,
        );

        let uptime = Gauge::<f64, AtomicU64>::default();
        registry.register(
            "librepaper_process_uptime_seconds",
            "Time since this process started.",
            uptime.clone(),
        );
        let snapshot_success = Gauge::<u64, AtomicU64>::default();
        registry.register(
            "librepaper_metrics_snapshot_success",
            "Whether the latest aggregate sample completed.",
            snapshot_success.clone(),
        );
        let snapshot_timestamp = Gauge::<u64, AtomicU64>::default();
        registry.register(
            "librepaper_metrics_snapshot_timestamp_seconds",
            "Unix timestamp of the last successful aggregate sample.",
            snapshot_timestamp.clone(),
        );

        // Every bounded series exists from startup so rate() sees a zero.
        let requests: Family<RequestLabels, Counter> = Family::default();
        let durations: DurationFamily =
            Family::new_with_constructor(new_duration_histogram as fn() -> Histogram);
        for &route in ROUTES {
            for &method in METHODS {
                durations.get_or_create(&DurationLabels { route, method });
                for &status_class in STATUS_CLASSES {
                    requests.get_or_create(&RequestLabels {
                        route,
                        method,
                        status_class,
                    });
                }
            }
        }
        registry.register(
            "librepaper_http_requests",
            "Completed HTTP requests by bounded route, method, and response class.",
            requests.clone(),
        );
        registry.register(
            "librepaper_http_request_duration_seconds",
            "HTTP request handling time through response headers.",
            durations.clone(),
        );

        let refusals: Family<RefusalLabels, Counter> = Family::default();
        for &reason in REFUSALS {
            refusals.get_or_create(&RefusalLabels { reason });
        }
        registry.register(
            "librepaper_resource_refusals",
            "Requests refused by bounded deployment resource reason.",
            refusals.clone(),
        );

        let samples = Arc::new(SampledGauges::default());
        registry.register_collector(Box::new(samples.clone()));

        Self {
            started: Instant::now(),
            registry,
            requests,
            durations,
            refusals,
            uptime,
            snapshot_success,
            snapshot_timestamp,
            samples,
        }
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(&self, route: &str, method: &str, status: u16, elapsed: Duration) {
        let route = ROUTES[route_index(route)];
        let method = METHODS[method_index(method)];
        let status_class =
            STATUS_CLASSES[usize::from(status / 100).saturating_sub(1).min(4)];
        self.requests
            .get_or_create(&RequestLabels {
                route,
                method,
                status_class,
            })
            .inc();
        self.durations
            .get_or_create(&DurationLabels { route, method })
            .observe(elapsed.as_secs_f64());
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
        if let Some(&reason) = REFUSALS.iter().find(|known| **known == reason) {
            self.refusals.get_or_create(&RefusalLabels { reason }).inc();
        }
    }

    /// Replace the cached gauges after a timed background sample. The caller
    /// supplies only values selected by `GAUGE_FIELDS` below.
    fn update_gauges(&self, snapshot: &Value, config: &librepaper_base::config::Configuration) {
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


        self.snapshot_success.set(1);
        self.snapshot_timestamp.set(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        *self
            .samples
            .values
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = gauges;
    }

    fn note_snapshot_failure(&self) {
        self.snapshot_success.set(0);
    }

    fn render(&self) -> String {
        self.uptime.set(self.started.elapsed().as_secs_f64());
        let mut out = String::with_capacity(32 * 1024);
        // Writing to a String cannot fail.
        let _ = encode(&mut out, &self.registry);
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
            .header(header::CONTENT_TYPE, OPENMETRICS_CONTENT_TYPE)
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
    use std::collections::HashSet;

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
            &librepaper_base::config::Configuration::default(),
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
            &librepaper_base::config::Configuration::default(),
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
            &librepaper_base::config::Configuration::default(),
        );
        let timestamp = metrics.snapshot_timestamp.get();
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
            &librepaper_base::config::Configuration::default(),
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
        metrics.update_gauges(
            &snapshot,
            &librepaper_base::config::Configuration::default(),
        );
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

    #[test]
    fn exposition_keeps_names_types_and_help_and_ends_with_eof() {
        let metrics = Metrics::new();
        metrics.update_gauges(
            &serde_json::json!({
                "rooms": {"documents": 3},
                "memory_budget": {"evicted_caches": 4},
            }),
            &librepaper_base::config::Configuration::default(),
        );
        let text = metrics.render();
        assert!(text.ends_with("# EOF\n"));
        for (name, kind) in [
            ("librepaper_build_info", "gauge"),
            ("librepaper_process_uptime_seconds", "gauge"),
            ("librepaper_metrics_snapshot_success", "gauge"),
            ("librepaper_metrics_snapshot_timestamp_seconds", "gauge"),
            ("librepaper_http_requests", "counter"),
            ("librepaper_http_request_duration_seconds", "histogram"),
            ("librepaper_resource_refusals", "counter"),
            ("librepaper_rooms_documents", "gauge"),
            ("librepaper_memory_budget_evicted_caches", "counter"),
        ] {
            assert!(
                text.contains(&format!("# TYPE {name} {kind}\n")),
                "missing TYPE for {name}"
            );
        }
        assert!(text.contains(&format!(
            "librepaper_build_info{{version=\"{}\"}} 1\n",
            env!("LIBREPAPER_PKG_VERSION")
        )));
        assert!(text.contains(
            "# HELP librepaper_http_requests Completed HTTP requests by bounded route, method, and response class.\n"
        ));
        assert!(text.contains("librepaper_memory_budget_evicted_caches_total 4"));
        assert!(!text.contains("librepaper_memory_budget_evicted_caches_total_total"));
        assert!(!text.contains("librepaper_http_requests_total_total"));
    }
}
