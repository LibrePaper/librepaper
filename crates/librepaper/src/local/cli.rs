//! `librepaper local <command>`: the command line front end for the loopback
//! service. See `docs/specs/wasmtex.md`, "App integration", and
//! `docs/specs/wasmtex-interfaces.md` section 7.
//!
//! This first version runs `start` in the foreground always: `--foreground`
//! is accepted and honoured (there is nothing else to do yet) but detaching
//! into a background process is platform work saved for later, as the spec
//! allows -- "Final command naming can follow the existing CLI conventions"
//! does not promise a daemon on day one, and a foreground process a terminal
//! can Ctrl-C is a perfectly good first local service.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use crate::cli::config_home;
use crate::cli::{LocalArgs, LocalCommand};
use crate::local::pairing::{generate_code, PairingStore, ServiceState};
use crate::local::protocol::{self, DEFAULT_PORT};
use crate::local::service::{LocalService, NativeRunner, Runner};
use crate::util::die;

pub async fn run(args: LocalArgs) {
    match args.command {
        LocalCommand::Start {
            port,
            foreground,
            code,
            tex_path,
        } => start(port, foreground, code, tex_path).await,
        LocalCommand::Status => status().await,
        LocalCommand::Doctor { tex_path } => doctor(tex_path).await,
        LocalCommand::Disconnect { origin, all } => disconnect(origin, all),
        LocalCommand::Rescan { tex_path } => rescan(tex_path).await,
    }
}

/// The cache-home base directory a job workspace lives under:
/// `<cache_home>/librepaper/local/jobs/<id>/`, never under a project directory
/// and never under the config home the tokens live in. Read here, not in
/// `service.rs`, so the service itself stays free of environment reads and
/// testable with an explicit directory -- the same reasoning
/// `crate::cli::config_home` gives for the token cache.
fn cache_home() -> PathBuf {
    match std::env::var("XDG_CACHE_HOME") {
        Ok(base) if !base.is_empty() => PathBuf::from(base),
        _ => {
            let home = std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| die("no home directory to store the job cache in"));
            PathBuf::from(home).join(".cache")
        }
    }
}

/// Whether `pid` still names a live process. Best-effort: `kill -0` is
/// portable across Linux and macOS without a process-inspection dependency;
/// on a platform where it is unavailable we assume the pid is gone rather
/// than refusing to start forever on a stale `service.json`. The real guard
/// either way is the port bind right after this check, which fails loudly
/// if something is actually still listening.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

async fn start(port: u16, foreground: bool, code: Option<String>, tex_path: Vec<PathBuf>) {
    if !foreground {
        println!(
            "note: --foreground has no effect yet; librepaper local always stays attached to \
             this terminal in this version. Detaching is planned but not built."
        );
    }

    let config_home = config_home();
    let pairing = PairingStore::new(&config_home, code.clone());
    let port = if port == 0 { DEFAULT_PORT } else { port };

    if let Some(existing) = pairing.read_service() {
        if existing.port == port && process_alive(existing.pid) {
            die(format!(
                "librepaper local is already running on port {} (pid {}). Use `librepaper local \
                 status` to check it or stop that process first.",
                existing.port, existing.pid
            ));
        }
    }

    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => die(format!(
            "port {port} is already in use. Pick another with --port, or stop whatever is \
             listening there."
        )),
        Err(err) => die(format!("could not listen on 127.0.0.1:{port}: {err}")),
    };
    // Best-effort: some machines have IPv6 disabled entirely, and the
    // loopback service works fine on IPv4 alone when it is.
    let listener_v6 = TcpListener::bind(("::1", port)).await.ok();

    let instance = hex::encode(crate::auth::random_bytes(8));
    let code = code
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(generate_code);
    let state = ServiceState {
        port,
        instance: instance.clone(),
        code: code.clone(),
        pid: std::process::id(),
        started: crate::auth::now_unix(),
    };
    if let Err(err) = pairing.write_service(&state) {
        die(format!("could not write service.json: {err}"));
    }

    let runner: Arc<dyn Runner> = Arc::new(NativeRunner { tex_path });
    let service = LocalService::new(
        port,
        instance,
        &config_home,
        &cache_home(),
        runner,
        Some(code.clone()),
    );
    let router = service.router();

    println!(
        "librepaper local listening on http://127.0.0.1:{port}{}",
        protocol::BASE_PATH
    );
    println!("pairing code: {code}");
    print_pairings(&pairing);
    println!("Ctrl-C to stop.");

    if let Some(v6) = listener_v6 {
        let make_v6 = router
            .clone()
            .into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move {
            let _ = axum::serve(v6, make_v6).await;
        });
    }

    let make_v4 = router.into_make_service_with_connect_info::<SocketAddr>();
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(err) = axum::serve(listener, make_v4)
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("error: {err}");
    }

    pairing.remove_service();
    println!("librepaper local stopped");
}

