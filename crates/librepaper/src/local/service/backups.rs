//! Persistent, account-scoped companion backups.
//!
//! Configuration is local to this companion and keyed by the authenticated
//! deployment origin plus account id. The only way to set its destination is
//! the native folder chooser; browser requests never supply a filesystem path.

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const FREQUENCIES: [u64; 5] = [1, 5, 15, 30, 60];
const SCHEDULER_POLL: Duration = Duration::from_secs(15);
const LOGIN_RECHECK_INTERVAL: Duration = Duration::from_secs(15);

fn valid_frequency(minutes: u64) -> bool {
    FREQUENCIES.contains(&minutes)
}

fn identity_error(identity: &Value, account_id: &str) -> Option<&'static str> {
    match identity.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
        None => Some("cached login is no longer valid; sign in again"),
        Some(id) if id != account_id => Some("cached login account does not match the requested account; sign in again with the intended account"),
        Some(_) => None,
    }
}

fn is_login_error(error: &str) -> bool {
    error.contains("not signed in")
        || error.contains("cached login")
        || error.contains("account does not match")
}

fn is_identity_error(error: &str) -> bool {
    is_login_error(error) || error.starts_with("could not verify account:")
}

fn identity_error_disables_schedule(error: &str) -> bool {
    error.contains("account does not match")
}

fn is_due(config: &BackupConfig, now: u64) -> bool {
    config.enabled
        && config.last_attempt.is_none_or(|attempt| {
            now.saturating_sub(attempt) >= config.frequency_minutes.saturating_mul(60)
        })
}

fn run_result_is_current(current: &BackupConfig, run: &BackupConfig) -> bool {
    current.enabled
        && current.run_generation == run.run_generation
        && current.destination == run.destination
        && current.origin == run.origin
        && current.account_id == run.account_id
}

fn destination_is_set(destination: &std::path::Path) -> bool {
    !destination.as_os_str().is_empty()
}

fn destination_label(destination: &std::path::Path) -> String {
    if !destination_is_set(destination) {
        return String::new();
    }
    destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Selected folder".into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BackupConfig {
    origin: String,
    account_id: String,
    enabled: bool,
    frequency_minutes: u64,
    destination: PathBuf,
    last_attempt: Option<u64>,
    last_success: Option<u64>,
    error: Option<String>,
    projects: usize,
    updated: usize,
    #[serde(default)]
    run_generation: u64,
    revision: u64,
    #[serde(skip)]
    running: bool,
}

impl BackupConfig {
    fn key(origin: &str, account_id: &str) -> String {
        format!(
            "{}\0{account_id}",
            crate::local::credentials::origin(origin)
        )
    }

    fn new(origin: &str, account_id: &str) -> Self {
        Self {
            origin: crate::local::credentials::origin(origin),
            account_id: account_id.to_owned(),
            enabled: false,
            frequency_minutes: 5,
            destination: PathBuf::new(),
            last_attempt: None,
            last_success: None,
            error: None,
            projects: 0,
            updated: 0,
            run_generation: 0,
            revision: 0,
            running: false,
        }
    }
}

#[derive(Default)]
struct BackupState {
    configs: HashMap<String, BackupConfig>,
    in_flight: HashSet<String>,
}

pub(super) struct BackupManager {
    path: PathBuf,
    state: Mutex<BackupState>,
    changed: Notify,
    auth_checks: std::sync::Mutex<HashMap<String, Instant>>,
}

impl BackupManager {
    pub(super) fn new(state_home: &std::path::Path) -> Self {
        let path = state_home
            .join("librepaper")
            .join("local")
            .join("backups.json");
        let mut state = BackupState::default();
        if let Ok(raw) = std::fs::read(&path) {
            if let Ok(configs) = serde_json::from_slice::<Vec<BackupConfig>>(&raw) {
                for mut config in configs {
                    config.running = false;
                    // A process may have stopped during an interval. Treat
                    // all enabled records as due so restart catches up.
                    if config.enabled {
                        config.last_attempt = None;
                    }
                    state.configs.insert(
                        BackupConfig::key(&config.origin, &config.account_id),
                        config,
                    );
                }
            }
        }
        Self {
            path,
            state: Mutex::new(state),
            changed: Notify::new(),
            auth_checks: std::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn persist(&self, state: &BackupState) -> Result<(), String> {
        let mut configs: Vec<_> = state.configs.values().cloned().collect();
        configs.sort_by(|a, b| {
            a.origin
                .cmp(&b.origin)
                .then_with(|| a.account_id.cmp(&b.account_id))
        });
        let bytes = serde_json::to_vec_pretty(&configs)
            .map_err(|error| format!("could not encode backup settings: {error}"))?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            crate::private_files::publish(&path, &bytes, &path.display().to_string())
                .map_err(|error| format!("could not save backup settings: {error}"))
        })
        .await
        .map_err(|error| format!("could not save backup settings: {error}"))??;
        Ok(())
    }

    async fn config(&self, origin: &str, account_id: &str) -> BackupConfig {
        let key = BackupConfig::key(origin, account_id);
        let mut state = self.state.lock().await;
        state
            .configs
            .entry(key)
            .or_insert_with(|| BackupConfig::new(origin, account_id))
            .clone()
    }

