//! Origin admission and response-body accounting. Mirror traffic never enters here.

use super::*;
use axum::body::{Bytes, HttpBody};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;

const WINDOW: i64 = 86_400;
const MAX_KEYS: usize = 4096;
const CLASSES: usize = 8;
const CLASS_NAMES: [&str; CLASSES] = [
    "shell",
    "fonts",
    "source",
    "assets",
    "collaboration",
    "mutations",
    "authentication",
    "administration",
];

#[derive(Clone, Default, Serialize, Deserialize)]
struct Minute {
    expires: i64,
    ordinary: u64,
    emergency: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Durable {
    minutes: VecDeque<Minute>,
    sent: [u64; CLASSES],
    #[serde(default)]
    responses: [[u64; 6]; CLASSES],
    #[serde(default)]
    mutation_outcomes: [u64; 9],
    #[serde(default)]
    bytes_by_status: [[u64; 6]; CLASSES],
    #[serde(default)]
    policy: Value,
    #[serde(default)]
    mode_nanoseconds: [u64; 2],
    #[serde(default)]
    response_size_histogram: [[u64; 7]; CLASSES],
}

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
    durable: Durable,
    ordinary_reserved: u64,
    emergency_reserved: u64,
    requests: Requests,
    emergency_requests: Requests,
    unavailable: bool,
    limited: bool,
    mode_sampled_at: std::time::Instant,
    pressure_until: [i64; 7],
}

pub struct CostMeter {
    config: Arc<Configuration>,
    catalog: Option<Arc<crate::storage::postgres::PostgresCatalog>>,
    state: Mutex<State>,
    checkpoint_gate: tokio::sync::Mutex<()>,
    pub transfers: Arc<tokio::sync::Semaphore>,
    work: Arc<tokio::sync::Semaphore>,
    emergency_work: Arc<tokio::sync::Semaphore>,
    incoming_memory: Arc<tokio::sync::Semaphore>,
}

impl CostMeter {
    pub async fn restore(&self) -> Result<(), String> {
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let Some(value) = catalog
            .runtime_state("cost-meter-v2")
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let durable: Durable = serde_json::from_value(value.get("state").cloned().unwrap_or(value))
            .map_err(|error| format!("invalid transfer checkpoint: {error}"))?;
        let mut state = self.state();
        state.durable = durable;
        Self::expire(&mut state, now_unix());
        state.durable.policy = self.config.effective_policy();
        self.refresh_mode(&mut state);
        Ok(())
    }

