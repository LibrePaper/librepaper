//! Origin admission and concurrency control. Mirror traffic never enters here.

use super::*;
use axum::body::HttpBody;
use std::sync::Mutex;

const MAX_KEYS: usize = 4096;

struct RequestBucket {
    tokens: f64,
    sampled: std::time::Instant,
}

#[derive(Default)]
struct Requests {
    keys: HashMap<String, RequestBucket>,
}

impl Requests {
    fn consume(&mut self, keys: &[(String, usize)]) -> bool {
        let now = std::time::Instant::now();
        if self.keys.len() + keys.len() > MAX_KEYS {
            self.keys
                .retain(|_, bucket| now.duration_since(bucket.sampled).as_secs() < 60);
        }
        let new_keys = keys
            .iter()
            .filter(|(key, _)| !self.keys.contains_key(key))
            .count();
        if self.keys.len() + new_keys > MAX_KEYS {
            return false;
        }
        for (key, limit) in keys {
            let bucket = self.keys.entry(key.clone()).or_insert(RequestBucket {
                tokens: *limit as f64,
                sampled: now,
            });
            bucket.tokens = (bucket.tokens
                + now.duration_since(bucket.sampled).as_secs_f64() * *limit as f64 / 60.0)
                .min(*limit as f64);
            bucket.sampled = now;
            if bucket.tokens < 1.0 {
                return false;
            }
        }
        for (key, _) in keys {
            self.keys.get_mut(key).unwrap().tokens -= 1.0;
        }
        true
    }
}

struct State {
    requests: Requests,
}

pub struct CostMeter {
    config: Arc<Configuration>,
    state: Mutex<State>,
    pub transfers: Arc<tokio::sync::Semaphore>,
    work: Arc<tokio::sync::Semaphore>,
    control_work: Arc<tokio::sync::Semaphore>,
}

