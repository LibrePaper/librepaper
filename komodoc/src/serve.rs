//! `komodoc serve`: the whole service in this process.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::assets::load_shell;
use crate::auth::{session_key, GithubApp, GoogleApp, Policy};
use crate::config::Configuration;
use crate::origins::DOCS_PREFIX;
use crate::retention::{describe_seconds, parse_expire_from, parse_retention};
use crate::room::RoomSet;
use crate::server::Server;
use crate::storage::{migrate_legacy_source, open_storage, StorageOptions};
use crate::store::Store;
use crate::util::{die, first_of};

/// With no --port, serve takes the first free port in this range, so a second
/// deployment on the same machine, or a port something else has already
/// taken, needs no thought.
const PORT_FIRST: u16 = 8080;
const PORT_LAST: u16 = 8099;

pub struct ServeOptions {
    pub port: u16,
    pub storage: StorageOptions,
    pub client_id: String,
    pub client_secret: String,
    pub publishers: String,
    pub commenters: String,
    pub no_listing: bool,
    pub expire_after: String,
    pub expire_from: String,
    /// Where this deployment reads LaTeX distributions from: an https bucket,
    /// a directory on this machine, or empty for a deployment that serves no
    /// LaTeX at all. See `crate::latex`.
    pub latex: String,
    pub config: Configuration,
}

/// Claims a port: the one asked for, or the first free one in the default
/// range when port is zero.
async fn listen(port: u16) -> TcpListener {
    if port != 0 {
        return match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => die(format!(
                "port {port} is already in use. Pick another with --port."
            )),
            Err(err) => die(format!("could not listen on port {port}: {err}")),
        };
    }
    for candidate in PORT_FIRST..=PORT_LAST {
        match TcpListener::bind(("0.0.0.0", candidate)).await {
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
             nobody can sign in that way. Set KOMODOC_GITHUB_CLIENT_ID and \
             KOMODOC_GITHUB_CLIENT_SECRET."
                .to_string(),
        );
    }
    if !google && (publishers.names_an_email_or_domain() || commenters.names_an_email_or_domain()) {
        advice.warnings.push(
            "warning: a policy names an email address or a domain, but there is no Google \
             client; nobody can sign in that way. Set KOMODOC_GOOGLE_CLIENT_ID and \
             KOMODOC_GOOGLE_CLIENT_SECRET."
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
             export KOMODOC_GITHUB_CLIENT_ID=...\n    export KOMODOC_GITHUB_CLIENT_SECRET=...\n\n  \
             Google, at https://console.cloud.google.com (Credentials, Web application):\n\n    \
             Authorised redirect URI  http://localhost{address}/auth/callback/google\n\n    \
             export KOMODOC_GOOGLE_CLIENT_ID=...\n    export KOMODOC_GOOGLE_CLIENT_SECRET=...\n\n  \
             Either callback has to match the port, so pass --port {port} to keep it fixed."
        ));
    }
    advice
}

