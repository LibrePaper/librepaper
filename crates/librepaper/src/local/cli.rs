//! `librepaper start` and `local` compatibility commands for the loopback service.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::net::TcpListener;

use clap::{Args, Subcommand};

use crate::local::pairing::{PairingStore, ServiceState};
use crate::local::paths::state_home_or_die as state_home;
use crate::local::protocol::{self, DEFAULT_PORT};
use crate::local::service::{LocalService, NativeRunner, Runner};
use crate::util::die;

/// Shared options for the default launch and explicit `start` aliases.
#[derive(Args, Clone, Debug, Default)]
pub struct LaunchArgs {
    /// Run in the foreground of this process instead of in the background
    #[arg(long, help_heading = "Companion")]
    pub foreground: bool,
    /// Also start the companion every time you log in
    #[arg(long, help_heading = "Companion")]
    pub at_login: bool,
    /// Port to listen on (default 8763)
    #[arg(
        long,
        value_name = "PORT",
        hide_default_value = true,
        env = "LIBREPAPER_LOCAL_PORT",
        help_heading = "Companion"
    )]
    pub port: Option<u16>,
    /// Extra directories searched before PATH for typst, pandoc and calepin
    #[arg(
        long,
        value_name = "DIRS",
        env = "LIBREPAPER_TOOL_PATH",
        value_delimiter = ':',
        help_heading = "Companion"
    )]
    pub tool_path: Vec<PathBuf>,
}

/// `librepaper local <command>` compatibility commands. Keep these aliases
/// until a removal cutoff is announced for supported external CLI clients.
/// See `crate::local::cli`.
#[derive(Subcommand, Clone, Debug)]
pub enum LocalCommand {
    /// Start the companion (in the background by default, or in this process with --foreground)
    #[command(hide = true)]
    Start(LaunchArgs),
    /// Ask a running companion to stop cleanly.
    #[command(hide = true)]
    Stop,
    /// Launch the companion and open a validated local connection link.
    /// The operating system runs this for `librepaper://` links; it is not
    /// listed because nobody types it.
    #[command(hide = true)]
    Open { url: String },
    /// Whether the service is running, its address, code and pairings, and which native tools were found
    #[command(hide = true)]
    Status {
        /// Extra directories searched before PATH for typst, pandoc and calepin, colon-separated
        #[arg(
            long,
            value_name = "DIRS",
            env = "LIBREPAPER_TOOL_PATH",
            value_delimiter = ':'
        )]
        tool_path: Vec<PathBuf>,
    },
    /// Approve a dialog request from the companion
    Approve {
        /// Approval code from the dialog
        code: String,
    },
    /// Revoke a site pairing
    Disconnect {
        /// Origin (e.g. https://papers.example)
        origin: String,
    },
    /// Teach this computer an ACP agent the sidebar can drive
    #[command(hide = true)]
    Agent {
        #[command(subcommand)]
        command: LocalAgentCommand,
    },
}

/// Declaring an ACP agent this machine offers. It lives here, as a local
/// command, rather than as a loopback route: a paired page picks which agent
/// to drive, never what command to run.
#[derive(Subcommand, Clone, Debug)]
pub enum LocalAgentCommand {
    /// Declare an agent. Everything after `--` is the command that speaks the
    /// Agent Client Protocol on stdio, for example:
    /// `librepaper agent add opencode --label opencode -- opencode acp`
    Add {
        /// Short lower-case id, shown in the sidebar's agent list.
        id: String,
        #[arg(long, value_name = "NAME", default_value = "")]
        label: String,
        #[arg(last = true, required = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// Which agents this computer has been taught
    List,
    Remove {
        id: String,
    },
}

/// The arguments `librepaper local` hands to `crate::local::run`.
#[derive(Clone, Debug)]
pub struct LocalArgs {
    pub command: LocalCommand,
}

pub async fn run(args: LocalArgs) {
    match args.command {
        LocalCommand::Start(args) => {
            let port = args.port.unwrap_or(0);
            if args.at_login {
                if let Err(error) = crate::local::lifecycle::set_startup(true) {
                    die(error);
                }
            }
            if args.foreground {
                start_foreground(port, args.tool_path).await
            } else {
                start_background(port, &args.tool_path).await
            }
        }
        LocalCommand::Stop => stop().await,
        LocalCommand::Open { url } => open(&url).await,
        LocalCommand::Status { tool_path } => status(tool_path).await,
        LocalCommand::Approve { code } => approve(&code).await,
        LocalCommand::Disconnect { origin } => disconnect(origin),
        LocalCommand::Agent { command } => local_agent(command),
    }
}

/// The cache-home base directory a job workspace lives under:
/// `<cache_home>/librepaper/local/jobs/<id>/`, never under a project directory
/// and never under the state home the tokens live in. Read here, not in
/// `service.rs`, so the service itself stays free of environment reads and
/// testable with an explicit directory -- the same reasoning
/// `crate::local::paths::state_home_or_die` gives for the token cache.
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

async fn start_background(port: u16, tool_path: &[PathBuf]) {
    match crate::local::lifecycle::spawn_background(port, tool_path).await {
        Ok(state) => println!(
            "Companion ready on port {}. Next: Settings → Local app.",
            state.port
        ),
        Err(error) => die(error),
    }
}

async fn start_foreground(port: u16, tool_path: Vec<PathBuf>) {
    let state_home = state_home();
    let pairing = PairingStore::new(&state_home, None);
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
        die("The companion is already running. Use `librepaper start` to reuse it.");
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

    let instance = hex::encode(crate::util::random_bytes(8));
    let state = ServiceState {
        port,
        instance: instance.clone(),
        pid: std::process::id(),
        started: crate::util::now_unix(),
    };
    if let Err(err) = pairing.write_service(&state) {
        die(format!("could not write service.json: {err}"));
    }

    // Every document renders in a workspace of its own under the cache,
    // written from the files the browser sends with each job: nothing has
    // to be bound by hand. A project folder chosen in the browser's local
    // app settings still wins for a project that keeps data the document
    // does not share.
    let workspaces = cache_home()
        .join("librepaper")
        .join("local")
        .join("workspaces");
    let runner: Arc<dyn Runner> = Arc::new(NativeRunner::with_hosted_workspaces(
        tool_path,
        &state_home,
        workspaces.clone(),
    ));
    let service = LocalService::with_hosted_workspaces_and_code(
        port,
        instance,
        &state_home,
        &cache_home(),
        runner,
        None,
        workspaces,
    );
    let router = service.router();

    println!(
        "Companion ready on http://127.0.0.1:{port}{}",
        protocol::BASE_PATH
    );
    println!("Next: Settings → Local app.");
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
    println!("Companion stopped");
}

async fn stop() {
    if let Err(error) = crate::local::lifecycle::stop(&state_home()).await {
        die(error);
    }
    println!("Companion stopped");
}

async fn open(url: &str) {
    // Validate before causing even a launch side effect.
    if let Err(error) = crate::local::lifecycle::connection_target(url, DEFAULT_PORT) {
        die(error);
    }
    let state = crate::local::lifecycle::spawn_background(0, &[])
        .await
        .unwrap_or_else(|error| die(error));
    let target = crate::local::lifecycle::connection_target(url, state.port)
        .unwrap_or_else(|error| die(error));
    let result = match target {
        crate::local::lifecycle::Target::Nothing => Ok(()),
        crate::local::lifecycle::Target::Open(target) => {
            crate::local::lifecycle::open_browser(&target)
        }
        crate::local::lifecycle::Target::Pair(link) => {
            crate::local::lifecycle::pair(&link, state.port).await
        }
    };
    if let Err(error) = result {
        die(error);
    }
}

/// Exits non-zero when nothing is answering, so a script can branch on it:
/// `make demo` uses it to decide whether to start a companion or leave the
/// one already running alone. Reports the service address, code and pairings,
/// then the native tools found and missing.
async fn status(tool_path: Vec<PathBuf>) {
    let home = state_home();
    let pairing = PairingStore::new(&home, None);
    let mut answering = true;

    match pairing.read_service() {
        Some(state) => {
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
                Ok(response) => {
                    println!(
                        "listening at {url} but answered unexpectedly: {}",
                        response.status()
                    );
                    answering = false;
                }
                Err(_) => {
                    println!(
                        "not reachable at {url}; service.json names pid {} but nothing answered",
                        state.pid
                    );
                    answering = false;
                }
            }
            print_pairings(&pairing);
        }
        None => {
            println!("Companion is not running");
            answering = false;
        }
    }