async fn status() {
    let pairing = PairingStore::new(&config_home(), None);
    let Some(state) = pairing.read_service() else {
        println!("librepaper local is not running");
        return;
    };
    let url = format!(
        "http://127.0.0.1:{}{}/health",
        state.port,
        protocol::BASE_PATH
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build();
    let reachable = match client {
        Ok(client) => client.get(&url).send().await,
        Err(err) => Err(err),
    };
    match reachable {
        Ok(response) if response.status().is_success() => {
            println!("reachable at {url} (pid {})", state.pid);
        }
        Ok(response) => println!(
            "listening at {url} but answered unexpectedly: {}",
            response.status()
        ),
        Err(_) => println!(
            "not reachable at {url}; service.json names pid {} but nothing answered",
            state.pid
        ),
    }
    println!("pairing code: {}", state.code);
    print_pairings(&pairing);
}

fn print_pairings(pairing: &PairingStore) {
    let pairings = pairing.active_pairings();
    if pairings.is_empty() {
        println!("no paired origins");
        return;
    }
    println!("paired origins:");
    for (origin, project) in pairings {
        println!("  {origin}  {project}");
    }
}

async fn doctor(tex_path: Vec<PathBuf>) {
    let capabilities = crate::local::discovery::discover(true, &tex_path).await;
    println!("librepaper local doctor");
    println!();
    println!("platform: {}", capabilities.platform);
    if let Some(distribution) = &capabilities.distribution {
        println!(
            "distribution: {} ({})",
            distribution.name, distribution.year
        );
    } else {
        println!("distribution: not detected");
    }
    println!();
    println!("tools:");
    print_tool("pdflatex", &capabilities.tools.pdflatex);
    print_tool("xelatex", &capabilities.tools.xelatex);
    print_tool("lualatex", &capabilities.tools.lualatex);
    print_tool("bibtex", &capabilities.tools.bibtex);
    print_tool("bibtex8", &capabilities.tools.bibtex8);
    print_tool("biber", &capabilities.tools.biber);
    print_tool("makeindex", &capabilities.tools.makeindex);
    println!();
    if capabilities.confinement.available {
        println!("confinement: {}", capabilities.confinement.kind);
    } else {
        println!(
            "confinement: none ({})",
            if capabilities.confinement.reason.is_empty() {
                "not available on this machine"
            } else {
                capabilities.confinement.reason.as_str()
            }
        );
        println!(
            "  a native compile will run unconfined until this is addressed; see \
             docs/specs/wasmtex-interfaces.md, \"Native execution boundary\""
        );
    }
}

fn print_tool(name: &str, tool: &protocol::Tool) {
    if tool.available {
        println!("  {name}: {}", tool.version.as_deref().unwrap_or("found"));
    } else {
        let note = if tool.note.is_empty() {
            "not found".to_string()
        } else {
            tool.note.clone()
        };
        println!("  {name}: {note}");
    }
}

fn disconnect(origin: Option<String>, all: bool) {
    if !all && origin.is_none() {
        die("pass --origin <URL> or --all");
    }
    let pairing = PairingStore::new(&config_home(), None);
    let target = if all { None } else { origin.as_deref() };
    let removed = pairing.revoke(target);
    if removed == 0 {
        println!("nothing to revoke");
    } else {
        println!("revoked {removed} pairing(s)");
    }
}

async fn rescan(tex_path: Vec<PathBuf>) {
    // This refreshes the on-disk cache `discovery.rs` keeps under
    // `<config_home>/librepaper/local/tools.json`, which a running service
    // picks up on its own next capability check since it consults the same
    // cache file. A future version could additionally ping a running
    // service's `capabilities/rescan` route so the change is visible sooner
    // than that service's next request, but the cache file is the shared
    // source of truth either way.
    let capabilities = crate::local::discovery::discover(true, &tex_path).await;
    let found = [
        ("pdflatex", capabilities.tools.pdflatex.available),
        ("xelatex", capabilities.tools.xelatex.available),
        ("lualatex", capabilities.tools.lualatex.available),
        ("bibtex", capabilities.tools.bibtex.available),
        ("bibtex8", capabilities.tools.bibtex8.available),
        ("biber", capabilities.tools.biber.available),
        ("makeindex", capabilities.tools.makeindex.available),
    ]
    .into_iter()
    .filter(|(_, available)| *available)
    .map(|(name, _)| name)
    .collect::<Vec<_>>();
    if found.is_empty() {
        println!("rescanned: no native tools found");
    } else {
        println!("rescanned: found {}", found.join(", "));
    }
}