    async fn status(&self, inner: &Inner, origin: &str, account_id: &str) -> Value {
        let key = BackupConfig::key(origin, account_id);
        let mut config = self.config(origin, account_id).await;
        let token_missing = crate::cli::stored_token_at(&inner.state_home, origin).is_empty();
        if !token_missing
            && config.error.as_deref().is_some_and(is_identity_error)
            && self.should_recheck_login(&key)
        {
            let verification = verify_account(inner, origin, account_id).await;
            let refreshed_error = verification.err();
            let mut state = self.state.lock().await;
            if let Some(current) = state.configs.get_mut(&key) {
                if current.error == config.error
                    && current.error.as_deref().is_some_and(is_identity_error)
                {
                    let previous = std::mem::replace(&mut current.error, refreshed_error);
                    if let Err(error) = self.persist(&state).await {
                        if let Some(current) = state.configs.get_mut(&key) {
                            current.error = previous;
                        }
                        if let Some(current) = state.configs.get_mut(&key) {
                            current.error = Some(error);
                        }
                    }
                }
            }
            if let Some(current) = state.configs.get(&key) {
                config = current.clone();
            }
        }
        json!({
            "enabled": config.enabled,
            "frequency_minutes": config.frequency_minutes,
            "destination": destination_label(&config.destination),
            "destination_set": destination_is_set(&config.destination),
            "running": config.running,
            "last_success": config.last_success,
            "error": config.error,
            "projects": config.projects,
            "updated": config.updated,
            "needs_login": token_missing || config.error.as_deref().is_some_and(is_login_error),
        })
    }

    fn should_recheck_login(&self, key: &str) -> bool {
        let now = Instant::now();
        let Ok(mut checks) = self.auth_checks.lock() else {
            return false;
        };
        if checks
            .get(key)
            .is_some_and(|checked| now.duration_since(*checked) < LOGIN_RECHECK_INTERVAL)
        {
            return false;
        }
        checks.insert(key.to_owned(), now);
        true
    }

    #[allow(clippy::result_large_err)] // The error is an HTTP response, as in the other service handlers.
    async fn choose_destination(
        &self,
        inner: &Inner,
        headers: &HeaderMap,
        origin: &str,
        account_id: &str,
    ) -> Result<(), Reply> {
        let Ok(_dialog) = inner.folder_dialog.try_lock() else {
            return Err(write_json(
                409,
                &json!({"error":"A folder chooser is already open on this computer."}),
            ));
        };
        let destination = super::super::folder::choose_directory(origin, "backup destination")
            .await
            .map_err(|error| write_json(400, &json!({"error": error})))?;
        let destination = tokio::task::spawn_blocking(move || destination.canonicalize())
            .await
            .map_err(|error| {
                write_json(
                    500,
                    &json!({"error": format!("could not resolve selected folder: {error}")}),
                )
            })?
            .map_err(|error| {
                write_json(
                    400,
                    &json!({"error": format!("could not resolve selected folder: {error}")}),
                )
            })?;
        if !destination.is_dir() {
            return Err(write_json(
                400,
                &json!({"error":"Select an existing backup destination."}),
            ));
        }
        if authenticate(inner, headers, Some(origin)).is_err() {
            return Err(write_json(
                401,
                &json!({"error":"This site is no longer connected."}),
            ));
        }
        let key = BackupConfig::key(origin, account_id);
        let mut state = self.state.lock().await;
        if state.in_flight.contains(&key) {
            return Err(write_json(
                409,
                &json!({"error":"A backup is running; choose a new destination when it finishes."}),
            ));
        }
        let previous = state.configs.get(&key).cloned();
        let config = state
            .configs
            .entry(key.clone())
            .or_insert_with(|| BackupConfig::new(origin, account_id));
        config.destination = destination;
        config.run_generation = config.run_generation.wrapping_add(1);
        config.last_attempt = None;
        config.last_success = None;
        config.projects = 0;
        config.updated = 0;
        config.error = None;
        config.revision = config.revision.wrapping_add(1);
        if let Err(error) = self.persist(&state).await {
            match previous {
                Some(previous) => {
                    state.configs.insert(key.clone(), previous);
                }
                None => {
                    state.configs.remove(&key);
                    let mut config = BackupConfig::new(origin, account_id);
                    config.error = Some(error.clone());
                    state.configs.insert(key.clone(), config);
                }
            }
            if let Some(config) = state.configs.get_mut(&key) {
                config.error = Some(error.clone());
            }
            return Err(write_json(500, &json!({"error": error})));
        }
        self.changed.notify_one();
        Ok(())
    }

