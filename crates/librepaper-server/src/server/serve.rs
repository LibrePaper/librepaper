//! `librepaper admin serve`: the whole service in this process.

use std::collections::HashMap;
use std::future::IntoFuture;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::server::origins::{Origins, DOCS_PREFIX};
use crate::server::Server;
use librepaper_base::auth::{
    session_key_file, trim_issuer, GithubApp, OidcProvider, Policy, PROVIDER_GITHUB,
    SESSION_MAX_AGE,
};
use librepaper_base::config::Configuration;
use librepaper_base::shell::ShellFile;
use librepaper_base::util::die;
use librepaper_document::document::retention::{
    describe_seconds, parse_expire_from, parse_retention,
};
use librepaper_engine::storage::store::Store;
use librepaper_engine::storage::{open_storage, StorageOptions};

/// With port 0 in `server.address`, serve takes the first free port in this range, so a
/// second deployment on the same machine needs no manually picked port.
const PORT_FIRST: u16 = 8080;
const PORT_LAST: u16 = 8099;

pub struct ServeOptions {
    pub address: std::net::SocketAddr,
    pub storage: StorageOptions,
    /// Apply pending schema changes at startup. Deployments with a separate
    /// migration job disable this and require a current schema instead.
    pub migrate: bool,
    /// GitHub OAuth app credentials resolved from the TOML configuration.
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    /// The OpenID Connect providers (Google, Entra and custom entries), in the
    /// order the sign-in page offers them, with their credentials resolved
    /// from the TOML configuration. GitHub is not among them.
    pub oidc: Vec<OidcProvider>,
    /// A bound on how long a browser session and a device token last before
    /// the person signs in again, or nothing, which leaves each its own
    /// lifetime. Already checked to lie between an hour and the thirty days
    /// of `SESSION_MAX_AGE`.
    pub session_lifetime: Option<std::time::Duration>,
    /// Who may publish and who may comment, already normalized per provider
    /// by the configuration loader.
    pub publishers: Policy,
    pub commenters: Policy,
    /// Write each new account's starter project as though it had been
    /// typed over this many days, so the history panel has something in it on a
    /// demonstration deployment. The operations are real; only the clock is
    /// invented. See `librepaper_engine::storage::seed::activity`.
    pub simulate_activity: Option<u32>,
    /// The reader origin browsers reach this deployment on, and the origin
    /// documents are served from. Without the first, the deployment answers on
    /// loopback alone.
    pub app_origin: Option<String>,
    pub docs_origin: Option<String>,
    /// The marketing site in front of this deployment, or nothing. Signing
    /// out goes there; a deployment without one sends a signed-out reader to
    /// its own front page.
    pub site_origin: Option<String>,
    /// The origin of the operator's admin console, and the GitHub user IDs
    /// allowed to sign in there. They are configured together.
    pub admin_origin: Option<String>,
    /// Numeric GitHub user IDs allowed to sign in on the admin origin.
    pub admin_github_ids: Vec<i64>,
    pub expire_after: Option<String>,
    pub expire_from: Option<String>,
    pub erase_inactive_after: Option<String>,
    /// HTTPS static mirror URL published to browsers. Renderer and LaTeX
    /// distribution bytes are fetched directly by the browser and never pass
    /// through this process.
    pub asset_mirror: String,
    /// Loads the compiled-in shell for the validated asset mirror. The caller
    /// supplies it so the server does not depend on the shell crate, whose
    /// input is a build output; it runs where the server used to call it, after
    /// the mirror is validated.
    pub load_shell: fn(&str) -> Result<HashMap<String, ShellFile>, String>,
    /// The pinned LaTeX release, supplied by the caller for the same reason.
    pub latex_release: &'static str,
    pub config: Configuration,
    pub metrics_address: Option<SocketAddr>,
}

