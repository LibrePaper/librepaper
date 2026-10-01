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

fn valid_frequency(minutes: u64) -> bool {
    FREQUENCIES.contains(&minutes)
}

fn identity_matches(identity: &Value, account_id: &str) -> bool {
    identity.get("id").and_then(Value::as_str) == Some(account_id)
}

fn is_due(config: &BackupConfig, now: u64) -> bool {
    config.enabled
        && config.last_attempt.is_none_or(|attempt| {
            now.saturating_sub(attempt) >= config.frequency_minutes.saturating_mul(60)
        })
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
    revision: u64,
    #[serde(skip)]
    running: bool,
}

impl BackupConfig {
    fn key(origin: &str, account_id: &str) -> String {
        format!("{}\0{account_id}", crate::local::credentials::origin(origin))
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
        let config = self.config(origin, account_id).await;
        let token_missing = crate::cli::stored_token_at(&inner.state_home, origin).is_empty();
        json!({
            "enabled": config.enabled,
            "frequency_minutes": config.frequency_minutes,
            "destination": config.destination.file_name().and_then(|name| name.to_str()).unwrap_or(""),
            "running": config.running,
            "last_success": config.last_success,
            "error": config.error,
            "projects": config.projects,
            "updated": config.updated,
            "needs_login": token_missing || config.error.as_deref().is_some_and(|error| {
                error.contains("not signed in")
                    || error.contains("cached login")
                    || error.contains("account does not match")
            }),
        })
    }

