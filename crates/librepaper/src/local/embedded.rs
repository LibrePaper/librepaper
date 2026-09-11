//! The local app that `librepaper admin serve` runs for the machine it is on.
//!
//! The standalone `librepaper local start` exists so a browser on the author's
//! machine can hand work to tools installed there, and it asks for a pairing
//! code and a per-project grant because that browser might be talking to a
//! deployment anywhere. When the deployment itself runs on this machine, both
//! ceremonies are answering a question that has no other answer: the server
//! owns the service, so it can mint a pairing for its own editors, and it owns
//! the documents, so it can keep a workspace per document to render in. This
//! module is that: the same loopback service, started in-process, reachable
//! only from a browser on this host, paired through the server rather than
//! through a code.
//!
//! State lives under the deployment's own data directory, not the user's XDG
//! state home, so it never mixes with a standalone local app's pairings or
//! grants.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::TcpListener;

use super::protocol::DEFAULT_PORT;
use super::service::{LocalService, NativeRunner};

/// A running embedded service: where a browser on this machine reaches it.
/// Pairing goes through the service's own consent page, exactly as with a
/// standalone local app, so the server holds no pairing authority of its own.
pub struct Embedded {
    /// `http://127.0.0.1:<port>/`.
    pub address: String,
}

/// Start the service. `base` is a private directory of the deployment
/// (pairings and workspaces both go under it); `tex_path` is the local
/// `--tex-path` list, empty for plain `PATH` discovery. The default local
/// port is tried first so a browser that already knows it finds the service
/// there; when a standalone local app holds it, any free port serves, since
/// the browser is told the address rather than guessing it.
pub async fn start(base: &Path, tex_path: Vec<PathBuf>) -> Result<Arc<Embedded>, String> {
    let state_home = base.join("state");
    let cache_home = base.join("cache");
    let workspaces = base.join("workspaces");
    for directory in [&state_home, &cache_home, &workspaces] {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("could not prepare {}: {error}", directory.display()))?;
    }
    let listener = match TcpListener::bind(("127.0.0.1", DEFAULT_PORT)).await {
        Ok(listener) => listener,
        Err(_) => TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("could not listen on loopback: {error}"))?,
    };
    let port = listener
        .local_addr()
        .map_err(|error| format!("could not read the local port: {error}"))?
        .port();
    let listener_v6 = TcpListener::bind(("::1", port)).await.ok();

    let instance = hex::encode(crate::auth::random_bytes(8));
    let runner = Arc::new(NativeRunner::with_hosted_workspaces(
        tex_path,
        &state_home,
        workspaces.clone(),
    ));
    let service = LocalService::with_hosted_workspaces_and_code(
        port,
        instance,
        &state_home,
        &cache_home,
        runner,
        None,
        workspaces,
    );
    let router = service.router();
    if let Some(v6) = listener_v6 {
        let make_v6 = router
            .clone()
            .into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move {
            let _ = axum::serve(v6, make_v6).await;
        });
    }
    let make_v4 = router.into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, make_v4).await;
    });

    Ok(Arc::new(Embedded {
        address: format!("http://127.0.0.1:{port}/"),
    }))
}
