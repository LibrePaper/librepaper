//! Origin admission and concurrency control. Mirror traffic never enters here.

use super::*;
use axum::body::HttpBody;
pub struct CostMeter {
    config: Arc<Configuration>,
    requests: Option<governor::DefaultKeyedRateLimiter<String>>,
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
            requests: std::num::NonZeroU32::new(
                u32::try_from(config.cost.requests_per_principal_minute).unwrap_or(u32::MAX),
            )
            .map(|per_minute| {
                governor::RateLimiter::keyed(governor::Quota::per_minute(per_minute))
            }),
        }
    }

    fn admit_request(&self, network: &str, principal: &str) -> bool {
        let key = if !principal.is_empty() {
            format!("p:{principal}")
        } else {
            format!("n:{network}")
        };
        match &self.requests {
            Some(requests) => requests.check_key(&key).is_ok(),
            None => false,
        }
    }

    /// Drops the principals whose allowance has refilled. They are
    /// indistinguishable from ones never seen, so this only frees memory and
    /// never shuts anyone out. Called from the server's one-second tick.
    pub fn housekeep(&self) {
        if let Some(requests) = &self.requests {
            requests.retain_recent();
        }
    }
}

impl Server {
    /// Lightweight metrics sample. Keep this separate from `/api/status`:
    /// that operator endpoint also asks the catalogue for a storage usage
    /// total, which is not suitable for a periodic scrape task.
    pub(crate) async fn metrics_snapshot(&self) -> Value {
        let mut snapshot = json!({});
        snapshot["rooms"] = self.rooms.cost_snapshot().await;
        snapshot["sockets"] = self.socket_budget.snapshot();
        snapshot["memory_budget"] = self.rooms.registry().budget().snapshot();
        snapshot["pending_budget"] = self.rooms.registry().pending().snapshot();
        snapshot["storage_ledger"] = self.rooms.registry().ledger().snapshot();
        snapshot["http_work"] = json!({
            "active": self.config.cost.work_concurrency - self.cost.work.available_permits(),
            "control_active": 16 - self.cost.control_work.available_permits(),
            "active_artifact_transfers": self.config.cost.artifact_transfers - self.cost.transfers.available_permits(),
        });
        snapshot["background"] = self.background.snapshot();
        snapshot["host"] = tokio::task::spawn_blocking(super::host_metrics::snapshot)
            .await
            .unwrap_or(Value::Null);
        snapshot["database"] = self.store.catalog.pool_snapshot();
        snapshot
    }

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
        // The coarse half of the owner storage check: what each owner and the
        // deployment are charged for figures, archives and logs as this
        // process believes, rebuilt from the catalogue at every admission.
        snapshot["storage_ledger"] = self.rooms.registry().ledger().snapshot();
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
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Reply {
    let started = std::time::Instant::now();
    let route = super::metrics::route_class(request.uri().path());
    let method = request.method().as_str().to_owned();
    let response = middleware_inner(
        axum::extract::State(server.clone()),
        ConnectInfo(peer),
        request,
        next,
    )
    .await;
    server
        .metrics
        .record_middleware_result(route, &method, response.status().as_u16(), started);
    response
}

async fn middleware_inner(
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
        let Some(length) = request.body().size_hint().exact() else {
            return plain(411, "a mutation must declare its content length");
        };
        let reservation = length.saturating_mul(4);
        match server
            .rooms
            .registry()
            .budget()
            .reserve(
                reservation,
                librepaper_engine::log::sequencer::RESERVE_PATIENCE,
            )
            .await
        {
            Ok(r) => Some(r),
            Err(_) => {
                server.metrics.record_refusal("request_memory");
                return refusal("request_memory", "deployment");
            }
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
            _ => {
                server.metrics.record_refusal("work_concurrency");
                return refusal("work_concurrency", "deployment");
            }
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
    let authorization = origins::header(request.headers(), "authorization");
    let delegated_agent = authorization
        .as_deref()
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token.starts_with(librepaper_base::auth::AGENT_GRANT_PREFIX));
    if delegated_agent && authentication.is_ok() && !agent_document_path(&server, &path) {
        return write_json(
            403,
            &json!({"error": "agent token is limited to its document"}),
        );
    }
    if !server.cost.admit_request(&network, &identity.id) {
        server.metrics.record_refusal("request_budget");
        let scope = if identity.id.is_empty() {
            "network"
        } else {
            "principal"
        };
        return refusal("request_budget", scope);
    }
    let permit = if path.starts_with("/api/fonts/")
        || path.starts_with("/published/")
        || path.starts_with("/assets/")
        || (path.starts_with("/api/") && path.contains("/assets/"))
    {
        match server.cost.transfers.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                server.metrics.record_refusal("transfer_concurrency");
                return refusal("transfer_concurrency", "deployment");
            }
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

fn agent_document_path(server: &Server, path: &str) -> bool {
    agent_document_slug(path).is_some_and(|slug| server.valid_slug(slug))
}

fn agent_document_slug(path: &str) -> Option<&str> {
    let slug = path
        .strip_prefix("/api/documents/")
        .or_else(|| path.strip_prefix("/ws/"))
        .and_then(|rest| rest.split('/').next())?;
    (!slug.is_empty()).then_some(slug)
}

/// Metadata and range selection precede reading. Range and conditional
/// requests never fetch the bytes that will not be sent to this caller.
///
/// The bytes are whatever someone uploaded, so the answer is never a page:
/// octet-stream with no sniffing, a download if navigated to, and a sandbox
/// with no script and an opaque origin if a browser renders it anyway. A
/// `fetch` reads the body unaffected, which is how the reader takes figures.
pub(super) async fn blob_response(
    blobs: Arc<dyn librepaper_engine::storage::blob::BlobStore>,
    key: String,
    digest: &str,
    headers: &HeaderMap,
    head: bool,
) -> Reply {
    let length = match blobs.length(&key).await {
        Ok(length) => length,
        Err(librepaper_engine::storage::blob::BlobError::NotFound) => {
            return plain(404, "not found")
        }
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
    set(&mut response, "x-content-type-options", "nosniff");
    set(&mut response, "content-disposition", "attachment");
    set(&mut response, "content-security-policy", "sandbox");
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

#[cfg(test)]
mod blob_response_tests {
    use super::*;
    use librepaper_engine::storage::blob::{BlobStore, FsStore};

    /// Stored bytes are someone's upload. Whole or in part, they are never
    /// answered as something a browser would sniff, display or run.
    #[tokio::test]
    async fn stored_bytes_are_never_a_page() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(FsStore::new(directory.path(), false));
        store
            .put_new(
                "figure",
                b"<svg><script>alert(1)</script></svg>".to_vec(),
                "image/svg+xml",
            )
            .await
            .unwrap();
        let mut ranged = HeaderMap::new();
        ranged.insert(header::RANGE, "bytes=0-3".parse().unwrap());
        for (headers, status) in [
            (HeaderMap::new(), StatusCode::OK),
            (ranged, StatusCode::PARTIAL_CONTENT),
        ] {
            let response =
                blob_response(store.clone(), "figure".into(), "abc", &headers, false).await;
            assert_eq!(response.status(), status);
            for (name, value) in [
                ("content-type", "application/octet-stream"),
                ("x-content-type-options", "nosniff"),
                ("content-disposition", "attachment"),
                ("content-security-policy", "sandbox"),
            ] {
                assert_eq!(response.headers()[name], value, "{status}: {name}");
            }
        }
    }
}

#[cfg(test)]
mod agent_scope_tests {
    use super::agent_document_slug;

    #[test]
    fn delegated_credentials_are_confined_to_document_and_socket_routes() {
        assert_eq!(
            agent_document_slug("/api/documents/paper/tools"),
            Some("paper")
        );
        assert_eq!(agent_document_slug("/ws/paper"), Some("paper"));
        assert_eq!(agent_document_slug("/api/list"), None);
        assert_eq!(agent_document_slug("/api/account/erase"), None);
        assert_eq!(agent_document_slug("/api/documents"), None);
        assert_eq!(agent_document_slug("/api/documents/"), None);
    }
}