    pub fn new(
        config: &Arc<Configuration>,
        catalog: Option<Arc<crate::storage::postgres::PostgresCatalog>>,
    ) -> Self {
        let durable = Durable::default();
        let unavailable = false;
        Self {
            config: config.clone(),
            catalog,
            checkpoint_gate: tokio::sync::Mutex::new(()),
            transfers: Arc::new(tokio::sync::Semaphore::new(config.cost.artifact_transfers)),
            work: Arc::new(tokio::sync::Semaphore::new(config.cost.work_concurrency)),
            emergency_work: Arc::new(tokio::sync::Semaphore::new(16)),
            incoming_memory: Arc::new(tokio::sync::Semaphore::new(
                config.cost.request_body_memory_bytes,
            )),
            state: Mutex::new(State {
                durable,
                ordinary_reserved: 0,
                emergency_reserved: 0,
                requests: Requests::default(),
                emergency_requests: Requests::default(),
                unavailable,
                limited: false,
                mode_sampled_at: std::time::Instant::now(),
                pressure_until: [0; 7],
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn expire(state: &mut State, now: i64) {
        while state
            .durable
            .minutes
            .front()
            .is_some_and(|minute| minute.expires <= now)
        {
            state.durable.minutes.pop_front();
        }
    }

    fn remaining(&self, state: &State, emergency: bool) -> u64 {
        if state.unavailable {
            return 0;
        }
        let limit = if emergency {
            Some(self.config.cost.emergency_bytes)
        } else {
            self.config.cost.transfer_bytes
        };
        let Some(limit) = limit else {
            return u64::MAX;
        };
        let used = state.durable.minutes.iter().fold(0_u64, |sum, m| {
            sum.saturating_add(if emergency { m.emergency } else { m.ordinary })
        });
        limit.saturating_sub(used).saturating_sub(if emergency {
            state.emergency_reserved
        } else {
            state.ordinary_reserved
        })
    }

    pub fn reserve(self: &Arc<Self>, bytes: u64, emergency: bool) -> Option<Reservation> {
        let mut state = self.state();
        Self::expire(&mut state, now_unix());
        if bytes > self.remaining(&state, emergency) {
            return None;
        }
        if emergency {
            state.emergency_reserved = state.emergency_reserved.saturating_add(bytes);
        } else {
            state.ordinary_reserved = state.ordinary_reserved.saturating_add(bytes);
        }
        Some(Reservation {
            meter: self.clone(),
            remaining: bytes,
            emergency,
        })
    }

    fn charge(
        &self,
        bytes: u64,
        emergency: bool,
        class: usize,
        reserved: bool,
        status_group: usize,
    ) -> u64 {
        let mut state = self.state();
        let now = now_unix();
        Self::expire(&mut state, now);
        let bytes = if reserved {
            bytes
        } else {
            bytes.min(self.remaining(&state, emergency))
        };
        if reserved {
            let held = if emergency {
                &mut state.emergency_reserved
            } else {
                &mut state.ordinary_reserved
            };
            *held = held.saturating_sub(bytes);
        }
        // Round expiry upward: at most 60 seconds conservative, never early.
        let expires = ((now.div_euclid(60) + 1) * 60 + WINDOW).max(
            state
                .durable
                .minutes
                .back()
                .map_or(0, |minute| minute.expires),
        );
        if state
            .durable
            .minutes
            .back()
            .is_none_or(|m| m.expires != expires)
        {
            state.durable.minutes.push_back(Minute {
                expires,
                ..Minute::default()
            });
        }
        if let Some(minute) = state.durable.minutes.back_mut() {
            let used = if emergency {
                &mut minute.emergency
            } else {
                &mut minute.ordinary
            };
            *used = used.saturating_add(bytes);
        }
        state.durable.sent[class] = state.durable.sent[class].saturating_add(bytes);
        state.durable.bytes_by_status[class][status_group] =
            state.durable.bytes_by_status[class][status_group].saturating_add(bytes);
        self.refresh_mode(&mut state);
        bytes
    }

    fn active_resources(&self, state: &State) -> Vec<&'static str> {
        let mut resources = Vec::new();
        if self.remaining(state, false) == 0 {
            resources.push("transfer");
        }
        for (index, name) in [
            "transfer",
            "requests",
            "work",
            "artifact_transfers",
            "sockets",
            "room_memory",
            "storage",
        ]
        .into_iter()
        .enumerate()
        {
            if state.pressure_until[index] > now_unix() && !resources.contains(&name) {
                resources.push(name);
            }
        }
        resources
    }

    pub(super) fn pressure(&self, index: usize) {
        let mut state = self.state();
        state.pressure_until[index] = now_unix() + 60;
        self.refresh_mode(&mut state);
    }

    fn refresh_mode(&self, state: &mut State) {
        let now = std::time::Instant::now();
        let elapsed = now
            .duration_since(state.mode_sampled_at)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        let index = usize::from(state.limited);
        state.durable.mode_nanoseconds[index] =
            state.durable.mode_nanoseconds[index].saturating_add(elapsed);
        state.mode_sampled_at = now;
        let resources = self.active_resources(state);
        let limited = !resources.is_empty();
        if limited != state.limited {
            state.limited = limited;
            println!(
                "{}",
                json!({"event":"cost_mode", "mode":if limited {"Limited"} else {"Normal"}, "resources":resources})
            );
        }
    }

    pub fn socket_bytes(&self, bytes: usize, durability: bool) -> bool {
        // WebSocket frames are indivisible; never emit a partial protocol message.
        let mut state = self.state();
        Self::expire(&mut state, now_unix());
        let emergency = durability && bytes as u64 > self.remaining(&state, false);
        if bytes as u64 > self.remaining(&state, emergency) {
            return false;
        }
        if emergency {
            state.emergency_reserved = state.emergency_reserved.saturating_add(bytes as u64);
        } else {
            state.ordinary_reserved = state.ordinary_reserved.saturating_add(bytes as u64);
        }
        drop(state);
        self.charge(bytes as u64, emergency, 4, true, 1);
        true
    }

    pub async fn checkpoint(&self) -> Result<(), String> {
        let _checkpoint = self.checkpoint_gate.lock().await;
        let Some(catalog) = &self.catalog else {
            return Ok(());
        };
        let saved = {
            let mut state = self.state();
            Self::expire(&mut state, now_unix());
            self.refresh_mode(&mut state);
            state.durable.policy = self.config.effective_policy();
            json!({"version":2,"state":state.durable})
        };
        catalog
            .set_runtime_state("cost-meter-v2", saved)
            .await
            .map_err(|error| error.to_string())
    }

    pub fn snapshot(&self) -> Value {
        let mut state = self.state();
        Self::expire(&mut state, now_unix());
        self.refresh_mode(&mut state);
        let limited = state.limited;
        let classes: serde_json::Map<String, Value> = CLASS_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| (name.to_string(), json!(state.durable.sent[i])))
            .collect();
        json!({"event":"cost_usage", "mode":if limited {"Limited"} else {"Normal"}, "resource":self.active_resources(&state).first(),"resources":self.active_resources(&state), "transfer_remaining":self.config.cost.transfer_bytes.map(|_| self.remaining(&state, false)), "emergency_remaining":self.remaining(&state, true), "response_bytes":classes, "mode_nanoseconds":state.durable.mode_nanoseconds, "response_size_histogram":state.durable.response_size_histogram, "response_size_upper_bounds_bytes":[1024,4096,16384,65536,1048576,16777216,null], "ordinary_reserved_bytes":state.ordinary_reserved,"emergency_reserved_bytes":state.emergency_reserved, "mutation_outcomes":state.durable.mutation_outcomes,"mutation_outcome_classes":["accepted","authentication","permission","size","requests","transfer","storage","work","other"], "responses_by_class_and_status_group":state.durable.responses, "bytes_by_class_and_status_group":state.durable.bytes_by_status, "policy":self.config.effective_policy()})
    }

    pub fn ordinary_available(&self) -> bool {
        let mut state = self.state();
        Self::expire(&mut state, now_unix());
        self.remaining(&state, false) > 0
    }

    fn admit_emergency_request(&self, network: &str) -> bool {
        self.state()
            .emergency_requests
            .consume(&[("global".into(), 300), (format!("n:{network}"), 60)])
    }

    fn admit_request(
        &self,
        network: &str,
        principal: &str,
        document: Option<&str>,
        class: usize,
    ) -> bool {
        let mut keys = vec![
            ("global".into(), self.config.cost.requests_per_minute),
            (
                format!("n:{network}"),
                self.config.cost.requests_per_network_minute,
            ),
            (format!("r:{class}"), self.config.cost.requests_per_minute),
        ];
        if !principal.is_empty() {
            keys.push((
                format!("p:{principal}"),
                self.config.cost.requests_per_principal_minute,
            ));
        }
        if let Some(document) = document {
            keys.push((
                format!("d:{document}"),
                self.config.cost.requests_per_document_minute,
            ));
        }
        self.state().requests.consume(&keys)
    }
}

pub struct Reservation {
    meter: Arc<CostMeter>,
    remaining: u64,
    emergency: bool,
}

#[derive(Clone)]
pub(super) struct Prepaid(Arc<Mutex<Option<Reservation>>>);

pub(super) fn attach_reservation(response: &mut Reply, reservation: Reservation) {
    response
        .extensions_mut()
        .insert(Prepaid(Arc::new(Mutex::new(Some(reservation)))));
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.meter.state();
        let held = if self.emergency {
            &mut state.emergency_reserved
        } else {
            &mut state.ordinary_reserved
        };
        *held = held.saturating_sub(self.remaining);
    }
}

impl Server {
    /// Operator-only aggregate view. No raw account, document, or network keys.
    pub async fn cost_snapshot(&self) -> Value {
        let mut snapshot = self.cost.snapshot();
        snapshot["policy"] = self.config.effective_policy();
        snapshot["rooms"] = self.rooms.cost_snapshot().await;
        snapshot["sockets"] = self.socket_budget.snapshot();
        snapshot["filesystem"] = self
            .store
            .blobs
            .capacity_snapshot()
            .await
            .unwrap_or(Value::Null);
        snapshot["http_work"] = json!({"active":self.config.cost.work_concurrency-self.cost.work.available_permits(),"emergency_active":16-self.cost.emergency_work.available_permits(),"request_body_reserved_bytes":self.config.cost.request_body_memory_bytes-self.cost.incoming_memory.available_permits(),"active_artifact_transfers":self.config.cost.artifact_transfers-self.cost.transfers.available_permits()});
        snapshot["host"] = tokio::task::spawn_blocking(super::host_metrics::snapshot)
            .await
            .unwrap_or(Value::Null);
        if let Some(catalog) = &self.store.catalog {
            snapshot["storage"] = match catalog.usage_bytes(None).await {
                Ok(bytes) => json!({"retained_bytes":bytes}),
                Err(_) => Value::Null,
            };
        }
        snapshot
    }
}

#[derive(Clone)]
struct Pressure(usize);

pub(super) fn refusal(reason: &str, scope: &str) -> Reply {
    let mut response = write_json(
        429,
        &json!({"error":"deployment resource allowance exhausted", "reason":reason, "scope":scope, "retryable":true, "retry_after":60}),
    );
    response.extensions_mut().insert(Pressure(match reason {
        "transfer_budget" => 0,
        "request_budget" => 1,
        "work_concurrency" => 2,
        "socket_budget" => 4,
        "room_memory" | "request_memory" => 5,
        "storage_budget" => 6,
        _ => 3,
    }));
    set(&mut response, "retry-after", "60");
    set(&mut response, "cache-control", "no-store");
    response
}

fn classify(path: &str, method: &Method) -> usize {
    if path.starts_with("/auth/")
        || path.starts_with("/api/auth/")
        || path == "/api/me"
        || path == "/api/config"
    {
        6
    } else if path.starts_with("/api/fonts/") {
        1
    } else if path.starts_with("/published/") {
        3
    } else if path.starts_with("/ws/") || path.contains("/agent/") {
        4
    } else if method != Method::GET && method != Method::HEAD {
        5
    } else if path.contains("/assets/") && path.starts_with("/api/") {
        3
    } else if path.starts_with("/api/documents") {
        2
    } else if path.starts_with("/api/") {
        7
    } else {
        0
    }
}

fn emergency(path: &str, method: &Method) -> bool {
    path == "/health"
        || path == "/api/status"
        || path == "/api/config"
        || path == "/api/me"
        || path.starts_with("/auth/")
        || path.starts_with("/api/auth/")
        || *method == Method::DELETE
        || path == "/api/account/erase"
        || path.ends_with("/quota")
        || path.ends_with("/quota-status")
        || path == "/api/account/storage"
        || path.ends_with("/export")
        || path.ends_with("/annotations")
        || path.ends_with("/links")
        || path.ends_with("/share")
        || path.ends_with("/transfer")
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

pub(super) async fn handle(
    axum::extract::State(server): axum::extract::State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Reply {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    let class = classify(&path, &method);
    let small_source_write = path.ends_with("/source")
        && matches!(method, Method::PUT | Method::PATCH)
        && request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|bytes| bytes <= 16 * 1024);
    let is_emergency = emergency(&path, &method) || small_source_write;
    // Reserve parsing and decoded-payload memory before reading a mutation.
    // Unknown-length bodies reserve the configured upload ceiling. The route
    // retains its own actual-byte ceiling, including multipart overhead.
    let _incoming = if matches!(method, Method::POST | Method::PUT | Method::PATCH) {
        let ceiling = server
            .config
            .max_document
            .saturating_add(server.config.max_assets.max(0) as usize)
            .saturating_add(super::MULTIPART_SLACK);
        let length = request
            .body()
            .size_hint()
            .exact()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(ceiling);
        // A gzip publication object can be tiny on the wire yet expand to a
        // 16 MiB HTML document or a 64 MiB asset. `stage_object` retains both
        // the encoded request body and its decoded buffer, so reserve that
        // bounded decoded peak before accepting the body.
        let compressed_publication_object = matches!(method, Method::PUT)
            && path.starts_with("/api/documents/")
            && path.contains("/publication/objects/")
            && request
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok())
                == Some("gzip");
        let reservation = if compressed_publication_object {
            length.saturating_add(crate::server::publication::MAX_ASSET_BYTES)
        } else {
            length.saturating_mul(4)
        };
        match u32::try_from(reservation).ok().and_then(|n| {
            server
                .cost
                .incoming_memory
                .clone()
                .try_acquire_many_owned(n)
                .ok()
        }) {
            Some(permit) => Some(permit),
            None => {
                return metered(
                    server.cost.clone(),
                    refusal("request_memory", "deployment"),
                    class,
                    true,
                    None,
                )
            }
        }
    } else {
        None
    };
    let _work = match server.cost.work.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => match is_emergency
            .then(|| server.cost.emergency_work.clone().try_acquire_owned())
            .transpose()
        {
            Ok(Some(permit)) => permit,
            _ => {
                return metered(
                    server.cost.clone(),
                    refusal("work_concurrency", "deployment"),
                    class,
                    true,
                    None,
                )
            }
        },
    };
    let parts = segments(&path);
    let document = match parts.as_slice() {
        ["api", "documents", slug, ..] | ["ws", slug] => Some(*slug),
        _ => None,
    };
    let network = client_network(&client_address(
        peer,
        request.headers(),
        &server.config.cost.trusted_proxies,
    ));
    // Keys stay internal and bounded; no identity becomes a metric label.
    let arrival = Arrival::from_peer(
        request.headers(),
        peer.ip(),
        &server.config.cost.trusted_proxies,
    );
    if !server.cost.admit_request(&network, "", document, class)
        && !(is_emergency && server.cost.admit_emergency_request(&network))
    {
        return metered(
            server.cost.clone(),
            refusal("request_budget", "deployment"),
            class,
            true,
            None,
        );
    }
    // Authenticate only after network/deployment admission bounds external work.
    let authentication = server
        .authenticated_identity(request.headers(), &arrival)
        .await;
    let identity = authentication.clone().unwrap_or_default();
    let principal_allowed = identity.id.is_empty() || {
        server.cost.state().requests.consume(&[(
            format!("p:{}", identity.id),
            server.config.cost.requests_per_principal_minute,
        )])
    };
    if !principal_allowed && !(is_emergency && server.cost.admit_emergency_request(&network)) {
        return metered(
            server.cost.clone(),
            refusal("request_budget", "principal"),
            class,
            true,
            None,
        );
    }
    if !is_emergency
        && !server.cost.ordinary_available()
        && (class == 4 || matches!(method, Method::POST | Method::PUT | Method::PATCH))
    {
        return metered(
            server.cost.clone(),
            refusal("transfer_budget", "deployment"),
            class,
            true,
            None,
        );
    }
    let publication_object_upload = matches!(method, Method::PUT)
        && path.starts_with("/api/documents/")
        && path.contains("/publication/objects/");
    let permit = if matches!(class, 1..=3)
        || publication_object_upload
        || path.starts_with("/wasm/")
        || path.starts_with("/assets/")
    {
        match server.cost.transfers.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return metered(
                    server.cost.clone(),
                    refusal("transfer_concurrency", "deployment"),
                    class,
                    true,
                    None,
                )
            }
        }
    } else {
        None
    };
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
        Server::with_request_authentication(
            authentication,
            dispatch(
                axum::extract::State(server.clone()),
                ConnectInfo(peer),
                request,
            ),
        )
        .await
    };
    if path.starts_with("/api/") && !path.starts_with("/api/fonts/")
        || path.starts_with("/raw/")
        || path.starts_with("/pdf/")
        || path.starts_with("/auth/")
    {
        set(&mut response, "cache-control", "private, no-store");
    }
    // A committed mutation must keep its real durability result when its
    // final acknowledgement crosses the remaining ordinary byte allowance.
    metered(
        server.cost.clone(),
        response,
        class,
        is_emergency || class == 5,
        permit,
    )
}