    async fn configure(
        self: &Arc<Self>,
        inner: &Inner,
        origin: &str,
        account_id: &str,
        enabled: bool,
        frequency_minutes: u64,
    ) -> Result<(), String> {
        if !valid_frequency(frequency_minutes) {
            return Err("frequency_minutes must be one of 1, 5, 15, 30, or 60".into());
        }
        let key = BackupConfig::key(origin, account_id);
        let state = self.state.lock().await;
        if enabled
            && !state
                .configs
                .get(&key)
                .is_some_and(|config| config.destination.is_dir())
        {
            return Err("Choose a backup destination before enabling backups.".into());
        }
        // Never hold the status/config lock over a network request. Apart
        // from keeping GETs responsive, re-check the destination below in
        // case it changed while identity verification was in flight.
        let expected_revision = state.configs.get(&key).map_or(0, |config| config.revision);
        drop(state);
        if enabled {
            if let Err(error) = verify_account(inner, origin, account_id).await {
                let mut state = self.state.lock().await;
                if state.configs.get(&key).map_or(0, |config| config.revision) != expected_revision
                {
                    return Err(
                        "backup settings changed during account verification; retry the request"
                            .into(),
                    );
                }
                let config = state
                    .configs
                    .entry(key.clone())
                    .or_insert_with(|| BackupConfig::new(origin, account_id));
                config.error = Some(error.clone());
                if identity_error_disables_schedule(&error)
                    || !inner.pairing.has_live_pairing(origin)
                {
                    if config.enabled {
                        config.revision = config.revision.wrapping_add(1);
                        config.run_generation = config.run_generation.wrapping_add(1);
                    }
                    config.enabled = false;
                }
                let error = match self.persist(&state).await {
                    Ok(()) => error,
                    Err(persist_error) => {
                        format!("{error}; could not save backup settings: {persist_error}")
                    }
                };
                if let Some(config) = state.configs.get_mut(&key) {
                    config.error = Some(error.clone());
                }
                return Err(error);
            }
        }
        let mut state = self.state.lock().await;
        if state.configs.get(&key).map_or(0, |config| config.revision) != expected_revision {
            return Err(
                "backup settings changed during account verification; retry the request".into(),
            );
        }
        if enabled
            && !state
                .configs
                .get(&key)
                .is_some_and(|config| config.destination.is_dir())
        {
            return Err("Choose a backup destination before enabling backups.".into());
        }
        let previous = state.configs.get(&key).cloned();
        let config = state
            .configs
            .entry(key.clone())
            .or_insert_with(|| BackupConfig::new(origin, account_id));
        let previous_enabled = config.enabled;
        let previous_frequency = config.frequency_minutes;
        config.enabled = enabled;
        config.frequency_minutes = frequency_minutes;
        config.error = None;
        if previous_enabled != enabled || previous_frequency != frequency_minutes {
            config.revision = config.revision.wrapping_add(1);
        }
        if previous_enabled != enabled {
            config.run_generation = config.run_generation.wrapping_add(1);
        }
        if enabled && !previous_enabled {
            // Enabling always queues an immediate initial save.
            config.last_attempt = None;
        }
        if let Err(error) = self.persist(&state).await {
            match previous {
                Some(previous) => {
                    state.configs.insert(key.clone(), previous);
                }
                None => {
                    state.configs.remove(&key);
                    let mut config = BackupConfig::new(origin, account_id);
                    config.error = Some(error.clone());
                    state.configs.insert(key.clone(), config);
                }
            }
            if let Some(config) = state.configs.get_mut(&key) {
                config.error = Some(error.clone());
            }
            return Err(error);
        }
        self.changed.notify_one();
        Ok(())
    }

    async fn admit_run(&self, origin: &str, account_id: &str) -> Result<BackupConfig, String> {
        let key = BackupConfig::key(origin, account_id);
        let mut state = self.state.lock().await;
        let Some(config) = state.configs.get(&key).cloned() else {
            return Err("Configure a backup destination first.".into());
        };
        if !config.enabled {
            return Err("Backups are disabled.".into());
        }
        if state.in_flight.contains(&key) {
            return Err("A backup is already running.".into());
        }
        let previous = config.clone();
        state.in_flight.insert(key.clone());
        if let Some(current) = state.configs.get_mut(&key) {
            current.running = true;
            current.last_attempt = Some(unix_now());
            current.error = None;
        }
        if let Err(error) = self.persist(&state).await {
            state.in_flight.remove(&key);
            state.configs.insert(key.clone(), previous);
            if let Some(current) = state.configs.get_mut(&key) {
                current.error = Some(error.clone());
            }
            return Err(error);
        }
        Ok(config)
    }

    fn start_run(self: &Arc<Self>, inner: Arc<Inner>, config: BackupConfig) {
        let manager = self.clone();
        tokio::spawn(async move {
            manager.finish_run(&inner, config).await;
        });
    }