impl CostMeter {
    // This meter is soft admission control, not a durable ledger: the
    // request-rate buckets and concurrency permits it tracks are rebuilt
    // from nothing on every restart, which is fine because none of it is
    // worth a query on a clock to save.
    pub fn new(config: &Arc<Configuration>) -> Self {
        Self {
            config: config.clone(),
            transfers: Arc::new(tokio::sync::Semaphore::new(config.cost.artifact_transfers)),
            work: Arc::new(tokio::sync::Semaphore::new(config.cost.work_concurrency)),
            control_work: Arc::new(tokio::sync::Semaphore::new(16)),
            state: Mutex::new(State {
                requests: Requests::default(),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn admit_request(&self, network: &str, principal: &str) -> bool {
        let key = if !principal.is_empty() {
            format!("p:{principal}")
        } else {
            format!("n:{network}")
        };
        self.state()
            .requests
            .consume(&[(key, self.config.cost.requests_per_principal_minute)])
    }
}

impl Server {
    /// Operator-only aggregate view. No raw account, document, or network keys.
    pub async fn cost_snapshot(&self) -> Value {
        let mut snapshot = json!({});
        snapshot["rooms"] = self.rooms.cost_snapshot().await;
        snapshot["sockets"] = self.socket_budget.snapshot();
        // §9.2's budget is the one resource that can refuse a read outright,
        // and until it was reported a deployment that thrashed -- every read
        // a rebuild because entries keep evicting one another -- looked from
        // outside exactly like one that never ran short.
        snapshot["memory_budget"] = self.rooms.registry().budget().snapshot();
        // The other half of §9.2's memory story, and the one a database
        // slowdown moves: unsaved source bytes, and the scratch that writes
        // them. Kept apart from `memory_budget` because they are different
        // pools with different rules -- decoded documents can be evicted to
        // make room and pending typing cannot (SPEC-frugal §2).
        snapshot["pending_budget"] = self.rooms.registry().pending().snapshot();
        snapshot["filesystem"] = self
            .store
            .blobs
            .capacity_snapshot()
            .await
            .unwrap_or(Value::Null);
        snapshot["http_work"] = json!({"active":self.config.cost.work_concurrency-self.cost.work.available_permits(),"control_active":16-self.cost.control_work.available_permits(),"active_artifact_transfers":self.config.cost.artifact_transfers-self.cost.transfers.available_permits()});
        // The worker refuses rather than grows when either of its bounds is
        // reached, and hands the work back to a durable rescan. That is
        // invisible from outside unless it is counted here.
        snapshot["background"] = self.background.snapshot();
        snapshot["host"] = tokio::task::spawn_blocking(super::host_metrics::snapshot)
            .await
            .unwrap_or(Value::Null);
        // The one pool, and the three waits kept apart: this is the wait for
        // a connection, `http_work` above is the wait for an admission slot,
        // and neither is the query itself. REVIEW-BIG-IDEAS.md §2.1 asked
        // whether the configured connections suffice under the configured
        // work concurrency; unmeasured, the honest answer was "nobody knows".
        snapshot["database"] = self.store.catalog.pool_snapshot();
        snapshot["storage"] = match self.store.catalog.usage_bytes(None).await {
            Ok(bytes) => json!({"retained_bytes":bytes}),
            Err(_) => Value::Null,
        };
        snapshot
    }
}

pub(super) fn refusal(reason: &str, scope: &str) -> Reply {
    let mut response = write_json(
        429,
        &json!({"error":"deployment resource allowance exhausted", "reason":reason, "scope":scope, "retryable":true, "retry_after":60}),
    );
    set(&mut response, "retry-after", "60");
    set(&mut response, "cache-control", "no-store");
    response
}

fn control(path: &str, method: &Method) -> bool {
    path == "/health"
        || path == "/api/status"
        || path == "/api/config"
        || path == "/api/me"
        || path.starts_with("/auth/")
        || path.starts_with("/api/auth/")
        || *method == Method::DELETE
}

fn loopback_origin(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => normalized_ip(IpAddr::V6(ip)).is_loopback(),
        None => false,
    }
}

fn direct_operator_request(peer: SocketAddr, headers: &HeaderMap) -> bool {
    normalized_ip(peer.ip()).is_loopback()
        && !["x-forwarded-for", "forwarded", "x-forwarded-host"]
            .iter()
            .any(|name| headers.contains_key(*name))
        && headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|host| loopback_origin(&format!("http://{host}")))
        && headers
            .get(header::ORIGIN)
            .is_none_or(|h| h.to_str().is_ok_and(loopback_origin))
        && headers
            .get("sec-fetch-site")
            .is_none_or(|h| h == "same-origin" || h == "none")
}

pub(super) async fn middleware(
    axum::extract::State(server): axum::extract::State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request<Body>,
    next: axum::middleware::Next,
) -> Reply {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    // Before anything is reserved or read: this deployment answers on the two
    // origins it was configured with and on loopback, and on nothing else. A
    // request addressed elsewhere is a proxy rewriting `Host`, a stray DNS
    // record, or a visitor naming a host we do not serve, and answering it on
    // a guess is what collapses the boundary between reader and document.
    let host = origins::header(request.headers(), "host").unwrap_or_default();
    let Some(arrival) = server.origins.resolve(&host) else {
        return plain(421, "this host is not served by this deployment");
    };
    // Mutations must declare their length, because without one the only bound
    // would be a ceiling, and there is no ceiling any more. Four times the
    // declared length is reserved from the memory budget -- the same budget
    // decoded documents live in -- and released when the request ends.
    let _incoming = if matches!(method, Method::POST | Method::PUT | Method::PATCH) {
        let Some(length) = request
            .body()
            .size_hint()
            .exact()
            .and_then(|n| u64::try_from(n).ok())
        else {
            return plain(411, "a mutation must declare its content length");
        };
        let reservation = length.saturating_mul(4);
        match server
            .rooms
            .registry()
            .budget()
            .reserve(reservation, crate::log::sequencer::RESERVE_PATIENCE)
            .await
        {
            Ok(r) => Some(r),
            Err(_) => return refusal("request_memory", "deployment"),
        }
    } else {
        None
    };
    let is_control = control(&path, &method);
    let _work = match server.cost.work.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => match is_control
            .then(|| server.cost.control_work.clone().try_acquire_owned())
            .transpose()
        {
            Ok(Some(permit)) => permit,
            _ => return refusal("work_concurrency", "deployment"),
        },
    };
    let network = client_network(&client_address(
        peer,
        request.headers(),
        &server.config.cost.trusted_proxies,
    ));
    // Authenticate before request-budget admission so the budget can be
    // keyed on the authenticated principal rather than only the network.
    let authentication = server
        .authenticated_identity(request.headers(), &arrival)
        .await;
    let identity = authentication.clone().unwrap_or_default();
    if !server.cost.admit_request(&network, &identity.id) {
        let scope = if identity.id.is_empty() {
            "network"
        } else {
            "principal"
        };
        return refusal("request_budget", scope);
    }
    let permit = if path.starts_with("/api/fonts/")
        || path.starts_with("/published/")
        || path.starts_with("/wasm/")
        || path.starts_with("/assets/")
        || (path.starts_with("/api/") && path.contains("/assets/"))
    {
        match server.cost.transfers.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return refusal("transfer_concurrency", "deployment"),
        }
    } else {
        None
    };
    request.extensions_mut().insert(RequestContext {
        arrival,
        peer,
        authentication: authentication.clone(),
    });
    let mut response = if path == "/api/status" {
        if !direct_operator_request(peer, request.headers()) {
            plain(403, "operator status requires a direct loopback connection")
        } else if method != Method::GET {
            plain(405, "method not allowed")
        } else {
            write_json(200, &server.cost_snapshot().await)
        }
    } else if path == "/health" {
        write_json(200, &json!({"ok":true}))
    } else {
        next.run(request).await
    };
    if path.starts_with("/api/") && !path.starts_with("/api/fonts/")
        || path.starts_with("/raw/")
        || path.starts_with("/pdf/")
        || path.starts_with("/auth/")
    {
        set(&mut response, "cache-control", "private, no-store");
    }
    match permit {
        Some(permit) => {
            let (parts, body) = response.into_parts();
            let stream = body.into_data_stream().map(move |item| {
                let _hold = &permit;
                item
            });
            Response::from_parts(parts, Body::from_stream(stream))
        }
        None => response,
    }
}