    async fn choose_destination(
        &self,
        inner: &Inner,
        headers: &HeaderMap,
        origin: &str,
        account_id: &str,
    ) -> Result<(), Reply> {
        let Ok(_dialog) = inner.folder_dialog.try_lock() else {
            return Err(write_json(409, &json!({"error":"A folder chooser is already open on this computer."})));
        };
        let destination = super::super::folder::choose_directory(origin, "backup destination")
            .await
            .map_err(|error| write_json(400, &json!({"error": error})))?;
        if authenticate(inner, headers, Some(origin)).is_err() {
            return Err(write_json(401, &json!({"error":"This site is no longer connected."})));
        }
        let key = BackupConfig::key(origin, account_id);
        let mut state = self.state.lock().await;
        if state.in_flight.contains(&key) {
            return Err(write_json(409, &json!({"error":"A backup is running; choose a new destination when it finishes."})));
        }
        let previous = state.configs.get(&key).cloned();
        let config = state
            .configs
            .entry(key.clone())
            .or_insert_with(|| BackupConfig::new(origin, account_id));
        config.destination = destination;
        config.last_attempt = None;
        config.last_success = None;
        config.projects = 0;
        config.updated = 0;
        config.error = None;
        config.revision = config.revision.wrapping_add(1);
        if let Err(error) = self.persist(&state).await {
            match previous {
                Some(previous) => { state.configs.insert(key.clone(), previous); }
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
        &self,
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
        let mut state = self.state.lock().await;
        if enabled && !state.configs.get(&key).is_some_and(|config| config.destination.is_dir()) {
            return Err("Choose a backup destination before enabling backups.".into());
        }
        // Never hold the status/config lock over a network request. Apart
        // from keeping GETs responsive, re-check the destination below in
        // case it changed while identity verification was in flight.
        drop(state);
        if enabled {
            if let Err(error) = verify_account(inner, origin, account_id).await {
                let mut state = self.state.lock().await;
                let config = state
                    .configs
                    .entry(key.clone())
                    .or_insert_with(|| BackupConfig::new(origin, account_id));
                config.error = Some(error.clone());
                if error.contains("account does not match")
                    || !inner.pairing.has_live_pairing(origin)
                {
                    if config.enabled {
                        config.revision = config.revision.wrapping_add(1);
                    }
                    config.enabled = false;
                }
                let error = match self.persist(&state).await {
                    Ok(()) => error,
                    Err(persist_error) => format!("{error}; could not save backup settings: {persist_error}"),
                };
                if let Some(config) = state.configs.get_mut(&key) {
                    config.error = Some(error.clone());
                }
                return Err(error);
            }
        }
        let mut state = self.state.lock().await;
        if enabled && !state.configs.get(&key).is_some_and(|config| config.destination.is_dir()) {
            return Err("Choose a backup destination before enabling backups.".into());
        }
        let previous = state.configs.get(&key).cloned();
        let config = state
            .configs
            .entry(key.clone())
            .or_insert_with(|| BackupConfig::new(origin, account_id));
        let was_enabled = config.enabled;
        config.enabled = enabled;
        config.frequency_minutes = frequency_minutes;
        config.error = None;
        if was_enabled != enabled {
            config.revision = config.revision.wrapping_add(1);
        }
        if enabled && !was_enabled {
            // Enabling always queues an immediate initial save.
            config.last_attempt = None;
        }
        if let Err(error) = self.persist(&state).await {
            match previous {
                Some(previous) => { state.configs.insert(key.clone(), previous); }
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

    async fn run_now(&self, inner: &Arc<Inner>, origin: &str, account_id: &str) -> Result<(), String> {
        let key = BackupConfig::key(origin, account_id);
        let config = {
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
            }
            if let Err(error) = self.persist(&state).await {
                state.in_flight.remove(&key);
                state.configs.insert(key, previous);
                if let Some(current) = state.configs.get_mut(&key) {
                    current.error = Some(error.clone());
                }
                return Err(error);
            }
            config
        };
        let run_revision = config.revision;
        let result = run_backup(inner, &config).await;
        let mut state = self.state.lock().await;
        state.in_flight.remove(&key);
        if let Some(current) = state.configs.get_mut(&key) {
            current.running = false;
            if current.revision == run_revision { match result {
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
                        || current.error.as_deref().is_some_and(|error| error.contains("account does not match"))
                    {
                        if current.enabled {
                            current.revision = current.revision.wrapping_add(1);
                        }
                        current.enabled = false;
                    }
                }
            }}
        }
        if let Err(error) = self.persist(&state).await {
            if let Some(current) = state.configs.get_mut(&key) {
                current.error = Some(error);
            }
        }
        self.changed.notify_one();
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
        return write_json(403, &json!({"error":"backups require a paired site origin"}));
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
        return write_json(403, &json!({"error":"backups require a paired site origin"}));
    };
    let body = match read_json_body::<AccountBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !account_id(&body.account_id) {
        return write_json(400, &json!({"error":"backups need a valid account id"}));
    }
    match inner.backups.choose_destination(inner, headers, origin, &body.account_id).await {
        Ok(()) => write_json(200, &inner.backups.status(inner, origin, &body.account_id).await),
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
        return write_json(403, &json!({"error":"backups require a paired site origin"}));
    };
    let body = match read_json_body::<ConfigureBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !account_id(&body.account_id) {
        return write_json(400, &json!({"error":"backups need a valid account id"}));
    }
    match inner.backups.configure(inner, origin, &body.account_id, body.enabled, body.frequency_minutes).await {
        Ok(()) => write_json(200, &inner.backups.status(inner, origin, &body.account_id).await),
        Err(error) => {
            let needs_login = error.contains("not signed in")
                || error.contains("could not verify account")
                || error.contains("cached login");
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
        return write_json(403, &json!({"error":"backups require a paired site origin"}));
    };
    let body = match read_json_body::<AccountBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !account_id(&body.account_id) {
        return write_json(400, &json!({"error":"backups need a valid account id"}));
    }
    match inner.backups.run_now(inner, origin, &body.account_id).await {
        Ok(()) => write_json(200, &inner.backups.status(inner, origin, &body.account_id).await),
        Err(error) => write_json(409, &json!({"error": error})),
    }
}

async fn verify_account(inner: &Inner, origin: &str, account_id: &str) -> Result<String, String> {
    if !inner.pairing.has_live_pairing(origin) {
        return Err("This site is no longer connected.".into());
    }
    let token = crate::cli::stored_token_at(&inner.state_home, origin);
    if token.is_empty() {
        return Err(format!("not signed in. Run: librepaper login --server {origin}"));
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
        return Err("could not verify account: the cached login is no longer valid".into());
    }
    let identity: Value = response
        .json()
        .await
        .map_err(|error| format!("could not verify account: {error}"))?;
    if !identity_matches(&identity, account_id) {
        return Err("cached login account does not match the requested account; sign in again with the intended account".into());
    }
    if !inner.pairing.has_live_pairing(origin) {
        return Err("This site is no longer connected.".into());
    }
    Ok(token)
}

async fn run_backup(inner: &Inner, config: &BackupConfig) -> Result<crate::local::backup::BackupReport, String> {
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
                        }
                    } else if !state.in_flight.contains(&key)
                        && is_due(&config, now)
                    {
                        if !config.destination.is_dir() {
                            if let Some(current) = state.configs.get_mut(&key) {
                                current.last_attempt = Some(now);
                                current.error = Some("The selected backup destination is unavailable.".into());
                            }
                            continue;
                        }
                        state.in_flight.insert(key);
                        if let Some(current) = state.configs.get_mut(&BackupConfig::key(&config.origin, &config.account_id)) {
                            current.running = true;
                            current.last_attempt = Some(now);
                        }
                        due.push(config);
                    }
                }
                if let Err(error) = inner.backups.persist(&state).await {
                    for config in state.configs.values_mut().filter(|config| config.running) {
                        config.error = Some(error.clone());
                    }
                }
                due
            };
            for config in due {
                let task_inner = inner.clone();
                tokio::spawn(async move {
                    let key = BackupConfig::key(&config.origin, &config.account_id);
                    let run_revision = config.revision;
                    let result = run_backup(&task_inner, &config).await;
                    let mut state = task_inner.backups.state.lock().await;
                    state.in_flight.remove(&key);
                    if let Some(current) = state.configs.get_mut(&key) {
                        current.running = false;
                        if current.revision == run_revision {
                        match result {
                            Ok(report) => {
                                current.last_success = Some(unix_now());
                                current.projects = report.projects;
                                current.updated = report.updated;
                                current.error = None;
                            }
                            Err(error) => {
                                if !task_inner.pairing.has_live_pairing(&current.origin)
                                    || error.contains("account does not match")
                                {
                                    if current.enabled {
                                        current.revision = current.revision.wrapping_add(1);
                                    }
                                    current.enabled = false;
                                }
                                current.error = Some(error);
                            }
                        }
                        }
                    }
                    if let Err(error) = task_inner.backups.persist(&state).await {
                        if let Some(current) = state.configs.get_mut(&key) {
                            current.error = Some(error);
                        }
                    }
                });
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

    #[test]
    fn identity_must_match_the_requested_signed_in_account() {
        let account = uuid::Uuid::now_v7().to_string();
        assert!(identity_matches(&json!({"id": account.clone()}), &account));
        assert!(!identity_matches(&json!({"id": ""}), &account));
        assert!(!identity_matches(&json!({"handle": "alice"}), &account));
        assert!(!identity_matches(&json!({"id": uuid::Uuid::now_v7().to_string()}), &account));
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
        assert!(serde_json::from_value::<AccountBody>(json!({"account_id": account, "destination": "/tmp"})).is_err());
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

    #[test]
    fn disabled_backups_are_never_due() {
        let config = BackupConfig::new("https://paper.example", "account");
        assert!(!config.enabled);
        assert!(config.last_attempt.is_none());
        // The scheduler's admission predicate starts from enabled configs.
        assert!(!is_due(&config, unix_now()));
    }
}
