//! Explicit TOML configuration for server deployments. Environment lookup is
//! permitted only when a leaf explicitly names an env reference.
use librepaper_base::{
    auth::Policy,
    config::{
        BackupPolicyOverrides, Configuration, DEFAULT_LOG_QUOTA_BYTES, DEFAULT_PENDING_BYTES,
    },
};
use librepaper_engine::storage::StorageOptions;
use serde::{de::DeserializeOwned, Deserialize};
use std::fmt;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

/// Original process PG* values captured before the admin runtime starts.
/// Environment references to PG* keys use this snapshot, never the scrubbed
/// process environment used by SQLx.
#[derive(Clone, Default)]
pub(crate) struct PostgresEnvSnapshot(BTreeMap<OsString, OsString>);

impl PostgresEnvSnapshot {
    pub(crate) fn capture() -> Self {
        Self(
            std::env::vars_os()
                .filter(|(key, _)| is_postgres_env_key(key.as_os_str()))
                .collect(),
        )
    }

    pub(crate) fn scrub_process_environment(&self) {
        for key in self.0.keys() {
            std::env::remove_var(key);
        }
        // An empty password disables SQLx's .pgpass fallback while retaining
        // the default authentication behavior when the URL has no password.
        std::env::set_var("PGPASSWORD", "");
    }
}

pub(crate) fn is_postgres_env_key(key: &OsStr) -> bool {
    let key = key.to_string_lossy();
    #[cfg(windows)]
    {
        key.get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("PG"))
    }
    #[cfg(not(windows))]
    {
        key.starts_with("PG")
    }
}

#[derive(Clone)]
pub(crate) struct ResolvedConfig {
    pub(crate) address: SocketAddr,
    pub(crate) app_origin: Option<String>,
    pub(crate) docs_origin: Option<String>,
    pub(crate) site_origin: Option<String>,
    pub(crate) local_companion: bool,
    pub(crate) storage: StorageOptions,
    pub(crate) github_client_id: Option<String>,
    pub(crate) github_client_secret: Option<String>,
    pub(crate) google_client_id: Option<String>,
    pub(crate) google_client_secret: Option<String>,
    pub(crate) publishers: Vec<String>,
    pub(crate) commenters: Vec<String>,
    pub(crate) simulate_activity: Option<u32>,
    pub(crate) expire_after: Option<String>,
    pub(crate) expire_from: Option<String>,
    pub(crate) asset_mirror: String,
    pub(crate) typst_fonts: Option<String>,
    pub(crate) metrics_address: Option<SocketAddr>,
    pub(crate) log_filter: String,
    pub(crate) config: Configuration,
    sources: BTreeMap<String, String>,
}

