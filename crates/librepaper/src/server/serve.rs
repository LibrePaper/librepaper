//! `librepaper admin serve`: the whole service in this process.

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::auth::{session_key_file, GithubApp, GoogleApp, Policy};
use crate::config::Configuration;
use crate::document::retention::{describe_seconds, parse_expire_from, parse_retention};
use crate::document::store::Store;
use crate::server::origins::{Origins, DOCS_PREFIX};
use crate::server::shell::load_shell;
use crate::server::Server;
use crate::storage::{open_storage, StorageOptions};
use crate::util::die;

/// With no --port, serve takes the first free port in this range, so a second
/// deployment on the same machine, or a port something else has already
/// taken, needs no thought.
const PORT_FIRST: u16 = 8080;
const PORT_LAST: u16 = 8099;

pub struct ServeOptions {
    pub bind: std::net::IpAddr,
    pub port: u16,
    pub storage: StorageOptions,
    /// The GitHub OAuth app's client id. Resolved by clap from `--github-client-id`
    /// or `LIBREPAPER_GITHUB_CLIENT_ID`; the client secret is never a flag, and
    /// is read straight from the environment by `secrets_from_environment`.
    pub github_client_id: Option<String>,
    /// The Google OAuth client's id, resolved the same way as the GitHub one.
    pub google_client_id: Option<String>,
    pub publishers: Option<String>,
    pub commenters: Option<String>,
    pub no_listing: bool,
    /// Write each new account's starter documents as though they had been
    /// typed over this many days, so the history panel has something in it on a
    /// demonstration deployment. The operations are real; only the clock is
    /// invented. See `crate::seed::activity`.
    pub simulate_activity: Option<u32>,
    /// The reader origin browsers reach this deployment on, and the origin
    /// documents are served from. Without the first, the deployment answers on
    /// loopback alone.
    pub origin: Option<String>,
    pub docs_origin: Option<String>,
    /// The marketing site in front of this deployment, or nothing. Signing
    /// out goes there; a deployment without one sends a signed-out reader to
    /// its own front page.
    pub site_origin: Option<String>,
    pub expire_after: Option<String>,
    pub expire_from: Option<String>,
    /// HTTPS static mirror URL published to browsers. Distribution bytes are
    /// fetched directly by the browser and never pass through this process.
    pub latex_mirror: String,
    /// A directory of font files served to typst documents, or nothing. See
    /// `crate::server::fonts`.
    pub typst_fonts: Option<String>,
    /// Do not run the local app for this machine. Without it, `serve` also
    /// starts the loopback service that renders Quarto documents for an
    /// editor whose browser is on this host; see `crate::local::embedded`.
    pub no_local: bool,
    pub config: Configuration,
}

/// Validate the only supported mirror shape before opening storage or binding
/// a port. A local directory and plain HTTP would make browser failures look
/// like missing compiler assets, so operators get an actionable startup error.
pub(crate) fn validate_latex_mirror(value: &str) -> Result<String, String> {
    let value = value.trim();
    let mut parsed = url::Url::parse(value).map_err(|_| {
        format!(
            "--latex-mirror must be an https: URL for a static mirror; see the mirror documentation (got {value:?})"
        )
    })?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("--latex-mirror must be an https: URL without credentials, a query, or a fragment; see https://github.com/LibrePaper/wasm-latex/blob/main/docs/mirror.md".to_string());
    }
    parsed.set_path(&format!("{}/", parsed.path().trim_end_matches('/')));
    Ok(parsed.to_string())
}

/// Validate the marketing site's address. It is handed to a browser as the
/// place signing out goes, so it has to be an origin a browser can be sent
/// to: a scheme and a host, no credentials, and nothing after the host. Plain
/// HTTP is allowed because `make deploy` serves the site on localhost, which
/// is the one place it is not a mistake.
pub(crate) fn validate_site_origin(value: &str) -> Result<String, String> {
    let value = value.trim();
    let parsed = url::Url::parse(value).map_err(|_| {
        format!("--site-origin must be a URL, for example https://paper.example (got {value:?})")
    })?;
    let loopback = matches!(parsed.host(), Some(url::Host::Domain("localhost")))
        || matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    if parsed.host_str().is_none()
        || (parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback))
    {
        return Err(format!(
            "--site-origin must be an https: URL, or http: on loopback (got {value:?})"
        ));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.path().trim_matches('/').is_empty()
    {
        return Err(
            "--site-origin must be an origin alone: no credentials, path, query or fragment"
                .to_string(),
        );
    }
    Ok(parsed.origin().ascii_serialization())
}

