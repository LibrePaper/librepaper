//! `komodoc serve`: the whole service in this process.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::assets::load_shell;
use crate::auth::{session_key, GithubApp, Policy};
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

pub async fn serve(options: ServeOptions) {
    let env = |name: &str| std::env::var(name).unwrap_or_default();
    let retention = parse_retention(&first_of(&[
        &options.expire_after,
        &env("KOMODOC_EXPIRE_AFTER"),
    ]))
    .unwrap_or_else(|err| die(format!("{err}; use a duration such as 24h or 30d")));
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

    // The OAuth app is only needed when something here asks for a GitHub
    // account; a wholly public server runs without one.
    if !app.configured() && !(publishers.public && commenters.public) {
        die(format!(
            "this needs a GitHub OAuth app.\n\n  \
             Create one at https://github.com/settings/developers (New OAuth App):\n\n    \
             Homepage URL          http://localhost{address}\n    \
             Authorization callback  http://localhost{address}/auth/callback\n\n  \
             Then generate a client secret and:\n\n    \
             export KOMODOC_GITHUB_CLIENT_ID=...\n    export KOMODOC_GITHUB_CLIENT_SECRET=...\n\n  \
             The callback has to match the port, so pass --port {port} to keep it fixed."
        ));
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
    instance.direct_reads = options.storage.direct_reads;
    // An operator who wants no public front page at all. A document that asks
    // to be `listed` behaves as `link` under it, and the share dialog does not
    // offer the choice.
    instance.listing = !options.no_listing;
    let instance = Arc::new(instance);

    println!("komodoc serving http://localhost{address}");
    println!("  documents on http://{DOCS_PREFIX}localhost{address}");
    println!("  data in {}", blobs.describe());
    println!("  publishing: {}", publishers.describe());
    println!("  commenting: {}", commenters.describe());
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