/// Validate the only supported mirror shape before opening storage or binding
/// a port. A local directory and plain HTTP would make browser failures look
/// like missing compiler assets, so operators get an actionable startup error.
pub(crate) fn validate_asset_mirror(value: &str) -> Result<String, String> {
    let value = value.trim();
    let mut parsed = url::Url::parse(value).map_err(|_| {
        format!("assets.mirror must be an https: URL for a static mirror (got {value:?})")
    })?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("assets.mirror must be an https: URL without credentials, a query, or a fragment; see https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md".to_string());
    }
    parsed.set_path(&format!("{}/", parsed.path().trim_end_matches('/')));
    Ok(parsed.to_string())
}

/// Validate the marketing site's address. It is handed to a browser as the
/// place signing out goes, so it has to be an origin a browser can be sent
/// to: a scheme and a host, no credentials, and nothing after the host. Plain
/// HTTP is allowed because `make demo` serves the site on localhost, which
/// is the one place it is not a mistake.
pub(crate) fn validate_site_origin(value: &str) -> Result<String, String> {
    let value = value.trim();
    let parsed = url::Url::parse(value).map_err(|_| {
        format!("origins.site must be a URL, for example https://paper.example (got {value:?})")
    })?;
    let loopback = matches!(parsed.host(), Some(url::Host::Domain("localhost")))
        || matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    if parsed.host_str().is_none()
        || (parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback))
    {
        return Err(format!(
            "origins.site must be an https: URL, or http: on loopback (got {value:?})"
        ));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.path().trim_matches('/').is_empty()
    {
        return Err(
            "origins.site must be an origin alone: no credentials, path, query or fragment"
                .to_string(),
        );
    }
    Ok(parsed.origin().ascii_serialization())
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// The origin set a deployment asked for: the reader and document origins,
/// and the admin origin when there is one. Settled before storage is opened or
/// a port claimed.
fn configured_origins(options: &ServeOptions) -> Result<Origins, String> {
    let origins = match non_blank(options.app_origin.as_deref()) {
        Some(reader) => Origins::configure(reader, options.docs_origin.as_deref())
            .map_err(|error| format!("origins.app/origins.docs: {error}"))?,
        None if options.docs_origin.is_some() => {
            return Err("origins.docs requires origins.app".into());
        }
        None => Origins::loopback_only(),
    };
    origins.with_admin(non_blank(options.admin_origin.as_deref()))
}

/// The admin console signs in with GitHub and admits only the listed user IDs,
/// so the origin and the IDs come together, and GitHub sign-in must be set up.
fn validate_admin_github(
    origin: Option<&str>,
    ids: &[i64],
    github_configured: bool,
) -> Result<(), String> {
    if non_blank(origin).is_some() != !ids.is_empty() {
        return Err("origins.admin and admin.github_ids must be configured together".into());
    }
    if ids.iter().any(|id| *id <= 0) {
        return Err("admin.github_ids holds numeric GitHub user IDs, which are positive".into());
    }
    if non_blank(origin).is_some() && !github_configured {
        return Err("the admin console signs in with GitHub; configure [auth.github]".into());
    }
    Ok(())
}

/// Claims a port: the one asked for, or the first free one in the default
/// range when port is zero.
async fn listen(bind: std::net::IpAddr, port: u16) -> TcpListener {
    if port != 0 {
        return match TcpListener::bind((bind, port)).await {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => die(format!(
                "port {port} is already in use. Pick another value for server.address."
            )),
            Err(err) => die(format!("could not listen on port {port}: {err}")),
        };
    }
    for candidate in PORT_FIRST..=PORT_LAST {
        match TcpListener::bind((bind, candidate)).await {
            Ok(listener) => return listener,
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(err) => die(format!("could not listen on port {candidate}: {err}")),
        }
    }
    die(format!(
        "ports {PORT_FIRST} to {PORT_LAST} are all in use. Set server.address to a free port."
    ))
}

/// What startup has to say about signing in, worked out here rather than inline
/// so the answer can be inspected without starting a server.
#[derive(Debug, Default)]
pub struct SignInAdvice {
    pub fatal: Option<String>,
    pub warnings: Vec<String>,
}

/// A provider is needed for publishing and source writes. The server can still
/// start for reading/commenting when no provider is configured, and reports
/// that publishing is unavailable.
///
/// `oidc` is whether any OpenID Connect provider is configured, ORCID
/// included. `callbacks` are the addresses an operator with no provider yet
/// would register one at.
pub fn sign_in_advice(
    github: bool,
    oidc: bool,
    // The origin a provider must have registered: the deployment's configured
    // reader origin rather than whatever this process is bound to. An operator
    // copying a callback URL out of this warning needs the one their users
    // will actually arrive on.
    reader_origin: &str,
    port: u16,
    callbacks: &[String],
) -> SignInAdvice {
    let mut advice = SignInAdvice::default();
    if !github && !oidc {
        advice.warnings.push(format!(
            "warning: no sign-in provider is configured; publishing and source writes are unavailable. Configure auth.github, auth.google, auth.entra, auth.orcid or an auth.oidc table in the TOML file. The callback URLs to register at the provider, served by {reader_origin} on port {port}, are {}.",
            callbacks.join(", ")
        ));
    }
    advice
}

/// The callback URLs an operator would register, one for each way of signing
/// in. A custom provider's depends on the slug its table is given.
fn callback_urls(reader_origin: &str) -> Vec<String> {
    vec![
        format!("{reader_origin}/auth/callback (GitHub)"),
        format!("{reader_origin}/auth/callback/google"),
        format!("{reader_origin}/auth/callback/entra"),
        format!("{reader_origin}/auth/callback/orcid"),
        format!("{reader_origin}/auth/callback/oidc-<slug> (one for each auth.oidc table)"),
    ]
}

/// The stable Loro peer identity this deployment authors source under
/// (§7.3). Kept in `server_runtime_state` under `librepaper_engine::log::DEPLOYMENT_PEER_STATE`
/// so a label written by one process and read by the next names one author,
/// rather than a fresh peer on every restart. Minted once, the first time a
/// deployment reads it absent.
async fn deployment_peer_key(
    catalog: &librepaper_engine::storage::postgres::PostgresCatalog,
) -> Result<String, String> {
    if let Some(existing) = catalog
        .runtime_state(librepaper_engine::log::DEPLOYMENT_PEER_STATE)
        .await
        .map_err(|error| format!("could not read the deployment peer key: {error}"))?
    {
        if let Some(key) = existing.get("key").and_then(|value| value.as_str()) {
            if !key.is_empty() {
                return Ok(key.to_string());
            }
        }
    }
    let key = librepaper_base::util::random_token();
    catalog
        .set_runtime_state(
            librepaper_engine::log::DEPLOYMENT_PEER_STATE,
            serde_json::json!({ "key": key }),
        )
        .await
        .map_err(|error| format!("could not store the deployment peer key: {error}"))?;
    Ok(key)
}

/// What the catalogue is allowed to hold, and how fast it may be asked to
/// hold it, as this deployment's configuration says.
fn storage_policy(config: &Configuration) -> librepaper_engine::storage::postgres::StoragePolicy {
    librepaper_engine::storage::postgres::StoragePolicy {
        owner_bytes: config.storage.per_owner,
        deployment_bytes: config.storage.total,
        asset_uploads_per_hour: config.storage.uploads_per_hour as i64,
    }
}

/// Seconds without a sign-in after which an account is erased; zero keeps them.
/// A window under two session lifetimes would erase people who still sign in.
/// The floor is twice `SESSION_MAX_AGE` whatever `auth.session_lifetime`
/// says: credentials issued before the setting was shortened keep their own
/// expiry, and a short setting must not make a two-hour erasure window legal.
pub(crate) fn parse_inactive_after(value: &str) -> Result<i64, String> {
    let seconds = parse_retention(value)?;
    let floor = 2 * SESSION_MAX_AGE.as_secs() as i64;
    if seconds != 0 && seconds < floor {
        return Err(format!("must be at least {}", describe_seconds(floor)));
    }
    Ok(seconds)
}

/// Validate every server setting before the server creates directories,
/// claims sockets, or connects to PostgreSQL. `admin config check` calls this
/// same pure preflight.
pub fn validate_serve_options(options: &ServeOptions) -> Result<(), String> {
    options.config.cost.validate()?;
    options.config.validate_pending()?;
    options.config.validate_budgets()?;
    librepaper_engine::storage::validate_storage_options(&options.storage)?;
    parse_retention(options.expire_after.as_deref().unwrap_or(""))
        .map_err(|error| format!("retention.expire_after: {error}"))?;
    parse_expire_from(options.expire_from.as_deref().unwrap_or(""))
        .map_err(|error| format!("retention.expire_from: {error}"))?;
    parse_inactive_after(options.erase_inactive_after.as_deref().unwrap_or(""))
        .map_err(|error| format!("retention.erase_inactive_after: {error}"))?;
    configured_origins(options)?;
    let github_id = non_blank(options.github_client_id.as_deref()).is_some();
    let github_secret = non_blank(options.github_client_secret.as_deref()).is_some();
    let github_configured = github_id && github_secret;
    validate_admin_github(
        options.admin_origin.as_deref(),
        &options.admin_github_ids,
        github_configured,
    )?;
    validate_asset_mirror(&options.asset_mirror)?;
    if let Some(site) = options.site_origin.as_deref() {
        validate_site_origin(site)?;
    }
    let mut configured: Vec<&str> = options
        .oidc
        .iter()
        .map(|provider| provider.id.as_str())
        .collect();
    if github_configured {
        configured.push(PROVIDER_GITHUB);
    }
    if !configured.is_empty() && !options.publishers.is_configured() {
        return Err("no [auth.*] table lists publishers; add publishers = [...] under the provider people sign in with".into());
    }
    for policy in [&options.publishers, &options.commenters] {
        if let Some(provider) = policy
            .providers
            .keys()
            .find(|provider| !configured.contains(&provider.as_str()))
        {
            return Err(format!(
                "access policy names provider {provider}, which is not configured"
            ));
        }
    }
    if github_id != github_secret {
        return Err(
            "auth.github.client_id and auth.github.client_secret must be configured together"
                .into(),
        );
    }
    Ok(())
}

/// One pass of the janitor: expired documents, then inactive accounts.
async fn retention_pass(
    server: &Server,
    now: i64,
    retention: i64,
    expire_from: &str,
    inactive_after: i64,
) {
    if retention > 0 {
        server.delete_expired(now, retention, expire_from).await;
    }
    if inactive_after > 0 {
        server.erase_inactive_accounts(now, inactive_after).await;
    }
}

pub async fn serve(options: ServeOptions) {
    validate_serve_options(&options).unwrap_or_else(|error| die(error));
    options
        .config
        .cost
        .validate()
        .unwrap_or_else(|error| die(format!("invalid cost policy: {error}")));
    let storage = options.storage.clone();
    let deployment_paths = storage.paths().unwrap_or_else(|err| die(err));
    let retention = parse_retention(options.expire_after.as_deref().unwrap_or(""))
        .unwrap_or_else(|err| die(format!("{err}; use a duration such as 24h or 30d")));
    let inactive_after =
        parse_inactive_after(options.erase_inactive_after.as_deref().unwrap_or(""))
            .unwrap_or_else(|err| die(format!("retention.erase_inactive_after: {err}")));
    // The origin pair is settled before storage is opened or a port claimed.
    // Two origins that share a host are not a deployment with a weaker
    // boundary; they are a deployment with none, so this refuses to start
    // rather than warn. It is a deployment diagnostic and not a claim that DNS
    // proves browser isolation: what the browser enforces is the
    // scheme/host/port tuple, and all startup can do is confirm the operator
    // asked for two different hosts and name the record and certificate that
    // have to exist for that to be true.
    let origins = configured_origins(&options).unwrap_or_else(|error| die(error));
    // Read before anything is opened or a port is claimed: a mirror value
    // which cannot work is a typo the operator is still standing in front of.
    let assets = validate_asset_mirror(&options.asset_mirror).unwrap_or_else(|err| die(err));
    let latex = format!("{assets}latex/{}/", options.latex_release);
    // Likewise the site. It becomes a destination a browser is sent to, so it
    // is an absolute http(s) origin or it is a mistake -- a bare host would
    // be read as a path on this deployment and send a signed-out reader to a
    // page that does not exist.
    let site = match options.site_origin.as_deref().unwrap_or("").trim() {
        "" => None,
        flag => Some(validate_site_origin(flag).unwrap_or_else(|err| die(err))),
    };
    let expire_from = parse_expire_from(options.expire_from.as_deref().unwrap_or(""))
        .unwrap_or_else(|err| die(err));
    let blobs = open_storage(storage.clone())
        .await
        .unwrap_or_else(|err| die(err));
    let reset_marker = deployment_paths.state.join("seed-reset.json");
    if reset_marker.exists() {
        die(format!(
            "refusing to serve while an interrupted local seed reset remains: {}",
            reset_marker.display()
        ));
    }
    // Claim the port first, so a port already in use costs nothing and the
    // advice below can name the callback URL this run would actually use.
    let listener = listen(options.address.ip(), options.address.port()).await;
    let metrics_listener = super::metrics::bind(options.metrics_address)
        .await
        .unwrap_or_else(|error| die(error));
    let port = listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(options.address.port());
    let address = format!(":{port}");

    let app = GithubApp {
        client_id: options.github_client_id.unwrap_or_default(),
        client_secret: options.github_client_secret.unwrap_or_default(),
        ..GithubApp::default()
    };
    let publishers = options.publishers.clone();
    let commenters = options.commenters.clone();
    let loopback_origin = format!("http://localhost{address}");
    let reader_origin = origins
        .reader()
        .map(|reader| reader.as_str())
        .unwrap_or(&loopback_origin)
        .to_string();

    let advice = sign_in_advice(
        app.configured(),
        !options.oidc.is_empty(),
        &reader_origin,
        port,
        &callback_urls(&reader_origin),
    );
    for warning in &advice.warnings {
        tracing::warn!(
            "{}",
            warning
                .strip_prefix("warning: ")
                .unwrap_or(warning.as_str())
        );
    }
    if let Some(fatal) = advice.fatal {
        die(fatal);
    }

    let config = Arc::new(options.config);
    let shell = (options.load_shell)(&assets).unwrap_or_else(|err| die(err));
    let key_path = deployment_paths.secrets.join("session.key");
    let mut database =
        librepaper_engine::storage::postgres::PostgresOptions::new(&storage.database_url);
    database.max_connections = storage.database_connections;
    database.policy = storage_policy(&config);
    let catalog = Arc::new(
        librepaper_engine::storage::postgres::PostgresCatalog::connect(database)
            .await
            .unwrap_or_else(|err| die(format!("could not connect to PostgreSQL: {err}"))),
    );
    let catalog_nonempty = catalog
        .has_deployment_state()
        .await
        .unwrap_or_else(|error| {
            die(format!(
                "could not inspect PostgreSQL deployment state before reading the session key: {error}"
            ))
        });
    // Create a fresh key before schema work so a later migration or startup
    // failure cannot leave a migrated database with a different key on retry.
    let key = session_key_file(&key_path, catalog_nonempty).unwrap_or_else(|error| die(error));
    if options.migrate {
        catalog
            .migrate()
            .await
            .unwrap_or_else(|error| die(format!("could not migrate PostgreSQL: {error}")));
    } else {
        catalog
            .validate_schema()
            .await
            .unwrap_or_else(|error| die(format!("PostgreSQL schema is not ready: {error}")));
    }
    // A subject is unique only within its issuer, so a provider id pointed at
    // another issuer would hand every account it made to whoever holds the
    // same subject there. The first run records the issuer and every later
    // run holds the configuration to it, before anybody can sign in.
    for provider in &options.oidc {
        let configured = trim_issuer(&provider.issuer);
        let bound = catalog
            .bind_sign_in_issuer(&provider.id, configured)
            .await
            .unwrap_or_else(|error| {
                die(format!(
                    "could not record the issuer of auth provider {}: {error}",
                    provider.id
                ))
            });
        if trim_issuer(&bound) != configured {
            die(format!(
                "auth provider {id} was bound to issuer {old} and is now configured with {new}. Existing {id} accounts belong to the old issuer. Restore the old issuer, or give the new one a different id.",
                id = provider.id,
                old = trim_issuer(&bound),
                new = configured,
            ));
        }
    }
    let writer = catalog
        .claim_writer()
        .await
        .unwrap_or_else(|error| die(format!("could not claim deployment writer: {error}")));
    catalog
        .clear_stale_pending_log_reservations()
        .await
        .unwrap_or_else(|error| {
            die(format!(
                "could not recover pending source quota reservations: {error}"
            ))
        });
    // The registry, the rooms and the background worker all share one
    // catalogue, blob store and configuration; they are wired together here
    // rather than separately so there is exactly one cache of any document
    // in this process (§4.3). The deployment's peer key is settled first
    // because the registry needs it to admit a sequencer.
    let peer_key = deployment_peer_key(&catalog)
        .await
        .unwrap_or_else(|err| die(err));
    let registry = librepaper_engine::log::Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        peer_key,
    );
    let rooms = librepaper_room::room::Rooms::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        registry.clone(),
    );
    let (worker, background) = librepaper_engine::storage::worker::Worker::new(
        catalog.clone(),
        blobs.clone(),
        registry.clone(),
        config.clone(),
    );
    // §8.4: a flush that leaves the backlog over the threshold asks this
    // worker for compaction. Wired after the worker exists and before it is
    // spawned, because the worker is built from the registry and so cannot
    // be handed to it at construction.
    registry.compacts_through(background.clone());
    tokio::spawn(worker.run());

    let store = Store::open_with_catalog(blobs.clone(), config.clone(), catalog, registry.clone());
    let mut instance = Server::new(
        store,
        rooms,
        background,
        shell,
        app,
        key,
        config.clone(),
        publishers.clone(),
        commenters.clone(),
    );
    instance.install_writer(writer);
    instance.oidc = options.oidc;
    instance.bound_sessions(options.session_lifetime);
    instance.simulate_activity = options.simulate_activity;
    instance.origins = origins.clone();
    instance.paths = Some(deployment_paths.clone());
    instance.admin_github_ids = options.admin_github_ids;
    instance.erase_inactive_after = inactive_after;
    // The history behind the graphs page opens only when the page has a name.
    // A file that cannot be opened costs the graphs and nothing else: the
    // server never refuses to start over disposable measurements.
    if origins.admin().is_some() {
        let state = deployment_paths.state.clone();
        match tokio::task::spawn_blocking(move || super::graphs::Graphs::open(&state)).await {
            Ok(Ok(graphs)) => instance.graphs = Some(Arc::new(graphs)),
            Ok(Err(error)) => tracing::warn!("operator graphs are off: {error}"),
            Err(error) => tracing::warn!("operator graphs are off: {error}"),
        }
    }
    instance.latex = Some(latex);
    instance.site = site;
    let instance = Arc::new(instance);
    // The sampler feeds the metrics page and the graphs, so either one is
    // reason enough to run it. Only the listener needs `[metrics] address`.
    if metrics_listener.is_some() || instance.graphs.is_some() {
        super::metrics::spawn_sampler(instance.clone());
    }
    if let Some(metrics_listener) = metrics_listener {
        if let Ok(address) = metrics_listener.local_addr() {
            println!("  private metrics: http://{address}/metrics");
        }
        let metrics_server = instance.clone();
        tokio::spawn(async move {
            if let Err(error) = super::metrics::serve(metrics_listener, metrics_server).await {
                tracing::error!("metrics listener stopped: {error}");
            }
        });
    }

    match (origins.reader(), origins.docs()) {
        (Some(reader), Some(docs)) => {
            println!("librepaper serving {}", reader.as_str());
            println!("  documents on {}", docs.as_str());
        }
        _ => {
            println!("librepaper serving http://localhost{address}");
            println!("  documents on http://{DOCS_PREFIX}localhost{address}");
            println!("  no origins.app: this deployment answers on loopback only");
        }
    }
    if let Some(admin) = origins.admin() {
        println!("  operator graphs: {}/", admin.as_str());
    }
    println!("  data in {}", blobs.describe());
    println!("  publishing: {}", publishers.describe());
    println!("  commenting: {}", commenters.describe());
    for provider in &instance.oidc {
        println!(
            "  sign-in {}: {} ({}), callback {reader_origin}/auth/callback/{}",
            provider.id, provider.label, provider.issuer, provider.id
        );
    }
    println!(
        "  sign-in lasts {}",
        describe_seconds(instance.session_lifetime.as_secs() as i64)
    );
    super::host_metrics::warn(&config, &deployment_paths.deployment, blobs.is_local());
    if std::io::stdout().is_terminal() {
        println!(
            "  storage: {} total bytes; {} bytes per owner",
            config.storage.total, config.storage.per_owner
        );
    }
    println!("  asset mirror: {assets}");
    if assets == librepaper_base::config::DEFAULT_ASSET_MIRROR {
        println!(
            "  the project mirror promises only releases carried by this build (latex {})",
            options.latex_release
        );
    }
    if retention > 0 {
        println!(
            "  expiry: {expire_from} after {}",
            describe_seconds(retention)
        );
    }
    if inactive_after > 0 {
        println!(
            "  inactive accounts: erased after {} without a sign-in",
            describe_seconds(inactive_after)
        );
    }
    // One hourly janitor serves both settings; a deployment with neither runs no timer.
    if retention > 0 || inactive_after > 0 {
        retention_pass(
            &instance,
            librepaper_base::util::now_unix(),
            retention,
            &expire_from,
            inactive_after,
        )
        .await;
        let janitor = instance.clone();
        let from = expire_from.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3600));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                retention_pass(
                    &janitor,
                    librepaper_base::util::now_unix(),
                    retention,
                    &from,
                    inactive_after,
                )
                .await;
            }
        });
    }

    // The one periodic thing left (§8.6): flush what is due and retire idle
    // sequencers, and only while something is resident. An idle deployment
    // ticks this timer but touches no room and issues no query, which is
    // what keeps the last test of §14.2 true -- nothing here polls a job
    // table, sweeps a link deadline, or checkpoints a cost meter on a clock;
    // link expiry is a per-connection deadline, and the background worker
    // above wakes on the durable state its own tasks write, and rescans it
    // once an hour with the orphan sweep for work written by another
    // process (§12).
    let sweeper = instance.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            sweeper.cost.housekeep();
            if !sweeper.rooms.registry().all().await.is_empty() {
                sweeper.rooms.housekeep().await;
            }
        }
    });

    // A server on its way down writes what it holds. An acknowledged edit
    // survives a restart because it was written when it was acknowledged; this
    // is about the unacknowledged ones, which have no reason to be lost to an
    // orderly shutdown.
    let closing = instance.clone();
    let shutdown = async move {
        shutdown_signal().await;
        closing.rooms.shutdown().await;
    };

    let closing_catalog = instance.store.catalog.clone();
    let router = instance.clone().router();
    let mut serving = Box::pin(
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .into_future(),
    );
    let serve_result = tokio::select! {
        result = &mut serving => result,
        _ = instance.writer_loss() => {
            // The room cache may contain unacknowledged edits whose flush
            // requires the lease that was just lost. Do not enter graceful
            // shutdown and flush through stale ownership; let the process
            // supervisor restart us after PostgreSQL releases the session.
            die("deployment writer ownership was lost; exiting for supervisor recovery");
        }
    };
    drop(serving);
    drop(instance);
    closing_catalog.close().await;
    if let Err(err) = serve_result {
        die(err);
    }
}

