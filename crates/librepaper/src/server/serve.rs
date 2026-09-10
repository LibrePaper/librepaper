//! `librepaper serve`: the whole service in this process.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::auth::{link_sealing_keyring_file, session_key_file, GithubApp, GoogleApp, Policy};
use crate::config::Configuration;
use crate::document::retention::{describe_seconds, parse_expire_from, parse_retention};
use crate::document::store::Store;
use crate::room::RoomSet;
use crate::server::origins::DOCS_PREFIX;
use crate::server::shell::load_shell;
use crate::server::Server;
use crate::storage::journal::JournalStore;
use crate::storage::maintenance::{DeletionLimits, DeletionWorker, JournalRetirementWorker};
use crate::storage::{migrate_legacy_source, open_storage, StorageOptions};
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
    pub expire_after: Option<String>,
    pub expire_from: Option<String>,
    /// Where this deployment reads LaTeX distributions from: an https bucket
    /// or a directory on this machine. Blank falls back to the project's
    /// own default mirror: LibrePaper always serves LaTeX. See
    /// `crate::server::latex`.
    pub latex: String,
    /// A directory of font files served to typst documents, or nothing. See
    /// `crate::server::fonts`.
    pub fonts: Option<String>,
    /// The browser bibliography VM's descriptor, as `<url>#<sha256>`, or
    /// nothing for a deployment that offers none. See `crate::server::latex::BiberVm`.
    pub biber_vm: Option<String>,
    /// Do not run the local app for this machine. Without it, `serve` also
    /// starts the loopback service that renders Quarto documents for an
    /// editor whose browser is on this host; see `crate::local::embedded`.
    pub no_local: bool,
    pub config: Configuration,
}