    async fn finish_run(&self, inner: &Inner, config: BackupConfig) {
        let key = BackupConfig::key(&config.origin, &config.account_id);
        let result = run_backup(inner, &config).await;
        let mut state = self.state.lock().await;
        state.in_flight.remove(&key);
        if let Some(current) = state.configs.get_mut(&key) {
            current.running = false;
            if run_result_is_current(current, &config) {
                match result {
                    Ok(report) => {
                        current.last_success = Some(unix_now());
                        current.projects = report.projects;
                        current.updated = report.updated;
                        current.error = None;
                    }
                    Err(error) => {
                        current.error = Some(error);
                        // Identity mismatch or revoked pairing permanently stops
                        // scheduled runs until the user explicitly enables again.
                        if !inner.pairing.has_live_pairing(&current.origin)
                            || current
                                .error
                                .as_deref()
                                .is_some_and(identity_error_disables_schedule)
                        {
                            if current.enabled {
                                current.revision = current.revision.wrapping_add(1);
                                current.run_generation = current.run_generation.wrapping_add(1);
                            }
                            current.enabled = false;
                        }
                    }
                }
            }
        }
        if let Err(error) = self.persist(&state).await {
            if let Some(current) = state.configs.get_mut(&key) {
                current.error = Some(error);
            }
        }
        self.changed.notify_one();
    }