/// Metadata and range selection precede reading. Range and conditional
/// requests never fetch the bytes that will not be sent to this caller.
pub(super) async fn blob_response(
    blobs: Arc<dyn crate::storage::blob::BlobStore>,
    key: String,
    digest: &str,
    headers: &HeaderMap,
    head: bool,
) -> Reply {
    let length = match blobs.length(&key).await {
        Ok(length) => length,
        Err(crate::storage::blob::BlobError::NotFound) => return plain(404, "not found"),
        Err(_) => return plain(503, "storage temporarily unavailable"),
    };
    let etag = format!("\"{digest}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        })
    {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        set(&mut response, "etag", &etag);
        return response;
    }
    let range = headers.get(header::RANGE).and_then(|h| h.to_str().ok());
    let range = if headers
        .get(header::IF_RANGE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|value| value != etag)
    {
        None
    } else {
        range
    };
    let (start, end, partial) = match range {
        None => (0, length, false),
        Some(range) => match byte_range(range, length) {
            Some((start, end)) => (start, end, true),
            None => {
                let mut response = plain(416, "range not satisfiable");
                set(&mut response, "content-range", &format!("bytes */{length}"));
                return response;
            }
        },
    };
    let body = if head {
        Body::empty()
    } else {
        let stream = futures_util::stream::unfold(
            (blobs, key, start),
            move |(blobs, key, offset)| async move {
                if offset >= end {
                    return None;
                }
                let next = offset.saturating_add(1024 * 1024).min(end);
                match blobs.get_range(&key, offset..next).await {
                    Ok(bytes) => Some((Ok::<_, std::io::Error>(bytes), (blobs, key, next))),
                    Err(error) => Some((
                        Err(std::io::Error::other(error.to_string())),
                        (blobs, key, end),
                    )),
                }
            },
        );
        Body::from_stream(stream)
    };
    let mut response = Response::new(body);
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    set(&mut response, "content-type", "application/octet-stream");
    set(&mut response, "content-length", &(end - start).to_string());
    set(&mut response, "accept-ranges", "bytes");
    set(&mut response, "etag", &etag);
    set(&mut response, "cache-control", "private, no-store");
    if partial {
        set(
            &mut response,
            "content-range",
            &format!("bytes {start}-{}/{length}", end - 1),
        );
    }
    response
}

fn byte_range(value: &str, length: u64) -> Option<(u64, u64)> {
    if length == 0 {
        return None;
    }
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        return (suffix > 0).then_some((length.saturating_sub(suffix), length));
    }
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        length
    } else {
        end.parse::<u64>().ok()?.saturating_add(1).min(length)
    };
    (start < end && start < length).then_some((start, end))
}