/// The LaTeX mirror flag LibrePaper actually uses. LibrePaper always serves
/// LaTeX, so a blank `--latex` falls back to the project's own mirror: there
/// is no "LaTeX off" mode.
pub(crate) fn resolve_latex_flag(flag: &str) -> &str {
    match flag.trim() {
        "" => crate::server::latex::DEFAULT_MIRROR,
        flag => flag,
    }
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

/// What startup has to say about signing in: at most one reason not to start,
/// and any number of warnings that it will. Worked out here rather than inline
/// so the answer can be asserted without starting a server.
#[derive(Debug, Default)]
pub struct SignInAdvice {
    pub fatal: Option<String>,
    pub warnings: Vec<String>,
}

/// A provider is only needed when something here asks for an account; a wholly
/// public server runs without any. Either one will do, so the refusal lists
/// both ways to get one.
///
/// The two warnings are not deaths: a deployment may be mid-migration, and a
/// list naming somebody no configured provider can produce is worth saying out
/// loud rather than refusing to start over.
pub fn sign_in_advice(
    github: bool,
    google: bool,
    publishers: &Policy,
    commenters: &Policy,
    address: &str,
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
    let needs_sign_in = !(publishers.public && commenters.public);
    if !github && !google && needs_sign_in {
        advice.fatal = Some(format!(
            "this needs a way to sign people in: a GitHub OAuth app, a Google client, or both.\n\n  \
             GitHub, at https://github.com/settings/developers (New OAuth App):\n\n    \
             Homepage URL          http://localhost{address}\n    \
             Authorization callback  http://localhost{address}/auth/callback\n\n  \
             Then generate a client secret and:\n\n    \
             export LIBREPAPER_GITHUB_CLIENT_ID=...\n    export LIBREPAPER_GITHUB_CLIENT_SECRET=...\n\n  \
             Google, at https://console.cloud.google.com (Credentials, Web application):\n\n    \
             Authorised redirect URI  http://localhost{address}/auth/callback/google\n\n    \
             export LIBREPAPER_GOOGLE_CLIENT_ID=...\n    export LIBREPAPER_GOOGLE_CLIENT_SECRET=...\n\n  \
             Either callback has to match the port, so pass --port {port} to keep it fixed."
        ));
    }
    advice
}

pub async fn serve(options: ServeOptions) {
    let storage = options.storage.clone();
    let durable = storage.fsync;
    let deployment_paths = storage.paths().unwrap_or_else(|err| die(err));
    let retention = parse_retention(options.expire_after.as_deref().unwrap_or(""))
        .unwrap_or_else(|err| die(format!("{err}; use a duration such as 24h or 30d")));
    // Read before anything is opened or a port is claimed: a mirror flag that
    // cannot work is a typo the operator is still standing in front of, and a
    // plain HTTP one would fail invisibly in every browser rather than here.
    // LibrePaper always serves LaTeX: an absent --latex falls back to the
    // project's own mirror, so there is no "LaTeX off" mode.
    let latex_flag = resolve_latex_flag(&options.latex);
    let latex = Some(crate::server::latex::Mirror::open(latex_flag).unwrap_or_else(|err| die(err)));
    // The font library likewise: a directory that is not there is a typo,
    // and every file in one that is gets read now, for the families it holds.
    let fonts = match options.fonts.as_deref().unwrap_or("").trim() {
        "" => None,
        flag => Some(crate::server::fonts::Library::open(flag).unwrap_or_else(|err| die(err))),
    };
    // Same startup-time refusal as --latex/--fonts: a malformed flag is a
    // typo the operator is still standing in front of.
    let biber_vm = match options.biber_vm.as_deref().unwrap_or("").trim() {
        "" => None,
        flag => Some(crate::server::latex::BiberVm::parse(flag).unwrap_or_else(|err| die(err))),
    };
    let expire_from = parse_expire_from(options.expire_from.as_deref().unwrap_or(""))
        .unwrap_or_else(|err| die(err));
    let blobs = open_storage(storage).await.unwrap_or_else(|err| die(err));
    let writer_lock =
        acquire_writer_lock(&deployment_paths.writer_lock).unwrap_or_else(|err| die(err));
    let reset_marker = deployment_paths.state.join("seed-reset.json");
    if reset_marker.exists() {
        die(format!(
            "refusing to serve while an interrupted local seed reset remains: {}",
            reset_marker.display()
        ));
    }
    // One pass, and nothing to do on a store that never had the old layout.
    let moved = migrate_legacy_source(blobs.as_ref()).await;
    if moved > 0 {
        println!("  moved {moved} source file(s) to the shared key layout");
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
    let publishers = Policy::parse(options.publishers.as_deref().unwrap_or(""));
    if !publishers.is_configured() {
        die("say who may publish, with --publishers.\n\n    \
             --publishers your-github-login      only you\n    \
             --publishers alice,bob              those accounts\n    \
             --publishers any                    any GitHub account\n    \
             --publishers anyone                 no sign-in at all");
    }
    let commenters = Policy::parse(options.commenters.as_deref().unwrap_or("anyone"));

    let advice = sign_in_advice(
        app.configured(),
        google.configured(),
        &publishers,
        &commenters,
        &address,
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
    let catalog_path = &deployment_paths.catalog;
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open_with(catalog_path, durable)
            .unwrap_or_else(|err| die(format!("could not open catalogue: {err}"))),
    );
    crate::config::DeploymentPaths::protect_file(catalog_path).unwrap_or_else(|err| die(err));
    let catalog_nonempty = catalog
        .totals()
        .map(|(_, documents)| documents != 0)
        .unwrap_or_else(|err| die(format!("could not inspect catalogue: {err}")));
    let deployment_id = deployment_paths
        .ensure_deployment_identity(catalog_nonempty)
        .unwrap_or_else(|err| die(err));
    let secrets = &deployment_paths.secrets;
    let key = session_key_file(&secrets.join("session.key"), catalog_nonempty)
        .unwrap_or_else(|err| die(err));
    let link_sealing_keys = link_sealing_keyring_file(&secrets.join("links.key"), catalog_nonempty)
        .unwrap_or_else(|err| die(err));
    catalog
        .set_link_sealing_key(&link_sealing_keys[0])
        .unwrap_or_else(|err| die(format!("could not configure link sealing: {err}")));
    for old in link_sealing_keys.iter().skip(1) {
        catalog
            .add_link_decryption_key(old)
            .unwrap_or_else(|err| die(format!("could not configure old link key: {err}")));
    }
    let store = Store::open_with_catalog(blobs.clone(), config.clone(), catalog)
        .await
        .unwrap_or_else(|err| die(err));
    // Reconcile durable publication state and reclaim only jobs already
    // queued by a committed catalogue transition before accepting traffic.
    // A complete object set is retried with the original ids; an incomplete
    // set is resolved as a known abort and queued for safe reclamation.
    let (deletion_worker, journal_retirement_worker) = if let Some(catalog) = store.catalog.as_ref()
    {
        let journal = JournalStore::new(catalog.clone());
        journal
            .initialize_local(&deployment_id)
            .unwrap_or_else(|err| die(format!("could not initialize local journal: {err}")));
        journal
            .reconcile_pending(blobs.as_ref())
            .await
            .unwrap_or_else(|err| die(format!("local journal reconciliation failed: {err}")));
        journal
            .reconcile_object_reservations(blobs.as_ref())
            .await
            .unwrap_or_else(|err| die(format!("local journal accounting recovery failed: {err}")));
        journal
            .require_recovered()
            .unwrap_or_else(|err| die(format!("local journal recovery is required: {err}")));
        let worker = Arc::new(
            DeletionWorker::new(catalog.clone(), blobs.clone(), DeletionLimits::default())
                .unwrap_or_else(|err| die(format!("could not initialize deletion worker: {err}"))),
        );
        worker
            .run_once(crate::util::now_unix())
            .await
            .unwrap_or_else(|err| die(format!("local deletion recovery failed: {err}")));
        let journal_worker = Arc::new(
            JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 1_000)
                .unwrap_or_else(|err| die(format!("could not initialize journal cleanup: {err}"))),
        );
        journal_worker
            .run_once(crate::util::now_unix())
            .await
            .unwrap_or_else(|err| die(format!("local journal cleanup failed: {err}")));
        (Some(worker), Some(journal_worker))
    } else {
        (None, None)
    };
    let rooms = RoomSet::new(blobs.clone(), config.clone());
    rooms.attach_deployment_lock(writer_lock);
    let mut instance = Server::new(
        store,
        rooms,
        shell,
        app,
        key,
        config.clone(),
        publishers.clone(),
        commenters.clone(),
    );
    if let Some(catalog) = instance.store.catalog.clone() {
        let journal = crate::storage::journal::JournalRuntime::new_with_policy(
            catalog,
            blobs.clone(),
            deployment_id.clone(),
            crate::storage::journal::CoordinatorLimits::from_persistence(&config.persistence()),
            config.persistence(),
            config.storage.per_owner,
            config.storage.total,
        )
        .unwrap_or_else(|err| die(format!("could not initialize journal coordinator: {err}")));
        instance.rooms.attach_journal(journal);
    }
    instance.google = google;
    // An operator who wants no public front page at all: the examples stop
    // being listed to people who hold nothing on them.
    instance.listing = !options.no_listing;
    instance.latex = latex;
    instance.fonts = fonts;
    instance.biber_vm = biber_vm;
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

    println!("librepaper serving http://localhost{address}");
    println!("  documents on http://{DOCS_PREFIX}localhost{address}");
    println!("  data in {}", blobs.describe());
    println!("  publishing: {}", publishers.describe());
    println!("  commenting: {}", commenters.describe());
    // A mirror that answers nothing is a card that spins, and the only place
    // anyone will connect the two is here. Unreachable is a warning, never a
    // death: a deployment that serves markdown has no business refusing to
    // start because a bucket is still filling.
    if let Some(library) = &instance.fonts {
        println!("  fonts: {}", library.describe());
    }
    if let Some(mirror) = &instance.latex {
        println!("  latex: {}", mirror.describe());
        if let Some(warning) = mirror.probe().await {
            eprintln!("{warning}");
        }
    }
    if let Some(vm) = &instance.biber_vm {
        println!("  biber vm: {}", vm.url);
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

    // The sweeper. A document nobody has open is still written out and still
    // gets its checkpoints: that is the whole difference between a session the
    // server holds and a session it relays, and it is why no browser has to
    // stay open to save another client's work.
    let sweeper = instance.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            sweeper.rooms.sweep().await;
        }
    });

    // A link's expiry is not a request anything hangs off of -- nobody asks
    // the server "has this key gone stale yet". A socket that dialed in while
    // it was still live would otherwise keep answering after it lapsed, so
    // this reruns every open socket's authorization once a second the same
    // way a sharing change or a transfer does on the spot.
    let reauthorizer = instance.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            reauthorizer.reauthorize_all().await;
        }
    });

    if let Some(worker) = deletion_worker.clone() {
        let erasure_catalog = instance.store.catalog.clone();
        let journal_worker = journal_retirement_worker.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(error) = worker.run_once(crate::util::now_unix()).await {
                    eprintln!("warning: local deletion maintenance failed: {error}");
                }
                if let Some(journal_worker) = &journal_worker {
                    if let Err(error) = journal_worker.run_once(crate::util::now_unix()).await {
                        eprintln!("warning: local journal cleanup failed: {error}");
                    }
                }
                if let Some(catalog) = &erasure_catalog {
                    let _ = catalog
                        .execute_catalog(512, |catalog| {
                            catalog.prune_checkpoint_budgets(crate::util::now_unix(), 1_000)
                        })
                        .await;
                    if let Err(error) = crate::storage::maintenance::run_erasure_pass_async(
                        catalog,
                        crate::util::now_unix(),
                        25,
                        250,
                    )
                    .await
                    {
                        eprintln!("warning: local account erasure failed: {error}");
                    }
                }
            }
        });
    }

    // A server on its way down writes what it holds. An acknowledged edit
    // survives a restart because it was written when it was acknowledged; this
    // is about the unacknowledged ones, which have no reason to be lost to an
    // orderly shutdown.
    let closing = instance.clone();
    let closing_deletion_worker = deletion_worker;
    let closing_journal_worker = journal_retirement_worker;
    let shutdown = async move {
        shutdown_signal().await;
        closing.rooms.flush().await;
        if let Some(worker) = closing_deletion_worker {
            if let Err(error) = worker.run_once(crate::util::now_unix()).await {
                eprintln!("warning: final local deletion maintenance failed: {error}");
            }
        }
        if let Some(worker) = closing_journal_worker {
            if let Err(error) = worker.run_once(crate::util::now_unix()).await {
                eprintln!("warning: final local journal cleanup failed: {error}");
            }
        }
    };

    let closing_catalog = instance.store.catalog.clone();
    let router = instance.router();
    let serve_result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await;
    // Only now: the graceful shutdown above stops accepting and then drains
    // the requests already in flight, and those requests still submit
    // catalogue jobs. Closing admission any earlier would fail a request that
    // was accepted before the signal. By this point the room set has been
    // flushed and the deletion and journal workers have run their final pass,
    // so what remains is whatever is still queued or executing: this rejects
    // the queue, lets the executing transaction and its completion hook
    // finish, and only then closes SQLite.
    if let Some(catalog) = closing_catalog.as_ref() {
        catalog.shutdown().await;
    }
    if let Err(err) = serve_result {
        die(err);
    }
}

pub(crate) fn acquire_writer_lock(path: &std::path::Path) -> Result<std::fs::File, String> {
    use fs2::FileExt;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|err| format!("could not open writer lock {}: {err}", path.display()))?;
    file.try_lock_exclusive().map_err(|err| {
        format!(
            "another server or maintenance command holds {}: {err}",
            path.display()
        )
    })?;
    Ok(file)
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