    println!();
    let capabilities = crate::local::discovery::discover(true, &tool_path).await;
    println!("platform: {}", capabilities.platform);
    println!("tools:");
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
        println!("  paired local builds run with the user's normal access");
    }

    // A stale `service.json` describes a companion that is gone. Reporting
    // that as success would have a script skip starting one.
    if !answering {
        std::process::exit(1);
    }
}

fn print_pairings(pairing: &PairingStore) {
    let pairings = pairing.active_pairings();
    if pairings.is_empty() {
        println!("no paired origins");
        return;
    }
    println!("paired origins:");
    for origin in pairings {
        println!("  {origin}");
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

async fn approve(code: &str) {
    let home = state_home();
    let pairing = PairingStore::new(&home, None);

    match pairing.read_service() {
        Some(state) => {
            let url = format!(
                "http://127.0.0.1:{}{}/approve",
                state.port,
                protocol::BASE_PATH
            );
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build();

            let result = match client {
                Ok(client) => {
                    client
                        .post(&url)
                        .header("content-type", "application/json")
                        .body(json!({"code": code}).to_string())
                        .send()
                        .await
                }
                Err(err) => Err(err),
            };

            match result {
                Ok(response) if response.status().is_success() => {
                    println!("Approval code accepted");
                }
                Ok(response) if response.status().as_u16() == 404 => {
                    die("Code not recognized");
                }
                _ => {
                    die("Could not reach the companion");
                }
            }
        }
        None => {
            die("Companion is not running");
        }
    }
}

fn disconnect(origin: String) {
    let pairing = PairingStore::new(&state_home(), None);
    if pairing.revoke(&origin) {
        println!("Disconnected {origin}");
    } else {
        println!("{origin} was not connected");
    }
}

/// Teach this computer an ACP agent, so the sidebar can drive an agent that
/// is not in the built-in table. This is how opencode, Pi, or something that
/// does not exist yet becomes a sidebar assistant without LibrePaper
/// guessing at a package name that may never have existed.
fn local_agent(command: LocalAgentCommand) {
    use LocalAgentCommand;
    let state_home = crate::local::paths::state_home_or_die();
    let store = crate::local::acp_agents::CustomStore::new(&state_home);
    match command {
        LocalAgentCommand::Add {
            id,
            label,
            command: acp,
        } => match store.add(&id, &label, &acp) {
            Ok(()) => println!("{id} will be offered in the document sidebar"),
            Err(error) => die(error),
        },
        LocalAgentCommand::List => {
            let all = store.list();
            if all.is_empty() {
                println!("No agents added on this computer. Built-in agents are detected on PATH.");
                return;
            }
            for (id, custom) in all {
                println!("{id}\t{}\t{}", custom.label, custom.command.join(" "));
            }
        }
        LocalAgentCommand::Remove { id } => {
            if store.remove(&id) {
                println!("removed {id}");
            } else {
                die(format!("no agent named '{id}'"));
            }
        }
    }
}