fn metered(
    meter: Arc<CostMeter>,
    response: Reply,
    class: usize,
    emergency: bool,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Reply {
    let (mut parts, body) = response.into_parts();
    if parts.status.as_u16() == 507 {
        meter.pressure(6);
    }
    if let Some(Pressure(index)) = parts.extensions.remove::<Pressure>() {
        meter.pressure(index);
    }
    if let Some(prepaid) = parts.extensions.remove::<Prepaid>() {
        let reservation = prepaid.0.lock().unwrap_or_else(|p| p.into_inner()).take();
        return wrap(meter, parts, body, reservation, false, class, permit);
    }
    let length = body.size_hint().exact().or_else(|| {
        parts
            .headers
            .get(header::CONTENT_LENGTH)?
            .to_str()
            .ok()?
            .parse()
            .ok()
    });
    if let Some(bytes) = length {
        let bucket = [1024, 4096, 16384, 65536, 1048576, 16777216]
            .iter()
            .position(|bound| bytes <= *bound)
            .unwrap_or(6);
        let mut state = meter.state();
        state.durable.response_size_histogram[class][bucket] =
            state.durable.response_size_histogram[class][bucket].saturating_add(1);
    }
    let mut reserve = length.and_then(|bytes| meter.reserve(bytes, false));
    let mut use_emergency = emergency && length.is_none();
    if length.is_some() && reserve.is_none() && emergency {
        reserve = length.and_then(|bytes| meter.reserve(bytes, true));
        use_emergency = true;
    }
    if length.is_some() && reserve.is_none() {
        // Refusal bodies are bounded too. If even the emergency reserve is
        // empty, a bodyless refusal cannot create an unmetered bypass.
        meter.pressure(0);
        let refused = refusal("transfer_budget", "deployment");
        let (refused_parts, refused_body) = refused.into_parts();
        let bytes = refused_body.size_hint().exact().unwrap_or(0);
        if let Some(held) = meter.reserve(bytes, true) {
            return wrap(
                meter,
                refused_parts,
                refused_body,
                Some(held),
                true,
                7,
                permit,
            );
        }
        return Response::from_parts(refused_parts, Body::empty());
    }
    if length.is_none() {
        parts.headers.remove(header::CONTENT_LENGTH);
    }
    wrap(meter, parts, body, reserve, use_emergency, class, permit)
}

/// Metadata and byte admission precede reading. Range and conditional requests
/// never fetch the bytes that will not be sent to this caller.
pub(super) async fn blob_response(
    meter: &Arc<CostMeter>,
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
    let reservation = if head {
        None
    } else {
        match meter.reserve(end - start, false) {
            Some(reservation) => Some(reservation),
            None => return refusal("transfer_budget", "deployment"),
        }
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
    if let Some(reservation) = reservation {
        attach_reservation(&mut response, reservation);
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

fn wrap(
    meter: Arc<CostMeter>,
    parts: axum::http::response::Parts,
    body: Body,
    reserve: Option<Reservation>,
    emergency: bool,
    class: usize,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Reply {
    {
        let mut state = meter.state();
        let count =
            &mut state.durable.responses[class][usize::from(parts.status.as_u16() / 100).min(5)];
        *count = count.saturating_add(1);
        if class == 5 {
            let reason = parts
                .extensions
                .get::<RefusalReason>()
                .map(|reason| reason.0.as_str())
                .unwrap_or("");
            let index = if parts.status.is_success() {
                0
            } else {
                match reason {
                    "authentication_required" => 1,
                    "permission_denied" => 2,
                    "size_limit" => 3,
                    "request_budget" => 4,
                    "transfer_budget" => 5,
                    "storage_budget" => 6,
                    "work_concurrency" | "artifact_concurrency" => 7,
                    _ => 8,
                }
            };
            state.durable.mutation_outcomes[index] =
                state.durable.mutation_outcomes[index].saturating_add(1);
        }
    }
    let status_group = usize::from(parts.status.as_u16() / 100).min(5);
    let stream = futures_util::stream::unfold(
        (body.into_data_stream(), reserve, permit, false),
        move |(mut stream, mut reserve, permit, done)| {
            let meter = meter.clone();
            async move {
                if done {
                    return None;
                }
                let item = stream.next().await?;
                let bytes = match item {
                    Ok(bytes) => bytes,
                    Err(error) => return Some((Err(error), (stream, reserve, permit, true))),
                };
                let sent = if let Some(held) = reserve.as_mut() {
                    let sent = meter.charge(
                        (bytes.len() as u64).min(held.remaining),
                        held.emergency,
                        class,
                        true,
                        status_group,
                    );
                    held.remaining -= sent;
                    sent
                } else {
                    let ordinary =
                        meter.charge(bytes.len() as u64, false, class, false, status_group);
                    if emergency && ordinary < bytes.len() as u64 {
                        ordinary
                            + meter.charge(
                                bytes.len() as u64 - ordinary,
                                true,
                                class,
                                false,
                                status_group,
                            )
                    } else {
                        ordinary
                    }
                } as usize;
                if sent == 0 && !bytes.is_empty() {
                    return None;
                }
                let done = sent < bytes.len();
                Some((
                    Ok::<Bytes, axum::Error>(bytes.slice(..sent)),
                    (stream, reserve, permit, done),
                ))
            }
        },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}
