//! `librepaper local <command>`: the command line front end for the loopback
//! service.
//!
//! `start` stays in the foreground for scripts; `launch` starts an
//! independent background process and waits for it to become ready.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use crate::cli::state_home;
use crate::cli::{LocalArgs, LocalCommand, LocalQuartoCommand, StartupCommand};
use crate::local::pairing::{generate_code, PairingStore, ServiceState};
use crate::local::protocol::{self, DEFAULT_PORT};
use crate::local::quarto::BindingStore;
use crate::local::service::{LocalService, NativeRunner, Runner};
use crate::util::die;

pub async fn run(args: LocalArgs) {
    match args.command {
        LocalCommand::Start {
            port,
            code,
            tex_path,
        } => start(port, true, code, tex_path).await,
        LocalCommand::Launch { port } => launch(port).await,
        LocalCommand::Manage => open("librepaper://manage").await,
        LocalCommand::Stop => stop().await,
        LocalCommand::Restart { port } => restart(port).await,
        LocalCommand::Open { url } => open(&url).await,
        LocalCommand::Startup { command } => startup(command),
        LocalCommand::Status => status().await,
        LocalCommand::Doctor { tex_path } => doctor(tex_path).await,
        LocalCommand::Disconnect { origin, all } => disconnect(origin, all),
        LocalCommand::Rescan { tex_path } => rescan(tex_path).await,
        LocalCommand::Quarto {
            command:
                LocalQuartoCommand::Bind {
                    origin,
                    project,
                    root,
                    main,
                },
        } => bind_quarto(&origin, &project, &root, &main),
        LocalCommand::Quarto {
            command: LocalQuartoCommand::Unbind { binding },
        } => unbind_quarto(&binding),
        LocalCommand::Quarto {
            command: LocalQuartoCommand::List { origin, project },
        } => list_quarto_bindings(&origin, &project),
    }
}

/// Grant the local app permission to execute one Quarto project. The caller
/// should print only the opaque binding id; `ProjectBinding::root` is local
/// state and must never be sent to a browser.
pub fn bind_quarto(origin: &str, project: &str, root: &str, main: &str) {
    let store = BindingStore::new(&state_home());
    match store.grant(origin, project, std::path::Path::new(root), main) {
        Ok(binding) => println!("quarto binding: {}", binding.id),
        Err(error) => die(error),
    }
}

pub fn unbind_quarto(binding: &str) {
    let store = BindingStore::new(&state_home());
    if store.revoke(binding) {
        println!("revoked quarto binding {binding}");
    } else {
        die("quarto binding not found");
    }
}

pub fn list_quarto_bindings(origin: &str, project: &str) {
    let store = BindingStore::new(&state_home());
    let bindings = store.list_scoped(origin, project);
    if bindings.is_empty() {
        println!("no quarto bindings");
        return;
    }
    for binding in bindings {
        println!(
            "{}\t{}\t{}",
            binding.id,
            binding.entrypoint,
            binding.root.display()
        );
    }
}

/// The cache-home base directory a job workspace lives under:
/// `<cache_home>/librepaper/local/jobs/<id>/`, never under a project directory
/// and never under the state home the tokens live in. Read here, not in
/// `service.rs`, so the service itself stays free of environment reads and
/// testable with an explicit directory -- the same reasoning
/// `crate::cli::state_home` gives for the token cache.
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