/// Supervisors normally stop Unix services with SIGTERM, while interactive
/// sessions use Ctrl-C. Both enter the same flush and maintenance path.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod site_origin_tests {
    use super::*;

    /// The value becomes a place a browser is sent after it has just given up
    /// its session, so what it may be is narrow: an origin, and nothing else.
    #[test]
    fn a_site_origin_is_an_origin_and_nothing_else() {
        assert_eq!(
            validate_site_origin("https://paper.example/"),
            Ok("https://paper.example".to_string()),
            "a trailing slash is a path of nothing, and the origin is what comes back"
        );
        assert_eq!(
            validate_site_origin("http://localhost:8082"),
            Ok("http://localhost:8082".to_string()),
            "plain HTTP on loopback is `make demo`, not a mistake"
        );
        for refused in [
            "paper.example",
            "http://paper.example",
            "https://paper.example/start.html",
            "https://user:pw@paper.example",
            "https://paper.example?next=x",
        ] {
            assert!(
                validate_site_origin(refused).is_err(),
                "{refused:?} is not an origin this deployment should send anyone to"
            );
        }
    }
}

#[cfg(test)]
mod inactive_account_tests {
    use super::*;

    #[test]
    fn the_inactive_account_window_is_off_or_at_least_two_sessions() {
        assert_eq!(parse_inactive_after("").unwrap(), 0);
        assert_eq!(parse_inactive_after("off").unwrap(), 0);
        assert_eq!(parse_inactive_after("365d").unwrap(), 365 * 86_400);
        assert_eq!(parse_inactive_after("60d").unwrap(), 60 * 86_400);
        assert!(parse_inactive_after("59d").is_err());
        assert!(parse_inactive_after("30d").is_err());
        assert!(parse_inactive_after("soon").is_err());
    }
}