    async fn run_now(
        self: &Arc<Self>,
        inner: &Arc<Inner>,
        origin: &str,
        account_id: &str,
    ) -> Result<(), String> {
        let config = self.admit_run(origin, account_id).await?;
        self.start_run(inner.clone(), config);
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountBody {
    account_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigureBody {
    account_id: String,
    enabled: bool,
    frequency_minutes: u64,
}

fn account_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

fn query_account(request: &Request<Body>) -> Option<String> {
    request
        .uri()
        .query()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .find(|(key, _)| key == "account")
        .map(|(_, value)| value.into_owned())
        .filter(|value| account_id(value))
}

pub(super) async fn handle_get(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Some(account_id) = query_account(&request) else {
        return write_json(400, &json!({"error":"backups needs a valid account query"}));
    };
    let Some(origin) = origin else {
        return write_json(
            403,
            &json!({"error":"backups require a paired site origin"}),
        );
    };
    write_json(200, &inner.backups.status(inner, origin, &account_id).await)
}

pub(super) async fn handle_folder(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Some(origin) = origin else {
        return write_json(
            403,
            &json!({"error":"backups require a paired site origin"}),
        );
    };
    let body = match read_json_body::<AccountBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !account_id(&body.account_id) {
        return write_json(400, &json!({"error":"backups need a valid account id"}));
    }
    match inner
        .backups
        .choose_destination(inner, headers, origin, &body.account_id)
        .await
    {
        Ok(()) => write_json(
            200,
            &inner.backups.status(inner, origin, &body.account_id).await,
        ),
        Err(response) => response,
    }
}

pub(super) async fn handle_put(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Some(origin) = origin else {
        return write_json(
            403,
            &json!({"error":"backups require a paired site origin"}),
        );
    };
    let body = match read_json_body::<ConfigureBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !account_id(&body.account_id) {
        return write_json(400, &json!({"error":"backups need a valid account id"}));
    }
    match inner
        .backups
        .configure(
            inner,
            origin,
            &body.account_id,
            body.enabled,
            body.frequency_minutes,
        )
        .await
    {
        Ok(()) => write_json(
            200,
            &inner.backups.status(inner, origin, &body.account_id).await,
        ),
        Err(error) => {
            let needs_login = is_login_error(&error);
            write_json(400, &json!({"error": error, "needs_login": needs_login}))
        }
    }
}

pub(super) async fn handle_run(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Some(origin) = origin else {
        return write_json(
            403,
            &json!({"error":"backups require a paired site origin"}),
        );
    };
    let body = match read_json_body::<AccountBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !account_id(&body.account_id) {
        return write_json(400, &json!({"error":"backups need a valid account id"}));
    }
    match inner.backups.run_now(inner, origin, &body.account_id).await {
        Ok(()) => write_json(
            200,
            &inner.backups.status(inner, origin, &body.account_id).await,
        ),
        Err(error) => write_json(409, &json!({"error": error})),
    }
}

async fn verify_account(inner: &Inner, origin: &str, account_id: &str) -> Result<String, String> {
    if !inner.pairing.has_live_pairing(origin) {
        return Err("This site is no longer connected.".into());
    }
    let token = crate::cli::stored_token_at(&inner.state_home, origin);
    if token.is_empty() {
        return Err(format!(
            "not signed in. Run: librepaper login --server {origin}"
        ));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("could not verify account: {error}"))?;
    let response = client
        .get(format!("{}/api/me", origin.trim_end_matches('/')))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|error| format!("could not verify account: {error}"))?;
    if response.status().is_redirection() {
        return Err("could not verify account: the server redirected the identity check".into());
    }
    if !response.status().is_success() {
        if matches!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            return Err("could not verify account: the cached login is no longer valid".into());
        }
        return Err(format!(
            "could not verify account: identity server returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let identity: Value = response
        .json()
        .await
        .map_err(|error| format!("could not verify account: {error}"))?;
    if let Some(error) = identity_error(&identity, account_id) {
        return Err(error.into());
    }
    if !inner.pairing.has_live_pairing(origin) {
        return Err("This site is no longer connected.".into());
    }
    Ok(token)
}

async fn run_backup(
    inner: &Inner,
    config: &BackupConfig,
) -> Result<crate::local::backup::BackupReport, String> {
    let token = verify_account(inner, &config.origin, &config.account_id).await?;
    if !inner.pairing.has_live_pairing(&config.origin) {
        return Err("This site is no longer connected.".into());
    }
    crate::local::backup::run_backup(
        &config.origin,
        &token,
        &config.account_id,
        &config.destination,
    )
    .await
}

pub(super) fn spawn_scheduler(inner: Arc<Inner>) {
    tokio::spawn(async move {
        loop {
            let due = {
                let mut state = inner.backups.state.lock().await;
                let now = unix_now();
                let mut changed = false;
                let candidates: Vec<_> = state
                    .configs
                    .iter()
                    .filter(|(_, config)| config.enabled)
                    .map(|(key, config)| (key.clone(), config.clone()))
                    .collect();
                let mut due = Vec::new();
                for (key, config) in candidates {
                    if !inner.pairing.has_live_pairing(&config.origin) {
                        if let Some(current) = state.configs.get_mut(&key) {
                            current.enabled = false;
                            current.error = Some("This site is no longer connected.".into());
                            current.revision = current.revision.wrapping_add(1);
                            current.run_generation = current.run_generation.wrapping_add(1);
                            changed = true;
                        }
                    } else if !state.in_flight.contains(&key) && is_due(&config, now) {
                        if !config.destination.is_dir() {
                            if let Some(current) = state.configs.get_mut(&key) {
                                current.last_attempt = Some(now);
                                current.error =
                                    Some("The selected backup destination is unavailable.".into());
                            }
                            changed = true;
                            continue;
                        }
                        state.in_flight.insert(key);
                        if let Some(current) = state
                            .configs
                            .get_mut(&BackupConfig::key(&config.origin, &config.account_id))
                        {
                            current.running = true;
                            current.last_attempt = Some(now);
                        }
                        changed = true;
                        due.push(config);
                    }
                }
                if changed {
                    if let Err(error) = inner.backups.persist(&state).await {
                        for config in state.configs.values_mut().filter(|config| config.running) {
                            config.error = Some(error.clone());
                        }
                    }
                }
                due
            };
            for config in due {
                inner.backups.start_run(inner.clone(), config);
            }
            tokio::select! {
                _ = tokio::time::sleep(SCHEDULER_POLL) => {},
                _ = inner.backups.changed.notified() => {},
            }
        }
    });
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path as AxumPath, State};
    use axum::routing::get;
    use axum::{Json, Router};
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct IdentityServerFixture {
        account_id: std::sync::Mutex<String>,
        available: AtomicBool,
    }

    async fn fixture_identity_status(
        State(fixture): State<Arc<IdentityServerFixture>>,
    ) -> (axum::http::StatusCode, Json<Value>) {
        if fixture.available.load(Ordering::SeqCst) {
            (
                axum::http::StatusCode::OK,
                Json(json!({
                    "id": fixture.account_id.lock().unwrap().clone()
                })),
            )
        } else {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "temporarily unavailable"})),
            )
        }
    }

    #[test]
    fn identity_must_match_the_requested_signed_in_account() {
        let account = uuid::Uuid::now_v7().to_string();
        assert!(identity_error(&json!({"id": ""}), &account)
            .unwrap()
            .contains("no longer valid"));
        assert!(identity_error(&json!({"handle": "alice"}), &account)
            .unwrap()
            .contains("no longer valid"));
        assert!(
            identity_error(&json!({"id": uuid::Uuid::now_v7().to_string()}), &account)
                .unwrap()
                .contains("does not match")
        );
        assert_eq!(
            identity_error(&json!({"id": account.clone()}), &account),
            None
        );
        assert!(!identity_error_disables_schedule(
            identity_error(&json!({"id": ""}), &account).unwrap()
        ));
        assert!(identity_error_disables_schedule(
            identity_error(&json!({"id": uuid::Uuid::now_v7().to_string()}), &account).unwrap()
        ));
    }

    #[test]
    fn frequency_changes_keep_admitted_run_result_but_destination_or_enable_changes_do_not() {
        let account = uuid::Uuid::now_v7().to_string();
        let mut run = BackupConfig::new("https://paper.example", &account);
        run.enabled = true;
        run.destination = PathBuf::from("/chosen/folder");
        let mut current = run.clone();

        // Frequency is scheduler policy, not the identity of a running save.
        current.frequency_minutes = 30;
        current.revision = current.revision.wrapping_add(1);
        assert!(run_result_is_current(&current, &run));

        current.run_generation = current.run_generation.wrapping_add(1);
        assert!(!run_result_is_current(&current, &run));
        current = run.clone();
        current.enabled = false;
        assert!(!run_result_is_current(&current, &run));
    }

    #[test]
    fn destination_status_handles_roots_and_non_utf8_folder_names() {
        assert!(!destination_is_set(std::path::Path::new("")));
        assert!(destination_is_set(std::path::Path::new("/")));
        assert_eq!(
            destination_label(std::path::Path::new("/")),
            "Selected folder"
        );

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let path = PathBuf::from(std::ffi::OsString::from_vec(vec![
                b'/', b't', b'm', b'p', b'/', 0xff,
            ]));
            let label = destination_label(&path);
            assert!(!label.is_empty());
            assert!(!label.contains('/'));
        }
    }

    #[tokio::test]
    async fn identity_server_outage_is_not_a_login_error_and_recovers() {
        let account_id = uuid::Uuid::now_v7().to_string();
        let fixture = Arc::new(IdentityServerFixture {
            account_id: std::sync::Mutex::new(account_id.clone()),
            available: AtomicBool::new(false),
        });
        let app = Router::new()
            .route("/api/me", get(fixture_identity_status))
            .with_state(fixture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path(), cache_home.path()).await;
        crate::cli::store_token_at(&inner.state_home, &origin, "backup-test-token").unwrap();
        let (pairing_token, _) = inner
            .pairing
            .issue(&origin, "identity outage test")
            .unwrap();
        {
            let key = BackupConfig::key(&origin, &account_id);
            let mut state = inner.backups.state.lock().await;
            state.configs.insert(
                key,
                BackupConfig {
                    destination: destination.path().to_path_buf(),
                    ..BackupConfig::new(&origin, &account_id)
                },
            );
            inner.backups.persist(&state).await.unwrap();
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {pairing_token}")).unwrap(),
        );
        let response = handle_put(
            &inner,
            &headers,
            Some(&origin),
            Request::put(format!("{BASE_PATH}/backups"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "account_id": account_id.clone(),
                        "enabled": true,
                        "frequency_minutes": 5,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), MAX_JSON_BYTES)
            .await
            .unwrap();
        let put_status: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(put_status["needs_login"], false);
        assert!(put_status["error"].as_str().unwrap().contains("HTTP 503"));
        {
            let key = BackupConfig::key(&origin, &account_id);
            let mut state = inner.backups.state.lock().await;
            state.configs.get_mut(&key).unwrap().error =
                Some("cached login account does not match the requested account".into());
            inner.backups.persist(&state).await.unwrap();
        }
        let status = inner.backups.status(&inner, &origin, &account_id).await;
        assert_eq!(status["needs_login"], false);
        assert!(status["error"].as_str().unwrap().contains("HTTP 503"));

        fixture.available.store(true, Ordering::SeqCst);
        *fixture.account_id.lock().unwrap() = uuid::Uuid::now_v7().to_string();
        inner
            .backups
            .auth_checks
            .lock()
            .unwrap()
            .remove(&BackupConfig::key(&origin, &account_id));
        let status = inner.backups.status(&inner, &origin, &account_id).await;
        assert_eq!(status["needs_login"], true);
        assert!(status["error"]
            .as_str()
            .unwrap()
            .contains("account does not match"));

        *fixture.account_id.lock().unwrap() = account_id.clone();
        inner
            .backups
            .auth_checks
            .lock()
            .unwrap()
            .remove(&BackupConfig::key(&origin, &account_id));
        let status = inner.backups.status(&inner, &origin, &account_id).await;
        assert_eq!(status["needs_login"], false);
        assert_eq!(status["error"], Value::Null);
        server_task.abort();
    }

    #[test]
    fn backup_settings_are_scoped_by_origin_and_account() {
        let account = uuid::Uuid::now_v7().to_string();
        let other_account = uuid::Uuid::now_v7().to_string();
        assert_eq!(
            BackupConfig::key("https://paper.example", &account),
            BackupConfig::key("https://paper.example:443/", &account),
        );
        assert_ne!(
            BackupConfig::key("https://paper.example", &account),
            BackupConfig::key("https://other.example", &account),
        );
        assert_ne!(
            BackupConfig::key("https://paper.example", &account),
            BackupConfig::key("https://paper.example", &other_account),
        );
    }

    #[test]
    fn only_supported_intervals_are_accepted() {
        for minutes in FREQUENCIES {
            assert!(valid_frequency(minutes));
        }
        for minutes in [0, 2, 10, 120] {
            assert!(!valid_frequency(minutes));
        }
    }

    #[test]
    fn browser_requests_cannot_supply_a_destination_path() {
        let account = uuid::Uuid::now_v7().to_string();
        assert!(serde_json::from_value::<AccountBody>(
            json!({"account_id": account, "destination": "/tmp"})
        )
        .is_err());
    }

    #[tokio::test]
    async fn settings_persist_and_enabled_work_is_due_after_restart() {
        let home = tempfile::tempdir().unwrap();
        let manager = BackupManager::new(home.path());
        let account = uuid::Uuid::now_v7().to_string();
        let key = BackupConfig::key("https://paper.example", &account);
        let mut state = manager.state.lock().await;
        let mut config = BackupConfig::new("https://paper.example", &account);
        config.enabled = true;
        config.frequency_minutes = 15;
        config.destination = home.path().to_path_buf();
        config.last_attempt = Some(unix_now());
        state.configs.insert(key.clone(), config);
        manager.persist(&state).await.unwrap();
        drop(state);

        let reloaded = BackupManager::new(home.path());
        let config = reloaded.config("https://paper.example", &account).await;
        assert!(config.enabled);
        assert_eq!(config.frequency_minutes, 15);
        assert!(config.destination.is_dir());
        assert!(config.last_attempt.is_none());
        assert!(!config.running);
    }

    #[derive(Clone)]
    struct BackupFixture {
        account_id: String,
        projection: crate::document::projection::Projection,
        text: String,
    }

    async fn fixture_me(State(fixture): State<Arc<BackupFixture>>) -> Json<Value> {
        tokio::time::sleep(Duration::from_millis(120)).await;
        Json(json!({"id": fixture.account_id.clone()}))
    }

    async fn fixture_list() -> Json<Value> {
        tokio::time::sleep(Duration::from_millis(250)).await;
        Json(json!({"documents":[{"slug":"scheduled-project","title":"Scheduled Project"}]}))
    }

    async fn fixture_project(
        State(fixture): State<Arc<BackupFixture>>,
        AxumPath(_slug): AxumPath<String>,
    ) -> Json<Value> {
        Json(json!({
            "schema": 1,
            "project_digest": fixture.projection.digest(),
            "slug": "scheduled-project",
            "title": "Scheduled Project",
            "format": "markdown",
            "main": fixture.projection.main,
            "tree": fixture.projection.clone(),
            "texts":{"project.md": fixture.text.clone()}
        }))
    }

    async fn start_backup_fixture(account_id: String) -> (String, tokio::task::JoinHandle<()>) {
        let text = "# scheduled backup\n".to_string();
        let mut projection = crate::document::projection::Projection {
            main: "project.md".into(),
            ..Default::default()
        };
        projection.files.insert(
            "project.md".into(),
            crate::document::projection::Entry {
                kind: "text".into(),
                id: "test-text".into(),
                digest: hex::encode(sha2::Sha256::digest(text.as_bytes())),
                bytes: text.len() as u64,
            },
        );
        let fixture = Arc::new(BackupFixture {
            account_id,
            projection,
            text,
        });
        let app = Router::new()
            .route("/api/me", get(fixture_me))
            .route("/api/backup/projects", get(fixture_list))
            .route("/api/documents/{slug}/project", get(fixture_project))
            .with_state(fixture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (server, task)
    }

    async fn test_inner(state_home: &Path, cache_home: &Path) -> Arc<Inner> {
        let jobs_root = cache_home.join("jobs");
        std::fs::create_dir_all(&jobs_root).unwrap();
        Arc::new(Inner {
            state_home: state_home.to_path_buf(),
            folder_dialog: Mutex::new(()),
            backups: Arc::new(BackupManager::new(state_home)),
            instance: "backup-test".into(),
            port: 8764,
            pairing: pairing::PairingStore::new(state_home, None),
            quarto_bindings: crate::local::quarto::BindingStore::new(state_home),
            previews: Mutex::new(Default::default()),
            runner: Arc::new(super::super::FakeRunner::default()),
            jobs: Mutex::new(HashMap::new()),
            queue: Mutex::new(Default::default()),
            work: Notify::new(),
            jobs_root,
            connect_attempts: Mutex::new(HashMap::new()),
            pending_pairs: Mutex::new(HashMap::new()),
            assistant_sessions: crate::assistant::registry::SessionRegistry::new(
                state_home.to_path_buf(),
            ),
        })
    }

    async fn configure_scheduled_backup(
        inner: &Inner,
        origin: &str,
        account_id: &str,
        destination: &Path,
    ) {
        crate::cli::store_token_at(&inner.state_home, origin, "backup-test-token").unwrap();
        inner
            .pairing
            .issue(origin, "backup integration fixture")
            .unwrap();
        let config = BackupConfig {
            enabled: true,
            destination: destination.to_path_buf(),
            ..BackupConfig::new(origin, account_id)
        };
        let key = BackupConfig::key(origin, account_id);
        let mut state = inner.backups.state.lock().await;
        state.configs.insert(key, config);
        inner.backups.persist(&state).await.unwrap();
    }

    #[tokio::test]
    async fn scheduler_runs_without_a_browser_and_publishes_a_real_project_zip() {
        let account_id = uuid::Uuid::now_v7().to_string();
        let (server, fixture_task) = start_backup_fixture(account_id.clone()).await;
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path(), cache_home.path()).await;
        configure_scheduled_backup(&inner, &server, &account_id, destination.path()).await;

        // This task is the only trigger: the test does not make a browser
        // request after enabling the persisted setting.
        spawn_scheduler(inner.clone());
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let status = inner.backups.status(&inner, &server, &account_id).await;
                if status["last_success"].as_u64().is_some()
                    && status["projects"] == 1
                    && status["running"] == false
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the scheduler completes the first backup");

        let namespace = hex::encode(sha2::Sha256::digest(
            format!(
                "{}\0{}",
                crate::local::credentials::origin(&server),
                account_id
            )
            .as_bytes(),
        ));
        let backup_dir = destination
            .path()
            .join("librepaper-backups")
            .join(namespace);
        let archive_path = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "zip"))
            .expect("the engine publishes a project archive");
        let mut archive = zip::ZipArchive::new(std::fs::File::open(archive_path).unwrap()).unwrap();
        let mut file = archive.by_name("project.md").unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "# scheduled backup\n");
        fixture_task.abort();
    }

    #[tokio::test]
    async fn manual_run_admits_and_detaches_the_backup_work() {
        let account_id = uuid::Uuid::now_v7().to_string();
        let (server, fixture_task) = start_backup_fixture(account_id.clone()).await;
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path(), cache_home.path()).await;
        configure_scheduled_backup(&inner, &server, &account_id, destination.path()).await;

        let (pairing_token, _) = inner.pairing.issue(&server, "manual run test").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {pairing_token}")).unwrap(),
        );
        let response = handle_run(
            &inner,
            &headers,
            Some(&server),
            Request::post(format!("{BASE_PATH}/backups/run"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"account_id": account_id.clone()}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), MAX_JSON_BYTES)
            .await
            .unwrap();
        let status: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status["running"], true);
        inner
            .backups
            .configure(&inner, &server, &account_id, false, 5)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = inner.backups.status(&inner, &server, &account_id).await;
                if status["running"] == false {
                    assert_eq!(status["enabled"], false);
                    assert!(status["last_success"].is_null());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the detached run finishes after its caller returns");
        fixture_task.abort();
    }

    #[tokio::test]
    async fn delayed_enable_verification_cannot_undo_a_concurrent_disable() {
        let account_id = uuid::Uuid::now_v7().to_string();
        let (server, fixture_task) = start_backup_fixture(account_id.clone()).await;
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path(), cache_home.path()).await;
        configure_scheduled_backup(&inner, &server, &account_id, destination.path()).await;

        let enable_inner = inner.clone();
        let enable_origin = server.clone();
        let enable_account = account_id.clone();
        let enabling = tokio::spawn(async move {
            enable_inner
                .backups
                .configure(&enable_inner, &enable_origin, &enable_account, true, 5)
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        inner
            .backups
            .configure(&inner, &server, &account_id, false, 5)
            .await
            .unwrap();
        let result = enabling.await.unwrap();
        assert!(result.unwrap_err().contains("settings changed"));
        assert_eq!(
            inner.backups.status(&inner, &server, &account_id).await["enabled"],
            false
        );
        fixture_task.abort();
    }

    #[tokio::test]
    async fn backup_status_route_rejects_wrong_origin_and_enable_rejects_mismatched_account() {
        let account_id = uuid::Uuid::now_v7().to_string();
        let (server, fixture_task) = start_backup_fixture(account_id.clone()).await;
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path(), cache_home.path()).await;
        configure_scheduled_backup(&inner, &server, &account_id, destination.path()).await;
        let other_account = uuid::Uuid::now_v7().to_string();
        let mut state = inner.backups.state.lock().await;
        state.configs.insert(
            BackupConfig::key(&server, &other_account),
            BackupConfig {
                destination: destination.path().to_path_buf(),
                ..BackupConfig::new(&server, &other_account)
            },
        );
        inner.backups.persist(&state).await.unwrap();
        drop(state);
        let (pairing_token, _) = inner.pairing.issue(&server, "test").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {pairing_token}")).unwrap(),
        );

        let wrong_origin = handle_get(
            &inner,
            &headers,
            Some("http://wrong.example"),
            Request::get(format!("{BASE_PATH}/backups?account={account_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(wrong_origin.status(), StatusCode::UNAUTHORIZED);

        let mismatch = handle_put(
            &inner,
            &headers,
            Some(&server),
            Request::put(format!("{BASE_PATH}/backups"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "account_id": other_account,
                        "enabled": true,
                        "frequency_minutes": 5,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);
        let response = axum::body::to_bytes(mismatch.into_body(), MAX_JSON_BYTES)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(value["needs_login"], true);
        assert!(value["error"]
            .as_str()
            .unwrap()
            .contains("account does not match"));
        fixture_task.abort();
    }

    #[tokio::test]
    async fn status_clears_stale_login_mismatch_after_correct_account_signs_in() {
        let account_id = uuid::Uuid::now_v7().to_string();
        let (server, fixture_task) = start_backup_fixture(account_id.clone()).await;
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path(), cache_home.path()).await;
        configure_scheduled_backup(&inner, &server, &account_id, destination.path()).await;
        {
            let key = BackupConfig::key(&server, &account_id);
            let mut state = inner.backups.state.lock().await;
            let config = state.configs.get_mut(&key).unwrap();
            config.enabled = false;
            config.error = Some(
                "cached login account does not match the requested account; sign in again with the intended account".into(),
            );
            inner.backups.persist(&state).await.unwrap();
        }

        let status = inner.backups.status(&inner, &server, &account_id).await;
        assert_eq!(status["needs_login"], false);
        assert_eq!(status["error"], Value::Null);
        assert_eq!(status["enabled"], false);

        inner
            .backups
            .configure(&inner, &server, &account_id, true, 5)
            .await
            .unwrap();
        assert_eq!(
            inner.backups.status(&inner, &server, &account_id).await["enabled"],
            true
        );
        fixture_task.abort();
    }

    #[test]
    fn disabled_backups_are_never_due() {
        let config = BackupConfig::new("https://paper.example", "account");
        assert!(!config.enabled);
        assert!(config.last_attempt.is_none());
        // The scheduler's admission predicate starts from enabled configs.
        assert!(!is_due(&config, unix_now()));
    }
}