async fn start(port: u16, foreground: bool, code: Option<String>, tex_path: Vec<PathBuf>) {
    if !foreground {
        if let Err(error) =
            crate::local::lifecycle::spawn_background(port, code.as_deref(), &tex_path).await
        {
            die(error);
        }
        println!("librepaper local companion started in the background");
        return;
    }

    let state_home = state_home();
    let pairing = PairingStore::new(&state_home, code.clone());
    let port = if port == 0 { DEFAULT_PORT } else { port };

    let local_dir = state_home.join("librepaper/local");
    if let Err(error) = std::fs::create_dir_all(&local_dir) {
        die(error);
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(local_dir.join("companion.lock"))
        .unwrap_or_else(|error| die(error));
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        die("The companion is already running. Use `librepaper local launch` to reuse it.");
    }
    crate::local::lifecycle::clear_stop_request(&state_home);

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

    // Every document renders in a workspace of its own under the cache,
    // written from the files the browser sends with each job: nothing has
    // to be bound by hand. A `quarto bind` grant still wins for a project
    // that keeps data the document does not share.
    let workspaces = cache_home()
        .join("librepaper")
        .join("local")
        .join("workspaces");
    let runner: Arc<dyn Runner> = Arc::new(NativeRunner::with_hosted_workspaces(
        tex_path,
        &state_home,
        workspaces.clone(),
    ));
    let service = LocalService::with_hosted_workspaces_and_code(
        port,
        instance,
        &state_home,
        &cache_home(),
        runner,
        Some(code.clone()),
        workspaces,
    );
    let router = service.router();

    println!(
        "librepaper local listening on http://127.0.0.1:{port}{}",
        protocol::BASE_PATH
    );
    println!("pairing code: {code}");
    print_pairings(&pairing);
    println!("Ctrl-C to stop.");

    let v6_task = if let Some(v6) = listener_v6 {
        let make_v6 = router
            .clone()
            .into_make_service_with_connect_info::<SocketAddr>();
        Some(tokio::spawn(async move {
            let _ = axum::serve(v6, make_v6).await;
        }))
    } else {
        None
    };

    let make_v4 = router.into_make_service_with_connect_info::<SocketAddr>();
    let shutdown = async move {
        let stop_poll = async {
            let mut poll = tokio::time::interval(Duration::from_millis(250));
            loop {
                poll.tick().await;
                if crate::local::lifecycle::stop_requested(&state_home) {
                    break;
                }
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = stop_poll => {},
        }
    };
    if let Err(err) = axum::serve(listener, make_v4)
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("error: {err}");
    }

    if let Some(task) = v6_task {
        task.abort();
    }
    service.stop_previews().await;
    pairing.remove_service();
    println!("librepaper local stopped");
}

async fn stop() {
    if let Err(error) = crate::local::lifecycle::stop(&state_home()).await {
        die(error);
    }
    println!("librepaper local companion stopped");
}

async fn launch(port: u16) {
    match crate::local::lifecycle::spawn_background(port, None, &[]).await {
        Ok(state) => println!("librepaper local companion ready on port {}", state.port),
        Err(error) => die(error),
    }
}

async fn restart(port: u16) {
    let home = state_home();
    let previous = PairingStore::new(&home, None).read_service();
    let port = if port == 0 {
        previous.map_or(DEFAULT_PORT, |state| state.port)
    } else {
        port
    };
    if let Err(error) = crate::local::lifecycle::stop(&home).await {
        die(error);
    }
    launch(port).await;
}

async fn open(url: &str) {
    // Validate before causing even a launch side effect.
    if let Err(error) = crate::local::lifecycle::connection_target(url, DEFAULT_PORT) {
        die(error);
    }
    let state = crate::local::lifecycle::spawn_background(0, None, &[])
        .await
        .unwrap_or_else(|error| die(error));
    let target = crate::local::lifecycle::connection_target(url, state.port)
        .unwrap_or_else(|error| die(error));
    if !target.is_empty() {
        if let Err(error) = crate::local::lifecycle::open_browser(&target) {
            die(error);
        }
    }
}

fn startup(command: StartupCommand) {
    let enabled = matches!(command, StartupCommand::Enable);
    match crate::local::lifecycle::set_startup(enabled) {
        Ok(()) => println!("startup {}", if enabled { "enabled" } else { "disabled" }),
        Err(error) => die(error),
    }
}

async fn status() {
    let pairing = PairingStore::new(&state_home(), None);
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
    print_tool("calepin", &capabilities.calepin);
    print_tool("quarto", &capabilities.quarto.tool);
    if !capabilities.quarto.formats.is_empty() {
        println!(
            "  quarto formats: {}",
            capabilities.quarto.formats.join(", ")
        );
    }
    for (runtime, tool) in &capabilities.quarto.runtime_checks {
        print_tool(runtime, tool);
    }
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
        println!("  a native compile will run unconfined until this is addressed");
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
    let pairing = PairingStore::new(&state_home(), None);
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
    // `<state_home>/librepaper/local/tools.json`, which a running service
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
        ("quarto", capabilities.quarto.tool.available),
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