#[cfg(test)]
mod admin_github_tests {
    use super::*;

    #[test]
    fn the_admin_origin_and_ids_come_together_with_github_sign_in() {
        let ids = [987057];
        assert!(validate_admin_github(None, &[], false).is_ok());
        assert!(validate_admin_github(Some(" "), &[], false).is_ok());
        assert!(validate_admin_github(Some("https://admin.paper.example"), &ids, true).is_ok());

        let error =
            validate_admin_github(Some("https://admin.paper.example"), &[], true).unwrap_err();
        assert!(error.contains("admin.github_ids"), "{error}");

        let error = validate_admin_github(None, &ids, true).unwrap_err();
        assert!(error.contains("admin.github_ids"), "{error}");

        let error =
            validate_admin_github(Some("https://admin.paper.example"), &[0], true).unwrap_err();
        assert!(error.contains("admin.github_ids"), "{error}");

        let error =
            validate_admin_github(Some("https://admin.paper.example"), &ids, false).unwrap_err();
        assert!(error.contains("auth.github"), "{error}");
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn sigterm_enters_graceful_shutdown() {
        const CHILD: &str = "LIBREPAPER_TEST_SHUTDOWN_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let shutdown = super::shutdown_signal();
                tokio::pin!(shutdown);
                // Poll the handler before sending SIGTERM, without installing
                // a process-wide handler in the parent test runner.
                tokio::select! {
                    biased;
                    _ = &mut shutdown => panic!("shutdown before signal"),
                    _ = async {} => {},
                }
                assert!(std::process::Command::new("kill")
                    .args(["-TERM", &std::process::id().to_string()])
                    .status()
                    .unwrap()
                    .success());
                tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
                    .await
                    .unwrap();
            });
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "server::serve::tests::sigterm_enters_graceful_shutdown",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "child failed: {:?}", output);
    }
}