/// The GitHub and Google OAuth app client secrets. Secrets never travel as
/// flags: a flag lands in the process table, where every other process on the
/// machine can read it, and in the shell history of whoever typed it. This is
/// the only place the server reads configuration straight out of the
/// environment; every other option is resolved by clap before it gets here.
struct Secrets {
    github_client_secret: String,
    google_client_secret: String,
}

fn secrets_from_environment() -> Secrets {
    Secrets {
        github_client_secret: std::env::var("LIBREPAPER_GITHUB_CLIENT_SECRET").unwrap_or_default(),
        google_client_secret: std::env::var("LIBREPAPER_GOOGLE_CLIENT_SECRET").unwrap_or_default(),
    }
}

/// Claims a port: the one asked for, or the first free one in the default
/// range when port is zero.
async fn listen(bind: std::net::IpAddr, port: u16) -> TcpListener {
    if port != 0 {
        return match TcpListener::bind((bind, port)).await {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => die(format!(
                "port {port} is already in use. Pick another with --port."
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
        "ports {PORT_FIRST} to {PORT_LAST} are all in use. Pick one with --port."
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
/// The two warnings are not deaths: a deployment may be mid-migration, and a
/// list naming somebody no configured provider can produce is worth saying out
/// loud rather than refusing to start over.
pub fn sign_in_advice(
    github: bool,
    google: bool,
    publishers: &Policy,
    commenters: &Policy,
    // The origin a provider must have registered: the deployment's configured
    // reader origin rather than whatever this process is bound to. An operator
    // copying a callback URL out of this warning needs the one their users
    // will actually arrive on.
    reader_origin: &str,
    port: u16,
) -> SignInAdvice {
    let mut advice = SignInAdvice::default();
    if !github && (publishers.names_a_login() || commenters.names_a_login()) {
        advice.warnings.push(
            "warning: a policy names a GitHub login, but there is no GitHub OAuth app; \
             nobody can sign in that way. Set LIBREPAPER_GITHUB_CLIENT_ID and \
             LIBREPAPER_GITHUB_CLIENT_SECRET."
                .to_string(),
        );
    }
    if !google && (publishers.names_an_email_or_domain() || commenters.names_an_email_or_domain()) {
        advice.warnings.push(
            "warning: a policy names an email address or a domain, but there is no Google \
             client; nobody can sign in that way. Set LIBREPAPER_GOOGLE_CLIENT_ID and \
             LIBREPAPER_GOOGLE_CLIENT_SECRET."
                .to_string(),
        );
    }
    if !github && !google {
        advice.warnings.push(format!(
            "warning: neither Google nor GitHub sign-in is configured; publishing and source writes are unavailable. Set LIBREPAPER_GITHUB_CLIENT_ID and LIBREPAPER_GITHUB_CLIENT_SECRET, or LIBREPAPER_GOOGLE_CLIENT_ID and LIBREPAPER_GOOGLE_CLIENT_SECRET (callbacks use {reader_origin}/auth/callback, or pass --port {port})."
        ));
    }
    advice
}

/// The stable Loro peer identity this deployment authors source under
/// (§7.3). Kept in `server_runtime_state` under `crate::log::DEPLOYMENT_PEER_STATE`
/// so a label written by one process and read by the next names one author,
/// rather than a fresh peer on every restart. Minted once, the first time a
/// deployment reads it absent.
async fn deployment_peer_key(
    catalog: &crate::storage::postgres::PostgresCatalog,
) -> Result<String, String> {
    if let Some(existing) = catalog
        .runtime_state(crate::log::DEPLOYMENT_PEER_STATE)
        .await
        .map_err(|error| format!("could not read the deployment peer key: {error}"))?
    {
        if let Some(key) = existing.get("key").and_then(|value| value.as_str()) {
            if !key.is_empty() {
                return Ok(key.to_string());
            }
        }
    }
    let key = crate::auth::random_token();
    catalog
        .set_runtime_state(
            crate::log::DEPLOYMENT_PEER_STATE,
            serde_json::json!({ "key": key }),
        )
        .await
        .map_err(|error| format!("could not store the deployment peer key: {error}"))?;
    Ok(key)
}

/// What the catalogue is allowed to hold, and how fast it may be asked to
/// hold it, as this deployment's configuration says.
fn storage_policy(config: &Configuration) -> crate::storage::postgres::StoragePolicy {
    crate::storage::postgres::StoragePolicy {
        owner_bytes: config.storage.per_owner,
        deployment_bytes: config.storage.total,
        asset_uploads_per_hour: config.storage.uploads_per_hour as i64,
    }
}

pub async fn serve(options: ServeOptions) {
    options
        .config
        .cost
        .validate()
        .unwrap_or_else(|error| die(format!("invalid cost policy: {error}")));
    let storage = options.storage.clone();
    let deployment_paths = storage.paths().unwrap_or_else(|err| die(err));
    let retention = parse_retention(options.expire_after.as_deref().unwrap_or(""))
        .unwrap_or_else(|err| die(format!("{err}; use a duration such as 24h or 30d")));
    // The origin pair is settled before storage is opened or a port claimed.
    // Two origins that share a host are not a deployment with a weaker
    // boundary; they are a deployment with none, so this refuses to start
    // rather than warn. It is a deployment diagnostic and not a claim that DNS
    // proves browser isolation: what the browser enforces is the
    // scheme/host/port tuple, and all startup can do is confirm the operator
    // asked for two different hosts and name the record and certificate that
    // have to exist for that to be true.
    let origins = match options
        .origin
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(reader) => Origins::configure(reader, options.docs_origin.as_deref())
            .unwrap_or_else(|error| die(error)),
        None => Origins::loopback_only(),
    };
    // Read before anything is opened or a port is claimed: a mirror value
    // which cannot work is a typo the operator is still standing in front of.
    let latex = validate_latex_mirror(&options.latex_mirror).unwrap_or_else(|err| die(err));
    // Likewise the site. It becomes a destination a browser is sent to, so it
    // is an absolute http(s) origin or it is a mistake -- a bare host would
    // be read as a path on this deployment and send a signed-out reader to a
    // page that does not exist.
    let site = match options.site_origin.as_deref().unwrap_or("").trim() {
        "" => None,
        flag => Some(validate_site_origin(flag).unwrap_or_else(|err| die(err))),
    };
    // The font library likewise: a directory that is not there is a typo,
    // and every file in one that is gets read now, for the families it holds.
    let fonts = match options.typst_fonts.as_deref().unwrap_or("").trim() {
        "" => None,
        flag => Some(crate::server::fonts::Library::open(flag).unwrap_or_else(|err| die(err))),
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
    let listener = listen(options.bind, options.port).await;
    let port = listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(options.port);
    let address = format!(":{port}");

    let oauth_secrets = secrets_from_environment();
    let app = GithubApp {
        client_id: options.github_client_id.unwrap_or_default(),
        client_secret: oauth_secrets.github_client_secret,
        ..GithubApp::default()
    };
    let google = GoogleApp {
        client_id: options.google_client_id.unwrap_or_default(),
        client_secret: oauth_secrets.google_client_secret,
        ..GoogleApp::default()
    };
    let publishers = Policy::parse_publishers(options.publishers.as_deref().unwrap_or(""))
        .unwrap_or_else(|error| die(error));
    if !publishers.is_configured() {
        die("say who may publish, with --publishers.\n\n    \
             --publishers your-github-login      only you\n    \
             --publishers alice,bob              those accounts\n    \
             --publishers any                    any authenticated Google or GitHub account");
    }
    let commenters = Policy::parse(options.commenters.as_deref().unwrap_or("anyone"));
    let loopback_origin = format!("http://localhost{address}");

    let advice = sign_in_advice(
        app.configured(),
        google.configured(),
        &publishers,
        &commenters,
        origins
            .reader()
            .map(|reader| reader.as_str())
            .unwrap_or(&loopback_origin),
        port,
    );
    for warning in &advice.warnings {
        eprintln!("{warning}");
    }
    if let Some(fatal) = advice.fatal {
        die(fatal);
    }

    let config = Arc::new(options.config);
    let shell = load_shell(&config).unwrap_or_else(|err| die(err));
    let secrets = &deployment_paths.secrets;
    let key_path = secrets.join("session.key");
    let key = session_key_file(&key_path, key_path.exists()).unwrap_or_else(|err| die(err));
    let mut database = crate::storage::postgres::PostgresOptions::new(&storage.database_url);
    database.max_connections = storage.database_connections;
    database.policy = storage_policy(&config);
    let catalog = Arc::new(
        crate::storage::postgres::PostgresCatalog::connect(database)
            .await
            .unwrap_or_else(|err| die(format!("could not connect to PostgreSQL: {err}"))),
    );
    catalog
        .migrate()
        .await
        .unwrap_or_else(|error| die(format!("could not migrate PostgreSQL: {error}")));
    let writer = catalog
        .claim_writer()
        .await
        .unwrap_or_else(|error| die(format!("could not claim deployment writer: {error}")));
    // The registry, the rooms and the background worker all share one
    // catalogue, blob store and configuration; they are wired together here
    // rather than separately so there is exactly one cache of any document
    // in this process (§4.3). The deployment's peer key is settled first
    // because the registry needs it to admit a sequencer.
    let peer_key = deployment_peer_key(&catalog)
        .await
        .unwrap_or_else(|err| die(err));
    let registry =
        crate::log::Registry::new(catalog.clone(), blobs.clone(), config.clone(), peer_key);
    let rooms = crate::room::Rooms::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        registry.clone(),
    );
    let (worker, background) = crate::storage::worker::Worker::new(
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
    instance.google = google;
    // An operator who wants no public front page at all: the examples stop
    // being listed to people who hold nothing on them.
    instance.listing = !options.no_listing;
    instance.simulate_activity = options.simulate_activity;
    instance.origins = origins.clone();
    instance.latex = Some(latex);
    instance.site = site;
    instance.fonts = fonts;
    // The local app for this machine: only a browser on this host can reach
    // it, and it lives under the deployment's private state so it shares
    // nothing with a standalone `librepaper local start`. Failing to start it
    // is a warning, not a death: the deployment serves documents regardless.
    let local = if options.no_local {
        None
    } else {
        match crate::local::embedded::start(&deployment_paths.state.join("local-app"), Vec::new())
            .await
        {
            Ok(local) => Some(local),
            Err(error) => {
                eprintln!("warning: local app not started: {error}");
                None
            }
        }
    };
    let instance = Arc::new(instance);

    match (origins.reader(), origins.docs()) {
        (Some(reader), Some(docs)) => {
            println!("librepaper serving {}", reader.as_str());
            println!("  documents on {}", docs.as_str());
        }
        _ => {
            println!("librepaper serving http://localhost{address}");
            println!("  documents on http://{DOCS_PREFIX}localhost{address}");
            println!("  no --origin: this deployment answers on loopback only");
        }
    }
    println!("  data in {}", blobs.describe());
    println!("  publishing: {}", publishers.describe());
    println!("  commenting: {}", commenters.describe());
    super::host_metrics::warn(&config, &deployment_paths.deployment);
    if std::io::stdout().is_terminal() {
        println!(
            "  storage: {} total bytes; {} bytes per owner",
            config.storage.total, config.storage.per_owner
        );
    }
    if let Some(library) = &instance.fonts {
        println!("  fonts: {}", library.describe());
    }
    if let Some(mirror) = &instance.latex {
        println!("  latex mirror: {mirror}");
        if mirror == crate::config::DEFAULT_LATEX_MIRROR {
            println!("  the project mirror promises only releases carried by this build");
        }
    }
    if let Some(local) = &local {
        println!(
            "  local app: {} (for browsers on this machine)",
            local.address
        );
    }
    if retention > 0 {
        println!(
            "  expiry: {expire_from} after {}",
            describe_seconds(retention)
        );
        instance
            .delete_expired(crate::util::now_unix(), retention, &expire_from)
            .await;
        let janitor = instance.clone();
        let from = expire_from.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3600));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                janitor
                    .delete_expired(crate::util::now_unix(), retention, &from)
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
    // above wakes on the durable state its own tasks write rather than on a
    // timer (§12).
    let sweeper = instance.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
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
    let router = instance.router();
    let serve_result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await;
    if let Some(local) = &local {
        local.stop().await;
    }
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
            "plain HTTP on loopback is `make deploy`, not a mistake"
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
