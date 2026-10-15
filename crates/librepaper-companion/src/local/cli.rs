//! `librepaper local`: the commands the LibrePaper desktop app runs to start,
//! reach and stop the loopback service, and the agent commands.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::net::TcpListener;

use clap::{Args, Subcommand};

use crate::local::paths::state_home_or_die as state_home;
use crate::local::protocol::{self, DEFAULT_PORT};
use crate::local::service::{LocalService, NativeRunner, Runner};
use crate::local::store::{ServiceFile, ServiceState};
use librepaper_base::util::die;

/// Options for `local start`.
#[derive(Args, Clone, Debug, Default)]
pub struct LaunchArgs {
    /// Run in the foreground of this process instead of in the background
    #[arg(long)]
    pub foreground: bool,
    /// Port to listen on (default 8763)
    #[arg(
        long,
        value_name = "PORT",
        hide_default_value = true,
        env = "LIBREPAPER_LOCAL_PORT"
    )]
    pub port: Option<u16>,
}

/// `librepaper local <command>`. The desktop app runs these; nobody types
/// them. See `crate::local::cli`.
#[derive(Subcommand, Clone, Debug)]
pub enum LocalCommand {
    /// Start the service (in the background by default, or in this process with --foreground)
    Start(LaunchArgs),
    /// Ask a running service to stop cleanly; succeeds when none is running
    Stop,
    /// Whether the service is running, its address, and which native tools were found
    Status,
    /// Start the service if needed and print how to reach it as one JSON
    /// line: address, credential, instance, version and protocol.
    Endpoint {
        /// Port to start on when the service is not running (default 8763)
        #[arg(long, value_name = "PORT", env = "LIBREPAPER_LOCAL_PORT")]
        port: Option<u16>,
    },
}

/// Declaring an ACP agent this machine offers. It is a command, not a
/// loopback route: a page picks which agent to drive, never what command to
/// run.
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
            if args.foreground {
                start_foreground(port).await
            } else {
                start_background(port).await
            }
        }
        LocalCommand::Stop => stop().await,
        LocalCommand::Status => status().await,
        LocalCommand::Endpoint { port } => endpoint(port.unwrap_or(0)).await,
    }
}

/// The cache-home base directory a job workspace lives under:
/// `<cache_home>/librepaper/local/jobs/<id>/`, never under a project directory
/// and never under the state home the tokens live in. Read here, not in
/// `service.rs`, so the service itself stays free of environment reads and
/// testable with an explicit directory -- the same reasoning
/// `crate::local::paths::state_home_or_die` gives for the token cache.
fn cache_home() -> PathBuf {
    crate::local::paths::cache_home()
        .unwrap_or_else(|| die("no home directory to store the job cache in"))
}

async fn start_background(port: u16) {
    match crate::local::lifecycle::spawn_background(port).await {
        Ok(state) => println!(
            "Local service ready on port {}.",
            state.port
        ),
        Err(error) => die(error),
    }
}

async fn start_foreground(port: u16) {
    let state_home = state_home();
    let service_file = ServiceFile::new(&state_home);
    let port = if port == 0 { DEFAULT_PORT } else { port };

    let local_dir = state_home.join("librepaper/local");
    if let Err(error) = std::fs::create_dir_all(&local_dir) {
        die(error);
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(crate::local::lifecycle::lock_path(&state_home))
        .unwrap_or_else(|error| die(error));
    if lock.try_lock().is_err() {
        die("The local service is already running.");
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

    let instance = hex::encode(librepaper_base::util::random_bytes(8));
    let state = ServiceState {
        port,
        instance: instance.clone(),
        pid: std::process::id(),
        started: librepaper_base::util::now_unix(),
    };
    if let Err(err) = service_file.write(&state) {
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
        &state_home,
        workspaces.clone(),
    ));
    // The credential is written to a private file before the first request
    // is served, so `endpoint` can read it as soon as `health` answers.
    let service = LocalService::with_hosted_workspaces(
        port,
        instance,
        &state_home,
        &cache_home(),
        runner,
        workspaces,
    )
    .unwrap_or_else(|error| die(error));
    let router = service.router();

    println!(
        "Local service ready on http://127.0.0.1:{port}{}",
        protocol::BASE_PATH
    );
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
        tracing::error!("{err}");
    }

    if let Some(task) = v6_task {
        task.abort();
    }
    service.stop_previews().await;
    service_file.remove();
    println!("Local service stopped");
}

async fn stop() {
    if let Err(error) = crate::local::lifecycle::stop(&state_home()).await {
        die(error);
    }
    println!("Local service stopped");
}

/// Starts the service if it is not running, then prints one JSON line the
/// desktop app reads: where to reach it and the credential that opens it.
/// Exits non-zero with a message when the service cannot be reached.
async fn endpoint(port: u16) {
    let state = crate::local::lifecycle::spawn_background(port)
        .await
        .unwrap_or_else(|error| die(error));
    let Some(token) =
        crate::local::service::control_token_for_instance(&state_home(), &state.instance)
    else {
        die("Local control credential is unavailable");
    };
    println!("{}", endpoint_line(&state, &token));
}

fn endpoint_line(state: &ServiceState, token: &str) -> String {
    json!({
        "address": format!("http://127.0.0.1:{}/", state.port),
        "token": token,
        "instance": state.instance,
        "version": crate::VERSION,
        "protocol": protocol::PROTOCOL_VERSIONS,
    })
    .to_string()
}

/// Exits non-zero when nothing is answering, so a script can branch on it:
/// `make demo` uses it to decide whether to start a service or leave the
/// one already running alone. Reports the service address, then the native
/// tools found and missing.
async fn status() {
    let home = state_home();
    let mut answering = true;

    match ServiceFile::new(&home).read() {
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
        }
        None => {
            println!("Local service is not running");
            answering = false;
        }
    }

    println!();
    let capabilities = crate::local::discovery::discover(true).await;
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
        println!("  local builds run with the user's normal access");
    }

    // A stale `service.json` describes a service that is gone. Reporting
    // that as success would have a script skip starting one.
    if !answering {
        std::process::exit(1);
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

/// Teach this computer an ACP agent, so the sidebar can drive an agent that
/// is not in the built-in table. This is how opencode, Pi, or something that
/// does not exist yet becomes a sidebar assistant without LibrePaper
/// guessing at a package name that may never have existed.
pub fn agent(command: LocalAgentCommand) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_line_is_one_json_object_with_the_connection_details() {
        let state = ServiceState {
            port: 8763,
            instance: "abc123".into(),
            pid: 1,
            started: 0,
        };
        let line = endpoint_line(&state, "secret-token");
        assert!(!line.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["address"], "http://127.0.0.1:8763/");
        assert_eq!(value["token"], "secret-token");
        assert_eq!(value["instance"], "abc123");
        assert_eq!(value["version"], crate::VERSION);
        assert_eq!(value["protocol"], json!(protocol::PROTOCOL_VERSIONS));
    }
}
