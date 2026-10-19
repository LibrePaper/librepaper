//! Explicit TOML configuration for server deployments. Environment lookup is
//! permitted only when a leaf explicitly names an env reference.
use librepaper_base::{
    auth::{
        acceptable_provider_url, normalize_entry, trim_issuer, valid_oidc_slug, OidcProvider,
        Policy, PROVIDER_ENTRA, PROVIDER_GITHUB, PROVIDER_GOOGLE, PROVIDER_ORCID, SESSION_MAX_AGE,
    },
    config::{Configuration, DEFAULT_LOG_QUOTA_BYTES, DEFAULT_PENDING_BYTES},
};
use librepaper_engine::storage::StorageOptions;
use serde::{de::DeserializeOwned, Deserialize};
use std::fmt;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    net::SocketAddr,
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
    pub(crate) admin_origin: Option<String>,
    pub(crate) admin_github_ids: Vec<i64>,
    pub(crate) migrate: bool,
    pub(crate) storage: StorageOptions,
    pub(crate) github_client_id: Option<String>,
    pub(crate) github_client_secret: Option<String>,
    pub(crate) oidc: Vec<OidcProvider>,
    pub(crate) session_lifetime: Option<std::time::Duration>,
    session_lifetime_text: Option<String>,
    pub(crate) publishers: Policy,
    pub(crate) commenters: Policy,
    pub(crate) simulate_activity: Option<u32>,
    pub(crate) expire_after: Option<String>,
    pub(crate) expire_from: Option<String>,
    pub(crate) erase_inactive_after: Option<String>,
    pub(crate) asset_mirror: String,
    pub(crate) metrics_address: Option<SocketAddr>,
    pub(crate) log_filter: String,
    pub(crate) config: Configuration,
    sources: BTreeMap<String, String>,
}

pub(crate) struct DatabaseOptions {
    pub(crate) database_url: String,
    pub(crate) database_connections: u32,
}

