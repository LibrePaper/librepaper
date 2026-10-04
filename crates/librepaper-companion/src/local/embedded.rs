//! The local app that `librepaper admin serve` runs for the machine it is on.
//!
//! The standalone `librepaper start` exists so a browser on the author's
//! machine can hand work to tools installed there, and it asks for a pairing
//! code and a per-site grant because that browser might be talking to a
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
    service: LocalService,
}

impl Embedded {
    pub async fn stop(&self) {
        self.service.deny_pending_approvals().await;
        self.service.stop_previews().await;
    }
}

/// Start the service. `base` is a private directory of the deployment
/// (pairings and workspaces both go under it); `tool_path` is the local
/// `--tool-path` list, empty for plain `PATH` discovery. The default local
/// port is tried first so a browser that already knows it finds the service
/// there; when a standalone local app holds it, any free port serves, since
/// the browser is told the address rather than guessing it.
pub async fn start(base: &Path, tool_path: Vec<PathBuf>) -> Result<Arc<Embedded>, String> {
    let state_home = base.join("state");
    let cache_home = base.join("cache");
    let workspaces = base.join("workspaces");
    for directory in [&state_home, &cache_home, &workspaces] {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("could not prepare {}: {error}", directory.display()))?;
    }
    let tool_path = crate::local::settings::tool_paths(&state_home, tool_path)?;
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

    let instance = hex::encode(librepaper_base::util::random_bytes(8));
    let runner = Arc::new(NativeRunner::with_hosted_workspaces(
        tool_path,
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
    service.enable_control_panel().await?;
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
        service,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::sync::{Mutex, MutexGuard};

    #[cfg(unix)]
    static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(unix)]
    struct Environment {
        _lock: MutexGuard<'static, ()>,
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    #[cfg(unix)]
    impl Environment {
        fn isolated(root: &Path) -> Self {
            let lock = ENVIRONMENT_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let home = root.join("home");
            let state = root.join("xdg-state");
            let cache = root.join("xdg-cache");
            let config = root.join("xdg-config");
            let data = root.join("xdg-data");
            for path in [&home, &state, &cache, &config, &data] {
                std::fs::create_dir_all(path).unwrap();
            }
            let values = [
                ("HOME", home),
                ("XDG_STATE_HOME", state),
                ("XDG_CACHE_HOME", cache),
                ("XDG_CONFIG_HOME", config),
                ("XDG_DATA_HOME", data),
            ];
            let previous = values
                .iter()
                .map(|(key, value)| {
                    let old = std::env::var_os(key);
                    std::env::set_var(key, value);
                    (*key, old)
                })
                .collect();
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for Environment {
        fn drop(&mut self) {
            for (key, previous) in self.previous.drain(..) {
                if let Some(value) = previous {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    #[cfg(unix)]
    fn fake_typst(directory: &Path, version: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::create_dir_all(directory).unwrap();
        let executable = directory.join("typst");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", version),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    async fn typst_version(app: &Embedded, state_home: &Path) -> String {
        let token_path = state_home.join("librepaper/local/control-token.json");
        let token: serde_json::Value =
            serde_json::from_slice(&std::fs::read(token_path).unwrap()).unwrap();
        let client = reqwest::Client::new();
        let url = format!("{}companion/api/state", app.address);
        let mut state = None;
        for _ in 0..50 {
            if let Ok(response) = client
                .get(&url)
                .bearer_auth(token["token"].as_str().unwrap())
                .send()
                .await
            {
                if response.status().is_success() {
                    state = Some(response.json::<serde_json::Value>().await.unwrap());
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let state = state.expect("embedded control state should become available");
        state["tools"]["builders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|builder| builder["id"] == "typst")
            .and_then(|builder| builder["version"].as_str())
            .unwrap()
            .to_string()
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn embedded_restart_reloads_saved_paths_and_explicit_paths_override_them() {
        let temporary = tempfile::tempdir().unwrap();
        let _environment = Environment::isolated(temporary.path());
        let base = temporary.path().join("deployment");
        let state_home = base.join("state");
        let saved_tools = temporary.path().join("saved-tools");
        let explicit_tools = temporary.path().join("explicit-tools");
        fake_typst(&saved_tools, "typst persisted marker");
        fake_typst(&explicit_tools, "typst explicit marker");
        let settings = crate::local::settings::LocalSettings {
            tool_paths: vec![saved_tools],
            ..crate::local::settings::LocalSettings::default()
        };
        crate::local::settings::save(&state_home, &settings).unwrap();

        let first = start(&base, Vec::new()).await.unwrap();
        assert_eq!(
            typst_version(&first, &state_home).await,
            "typst persisted marker"
        );
        first.stop().await;
        drop(first);

        let restarted = start(&base, Vec::new()).await.unwrap();
        assert_eq!(
            typst_version(&restarted, &state_home).await,
            "typst persisted marker"
        );
        restarted.stop().await;

        let overridden = start(&base, vec![explicit_tools]).await.unwrap();
        assert_eq!(
            typst_version(&overridden, &state_home).await,
            "typst explicit marker"
        );
        overridden.stop().await;
    }
}