impl fmt::Debug for ResolvedConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedConfig")
            .field("address", &self.address)
            .field("app_origin", &self.app_origin)
            .field("docs_origin", &self.docs_origin)
            .field("site_origin", &self.site_origin)
            .field("local_companion", &self.local_companion)
            .field("storage", &"<redacted>")
            .field("github_client_id", &self.github_client_id)
            .field(
                "github_client_secret",
                &self.github_client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("google_client_id", &self.google_client_id)
            .field(
                "google_client_secret",
                &self.google_client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("publishers", &self.publishers)
            .field("commenters", &self.commenters)
            .field("simulate_activity", &self.simulate_activity)
            .field("expire_after", &self.expire_after)
            .field("expire_from", &self.expire_from)
            .field("asset_mirror", &self.asset_mirror)
            .field("typst_fonts", &self.typst_fonts)
            .field("metrics_address", &self.metrics_address)
            .field("log_filter", &self.log_filter)
            .field("config", &self.config)
            .finish()
    }
}
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Input<T> {
    Literal(T),
    Ref(Reference),
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    env: Option<String>,
    file: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Raw {
    server: Server,
    origins: Origins,
    storage: Storage,
    auth: Auth,
    access: Access,
    limits: Limits,
    retention: Retention,
    assets: Assets,
    metrics: Metrics,
    proxy: Proxy,
    backup: RawBackup,
    demo: Demo,
    logging: Logging,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Server {
    address: Option<Input<SocketAddr>>,
    local_companion: Option<Input<bool>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Origins {
    app: Option<Input<String>>,
    docs: Option<Input<String>>,
    site: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Storage {
    directory: Option<Input<PathBuf>>,
    database_url: Option<Input<String>>,
    database_connections: Option<Input<u32>>,
    fsync: Option<Input<bool>>,
    object_store: Option<Input<String>>,
    s3: S3,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct S3 {
    endpoint: Option<Input<String>>,
    region: Option<Input<String>>,
    bucket: Option<Input<String>>,
    allow_http: Option<Input<bool>>,
    access_key_id: Option<Input<String>>,
    secret_access_key: Option<Input<String>>,
    session_token: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Auth {
    github: OAuth,
    google: OAuth,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OAuth {
    client_id: Option<Input<String>>,
    client_secret: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Access {
    publishers: Option<Input<Vec<String>>>,
    commenters: Option<Input<Vec<String>>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Limits {
    publisher_storage_mib: Option<Input<usize>>,
    deployment_storage_mib: Option<Input<usize>>,
    publisher_uploads_per_hour: Option<Input<usize>>,
    session_peer_queue_frames: Option<Input<usize>>,
    pending_mib: Option<Input<u64>>,
    pending_scratch_mib: Option<Input<u64>>,
    log_quota_mib: Option<Input<u64>>,
    memory_budget_mib: Option<Input<u64>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Retention {
    expire_after: Option<Input<String>>,
    expire_from: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Assets {
    mirror: Option<Input<String>>,
    typst_fonts: Option<Input<PathBuf>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Metrics {
    address: Option<Input<SocketAddr>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Proxy {
    trusted_networks: Option<Input<Vec<String>>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Demo {
    simulate_activity_days: Option<Input<u32>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Logging {
    filter: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawBackup {
    destination_class: Option<Input<String>>,
    interval: Option<Input<String>>,
    retained_count: Option<Input<usize>>,
    encrypted: Option<Input<bool>>,
    warning_count: Option<Input<usize>>,
}

struct Resolver<'a> {
    base: &'a Path,
    sources: BTreeMap<String, String>,
    postgres_env: Option<&'a PostgresEnvSnapshot>,
}
impl Resolver<'_> {
    fn resolve<T: DeserializeOwned>(&mut self, key: &str, input: Input<T>) -> Result<T, String> {
        match input {
            Input::Literal(v) => {
                self.sources.insert(key.into(), "config".into());
                Ok(v)
            }
            Input::Ref(reference) => {
                let (kind, target) = match (&reference.env, &reference.file) {
                    (Some(n), None) if !n.is_empty() => ("env", n.as_str()),
                    (None, Some(p)) if !p.is_empty() => ("file", p.as_str()),
                    _ => {
                        return Err(format!(
                            "{key}: reference must contain exactly one non-empty env or file"
                        ))
                    }
                };
                self.sources.insert(key.into(), format!("{kind}:{target}"));
                let raw = if kind == "env" {
                    read_env(target, self.postgres_env).map_err(|_| {
                        format!("{key}: referenced environment variable {target:?} is unavailable")
                    })?
                } else {
                    let path = self.path(Path::new(target));
                    std::fs::read_to_string(&path).map_err(|_| {
                        format!("{key}: could not read referenced file {}", path.display())
                    })?
                };
                let raw = raw.trim_end_matches(['\r', '\n']);
                if raw.trim().is_empty() {
                    return Err(format!("{key}: referenced value is empty"));
                }
                parse_reference(raw)
                    .map_err(|_| format!("{key}: referenced value has the wrong type"))
            }
        }
    }
    fn value<T: DeserializeOwned>(
        &mut self,
        key: &str,
        input: Option<Input<T>>,
        default: T,
    ) -> Result<T, String> {
        match input {
            Some(v) => self.resolve(key, v),
            None => {
                self.sources.insert(key.into(), "default".into());
                Ok(default)
            }
        }
    }
    fn optional<T: DeserializeOwned>(
        &mut self,
        key: &str,
        input: Option<Input<T>>,
    ) -> Result<Option<T>, String> {
        match input {
            Some(v) => self.resolve(key, v).map(Some),
            None => {
                self.sources.insert(key.into(), "default".into());
                Ok(None)
            }
        }
    }
    fn path(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.into()
        } else {
            self.base.join(p)
        }
    }
}

fn read_env(target: &str, postgres_env: Option<&PostgresEnvSnapshot>) -> Result<String, ()> {
    if is_postgres_env_key(OsStr::new(target)) {
        if let Some(snapshot) = postgres_env {
            let value = snapshot
                .0
                .iter()
                .find(|(key, _)| {
                    #[cfg(windows)]
                    {
                        key.to_string_lossy().eq_ignore_ascii_case(target)
                    }
                    #[cfg(not(windows))]
                    {
                        key.as_os_str() == OsStr::new(target)
                    }
                })
                .map(|(_, value)| value.clone())
                .ok_or(())?;
            return value.into_string().map_err(|_| ());
        }
    }
    std::env::var(target).map_err(|_| ())
}
fn parse_reference<T: DeserializeOwned>(raw: &str) -> Result<T, ()> {
    if let Ok(v) = T::deserialize(toml::Value::String(raw.to_owned())) {
        return Ok(v);
    }
    let v: toml::Value = toml::from_str(&format!("value = {raw}")).map_err(|_| ())?;
    if !matches!(v.as_table(), Some(table) if table.len() == 1) {
        return Err(());
    }
    T::deserialize(v.get("value").cloned().ok_or(())?).map_err(|_| ())
}
fn check_keys(v: &toml::Value) -> Result<(), String> {
    fn table(v: &toml::Value, prefix: &str, allowed: &[&str]) -> Result<(), String> {
        let t = v
            .as_table()
            .ok_or_else(|| format!("configuration section {prefix} must be a table"))?;
        for k in t.keys() {
            if !allowed.contains(&k.as_str()) {
                return Err(format!("unknown configuration key {prefix}{k}"));
            }
        }
        Ok(())
    }
    fn section(v: &toml::Value, name: &str, keys: &[&str]) -> Result<(), String> {
        if let Some(x) = v.get(name) {
            table(x, &format!("{name}."), keys)?;
        }
        Ok(())
    }
    table(
        v,
        "",
        &[
            "server",
            "origins",
            "storage",
            "auth",
            "access",
            "limits",
            "retention",
            "assets",
            "metrics",
            "proxy",
            "backup",
            "demo",
            "logging",
        ],
    )?;
    section(
        v,
        "server",
        &[
            "address",
            "local_companion",
        ],
    )?;
    section(
        v,
        "origins",
        &[
            "app",
            "docs",
            "site",
        ],
    )?;
    section(
        v,
        "storage",
        &[
            "directory",
            "database_url",
            "database_connections",
            "fsync",
            "object_store",
            "s3",
        ],
    )?;
    if let Some(x) = v.get("storage").and_then(|x| x.get("s3")) {
        table(
            x,
            "storage.s3.",
            &[
                "endpoint",
                "region",
                "bucket",
                "allow_http",
                "access_key_id",
                "secret_access_key",
                "session_token",
            ],
        )?;
    }
    section(v, "auth", &["github", "google"])?;
    if let Some(x) = v.get("auth") {
        for p in ["github", "google"] {
            if let Some(y) = x.get(p) {
                table(y, &format!("auth.{p}."), &["client_id", "client_secret"])?;
            }
        }
    }
    section(v, "access", &["publishers", "commenters"])?;
    section(
        v,
        "limits",
        &[
            "publisher_storage_mib",
            "deployment_storage_mib",
            "publisher_uploads_per_hour",
            "session_peer_queue_frames",
            "pending_mib",
            "pending_scratch_mib",
            "log_quota_mib",
            "memory_budget_mib",
        ],
    )?;
    section(v, "retention", &["expire_after", "expire_from"])?;
    section(v, "assets", &["mirror", "typst_fonts"])?;
    section(v, "metrics", &["address"])?;
    section(v, "proxy", &["trusted_networks"])?;
    section(
        v,
        "backup",
        &[
            "destination_class",
            "interval",
            "retained_count",
            "encrypted",
            "warning_count",
        ],
    )?;
    section(v, "demo", &["simulate_activity_days"])?;
    section(v, "logging", &["filter"])?;
    Ok(())
}
fn read_raw(path: &Path) -> Result<(Raw, PathBuf), String> {
    let path = if path.is_absolute() {
        path.into()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("could not read server configuration {}", path.display()))?;
    let v: toml::Value = toml::from_str(&text).map_err(|error| {
        let location = error
            .span()
            .map(|span| {
                let prefix = text.get(..span.start).unwrap_or(&text);
                let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
                let column = prefix.rsplit('\n').next().unwrap_or(prefix).chars().count() + 1;
                format!(" at line {line}, column {column}")
            })
            .unwrap_or_default();
        format!(
            "could not parse TOML server configuration {}{location}",
            path.display()
        )
    })?;
    check_keys(&v)?;
    let raw: Raw = serde_path_to_error::deserialize(v)
        .map_err(|error| format!("invalid configuration value at {}", error.path()))?;
    Ok((raw, path.parent().unwrap_or(Path::new(".")).into()))
}
fn resolve_storage(s: Storage, r: &mut Resolver<'_>) -> Result<StorageOptions, String> {
    let dir = r.value(
        "storage.directory",
        s.directory,
        PathBuf::from("librepaper-data"),
    )?;
    let result = StorageOptions {
        dir: r.path(&dir),
        database_url: r.value(
            "storage.database_url",
            s.database_url,
            "postgresql:///librepaper".into(),
        )?,
        database_connections: r.value(
            "storage.database_connections",
            s.database_connections,
            20,
        )?,
        fsync: r.value("storage.fsync", s.fsync, true)?,
        object_store: r.value("storage.object_store", s.object_store, "filesystem".into())?,
        s3_endpoint: r.optional("storage.s3.endpoint", s.s3.endpoint)?,
        s3_region: r.value("storage.s3.region", s.s3.region, "us-east-1".into())?,
        s3_bucket: r.optional("storage.s3.bucket", s.s3.bucket)?,
        s3_allow_http: r.value("storage.s3.allow_http", s.s3.allow_http, false)?,
        s3_access_key_id: r.optional("storage.s3.access_key_id", s.s3.access_key_id)?,
        s3_secret_access_key: r.optional("storage.s3.secret_access_key", s.s3.secret_access_key)?,
        s3_session_token: r.optional("storage.s3.session_token", s.s3.session_token)?,
    };
    validate_storage(&result)?;
    Ok(result)
}
fn validate_storage(s: &StorageOptions) -> Result<(), String> {
    librepaper_engine::storage::validate_storage_options(s)
}
#[cfg(test)]
pub(crate) fn load_storage_with_log_filter(
    path: &Path,
) -> Result<(StorageOptions, String), String> {
    load_storage_with_log_filter_and_env(path, None)
}

pub(crate) fn load_storage_with_log_filter_and_env(
    path: &Path,
    postgres_env: Option<&PostgresEnvSnapshot>,
) -> Result<(StorageOptions, String), String> {
    let (raw, base) = read_raw(path)?;
    let mut resolver = Resolver {
        base: &base,
        sources: BTreeMap::new(),
        postgres_env,
    };
    let storage = resolve_storage(raw.storage, &mut resolver)?;
    let filter = resolver.value("logging.filter", raw.logging.filter, "info".to_owned())?;
    tracing_subscriber::EnvFilter::try_new(&filter)
        .map_err(|_| "logging.filter is not a valid filter directive".to_owned())?;
    Ok((storage, filter))
}
#[cfg(test)]
pub(crate) fn load(path: &Path) -> Result<ResolvedConfig, String> {
    load_with_postgres_env(path, None)
}

pub(crate) fn load_with_postgres_env(
    path: &Path,
    postgres_env: Option<&PostgresEnvSnapshot>,
) -> Result<ResolvedConfig, String> {
    let (raw, base) = read_raw(path)?;
    let mut r = Resolver {
        base: &base,
        sources: BTreeMap::new(),
        postgres_env,
    };
    let address = r.value(
        "server.address",
        raw.server.address,
        "0.0.0.0:8080".parse::<SocketAddr>().unwrap(),
    )?;
    let local_companion = r.value("server.local_companion", raw.server.local_companion, false)?;
    let app_origin = r.optional("origins.app", raw.origins.app)?;
    let docs_origin = r.optional("origins.docs", raw.origins.docs)?;
    let site_origin = r.optional("origins.site", raw.origins.site)?;
    let storage = resolve_storage(raw.storage, &mut r)?;
    let github_client_id = r.optional("auth.github.client_id", raw.auth.github.client_id)?;
    let github_client_secret =
        r.optional("auth.github.client_secret", raw.auth.github.client_secret)?;
    let google_client_id = r.optional("auth.google.client_id", raw.auth.google.client_id)?;
    let google_client_secret =
        r.optional("auth.google.client_secret", raw.auth.google.client_secret)?;
    let publishers = r.value(
        "access.publishers",
        raw.access.publishers,
        Vec::<String>::new(),
    )?;
    let commenters = r.value(
        "access.commenters",
        raw.access.commenters,
        vec!["anyone".into()],
    )?;
    Policy::parse_publishers(&publishers.join(","))
        .map_err(|e| format!("access.publishers: {e}"))?;
    let _ = Policy::parse(&commenters.join(","));
    let expire_after = r.optional("retention.expire_after", raw.retention.expire_after)?;
    let expire_from = Some(r.value(
        "retention.expire_from",
        raw.retention.expire_from,
        "updated".into(),
    )?);
    let asset_mirror = r.value(
        "assets.mirror",
        raw.assets.mirror,
        librepaper_base::config::DEFAULT_ASSET_MIRROR.into(),
    )?;
    let typst_fonts = r
        .optional("assets.typst_fonts", raw.assets.typst_fonts)?
        .map(|p| r.path(&p).to_string_lossy().into_owned());
    let metrics_address = r.optional("metrics.address", raw.metrics.address)?;
    let log_filter = r.value("logging.filter", raw.logging.filter, "info".into())?;
    tracing_subscriber::EnvFilter::try_new(&log_filter)
        .map_err(|_| "logging.filter is not a valid filter directive".to_owned())?;
    let simulate_activity = r.optional("demo.simulate_activity_days", raw.demo.simulate_activity_days)?;
    let mut config = Configuration::default();
    let l = raw.limits;
    let publisher_storage_mb = r.optional("limits.publisher_storage_mib", l.publisher_storage_mib)?;
    let deployment_storage_mb =
        r.optional("limits.deployment_storage_mib", l.deployment_storage_mib)?;
    let max_storage_mb = ((i64::MAX as u128).min(usize::MAX as u128) / 1_048_576) as usize;
    for (key, value) in [
        ("limits.publisher_storage_mib", publisher_storage_mb),
        ("limits.deployment_storage_mib", deployment_storage_mb),
    ] {
        if value.is_some_and(|mb| mb > max_storage_mb) {
            return Err(format!("{key} must fit in the storage byte limit"));
        }
    }
    config.set_storage(publisher_storage_mb, deployment_storage_mb)?;
    let uploads_per_hour = r.optional(
        "limits.publisher_uploads_per_hour",
        l.publisher_uploads_per_hour,
    )?;
    if uploads_per_hour.is_some_and(|count| count as u128 > i64::MAX as u128) {
        return Err("limits.publisher_uploads_per_hour must fit in a signed 64-bit count".into());
    }
    config.set_uploads_per_hour(uploads_per_hour)?;
    config.set_peer_queue(r.optional("limits.session_peer_queue_frames", l.session_peer_queue_frames)?)?;
    let log_quota = r.value(
        "limits.log_quota_mib",
        l.log_quota_mib,
        (DEFAULT_LOG_QUOTA_BYTES / 1024 / 1024) as u64,
    )?;
    config.set_log_quota(Some(log_quota))?;
    let pend = r.value(
        "limits.pending_mib",
        l.pending_mib,
        DEFAULT_PENDING_BYTES / 1024 / 1024,
    )?;
    let scratch = r.value(
        "limits.pending_scratch_mib",
        l.pending_scratch_mib,
        config.pending_scratch_bytes.div_ceil(1024 * 1024),
    )?;
    config.set_pending(Some(pend), Some(scratch))?;
    config.set_memory_budget(Some(r.value(
        "limits.memory_budget_mib",
        l.memory_budget_mib,
        512u64,
    )?))?;
    config.validate_pending()?;
    config.validate_budgets()?;
    config.cost.trusted_proxies = r
        .value(
            "proxy.trusted_networks",
            raw.proxy.trusted_networks,
            Vec::<String>::new(),
        )?
        .iter()
        .map(|x| librepaper_base::config::parse_trusted_proxy(x))
        .collect::<Result<_, _>>()?;
    config.cost.validate()?;
    let b = raw.backup;
    let backup_interval_seconds = if let Some(interval_str) = r.optional("backup.interval", b.interval)? {
        let seconds = librepaper_document::document::retention::parse_retention(&interval_str)
            .map_err(|_| "backup.interval must be a duration such as 1d or 24h".to_string())?;
        Some(seconds as u64)
    } else {
        None
    };
    config.apply_backup_overrides(BackupPolicyOverrides {
        destination_class: r.optional("backup.destination_class", b.destination_class)?,
        frequency: backup_interval_seconds,
        retained_count: r.optional("backup.retained_count", b.retained_count)?,
        encrypted: r.optional("backup.encrypted", b.encrypted)?,
        warning_count: r.optional("backup.warning_count", b.warning_count)?,
    })?;
    if github_client_id.is_some() != github_client_secret.is_some() {
        return Err("auth.github.client_id and client_secret must be configured together".into());
    }
    if github_client_id
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
        || github_client_secret
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err("auth.github.client_id and client_secret must not be blank".into());
    }
    if google_client_id.is_some() != google_client_secret.is_some() {
        return Err("auth.google.client_id and client_secret must be configured together".into());
    }
    if google_client_id
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
        || google_client_secret
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err("auth.google.client_id and client_secret must not be blank".into());
    }
    validate_public_urls(
        app_origin.as_deref(),
        docs_origin.as_deref(),
        site_origin.as_deref(),
        &asset_mirror,
    )?;
    Ok(ResolvedConfig {
        address,
        app_origin,
        docs_origin,
        site_origin,
        local_companion,
        storage,
        github_client_id,
        github_client_secret,
        google_client_id,
        google_client_secret,
        publishers,
        commenters,
        simulate_activity,
        expire_after,
        expire_from,
        asset_mirror,
        typst_fonts,
        metrics_address,
        log_filter,
        config,
        sources: r.sources,
    })
}

fn validate_public_urls(
    app_origin: Option<&str>,
    docs_origin: Option<&str>,
    site_origin: Option<&str>,
    asset_mirror: &str,
) -> Result<(), String> {
    validate_public_url("origins.app", app_origin, false, true, true)?;
    validate_public_url("origins.docs", docs_origin, false, true, true)?;
    validate_public_url("origins.site", site_origin, false, true, true)?;
    validate_public_url("assets.mirror", Some(asset_mirror), true, false, false)
}

fn validate_public_url(
    field: &str,
    value: Option<&str>,
    https_only: bool,
    origin_only: bool,
    allow_blank: bool,
) -> Result<(), String> {
    let Some(value) = value.map(str::trim) else {
        return Ok(());
    };
    if value.is_empty() {
        return if allow_blank {
            Ok(())
        } else {
            Err(format!("{field} must be an absolute HTTPS URL"))
        };
    }
    let parsed =
        url::Url::parse(value).map_err(|_| format!("{field} must be an absolute HTTP(S) URL"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || (https_only && parsed.scheme() != "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(format!(
            "{field} must be an absolute {} URL without credentials, a query, or a fragment",
            if https_only { "HTTPS" } else { "HTTP(S)" }
        ));
    }
    if origin_only && !parsed.path().trim_matches('/').is_empty() {
        return Err(format!("{field} must be an origin without a path"));
    }
    Ok(())
}
pub(crate) fn show_resolved(c: &ResolvedConfig) -> String {
    let mut out =
        String::from("# Effective configuration. Secrets and the database URL are redacted.\n");
    section(&mut out, "server");
    value(
        &mut out,
        &c.sources,
        "server.address",
        "address",
        &quote(&c.address.to_string()),
    );
    value(
        &mut out,
        &c.sources,
        "server.local_companion",
        "local_companion",
        &c.local_companion.to_string(),
    );

    section(&mut out, "origins");
    optional(
        &mut out,
        &c.sources,
        "origins.app",
        "app",
        c.app_origin.as_deref(),
    );
    optional(
        &mut out,
        &c.sources,
        "origins.docs",
        "docs",
        c.docs_origin.as_deref(),
    );
    optional(
        &mut out,
        &c.sources,
        "origins.site",
        "site",
        c.site_origin.as_deref(),
    );

    section(&mut out, "storage");
    value(
        &mut out,
        &c.sources,
        "storage.directory",
        "directory",
        &quote(&c.storage.dir.display().to_string()),
    );
    redacted(
        &mut out,
        &c.sources,
        "storage.database_url",
        "database_url",
        true,
    );
    value(
        &mut out,
        &c.sources,
        "storage.database_connections",
        "database_connections",
        &c.storage.database_connections.to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "storage.fsync",
        "fsync",
        &c.storage.fsync.to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "storage.object_store",
        "object_store",
        &quote(&c.storage.object_store),
    );
    section(&mut out, "storage.s3");
    optional(
        &mut out,
        &c.sources,
        "storage.s3.endpoint",
        "endpoint",
        c.storage.s3_endpoint.as_deref(),
    );
    value(
        &mut out,
        &c.sources,
        "storage.s3.region",
        "region",
        &quote(&c.storage.s3_region),
    );
    optional(
        &mut out,
        &c.sources,
        "storage.s3.bucket",
        "bucket",
        c.storage.s3_bucket.as_deref(),
    );
    value(
        &mut out,
        &c.sources,
        "storage.s3.allow_http",
        "allow_http",
        &c.storage.s3_allow_http.to_string(),
    );
    redacted(
        &mut out,
        &c.sources,
        "storage.s3.access_key_id",
        "access_key_id",
        c.storage.s3_access_key_id.is_some(),
    );
    redacted(
        &mut out,
        &c.sources,
        "storage.s3.secret_access_key",
        "secret_access_key",
        c.storage.s3_secret_access_key.is_some(),
    );
    redacted(
        &mut out,
        &c.sources,
        "storage.s3.session_token",
        "session_token",
        c.storage.s3_session_token.is_some(),
    );

    section(&mut out, "auth.github");
    optional(
        &mut out,
        &c.sources,
        "auth.github.client_id",
        "client_id",
        c.github_client_id.as_deref(),
    );
    redacted(
        &mut out,
        &c.sources,
        "auth.github.client_secret",
        "client_secret",
        c.github_client_secret.is_some(),
    );
    section(&mut out, "auth.google");
    optional(
        &mut out,
        &c.sources,
        "auth.google.client_id",
        "client_id",
        c.google_client_id.as_deref(),
    );
    redacted(
        &mut out,
        &c.sources,
        "auth.google.client_secret",
        "client_secret",
        c.google_client_secret.is_some(),
    );

    section(&mut out, "access");
    value(
        &mut out,
        &c.sources,
        "access.publishers",
        "publishers",
        &array(&c.publishers),
    );
    value(
        &mut out,
        &c.sources,
        "access.commenters",
        "commenters",
        &array(&c.commenters),
    );
    section(&mut out, "limits");
    value(
        &mut out,
        &c.sources,
        "limits.publisher_storage_mib",
        "publisher_storage_mib",
        &(c.config.storage.per_owner >> 20).to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "limits.deployment_storage_mib",
        "deployment_storage_mib",
        &(c.config.storage.total >> 20).to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "limits.publisher_uploads_per_hour",
        "publisher_uploads_per_hour",
        &c.config.storage.uploads_per_hour.to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "limits.session_peer_queue_frames",
        "session_peer_queue_frames",
        &c.config.session.peer_queue.to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "limits.pending_mib",
        "pending_mib",
        &(c.config.pending_bytes >> 20).to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "limits.pending_scratch_mib",
        "pending_scratch_mib",
        &c.config
            .pending_scratch_bytes
            .div_ceil(1024 * 1024)
            .to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "limits.log_quota_mib",
        "log_quota_mib",
        &(c.config.log_quota_bytes >> 20).to_string(),
    );
    value(
        &mut out,
        &c.sources,
        "limits.memory_budget_mib",
        "memory_budget_mib",
        &(c.config.memory_budget_bytes >> 20).to_string(),
    );

    section(&mut out, "retention");
    optional(
        &mut out,
        &c.sources,
        "retention.expire_after",
        "expire_after",
        c.expire_after.as_deref(),
    );
    optional(
        &mut out,
        &c.sources,
        "retention.expire_from",
        "expire_from",
        c.expire_from.as_deref(),
    );
    section(&mut out, "assets");
    value(
        &mut out,
        &c.sources,
        "assets.mirror",
        "mirror",
        &quote(&c.asset_mirror),
    );
    optional(
        &mut out,
        &c.sources,
        "assets.typst_fonts",
        "typst_fonts",
        c.typst_fonts.as_deref(),
    );
    section(&mut out, "metrics");
    let metrics_address = c.metrics_address.map(|value| value.to_string());
    optional(
        &mut out,
        &c.sources,
        "metrics.address",
        "address",
        metrics_address.as_deref(),
    );
    section(&mut out, "proxy");
    let proxies: Vec<String> = c
        .config
        .cost
        .trusted_proxies
        .iter()
        .map(ToString::to_string)
        .collect();
    value(
        &mut out,
        &c.sources,
        "proxy.trusted_networks",
        "trusted_networks",
        &array(&proxies),
    );
    section(&mut out, "backup");
    optional(
        &mut out,
        &c.sources,
        "backup.destination_class",
        "destination_class",
        c.config.backup.destination_class.as_deref(),
    );
    optional_num(
        &mut out,
        &c.sources,
        "backup.interval",
        "interval",
        c.config.backup.frequency,
    );
    optional_num(
        &mut out,
        &c.sources,
        "backup.retained_count",
        "retained_count",
        c.config.backup.retained_count,
    );
    optional_num(
        &mut out,
        &c.sources,
        "backup.encrypted",
        "encrypted",
        c.config.backup.encrypted,
    );
    value(
        &mut out,
        &c.sources,
        "backup.warning_count",
        "warning_count",
        &c.config.backup.warning_count.to_string(),
    );
    section(&mut out, "demo");
    optional_num(
        &mut out,
        &c.sources,
        "demo.simulate_activity_days",
        "simulate_activity_days",
        c.simulate_activity,
    );
    section(&mut out, "logging");
    value(
        &mut out,
        &c.sources,
        "logging.filter",
        "filter",
        &quote(&c.log_filter),
    );
    out
}

fn quote(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}
fn array(values: &[String]) -> String {
    toml::Value::Array(values.iter().cloned().map(toml::Value::String).collect()).to_string()
}
fn section(out: &mut String, name: &str) {
    out.push_str(&format!("\n[{name}]\n"));
}
fn source(out: &mut String, sources: &BTreeMap<String, String>, key: &str) {
    if let Some(from) = sources.get(key) {
        let safe: String = from
            .chars()
            .map(|ch| if ch == '\r' || ch == '\n' { ' ' } else { ch })
            .collect();
        out.push_str(&format!("# source: {safe}\n"));
    }
}
fn value(out: &mut String, sources: &BTreeMap<String, String>, path: &str, key: &str, value: &str) {
    source(out, sources, path);
    out.push_str(&format!("{key} = {value}\n"));
}
fn optional(
    out: &mut String,
    sources: &BTreeMap<String, String>,
    path: &str,
    key: &str,
    value: Option<&str>,
) {
    source(out, sources, path);
    match value {
        Some(v) => out.push_str(&format!("{key} = {}\n", quote(v))),
        None => out.push_str(&format!("# {key} is unset\n")),
    }
}
fn optional_num<T: std::fmt::Display>(
    out: &mut String,
    sources: &BTreeMap<String, String>,
    path: &str,
    key: &str,
    value: Option<T>,
) {
    source(out, sources, path);
    match value {
        Some(v) => out.push_str(&format!("{key} = {v}\n")),
        None => out.push_str(&format!("# {key} is unset\n")),
    }
}
fn redacted(
    out: &mut String,
    sources: &BTreeMap<String, String>,
    path: &str,
    key: &str,
    configured: bool,
) {
    source(out, sources, path);
    if configured {
        out.push_str(&format!("{key} = \"<redacted>\"\n"));
    } else {
        out.push_str(&format!("# {key} is unset\n"));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, sync::Mutex};
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    struct RestoreEnv(Vec<(String, Option<OsString>)>);
    impl RestoreEnv {
        fn set(entries: &[(&str, &str)]) -> Self {
            let saved = entries
                .iter()
                .map(|(key, value)| {
                    let previous = std::env::var_os(key);
                    std::env::set_var(key, value);
                    ((*key).to_owned(), previous)
                })
                .collect();
            Self(saved)
        }
    }
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }
    #[test]
    fn public_url_credentials_and_queries_are_rejected_without_echoing_secrets() {
        let fields = [
            ("origins", "app", "origins.app"),
            ("origins", "docs", "origins.docs"),
            ("origins", "site", "origins.site"),
            ("assets", "mirror", "assets.mirror"),
        ];
        let bad_urls = [
            "https://user:url-userinfo-secret@example.org",
            "https://example.org/?token=url-query-secret",
            "https://user:malformed-url-secret@[",
        ];

        for (section, key, field) in fields {
            for url in bad_urls {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("config.toml");
                let contents = format!("[{section}]\n{key} = {url:?}\n");
                std::fs::write(&path, contents).unwrap();

                let error = load(&path).unwrap_err();
                assert!(error.contains(field), "{error}");
                assert!(!error.contains("url-userinfo-secret"));
                assert!(!error.contains("url-query-secret"));
                assert!(!error.contains("malformed-url-secret"));
            }
        }
    }

    #[test]
    fn accepts_well_formed_public_urls() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[server]\n[origins]\napp = \"https://app.example.org\"\ndocs = \"https://docs.example.org\"\nsite = \"https://example.org\"\n[assets]\nmirror = \"https://assets.example.org/prefix\"",
        )
        .unwrap();
        assert!(load(&path).is_ok());
    }

    #[test]
    fn safe_parse_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(
            &p,
            "[auth.github]\nclient_secret = \"SECRET-INPUT\"\nport =",
        )
        .unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("line 3, column"));
        assert!(!e.to_string().contains("SECRET-INPUT"));
    }
    #[test]
    fn unknown_keys() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(
            &p,
            "[auth.github]\nclient_secret = \"SECRET-INPUT\"\nwat = 1",
        )
        .unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("auth.github.wat"));
        assert!(!e.contains("SECRET-INPUT"));
    }
    #[test]
    fn file_refs_are_relative_and_trim_crlf() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(d.path().join("secret"), "secret\r\n").unwrap();
        std::fs::write(
            &p,
            "[auth.github]\nclient_id = \"id\"\nclient_secret = { file = \"secret\" }",
        )
        .unwrap();
        assert_eq!(
            load(&p).unwrap().github_client_secret.as_deref(),
            Some("secret")
        );
    }
    #[test]
    fn empty_config_uses_schema_defaults() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "").unwrap();
        let loaded = load(&p).unwrap();
        assert_eq!(loaded.address.port(), 8080);
        assert_eq!(loaded.storage.database_url, "postgresql:///librepaper");
        let scratch_mb = Configuration::default()
            .pending_scratch_bytes
            .div_ceil(1024 * 1024);
        assert_eq!(
            loaded.config.pending_scratch_bytes,
            scratch_mb * 1024 * 1024
        );
        assert!(
            loaded.config.pending_scratch_bytes >= Configuration::default().pending_scratch_bytes
        );
        assert!(show_resolved(&loaded).contains(&format!("pending_scratch_mib = {scratch_mb}")));
    }
    #[test]
    fn resolves_explicit_env_and_array_refs() {
        let _lock = ENV_LOCK.lock().unwrap();
        let address = format!("LIBREPAPER_CONFIG_TEST_ADDRESS_{}", std::process::id());
        let publishers = format!("LIBREPAPER_CONFIG_TEST_PUBLISHERS_{}", std::process::id());
        let _restore = RestoreEnv::set(&[(&address, "127.0.0.1:9091"), (&publishers, "[\"alice\", \"bob\"]")]);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        let text=format!("[server]\naddress = {{ env = \"{address}\" }}\n[access]\npublishers = {{ env = \"{publishers}\" }}");
        std::fs::write(&p, text).unwrap();
        let loaded = load(&p).unwrap();
        assert_eq!(loaded.address.port(), 9091);
        assert_eq!(
            loaded.publishers,
            vec!["alice".to_owned(), "bob".to_owned()]
        );
    }
    #[test]
    fn missing_and_wrong_type_refs_are_safe() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p,"[auth.github]\nclient_id = \"id\"\nclient_secret = { env = \"LIBREPAPER_MISSING_CONFIG_SECRET\" }").unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("LIBREPAPER_MISSING_CONFIG_SECRET"));
        assert!(!e.contains("secret-value"));
        std::fs::write(&p, "[server]\naddress = \"not-a-socket-addr-secret\"").unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("server.address"));
        assert!(!e.contains("not-a-socket-addr-secret"));
    }
    #[test]
    fn reference_requires_exactly_one_source() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[origins]\napp = { env = \"A\", file = \"B\" }").unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("exactly one"));
    }
    #[test]
    fn missing_file_reference_names_path_without_contents() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(
            &p,
            "[auth.github]\nclient_id = \"id\"\nclient_secret = { file = \"missing-secret-file\" }",
        )
        .unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("missing-secret-file"));
        assert!(!e.contains("secret-value"));
    }
    #[test]
    fn storage_only_loader_skips_oauth_references() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p,"[auth.github]\nclient_id = \"id\"\nclient_secret = { env = \"LIBREPAPER_MISSING_OAUTH_SECRET\" }").unwrap();
        assert!(load_storage_with_log_filter(&p).is_ok());
    }
    #[test]
    fn rejects_mb_values_that_overflow_storage_bytes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[limits]\ndeployment_storage_mib = 10000000000000").unwrap();
        let e = load(&p).unwrap_err();
        assert!(e.contains("limits.deployment_storage_mib"));
    }
    #[test]
    fn relative_storage_directory_uses_config_directory() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[storage]\ndirectory = \"state/data\"").unwrap();
        let loaded = load_storage_with_log_filter(&p).unwrap();
        assert_eq!(loaded.0.dir, d.path().join("state/data"));
    }
    #[test]
    fn explicit_scratch_is_preserved_when_log_quota_changes() {
        let old_default_mb = Configuration::default()
            .pending_scratch_bytes
            .div_ceil(1024 * 1024);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        let text=format!("[limits]\nlog_quota_mib = 28\npending_scratch_mib = {old_default_mb}\nmemory_budget_mib = 512");
        std::fs::write(&p, text).unwrap();
        let loaded = load(&p).unwrap();
        assert_eq!(loaded.config.log_quota_bytes, 28 * 1024 * 1024);
        assert_eq!(
            loaded.config.pending_scratch_bytes,
            old_default_mb * 1024 * 1024
        );
    }
    #[test]
    fn display_redacts_database_and_auth_and_is_toml() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p,"[storage]\ndatabase_url = \"postgres://user:DBSECRET@host/db\"\n[auth.google]\nclient_id = \"id\"\nclient_secret = \"OAUTHSECRET\"").unwrap();
        let loaded = load(&p).unwrap();
        let s = show_resolved(&loaded);
        assert!(!s.contains("DBSECRET"));
        assert!(!s.contains("OAUTHSECRET"));
        assert!(s.contains("<redacted>"));
        assert!(toml::from_str::<toml::Value>(&s).is_ok());
        assert!(s.contains("# source: default"));
    }
}