impl fmt::Debug for ResolvedConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedConfig")
            .field("address", &self.address)
            .field("app_origin", &self.app_origin)
            .field("docs_origin", &self.docs_origin)
            .field("site_origin", &self.site_origin)
            .field("admin_origin", &self.admin_origin)
            .field("admin_github_ids", &self.admin_github_ids)
            .field("migrate", &self.migrate)
            .field("storage", &"<redacted>")
            .field("github_client_id", &self.github_client_id)
            .field(
                "github_client_secret",
                &self.github_client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("oidc_count", &self.oidc.len())
            .field(
                "oidc_ids",
                &self
                    .oidc
                    .iter()
                    .map(|provider| provider.id.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("session_lifetime", &self.session_lifetime)
            .field("publishers", &self.publishers)
            .field("commenters", &self.commenters)
            .field("simulate_activity", &self.simulate_activity)
            .field("expire_after", &self.expire_after)
            .field("expire_from", &self.expire_from)
            .field("erase_inactive_after", &self.erase_inactive_after)
            .field("asset_mirror", &self.asset_mirror)
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
    admin: Admin,
    storage: Storage,
    auth: Auth,
    access: Access,
    limits: Limits,
    retention: Retention,
    assets: Assets,
    metrics: Metrics,
    proxy: Proxy,
    demo: Demo,
    logging: Logging,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Server {
    address: Option<Input<SocketAddr>>,
    migrate: Option<Input<bool>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Origins {
    app: Option<Input<String>>,
    docs: Option<Input<String>>,
    site: Option<Input<String>>,
    admin: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Admin {
    github_ids: Option<Input<Vec<i64>>>,
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
    entra: Entra,
    orcid: Orcid,
    oidc: BTreeMap<String, Custom>,
    session_lifetime: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OAuth {
    client_id: Option<Input<String>>,
    client_secret: Option<Input<String>>,
    publishers: Option<Input<Vec<String>>>,
    commenters: Option<Input<Vec<String>>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Orcid {
    client_id: Option<Input<String>>,
    client_secret: Option<Input<String>>,
    publishers: Option<Input<Vec<String>>>,
    commenters: Option<Input<Vec<String>>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Entra {
    tenant: Option<Input<String>>,
    client_id: Option<Input<String>>,
    client_secret: Option<Input<String>>,
    label: Option<Input<String>>,
    publishers: Option<Input<Vec<String>>>,
    commenters: Option<Input<Vec<String>>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Custom {
    issuer: Option<Input<String>>,
    client_id: Option<Input<String>>,
    client_secret: Option<Input<String>>,
    label: Option<Input<String>>,
    scopes: Option<Input<String>>,
    publishers: Option<Input<Vec<String>>>,
    commenters: Option<Input<Vec<String>>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Access {
    anonymous_comments: Option<Input<bool>>,
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
    erase_inactive_after: Option<Input<String>>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Assets {
    mirror: Option<Input<String>>,
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
/// The access lists used to live under `[access]`. A deployment still written
/// that way gets the new location rather than a bare unknown-key error.
fn check_moved_keys(v: &toml::Value) -> Result<(), String> {
    let Some(access) = v.get("access") else {
        return Ok(());
    };
    if access.get("publishers").is_some() {
        return Err("access.publishers has moved: list publishers under each [auth.<provider>] table, for example [auth.github] publishers = [\"alice\"] or [auth.google] publishers = [\"@example.org\"]".into());
    }
    if access.get("commenters").is_some() {
        return Err("access.commenters has moved: list commenters under each [auth.<provider>] table, for example [auth.github] commenters = [\"*\"] or [auth.google] commenters = [\"@example.org\"]; for comments without sign-in set [access] anonymous_comments = true".into());
    }
    Ok(())
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
    if v.get("resticprofile").is_some() {
        return Err("[resticprofile] belongs in /etc/resticprofile/resticprofile.toml; move backup settings out of the application configuration".into());
    }
    table(
        v,
        "",
        &[
            "server",
            "origins",
            "admin",
            "storage",
            "auth",
            "access",
            "limits",
            "retention",
            "assets",
            "metrics",
            "proxy",
            "demo",
            "logging",
        ],
    )?;
    section(v, "server", &["address", "migrate"])?;
    section(v, "origins", &["app", "docs", "site", "admin"])?;
    section(v, "admin", &["github_ids"])?;
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
    section(
        v,
        "auth",
        &["github", "google", "entra", "orcid", "oidc", "session_lifetime"],
    )?;
    if let Some(x) = v.get("auth") {
        for p in ["github", "google"] {
            if let Some(y) = x.get(p) {
                table(
                    y,
                    &format!("auth.{p}."),
                    &["client_id", "client_secret", "publishers", "commenters"],
                )?;
            }
        }
        if let Some(y) = x.get("orcid") {
            table(
                y,
                "auth.orcid.",
                &["client_id", "client_secret", "publishers", "commenters"],
            )?;
        }
        if let Some(y) = x.get("entra") {
            table(
                y,
                "auth.entra.",
                &[
                    "tenant",
                    "client_id",
                    "client_secret",
                    "label",
                    "publishers",
                    "commenters",
                ],
            )?;
        }
        if let Some(providers) = x.get("oidc").and_then(toml::Value::as_table) {
            for (slug, y) in providers {
                table(
                    y,
                    &format!("auth.oidc.{slug}."),
                    &[
                        "issuer",
                        "client_id",
                        "client_secret",
                        "label",
                        "scopes",
                        "publishers",
                        "commenters",
                    ],
                )?;
            }
        }
    }
    section(v, "access", &["anonymous_comments"])?;
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
    section(
        v,
        "retention",
        &["expire_after", "expire_from", "erase_inactive_after"],
    )?;
    section(v, "assets", &["mirror"])?;
    section(v, "metrics", &["address"])?;
    section(v, "proxy", &["trusted_networks"])?;
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
    check_moved_keys(&v)?;
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

/// Default tracing filter. Loro logs a block dump at INFO on every export.
pub(crate) const DEFAULT_LOG_FILTER: &str = "info,loro_internal=warn";

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
    let filter = resolver.value(
        "logging.filter",
        raw.logging.filter,
        DEFAULT_LOG_FILTER.to_owned(),
    )?;
    tracing_subscriber::EnvFilter::try_new(&filter)
        .map_err(|_| "logging.filter is not a valid filter directive".to_owned())?;
    Ok((storage, filter))
}

/// Resolve only database settings and their explicitly referenced secrets.
/// One-shot migration commands do not need runtime auth, listener, or object
/// storage credentials.
pub(crate) fn load_database_with_env(
    path: &Path,
    postgres_env: Option<&PostgresEnvSnapshot>,
) -> Result<DatabaseOptions, String> {
    let (raw, base) = read_raw(path)?;
    let mut resolver = Resolver {
        base: &base,
        sources: BTreeMap::new(),
        postgres_env,
    };
    let database_url = resolver.value(
        "storage.database_url",
        raw.storage.database_url,
        "postgresql:///librepaper".to_owned(),
    )?;
    let database_connections = resolver.value(
        "storage.database_connections",
        raw.storage.database_connections,
        20,
    )?;
    librepaper_engine::storage::validate_database_options(&database_url, database_connections)?;
    Ok(DatabaseOptions {
        database_url,
        database_connections,
    })
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
    let migrate = r.value("server.migrate", raw.server.migrate, true)?;
    let app_origin = r.optional("origins.app", raw.origins.app)?;
    let docs_origin = r.optional("origins.docs", raw.origins.docs)?;
    let site_origin = r.optional("origins.site", raw.origins.site)?;
    let admin_origin = r.optional("origins.admin", raw.origins.admin)?;
    let admin_github_ids = r.value("admin.github_ids", raw.admin.github_ids, Vec::new())?;
    let storage = resolve_storage(raw.storage, &mut r)?;
    let github_client_id = r.optional("auth.github.client_id", raw.auth.github.client_id)?;
    let github_client_secret =
        r.optional("auth.github.client_secret", raw.auth.github.client_secret)?;
    let anonymous_comments = r.value(
        "access.anonymous_comments",
        raw.access.anonymous_comments,
        false,
    )?;
    let expire_after = r.optional("retention.expire_after", raw.retention.expire_after)?;
    let erase_inactive_after = r.optional(
        "retention.erase_inactive_after",
        raw.retention.erase_inactive_after,
    )?;
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
    let metrics_address = r.optional("metrics.address", raw.metrics.address)?;
    let log_filter = r.value(
        "logging.filter",
        raw.logging.filter,
        DEFAULT_LOG_FILTER.into(),
    )?;
    tracing_subscriber::EnvFilter::try_new(&log_filter)
        .map_err(|_| "logging.filter is not a valid filter directive".to_owned())?;
    let simulate_activity = r.optional(
        "demo.simulate_activity_days",
        raw.demo.simulate_activity_days,
    )?;
    let mut config = Configuration::default();
    let l = raw.limits;
    let publisher_storage_mb =
        r.optional("limits.publisher_storage_mib", l.publisher_storage_mib)?;
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
    config.set_peer_queue(r.optional(
        "limits.session_peer_queue_frames",
        l.session_peer_queue_frames,
    )?)?;
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
    let mut lists = Lists::default();
    resolve_lists(
        "auth.github",
        PROVIDER_GITHUB,
        github_client_id.is_some(),
        raw.auth.github.publishers,
        raw.auth.github.commenters,
        &mut r,
        &mut lists,
    )?;
    let (oidc, session_lifetime_text, session_lifetime) = resolve_auth(
        raw.auth.google,
        raw.auth.entra,
        raw.auth.orcid,
        raw.auth.oidc,
        raw.auth.session_lifetime,
        &mut r,
        &mut lists,
    )?;
    validate_public_urls(
        app_origin.as_deref(),
        docs_origin.as_deref(),
        site_origin.as_deref(),
        admin_origin.as_deref(),
        &asset_mirror,
    )?;
    let github_configured = non_blank(github_client_id.as_deref()).is_some()
        && non_blank(github_client_secret.as_deref()).is_some();
    validate_admin(
        admin_origin.as_deref(),
        &admin_github_ids,
        github_configured,
        app_origin.as_deref(),
        docs_origin.as_deref(),
    )?;
    Ok(ResolvedConfig {
        address,
        app_origin,
        docs_origin,
        site_origin,
        admin_origin,
        admin_github_ids,
        migrate,
        storage,
        github_client_id,
        github_client_secret,
        oidc,
        session_lifetime,
        session_lifetime_text,
        publishers: lists.publishers,
        commenters: Policy {
            public: anonymous_comments,
            ..lists.commenters
        },
        simulate_activity,
        expire_after,
        expire_from,
        erase_inactive_after,
        asset_mirror,
        metrics_address,
        log_filter,
        config,
        sources: r.sources,
    })
}

/// Largest number of custom OpenID Connect providers.
const MAX_OIDC_PROVIDERS: usize = 8;

/// The providers, the session lifetime as written, and as a duration.
type ResolvedAuth = (
    Vec<OidcProvider>,
    Option<String>,
    Option<std::time::Duration>,
);

/// Resolves the OpenID Connect providers and the browser session lifetime.
/// Providers come out in sign-in page order: Google, Entra, ORCID, then the
/// custom entries sorted by slug. Each configured provider's publishers and
/// commenters lists are added to `lists` as it is read.
fn resolve_auth(
    google: OAuth,
    entra: Entra,
    orcid: Orcid,
    custom: BTreeMap<String, Custom>,
    session_lifetime: Option<Input<String>>,
    r: &mut Resolver<'_>,
    lists: &mut Lists,
) -> Result<ResolvedAuth, String> {
    let mut providers = Vec::new();

    let google_id = r.optional("auth.google.client_id", google.client_id)?;
    let google_secret = r.optional("auth.google.client_secret", google.client_secret)?;
    let google_credentials = credentials("google", google_id, google_secret)?;
    resolve_lists(
        "auth.google",
        PROVIDER_GOOGLE,
        google_credentials.is_some(),
        google.publishers,
        google.commenters,
        r,
        lists,
    )?;
    if let Some((client_id, client_secret)) = google_credentials {
        providers.push(OidcProvider::google(&client_id, &client_secret));
    }

    let tenant = r.optional("auth.entra.tenant", entra.tenant)?;
    let entra_id = r.optional("auth.entra.client_id", entra.client_id)?;
    let entra_secret = r.optional("auth.entra.client_secret", entra.client_secret)?;
    let entra_label = r.optional("auth.entra.label", entra.label)?;
    let entra_set =
        tenant.is_some() || entra_id.is_some() || entra_secret.is_some() || entra_label.is_some();
    let entra_credentials = credentials("entra", entra_id, entra_secret)?;
    resolve_lists(
        "auth.entra",
        PROVIDER_ENTRA,
        entra_credentials.is_some(),
        entra.publishers,
        entra.commenters,
        r,
        lists,
    )?;
    match entra_credentials {
        Some((client_id, client_secret)) => {
            let tenant = entra_tenant_id(&tenant.ok_or("auth.entra.tenant is required")?)?;
            if let Some(label) = &entra_label {
                check_label("auth.entra.label", label)?;
            }
            providers.push(OidcProvider::entra(
                &tenant,
                &client_id,
                &client_secret,
                entra_label.as_deref(),
            ));
        }
        None if entra_set => {
            return Err(
                "auth.entra.client_id and client_secret must be set with the other auth.entra settings"
                    .into(),
            );
        }
        None => {}
    }

    let orcid_id = r.optional("auth.orcid.client_id", orcid.client_id)?;
    let orcid_secret = r.optional("auth.orcid.client_secret", orcid.client_secret)?;
    let orcid_credentials = credentials("orcid", orcid_id, orcid_secret)?;
    resolve_lists(
        "auth.orcid",
        PROVIDER_ORCID,
        orcid_credentials.is_some(),
        orcid.publishers,
        orcid.commenters,
        r,
        lists,
    )?;
    if let Some((client_id, client_secret)) = orcid_credentials {
        providers.push(OidcProvider::orcid(&client_id, &client_secret));
    }

    if custom.len() > MAX_OIDC_PROVIDERS {
        return Err("auth.oidc: at most 8 providers".into());
    }
    for (slug, entry) in custom {
        if !valid_oidc_slug(&slug) {
            return Err(format!(
                "auth.oidc.{slug}: the name must match ^[a-z][a-z0-9-]{{0,23}}$"
            ));
        }
        let key = |field: &str| format!("auth.oidc.{slug}.{field}");
        let client_id = r.optional(&key("client_id"), entry.client_id)?;
        let client_secret = r.optional(&key("client_secret"), entry.client_secret)?;
        let Some((client_id, client_secret)) =
            credentials(&format!("oidc.{slug}"), client_id, client_secret)?
        else {
            return Err(format!(
                "auth.oidc.{slug}.client_id and client_secret must be set"
            ));
        };
        let issuer = r.optional(&key("issuer"), entry.issuer)?;
        let issuer = issuer.ok_or_else(|| format!("auth.oidc.{slug}.issuer is required"))?;
        check_issuer(&key("issuer"), &issuer)?;
        let label = r.optional(&key("label"), entry.label)?;
        let label = label.ok_or_else(|| format!("auth.oidc.{slug}.label is required"))?;
        check_label(&key("label"), &label)?;
        let scopes = r.optional(&key("scopes"), entry.scopes)?;
        if let Some(scopes) = &scopes {
            check_scopes(&key("scopes"), scopes)?;
        }
        let provider = OidcProvider::custom(
            &slug,
            &issuer,
            &client_id,
            &client_secret,
            &label,
            scopes.as_deref(),
        );
        resolve_lists(
            &format!("auth.oidc.{slug}"),
            &provider.id,
            true,
            entry.publishers,
            entry.commenters,
            r,
            lists,
        )?;
        providers.push(provider);
    }
    check_issuers(&providers)?;

    // Absent, a browser session and a device token keep their own lifetimes.
    // Set, it bounds both. The dump shows the text as the operator wrote it.
    let configured = r.optional("auth.session_lifetime", session_lifetime)?;
    let session_lifetime = match &configured {
        None => None,
        Some(text) => {
            // Parse failures and never, off and zero all fall outside the range.
            let seconds =
                librepaper_document::document::retention::parse_retention(text).unwrap_or(0);
            let longest = SESSION_MAX_AGE.as_secs() as i64;
            if !(3600..=longest).contains(&seconds) {
                return Err("auth.session_lifetime must be between 1h and 30d".into());
            }
            Some(std::time::Duration::from_secs(seconds as u64))
        }
    };
    Ok((providers, configured, session_lifetime))
}

/// The publishers and commenters policies gathered from the provider tables.
#[derive(Default)]
struct Lists {
    publishers: Policy,
    commenters: Policy,
}

/// Reads one provider table's publishers and commenters lists and adds them to
/// `lists`. Publishers default to nobody and commenters to any account of the
/// provider. A table without credentials offers no sign-in, so a list on it is
/// a mistake and not something to carry silently. `table` is the TOML path and
/// `provider` the id the sign-in produces, which is what entries are checked
/// against.
fn resolve_lists(
    table: &str,
    provider: &str,
    configured: bool,
    publishers: Option<Input<Vec<String>>>,
    commenters: Option<Input<Vec<String>>>,
    r: &mut Resolver<'_>,
    lists: &mut Lists,
) -> Result<(), String> {
    for (field, input) in [("publishers", &publishers), ("commenters", &commenters)] {
        if input.is_some() && !configured {
            return Err(format!(
                "{table}.{field} is set but {table} has no client_id and client_secret"
            ));
        }
    }
    if !configured {
        return Ok(());
    }
    let publishers = r.value(&format!("{table}.publishers"), publishers, Vec::new())?;
    let commenters = r.value(
        &format!("{table}.commenters"),
        commenters,
        vec!["*".to_string()],
    )?;
    let normalize = |field: &str, entries: Vec<String>| -> Result<Vec<String>, String> {
        entries
            .iter()
            .map(|entry| {
                normalize_entry(provider, entry).map_err(|reason| format!("{table}.{field}: {reason}"))
            })
            .collect()
    };
    let publishers = normalize("publishers", publishers)?;
    let commenters = normalize("commenters", commenters)?;
    lists.publishers = std::mem::take(&mut lists.publishers).with(
        provider,
        &publishers.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    lists.commenters = std::mem::take(&mut lists.commenters).with(
        provider,
        &commenters.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    Ok(())
}

/// client_id and client_secret come as a pair. Neither set means the table
/// is not configured at all.
fn credentials(
    table: &str,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<Option<(String, String)>, String> {
    match (client_id, client_secret) {
        (None, None) => Ok(None),
        (Some(client_id), Some(client_secret)) => {
            if client_id.trim().is_empty() || client_secret.trim().is_empty() {
                return Err(format!(
                    "auth.{table}.client_id and client_secret must not be blank"
                ));
            }
            Ok(Some((client_id, client_secret)))
        }
        _ => Err(format!(
            "auth.{table}.client_id and client_secret must be configured together"
        )),
    }
}

/// Entra writes the tenant GUID in lowercase in its issuer, and the issuer
/// check is exact, so the GUID is lowercased here. One tenant only.
fn entra_tenant_id(value: &str) -> Result<String, String> {
    let tenant = value.to_ascii_lowercase();
    if matches!(tenant.as_str(), "common" | "organizations" | "consumers") {
        return Err(
            "auth.entra.tenant must name one tenant; multi-tenant sign-in is not supported".into(),
        );
    }
    if !is_guid(&tenant) {
        return Err(
            "auth.entra.tenant must be the Directory (tenant) ID, a GUID; a domain name produces a different issuer"
                .into(),
        );
    }
    Ok(tenant)
}

fn is_guid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(group, length)| {
            group.len() == length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

/// The tenant is the path segment of the issuer that the Entra preset builds.
fn tenant_of_entra_issuer(issuer: &str) -> &str {
    issuer
        .strip_prefix("https://login.microsoftonline.com/")
        .and_then(|rest| rest.strip_suffix("/v2.0"))
        .unwrap_or(issuer)
}

/// Issuers are operator text that the server fetches from, so they must be
/// https, or http on loopback, with no query and no fragment.
fn check_issuer(key: &str, issuer: &str) -> Result<(), String> {
    let refused = || {
        format!(
            "{key} must be an https:// URL (http:// only on localhost) with no query or fragment"
        )
    };
    let parsed = url::Url::parse(issuer).map_err(|_| refused())?;
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(refused());
    }
    acceptable_provider_url(issuer).map_err(|_| refused())
}

/// One identity provider reached under two ids would split its users into
/// two sets of accounts. A trailing slash does not make a new issuer.
fn check_issuers(providers: &[OidcProvider]) -> Result<(), String> {
    for (index, provider) in providers.iter().enumerate() {
        let issuer = trim_issuer(&provider.issuer);
        if let Some(earlier) = providers[..index]
            .iter()
            .find(|other| trim_issuer(&other.issuer) == issuer)
        {
            let key = match provider.id.strip_prefix("oidc-") {
                Some(slug) => format!("auth.oidc.{slug}.issuer"),
                None => format!("auth.{}.issuer", provider.id),
            };
            return Err(format!("{key} duplicates the issuer of {}", earlier.id));
        }
    }
    Ok(())
}

/// Labels are shown on the sign-in page, so blank, long and control
/// characters are refused.
fn check_label(key: &str, label: &str) -> Result<(), String> {
    if label.trim().is_empty() || label.chars().count() > 40 || label.chars().any(char::is_control)
    {
        return Err(format!(
            "{key} must be 1 to 40 characters with no control characters"
        ));
    }
    Ok(())
}

/// The id token only comes back when the scopes ask for openid.
fn check_scopes(key: &str, scopes: &str) -> Result<(), String> {
    let openid = scopes.split_whitespace().any(|scope| scope == "openid");
    if !openid || scopes.chars().count() > 200 {
        return Err(format!(
            "{key} must contain openid and be at most 200 characters"
        ));
    }
    Ok(())
}

fn validate_public_urls(
    app_origin: Option<&str>,
    docs_origin: Option<&str>,
    site_origin: Option<&str>,
    admin_origin: Option<&str>,
    asset_mirror: &str,
) -> Result<(), String> {
    validate_public_url("origins.app", app_origin, false, true, true)?;
    validate_public_url("origins.docs", docs_origin, false, true, true)?;
    validate_public_url("origins.site", site_origin, false, true, true)?;
    validate_public_url("origins.admin", admin_origin, false, true, true)?;
    validate_public_url("assets.mirror", Some(asset_mirror), true, false, false)
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn host_of(origin: &str) -> Option<String> {
    url::Url::parse(origin)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_lowercase))
}

/// The admin console signs in with GitHub and admits only the listed GitHub
/// user IDs. It has a host of its own, and https off loopback.
fn validate_admin(
    admin_origin: Option<&str>,
    github_ids: &[i64],
    github_configured: bool,
    app_origin: Option<&str>,
    docs_origin: Option<&str>,
) -> Result<(), String> {
    let admin = match (non_blank(admin_origin), github_ids.is_empty()) {
        (None, true) => return Ok(()),
        (Some(admin), false) => admin,
        _ => {
            return Err("origins.admin and admin.github_ids must be configured together".into());
        }
    };
    if github_ids.iter().any(|id| *id <= 0) {
        return Err("admin.github_ids holds numeric GitHub user IDs, which are positive".into());
    }
    if !github_configured {
        return Err("the admin console signs in with GitHub; configure [auth.github]".into());
    }
    let parsed =
        url::Url::parse(admin).map_err(|_| "origins.admin must be an absolute URL".to_string())?;
    let loopback = match parsed.host() {
        Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err("origins.admin must be an https: URL, or http: on loopback".into());
    }
    let Some(app) = non_blank(app_origin) else {
        return Err("origins.admin requires origins.app".into());
    };
    let admin_host = host_of(admin);
    if admin_host == host_of(app) {
        return Err(
            "origins.admin must be a different host from origins.app: the page must not share a host with the reader's session"
                .into(),
        );
    }
    // Without origins.docs the documents are served from docs.<app host>.
    let docs_host = match non_blank(docs_origin) {
        Some(docs) => host_of(docs),
        None => host_of(app).map(|host| format!("docs.{host}")),
    };
    if admin_host == docs_host {
        return Err(
            "origins.admin must be a different host from origins.docs: the page must not share a host with documents"
                .into(),
        );
    }
    Ok(())
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
        "server.migrate",
        "migrate",
        &c.migrate.to_string(),
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
    optional(
        &mut out,
        &c.sources,
        "origins.admin",
        "admin",
        c.admin_origin.as_deref(),
    );

    section(&mut out, "admin");
    value(
        &mut out,
        &c.sources,
        "admin.github_ids",
        "github_ids",
        &format!(
            "[{}]",
            c.admin_github_ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
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

    section(&mut out, "auth");
    // Unset means a browser session keeps its 30d and a device token ends
    // with the session that approved it, which no single value would show.
    optional(
        &mut out,
        &c.sources,
        "auth.session_lifetime",
        "session_lifetime",
        c.session_lifetime_text.as_deref(),
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
    list_lines(&mut out, c, "auth.github", "github");
    let google = c.oidc.iter().find(|provider| provider.id == "google");
    section(&mut out, "auth.google");
    optional(
        &mut out,
        &c.sources,
        "auth.google.client_id",
        "client_id",
        google.map(|provider| provider.client_id.as_str()),
    );
    redacted(
        &mut out,
        &c.sources,
        "auth.google.client_secret",
        "client_secret",
        google.is_some(),
    );
    list_lines(&mut out, c, "auth.google", "google");
    if let Some(entra) = c.oidc.iter().find(|provider| provider.id == "entra") {
        section(&mut out, "auth.entra");
        optional(
            &mut out,
            &c.sources,
            "auth.entra.tenant",
            "tenant",
            Some(tenant_of_entra_issuer(&entra.issuer)),
        );
        optional(
            &mut out,
            &c.sources,
            "auth.entra.client_id",
            "client_id",
            Some(entra.client_id.as_str()),
        );
        redacted(
            &mut out,
            &c.sources,
            "auth.entra.client_secret",
            "client_secret",
            true,
        );
        optional(
            &mut out,
            &c.sources,
            "auth.entra.label",
            "label",
            Some(entra.label.as_str()),
        );
        list_lines(&mut out, c, "auth.entra", "entra");
    }
    if let Some(orcid) = c.oidc.iter().find(|provider| provider.id == "orcid") {
        section(&mut out, "auth.orcid");
        optional(
            &mut out,
            &c.sources,
            "auth.orcid.client_id",
            "client_id",
            Some(orcid.client_id.as_str()),
        );
        redacted(
            &mut out,
            &c.sources,
            "auth.orcid.client_secret",
            "client_secret",
            true,
        );
        list_lines(&mut out, c, "auth.orcid", "orcid");
    }
    for provider in c
        .oidc
        .iter()
        .filter(|provider| provider.id.starts_with("oidc-"))
    {
        let slug = &provider.id["oidc-".len()..];
        let key = |field: &str| format!("auth.oidc.{slug}.{field}");
        section(&mut out, &format!("auth.oidc.{slug}"));
        optional(
            &mut out,
            &c.sources,
            &key("issuer"),
            "issuer",
            Some(provider.issuer.as_str()),
        );
        optional(
            &mut out,
            &c.sources,
            &key("client_id"),
            "client_id",
            Some(provider.client_id.as_str()),
        );
        redacted(
            &mut out,
            &c.sources,
            &key("client_secret"),
            "client_secret",
            true,
        );
        optional(
            &mut out,
            &c.sources,
            &key("label"),
            "label",
            Some(provider.label.as_str()),
        );
        optional(
            &mut out,
            &c.sources,
            &key("scopes"),
            "scopes",
            Some(provider.scopes.as_str()),
        );
        list_lines(&mut out, c, &format!("auth.oidc.{slug}"), &provider.id);
    }

    section(&mut out, "access");
    value(
        &mut out,
        &c.sources,
        "access.anonymous_comments",
        "anonymous_comments",
        &c.commenters.public.to_string(),
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
    optional(
        &mut out,
        &c.sources,
        "retention.erase_inactive_after",
        "erase_inactive_after",
        c.erase_inactive_after.as_deref(),
    );
    section(&mut out, "assets");
    value(
        &mut out,
        &c.sources,
        "assets.mirror",
        "mirror",
        &quote(&c.asset_mirror),
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
/// The publishers and commenters lines of one provider's table.
fn list_lines(out: &mut String, c: &ResolvedConfig, table: &str, provider: &str) {
    for (field, policy) in [("publishers", &c.publishers), ("commenters", &c.commenters)] {
        if let Some(entries) = policy.providers.get(provider) {
            value(
                out,
                &c.sources,
                &format!("{table}.{field}"),
                field,
                &array(entries),
            );
        }
    }
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
    fn resticprofile_settings_are_rejected_from_application_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");

        for config in [
            "[resticprofile]\nrepository = \"profile-secret\"\n",
            "[ \"resticprofile\" ]\nrepository = \"profile-secret\"\n",
            "[resticprofile.backup]\nschedule = \"daily\"\n",
            "resticprofile = \"profile-secret\"\n",
        ] {
            std::fs::write(&path, config).unwrap();
            let error = load(&path).unwrap_err();
            assert!(
                error.contains("/etc/resticprofile/resticprofile.toml"),
                "{error}"
            );
            assert!(!error.contains("profile-secret"));
        }
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
    fn load_text(text: &str) -> Result<ResolvedConfig, String> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        load(&path)
    }

    fn admin_config(origins: &str, ids: Option<&str>, github: bool) -> String {
        let mut text = format!("[origins]\n{origins}\n");
        if let Some(ids) = ids {
            text.push_str(&format!("[admin]\ngithub_ids = {ids}\n"));
        }
        if github {
            text.push_str("[auth.github]\nclient_id = \"id\"\nclient_secret = \"secret\"\n");
        }
        text
    }

    #[test]
    fn the_admin_origin_and_github_ids_are_accepted_on_loopback_and_on_https() {
        let loaded = load_text(&admin_config(
            "app = \"http://localhost:8080\"\nadmin = \"http://admin.localhost:8080\"",
            Some("[987057]"),
            true,
        ))
        .unwrap();
        assert_eq!(
            loaded.admin_origin.as_deref(),
            Some("http://admin.localhost:8080")
        );
        assert_eq!(loaded.admin_github_ids, vec![987057]);

        let loaded = load_text(&admin_config(
            "app = \"https://paper.example\"\nadmin = \"https://admin.paper.example\"",
            Some("[987057]"),
            true,
        ))
        .unwrap();
        assert_eq!(
            loaded.admin_origin.as_deref(),
            Some("https://admin.paper.example")
        );
        assert_eq!(loaded.admin_github_ids, vec![987057]);

        let loaded = load_text("").unwrap();
        assert!(loaded.admin_origin.is_none() && loaded.admin_github_ids.is_empty());
    }

    #[test]
    fn the_admin_origin_and_github_ids_are_refused_unless_they_come_together() {
        let error = load_text(&admin_config(
            "app = \"https://paper.example\"\nadmin = \"https://admin.paper.example\"",
            None,
            true,
        ))
        .unwrap_err();
        assert!(error.contains("origins.admin"), "{error}");
        assert!(error.contains("admin.github_ids"), "{error}");
        assert!(error.contains("together"), "{error}");

        let error = load_text(&admin_config(
            "app = \"https://paper.example\"",
            Some("[987057]"),
            true,
        ))
        .unwrap_err();
        assert!(error.contains("origins.admin"), "{error}");
        assert!(error.contains("admin.github_ids"), "{error}");
        assert!(error.contains("together"), "{error}");

        let error = load_text(&admin_config(
            "admin = \"https://admin.paper.example\"",
            Some("[987057]"),
            true,
        ))
        .unwrap_err();
        assert!(
            error.contains("origins.admin requires origins.app"),
            "{error}"
        );
    }

    #[test]
    fn the_admin_github_ids_need_github_sign_in() {
        let error = load_text(&admin_config(
            "app = \"https://paper.example\"\nadmin = \"https://admin.paper.example\"",
            Some("[987057]"),
            false,
        ))
        .unwrap_err();
        assert!(error.contains("auth.github"), "{error}");
    }

    #[test]
    fn the_admin_github_ids_must_be_positive() {
        for ids in ["[0]", "[-7]", "[987057, 0]"] {
            let error = load_text(&admin_config(
                "app = \"https://paper.example\"\nadmin = \"https://admin.paper.example\"",
                Some(ids),
                true,
            ))
            .unwrap_err();
            assert!(error.contains("admin.github_ids"), "{ids}: {error}");
        }
    }

    #[test]
    fn the_admin_host_must_differ_from_the_app_and_document_hosts() {
        for (origins, key) in [
            (
                "app = \"https://paper.example\"\nadmin = \"https://paper.example\"",
                "origins.app",
            ),
            (
                "app = \"https://paper.example\"\ndocs = \"https://sandbox.example\"\nadmin = \"https://sandbox.example\"",
                "origins.docs",
            ),
            (
                "app = \"https://paper.example\"\nadmin = \"https://docs.paper.example\"",
                "origins.docs",
            ),
        ] {
            let error = load_text(&admin_config(origins, Some("[987057]"), true)).unwrap_err();
            assert!(error.contains("origins.admin"), "{origins}: {error}");
            assert!(error.contains(key), "{origins}: {error}");
            assert!(error.contains("different host"), "{origins}: {error}");
        }
    }

    #[test]
    fn plain_http_is_refused_for_the_admin_origin_off_loopback() {
        for admin in [
            "http://admin.paper.example",
            "http://admin.localhost.example",
            "http://10.0.0.5",
        ] {
            let error = load_text(&admin_config(
                &format!("app = \"https://paper.example\"\nadmin = \"{admin}\""),
                Some("[987057]"),
                true,
            ))
            .unwrap_err();
            assert!(
                error.contains("origins.admin must be an https: URL, or http: on loopback"),
                "{admin}: {error}"
            );
        }
        for admin in [
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            let result = load_text(&admin_config(
                &format!("app = \"http://app.example\"\nadmin = \"{admin}\""),
                Some("[987057]"),
                true,
            ));
            assert!(result.is_ok(), "{admin}: {result:?}");
        }
    }

    #[test]
    fn the_admin_github_ids_resolve_and_show_in_the_display() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[origins]\napp = \"https://paper.example\"\nadmin = \"https://admin.paper.example\"\n[admin]\ngithub_ids = [987057, 42]\n[auth.github]\nclient_id = \"id\"\nclient_secret = \"secret\"\n",
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.admin_github_ids, vec![987057, 42]);
        let shown = show_resolved(&loaded);
        assert!(shown.contains("github_ids = [987057, 42]"), "{shown}");
        assert!(
            shown.contains("admin = \"https://admin.paper.example\""),
            "{shown}"
        );
        assert!(toml::from_str::<toml::Value>(&shown).is_ok());

        std::fs::write(&path, "[admin]\nwat = 1\n").unwrap();
        assert!(load(&path).unwrap_err().contains("admin.wat"));
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
        let _restore = RestoreEnv::set(&[
            (&address, "127.0.0.1:9091"),
            (&publishers, "[\"alice\", \"bob\"]"),
        ]);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        let text=format!("[server]\naddress = {{ env = \"{address}\" }}\n[auth.github]\nclient_id = \"id\"\nclient_secret = \"secret\"\npublishers = {{ env = \"{publishers}\" }}");
        std::fs::write(&p, text).unwrap();
        let loaded = load(&p).unwrap();
        assert_eq!(loaded.address.port(), 9091);
        assert_eq!(
            loaded.publishers,
            Policy::allowing("github", &["alice", "bob"])
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
    fn migration_loader_resolves_only_database_settings() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(
            d.path().join("database-url"),
            "postgresql://localhost/librepaper\n",
        )
        .unwrap();
        std::fs::write(
            &p,
            r#"
[storage]
database_url = { file = "database-url" }
object_store = "s3"
[storage.s3]
region = { file = "missing-s3-region" }
bucket = { file = "missing-s3-bucket" }
access_key_id = { file = "missing-s3-access-key" }
secret_access_key = { file = "missing-s3-secret-key" }
[auth.github]
client_id = "configured"
client_secret = { file = "missing-oauth-secret" }
"#,
        )
        .unwrap();

        let database = load_database_with_env(&p, None).unwrap();
        assert_eq!(database.database_url, "postgresql://localhost/librepaper");
        assert_eq!(database.database_connections, 20);

        std::fs::write(
            &p,
            "[storage]\ndatabase_url = { file = \"missing-database-url\" }\n",
        )
        .unwrap();
        let error = load_database_with_env(&p, None).err().unwrap();
        assert!(error.contains("missing-database-url"), "{error}");
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
    fn the_inactive_account_window_is_read_from_retention() {
        let loaded = load_text("[retention]\nerase_inactive_after = \"365d\"\n").unwrap();
        assert_eq!(loaded.erase_inactive_after, Some("365d".into()));
        let shown = show_resolved(&loaded);
        assert!(shown.contains("erase_inactive_after = \"365d\""));
        let unset = load_text("").unwrap();
        assert_eq!(unset.erase_inactive_after, None);
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

    const GITHUB_TABLE: &str = "[auth.github]\nclient_id = \"id\"\nclient_secret = \"secret\"\n";
    const ORCID_TABLE: &str = "[auth.orcid]\nclient_id = \"orcid-id\"\nclient_secret = \"orcid-secret\"\n";

    #[test]
    fn the_old_access_lists_are_refused_with_their_new_home() {
        let error = load_text("[access]\npublishers = [\"alice\"]\n").unwrap_err();
        assert!(error.contains("access.publishers has moved"), "{error}");
        assert!(error.contains("[auth.github] publishers"), "{error}");
        let error = load_text("[access]\ncommenters = [\"*\"]\n").unwrap_err();
        assert!(error.contains("access.commenters has moved"), "{error}");
        assert!(error.contains("anonymous_comments = true"), "{error}");
    }

    #[test]
    fn commenters_default_to_any_account_of_each_configured_provider() {
        let loaded = load_text(&format!("{GITHUB_TABLE}{GOOGLE_TABLE}")).unwrap();
        assert_eq!(
            loaded.commenters,
            Policy::allowing("github", &["*"]).with("google", &["*"])
        );
        assert!(!loaded.publishers.is_configured());
        assert!(!loaded.commenters.public);
        assert!(show_resolved(&loaded).contains("commenters = [\"*\"]"));
    }

    #[test]
    fn lists_are_normalized_for_their_own_provider() {
        let text = format!(
            "{GITHUB_TABLE}publishers = [\"Alice\"]\n{GOOGLE_TABLE}publishers = [\"@Example.org\", \"Bob@Example.org\"]\ncommenters = []\n{ORCID_TABLE}publishers = [\"0000-0002-1694-233x\"]\n"
        );
        let loaded = load_text(&text).unwrap();
        assert_eq!(
            loaded.publishers,
            Policy::allowing("github", &["alice"])
                .with("google", &["@example.org", "bob@example.org"])
                .with("orcid", &["0000-0002-1694-233X"])
        );
        assert_eq!(loaded.commenters.providers["google"], Vec::<String>::new());
        assert_eq!(oidc_ids(&loaded), ["google", "orcid"]);
        assert!(show_resolved(&loaded).contains("[auth.orcid]"));
    }

    #[test]
    fn a_bad_entry_names_its_table_and_reason() {
        let error = load_text(&format!("{GITHUB_TABLE}publishers = [\"@alice\"]\n")).unwrap_err();
        assert_eq!(error, "auth.github.publishers: write the login without @");
        let error = load_text(&format!("{GOOGLE_TABLE}commenters = [\"alice\"]\n")).unwrap_err();
        assert_eq!(
            error,
            "auth.google.commenters: expected an email address, @domain or *"
        );
        let error = load_text(&format!("{ORCID_TABLE}publishers = [\"alice\"]\n")).unwrap_err();
        assert!(error.starts_with("auth.orcid.publishers: expected an ORCID iD"), "{error}");
        let body = format!("{CUSTOM_BODY}publishers = [\"nobody\"]\n");
        let error = load_text(&custom_entry("keycloak", &body)).unwrap_err();
        assert!(error.starts_with("auth.oidc.keycloak.publishers: "), "{error}");
    }

    #[test]
    fn a_custom_provider_list_is_keyed_by_its_provider_id() {
        let body = format!("{CUSTOM_BODY}publishers = [\"@example.org\"]\n");
        let loaded = load_text(&custom_entry("keycloak", &body)).unwrap();
        assert_eq!(
            loaded.publishers,
            Policy::allowing("oidc-keycloak", &["@example.org"])
        );
    }

    #[test]
    fn lists_on_a_table_without_credentials_are_refused() {
        let error = load_text("[auth.github]\npublishers = [\"alice\"]\n").unwrap_err();
        assert_eq!(
            error,
            "auth.github.publishers is set but auth.github has no client_id and client_secret"
        );
        let error = load_text("[auth.orcid]\ncommenters = [\"*\"]\n").unwrap_err();
        assert!(error.contains("auth.orcid.commenters is set"), "{error}");
    }

    #[test]
    fn anonymous_comments_make_the_commenter_policy_public() {
        let loaded = load_text("[access]\nanonymous_comments = true\n").unwrap();
        assert!(loaded.commenters.public);
        assert!(!load_text("").unwrap().commenters.public);
    }

    fn oidc_ids(loaded: &ResolvedConfig) -> Vec<&str> {
        loaded
            .oidc
            .iter()
            .map(|provider| provider.id.as_str())
            .collect()
    }

    const GOOGLE_TABLE: &str =
        "[auth.google]\nclient_id = \"google-id\"\nclient_secret = \"google-secret\"\n";
    const ENTRA_TENANT: &str = "00000000-0000-0000-0000-000000000000";
    const ENTRA_CREDENTIALS: &str = "client_id = \"entra-id\"\nclient_secret = \"entra-secret\"\n";
    const CUSTOM_BODY: &str = "issuer = \"https://sso.example.org/realms/staff\"\nclient_id = \"librepaper\"\nclient_secret = \"custom-secret\"\nlabel = \"Example SSO\"\n";

    fn entra_text(tenant: &str, body: &str) -> String {
        format!("[auth.entra]\ntenant = \"{}\"\n{}", tenant, body)
    }

    fn custom_entry(slug: &str, body: &str) -> String {
        format!("[auth.oidc.{}]\n{}", slug, body)
    }

    #[test]
    fn google_needs_both_credentials_and_non_blank_values() {
        let error = load_text("[auth.google]\nclient_id = \"id\"\n").unwrap_err();
        assert!(
            error.contains("auth.google.client_id and client_secret must be configured together"),
            "{error}"
        );
        let error = load_text("[auth.google]\nclient_secret = \"secret\"\n").unwrap_err();
        assert!(
            error.contains("auth.google.client_id and client_secret must be configured together"),
            "{error}"
        );
        let error = load_text("[auth.google]\nclient_id = \" \"\nclient_secret = \"secret\"\n")
            .unwrap_err();
        assert!(
            error.contains("auth.google.client_id and client_secret must not be blank"),
            "{error}"
        );
        let loaded = load_text(GOOGLE_TABLE).unwrap();
        assert_eq!(oidc_ids(&loaded), ["google"]);
        assert_eq!(loaded.oidc[0].issuer, "https://accounts.google.com");
    }

    #[test]
    fn entra_needs_a_credential_pair_and_one_guid_tenant() {
        let pair = "auth.entra.client_id and client_secret must be configured together";
        let blank = "auth.entra.client_id and client_secret must not be blank";
        let missing = "auth.entra.client_id and client_secret must be set";
        let guid = "auth.entra.tenant must be the Directory (tenant) ID, a GUID";
        let domain = "a domain name produces a different issuer";
        let multi = "multi-tenant sign-in is not supported";
        let long_label = format!(
            "{}label = \"{}\"\n",
            entra_text(ENTRA_TENANT, ENTRA_CREDENTIALS),
            "x".repeat(41)
        );
        let cases: Vec<(String, &str)> = vec![
            (entra_text(ENTRA_TENANT, "client_id = \"id\"\n"), pair),
            (
                entra_text(ENTRA_TENANT, "client_secret = \"secret\"\n"),
                pair,
            ),
            (
                entra_text(
                    ENTRA_TENANT,
                    "client_id = \" \"\nclient_secret = \"secret\"\n",
                ),
                blank,
            ),
            (entra_text(ENTRA_TENANT, ""), missing),
            (
                "[auth.entra]\nclient_id = \"id\"\nclient_secret = \"secret\"\n".to_owned(),
                "auth.entra.tenant is required",
            ),
            (
                entra_text("contoso.onmicrosoft.com", ENTRA_CREDENTIALS),
                domain,
            ),
            (entra_text("0000-0000", ENTRA_CREDENTIALS), guid),
            (
                entra_text("00000000000000000000000000000000", ENTRA_CREDENTIALS),
                guid,
            ),
            (entra_text("common", ENTRA_CREDENTIALS), multi),
            (entra_text("Organizations", ENTRA_CREDENTIALS), multi),
            (entra_text("CONSUMERS", ENTRA_CREDENTIALS), multi),
            (long_label, "auth.entra.label must be 1 to 40 characters"),
        ];
        for (text, expected) in cases {
            let error = load_text(&text).unwrap_err();
            assert!(error.contains(expected), "{text}: {error}");
        }

        let loaded = load_text(&entra_text(
            "ABCDEF12-3456-7890-ABCD-EF1234567890",
            ENTRA_CREDENTIALS,
        ))
        .unwrap();
        assert_eq!(oidc_ids(&loaded), ["entra"]);
        assert_eq!(
            loaded.oidc[0].issuer,
            "https://login.microsoftonline.com/abcdef12-3456-7890-abcd-ef1234567890/v2.0"
        );
        assert_eq!(loaded.oidc[0].label, "Microsoft");
        let loaded = load_text(&format!(
            "{}label = \"Contoso\"\n",
            entra_text(ENTRA_TENANT, ENTRA_CREDENTIALS)
        ))
        .unwrap();
        assert_eq!(loaded.oidc[0].label, "Contoso");
    }

    #[test]
    fn custom_entries_pass_and_refuse_by_rule() {
        let loaded = load_text(&custom_entry("keycloak", CUSTOM_BODY)).unwrap();
        assert_eq!(oidc_ids(&loaded), ["oidc-keycloak"]);
        assert_eq!(
            loaded.oidc[0].issuer,
            "https://sso.example.org/realms/staff"
        );
        assert_eq!(loaded.oidc[0].label, "Example SSO");

        for issuer in [
            "http://localhost:8080/realms/staff",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            let body = CUSTOM_BODY.replace("https://sso.example.org/realms/staff", issuer);
            assert!(
                load_text(&custom_entry("keycloak", &body)).is_ok(),
                "{issuer}"
            );
        }

        let custom_error = |body: &str| load_text(&custom_entry("keycloak", body)).unwrap_err();
        let issuer_error = "auth.oidc.keycloak.issuer must be an https:// URL (http:// only on localhost) with no query or fragment";
        for issuer in [
            "http://sso.example.org/realms/staff",
            "https://sso.example.org/realms/staff?tenant=1",
            "https://sso.example.org/realms/staff#top",
            "sso.example.org/realms/staff",
        ] {
            let body = CUSTOM_BODY.replace("https://sso.example.org/realms/staff", issuer);
            assert!(custom_error(&body).contains(issuer_error), "{issuer}");
        }

        let body = CUSTOM_BODY.replace("issuer = \"https://sso.example.org/realms/staff\"\n", "");
        assert!(custom_error(&body).contains("auth.oidc.keycloak.issuer is required"));
        let body = CUSTOM_BODY.replace("label = \"Example SSO\"\n", "");
        assert!(custom_error(&body).contains("auth.oidc.keycloak.label is required"));
        let body = CUSTOM_BODY.replace("client_secret = \"custom-secret\"\n", "");
        assert!(custom_error(&body).contains(
            "auth.oidc.keycloak.client_id and client_secret must be configured together"
        ));
        let body = CUSTOM_BODY.replace(
            "client_id = \"librepaper\"\nclient_secret = \"custom-secret\"\n",
            "",
        );
        assert!(custom_error(&body)
            .contains("auth.oidc.keycloak.client_id and client_secret must be set"));
        let body = CUSTOM_BODY.replace("custom-secret", " ");
        assert!(custom_error(&body)
            .contains("auth.oidc.keycloak.client_id and client_secret must not be blank"));

        let label_error = "auth.oidc.keycloak.label must be 1 to 40 characters";
        let long_label = "x".repeat(41);
        for label in ["", " ", "Bad\\u0007Label", long_label.as_str()] {
            let body =
                CUSTOM_BODY.replace("label = \"Example SSO\"", &format!("label = \"{}\"", label));
            assert!(custom_error(&body).contains(label_error), "{label}");
        }
        let body = CUSTOM_BODY.replace(
            "label = \"Example SSO\"",
            &format!("label = \"{}\"", "x".repeat(40)),
        );
        assert!(load_text(&custom_entry("keycloak", &body)).is_ok());

        let scopes_error =
            "auth.oidc.keycloak.scopes must contain openid and be at most 200 characters";
        for scopes in [
            "profile email".to_owned(),
            String::new(),
            format!("openid {}", "p".repeat(194)),
        ] {
            let body = format!("{}scopes = \"{}\"\n", CUSTOM_BODY, scopes);
            assert!(custom_error(&body).contains(scopes_error), "{scopes}");
        }
        let body = format!("{}scopes = \"openid {}\"\n", CUSTOM_BODY, "p".repeat(193));
        assert!(load_text(&custom_entry("keycloak", &body)).is_ok());

        let error = custom_error(&format!("{}wat = 1\n", CUSTOM_BODY));
        assert!(error.contains("auth.oidc.keycloak.wat"), "{error}");
    }

    #[test]
    fn custom_names_match_the_slug_rule_and_the_count_is_capped() {
        let long = "a".repeat(25);
        for slug in ["Keycloak", "1sso", long.as_str()] {
            let error = load_text(&custom_entry(slug, CUSTOM_BODY)).unwrap_err();
            assert!(
                error.contains(
                    format!(
                        "auth.oidc.{}: the name must match ^[a-z][a-z0-9-]{{0,23}}$",
                        slug
                    )
                    .as_str()
                ),
                "{error}"
            );
        }
        assert!(load_text(&custom_entry(&"a".repeat(24), CUSTOM_BODY)).is_ok());

        let numbered = |count: usize| -> String {
            (0..count)
                .map(|n| {
                    custom_entry(
                        &format!("p{}", n),
                        &format!(
                            "issuer = \"https://p{}.example.org\"\nclient_id = \"id\"\nclient_secret = \"secret\"\nlabel = \"Example\"\n",
                            n
                        ),
                    )
                })
                .collect()
        };
        assert!(load_text(&numbered(8)).is_ok());
        let error = load_text(&numbered(9)).unwrap_err();
        assert!(error.contains("auth.oidc: at most 8 providers"), "{error}");
    }

    #[test]
    fn oidc_issuers_must_be_unique_after_one_trailing_slash() {
        let mirror = "issuer = \"https://accounts.google.com/\"\nclient_id = \"id\"\nclient_secret = \"secret\"\nlabel = \"Mirror\"\n";
        let error = load_text(&format!(
            "{}{}",
            GOOGLE_TABLE,
            custom_entry("mirror", mirror)
        ))
        .unwrap_err();
        assert!(
            error.contains("auth.oidc.mirror.issuer duplicates the issuer of google"),
            "{error}"
        );

        let entra_mirror = format!(
            "issuer = \"https://login.microsoftonline.com/{}/v2.0/\"\nclient_id = \"id\"\nclient_secret = \"secret\"\nlabel = \"Mirror\"\n",
            ENTRA_TENANT
        );
        let error = load_text(&format!(
            "{}{}",
            entra_text(ENTRA_TENANT, ENTRA_CREDENTIALS),
            custom_entry("mirror", &entra_mirror)
        ))
        .unwrap_err();
        assert!(
            error.contains("auth.oidc.mirror.issuer duplicates the issuer of entra"),
            "{error}"
        );

        let error = load_text(&format!(
            "{}{}",
            custom_entry("a", CUSTOM_BODY),
            custom_entry("b", CUSTOM_BODY)
        ))
        .unwrap_err();
        assert!(
            error.contains("auth.oidc.b.issuer duplicates the issuer of oidc-a"),
            "{error}"
        );
    }

    #[test]
    fn providers_come_out_in_sign_in_order() {
        let other_issuer = CUSTOM_BODY.replace("staff", "other");
        let text = format!(
            "{}{}{}{}",
            custom_entry("zeta", CUSTOM_BODY),
            custom_entry("alpha", &other_issuer),
            entra_text(ENTRA_TENANT, ENTRA_CREDENTIALS),
            GOOGLE_TABLE
        );
        let loaded = load_text(&text).unwrap();
        assert_eq!(
            oidc_ids(&loaded),
            ["google", "entra", "oidc-alpha", "oidc-zeta"]
        );
    }

    fn session_lifetime(text: &str) -> Result<Option<std::time::Duration>, String> {
        load_text(&format!("[auth]\nsession_lifetime = \"{}\"\n", text))
            .map(|loaded| loaded.session_lifetime)
    }

    #[test]
    fn session_lifetime_is_bounded_and_absent_by_default() {
        assert_eq!(
            session_lifetime("24h").unwrap(),
            Some(std::time::Duration::from_secs(86_400))
        );
        assert_eq!(
            session_lifetime("7d").unwrap(),
            Some(std::time::Duration::from_secs(7 * 86_400))
        );
        assert_eq!(
            session_lifetime("1h").unwrap(),
            Some(std::time::Duration::from_secs(3_600))
        );
        assert_eq!(session_lifetime("30d").unwrap(), Some(SESSION_MAX_AGE));
        for text in ["30m", "31d", "never", "0"] {
            let error = session_lifetime(text).unwrap_err();
            assert!(
                error.contains("auth.session_lifetime must be between 1h and 30d"),
                "{text}: {error}"
            );
        }
        let loaded = load_text("").unwrap();
        assert_eq!(loaded.session_lifetime, None);
        assert!(show_resolved(&loaded).contains("# session_lifetime is unset"));
    }

    #[test]
    fn unknown_keys_in_the_oidc_tables_are_refused() {
        let error = load_text(&entra_text(ENTRA_TENANT, "wat = 1\n")).unwrap_err();
        assert!(error.contains("auth.entra.wat"), "{error}");
        let error = load_text("[auth]\nnever = 1\n").unwrap_err();
        assert!(error.contains("auth.never"), "{error}");
    }

    #[test]
    fn the_dump_shows_the_oidc_sections_without_secrets() {
        let text = format!(
            "[auth]\nsession_lifetime = \"24h\"\n{}{}{}",
            "[auth.google]\nclient_id = \"google-id\"\nclient_secret = \"GOOGLE-SECRET\"\n",
            entra_text(
                ENTRA_TENANT,
                "client_id = \"entra-id\"\nclient_secret = \"ENTRA-SECRET\"\nlabel = \"Contoso\"\n",
            ),
            custom_entry(
                "keycloak",
                "issuer = \"https://sso.example.org/realms/staff\"\nclient_id = \"librepaper\"\nclient_secret = \"KEYCLOAK-SECRET\"\nlabel = \"Example SSO\"\nscopes = \"openid email profile\"\n",
            ),
        );
        let loaded = load_text(&text).unwrap();
        let shown = show_resolved(&loaded);
        let debug = format!("{:?}", loaded);
        for secret in ["GOOGLE-SECRET", "ENTRA-SECRET", "KEYCLOAK-SECRET"] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!debug.contains(secret), "{debug}");
        }
        for expected in [
            "session_lifetime = \"24h\"",
            "[auth.google]",
            "[auth.entra]",
            "tenant = \"00000000-0000-0000-0000-000000000000\"",
            "label = \"Contoso\"",
            "[auth.oidc.keycloak]",
            "issuer = \"https://sso.example.org/realms/staff\"",
            "scopes = \"openid email profile\"",
        ] {
            assert!(shown.contains(expected), "{expected}: {shown}");
        }
        assert!(toml::from_str::<toml::Value>(&shown).is_ok());
    }
}