pub async fn serve(options: ServeOptions) {
    let env = |name: &str| std::env::var(name).unwrap_or_default();
    let retention = parse_retention(&first_of(&[
        &options.expire_after,
        &env("KOMODOC_EXPIRE_AFTER"),
    ]))
    .unwrap_or_else(|err| die(format!("{err}; use a duration such as 24h or 30d")));
    // Read before anything is opened or a port is claimed: a mirror flag that
    // cannot work is a typo the operator is still standing in front of, and a
    // plain HTTP one would fail invisibly in every browser rather than here.
    let latex = match first_of(&[&options.latex, &env("KOMODOC_LATEX")]).trim() {
        "" => None,
        flag => Some(crate::latex::Mirror::open(flag).unwrap_or_else(|err| die(err))),
    };
    let expire_from = parse_expire_from(&first_of(&[
        &options.expire_from,
        &env("KOMODOC_EXPIRE_FROM"),
    ]))
    .unwrap_or_else(|err| die(err));
    let blobs = open_storage(options.storage.clone())
        .await
        .unwrap_or_else(|err| die(err));
    // One pass, and nothing to do on a store that never had the old layout.
    let moved = migrate_legacy_source(blobs.as_ref()).await;
    if moved > 0 {
        println!("  moved {moved} source file(s) to the shared key layout");
    }

    // Claim the port first, so a port already in use costs nothing and the
    // advice below can name the callback URL this run would actually use.
    let listener = listen(options.port).await;
    let port = listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(options.port);
    let address = format!(":{port}");

    let app = GithubApp {
        client_id: first_of(&[&options.client_id, &env("KOMODOC_GITHUB_CLIENT_ID")]),
        client_secret: first_of(&[&options.client_secret, &env("KOMODOC_GITHUB_CLIENT_SECRET")]),
        ..GithubApp::default()
    };
    // Google has no flags: a client secret belongs in the environment, and the
    // README already tells operators to keep it there.
    let google = GoogleApp {
        client_id: first_of(&[&env("KOMODOC_GOOGLE_CLIENT_ID")]),
        client_secret: first_of(&[&env("KOMODOC_GOOGLE_CLIENT_SECRET")]),
        ..GoogleApp::default()
    };
    let publishers = Policy::parse(&first_of(&[
        &options.publishers,
        &env("KOMODOC_PUBLISHERS"),
    ]));
    if !publishers.is_configured() {
        die("say who may publish, with --publishers.\n\n    \
             --publishers your-github-login      only you\n    \
             --publishers alice,bob              those accounts\n    \
             --publishers any                    any GitHub account\n    \
             --publishers anyone                 no sign-in at all");
    }
    let commenters = Policy::parse(&first_of(&[
        &options.commenters,
        &env("KOMODOC_COMMENTERS"),
        "anyone",
    ]));

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
    let key = session_key(blobs.as_ref())
        .await
        .unwrap_or_else(|err| die(err));
    let store = Store::open(blobs.clone(), config.clone())
        .await
        .unwrap_or_else(|err| die(err));
    let rooms = RoomSet::new(blobs.clone(), config.clone());
    let mut instance = Server::new(
        store,
        rooms,
        shell,
        app,
        key,
        config,
        publishers.clone(),
        commenters.clone(),
    );
    instance.google = google;
    instance.direct_reads = options.storage.direct_reads;
    // An operator who wants no public front page at all. A document that asks
    // to be `listed` behaves as `link` under it, and the share dialog does not
    // offer the choice.
    instance.listing = !options.no_listing;
    instance.latex = latex;
    let instance = Arc::new(instance);

    println!("komodoc serving http://localhost{address}");
    println!("  documents on http://{DOCS_PREFIX}localhost{address}");
    println!("  data in {}", blobs.describe());
    println!("  publishing: {}", publishers.describe());
    println!("  commenting: {}", commenters.describe());
    // A mirror that answers nothing is a card that spins, and the only place
    // anyone will connect the two is here. Unreachable is a warning, never a
    // death: a deployment that serves markdown has no business refusing to
    // start because a bucket is still filling.
    if let Some(mirror) = &instance.latex {
        println!("  latex: {}", mirror.describe());
        if let Some(warning) = mirror.probe().await {
            eprintln!("{warning}");
        }
    }
    if retention > 0 {
        println!(
            "  expiry: {expire_from} after {}",
            describe_seconds(retention)
        );
        instance
            .delete_expired(crate::clock::now_unix(), retention, &expire_from)
            .await;
        let janitor = instance.clone();
        let from = expire_from.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3600));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                janitor
                    .delete_expired(crate::clock::now_unix(), retention, &from)
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
        loop {
            ticker.tick().await;
            sweeper.rooms.sweep().await;
        }
    });

    // A server on its way down writes what it holds. An acknowledged edit
    // survives a restart because it was written when it was acknowledged; this
    // is about the unacknowledged ones, which have no reason to be lost to an
    // orderly shutdown.
    let closing = instance.clone();
    let shutdown = async move {
        let _ = tokio::signal::ctrl_c().await;
        closing.rooms.flush().await;
    };

    let router = instance.router();
    if let Err(err) = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
    {
        die(err);
    }
}
