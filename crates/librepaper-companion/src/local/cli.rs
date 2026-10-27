//! `librepaper local`: the commands the LibrePaper desktop app runs to start,
//! reach and stop the loopback service.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::net::TcpListener;

use clap::{Args, Subcommand};

use crate::local::paths::state_home_or_die as state_home;
use crate::local::protocol;
use crate::local::service::{LocalService, NativeRunner, Runner};
use crate::local::store::{ServiceFile, ServiceState};
use librepaper_base::util::die;

/// Options for `local start`.
#[derive(Args, Clone, Debug, Default)]
pub struct LaunchArgs {
    /// Run in the foreground of this process instead of in the background
    #[arg(long)]
    pub foreground: bool,
    /// Port to listen on (default: a free port the system picks)
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
        /// Port to start on when the service is not running (default: a free port the system picks)
        #[arg(long, value_name = "PORT", env = "LIBREPAPER_LOCAL_PORT")]
        port: Option<u16>,
    },
    /// Print a plain-text report for a bug report: version, system, PATH,
    /// the service, the tools found and the end of the log. With --log-dir,
    /// print only the folder that holds the log.
    Diagnostics {
        #[arg(long)]
        log_dir: bool,
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
        LocalCommand::Diagnostics { log_dir } => diagnostics(log_dir).await,
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
        Ok((state, _)) => println!("Local service ready on port {}.", state.port),
        Err(error) => die(error),
    }
}

async fn start_foreground(port: u16) {
    let state_home = state_home();
    let service_file = ServiceFile::new(&state_home);

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
    // Port 0 lets the system pick a free port, so two users on one machine
    // do not collide. Everything after this uses the port it picked.
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(err) => die(format!("could not read the listening port: {err}")),
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
    // to be bound by hand.
    let workspaces = cache_home()
        .join("librepaper")
        .join("local")
        .join("workspaces");
    let runner: Arc<dyn Runner> =
        Arc::new(NativeRunner::with_hosted_workspaces(workspaces.clone()));
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
    // The stop request is a file because `librepaper local stop` is another
    // process. Reading it is the one place that file is polled; it cancels
    // the token the rest of the service waits on.
    let mut tasks = librepaper_base::tasks::Tasks::new();
    let stopping = tasks.shutdown();
    let requested = stopping.clone();
    let watched_home = state_home.clone();
    tasks.every(
        "stop-request",
        Duration::from_millis(250),
        tokio::time::MissedTickBehavior::Delay,
        move || {
            let requested = requested.clone();
            let home = watched_home.clone();
            async move {
                if crate::local::lifecycle::stop_requested(&home) {
                    requested.cancel();
                }
            }
        },
    );
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = stopping.cancelled() => {},
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
    for name in tasks.stop(Duration::from_secs(5)).await {
        tracing::warn!("background task {name} did not stop in time");
    }
    for name in service.stop(Duration::from_secs(5)).await {
        tracing::warn!("background task {name} did not stop in time");
    }
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
    let (state, health) = crate::local::lifecycle::spawn_background(port)
        .await
        .unwrap_or_else(|error| die(error));
    let Some(token) =
        crate::local::service::control_token_for_instance(&state_home(), &state.instance)
    else {
        die("Local control credential is unavailable");
    };
    println!("{}", endpoint_line(&state, &health, &token));
}

/// Reports the running service's own version and protocol, which may differ
/// from this command's when an older service was left running.
fn endpoint_line(state: &ServiceState, health: &protocol::Health, token: &str) -> String {
    json!({
        "address": format!("http://127.0.0.1:{}/", state.port),
        "token": token,
        "instance": state.instance,
        "version": health.version,
        "protocol": health.protocol,
    })
    .to_string()
}

/// Exits non-zero when nothing is answering, so a script can branch on it:
/// `make demo` uses it to decide whether to start a service or leave the
/// one already running alone. Reports the service address, then the native
/// tools found and missing.
async fn status() {
    // A stale `service.json` describes a service that is gone. Reporting
    // that as success would have a script skip starting one.
    if !report_status().await {
        std::process::exit(1);
    }
}

/// Prints the service address and the tools, and returns whether the service
/// answered. Never exits, so `diagnostics` can print it on its own.
async fn report_status() -> bool {
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
    let capabilities = crate::local::render::capabilities(true).await;
    println!("platform: {}", capabilities.platform);
    println!("renderers:");
    for (key, renderer) in &capabilities.renderers {
        let program = renderer.command.first().map_or("", String::as_str);
        let state = match (&renderer.version, renderer.found) {
            (Some(version), _) => version.clone(),
            (None, true) => "found".to_string(),
            (None, false) => "not found".to_string(),
        };
        let edited = if renderer.edited { " (edited)" } else { "" };
        println!("  {key}: {program}: {state}{edited}");
    }
    println!();
    println!("local renders run with the user's normal access");

    answering
}

/// The report a bug report needs. Unlike `status`, it never exits: it prints
/// the same service and tool lines whether or not the service runs.
async fn diagnostics(log_dir_only: bool) {
    let dir = crate::local::lifecycle::log_dir(&state_home());
    if log_dir_only {
        println!("{}", dir.display());
        return;
    }
    println!("LibrePaper command line {}", crate::VERSION);
    println!(
        "System: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("Log folder: {}", dir.display());
    println!("PATH:");
    for entry in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        println!("  {}", entry.display());
    }
    println!();
    report_status().await;
    println!();
    println!("Recent log (companion.log, last 200 lines):");
    match std::fs::read(dir.join("companion.log")) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            for line in &lines[lines.len().saturating_sub(200)..] {
                println!("{line}");
            }
        }
        Err(_) => println!("(no log yet)"),
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
        let health = protocol::Health {
            service: "librepaper-local".into(),
            protocol: vec![7],
            version: "9.9.9-test".into(),
            instance: "abc123".into(),
        };
        let line = endpoint_line(&state, &health, "secret-token");
        assert!(!line.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["address"], "http://127.0.0.1:8763/");
        assert_eq!(value["token"], "secret-token");
        assert_eq!(value["instance"], "abc123");
        assert_eq!(value["version"], "9.9.9-test");
        assert_eq!(value["protocol"], json!([7]));
    }

    #[tokio::test]
    async fn two_services_on_one_machine_get_different_free_ports() {
        let first = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let second = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let first_port = first.local_addr().unwrap().port();
        let second_port = second.local_addr().unwrap().port();
        assert_ne!(first_port, 0);
        assert_ne!(second_port, 0);
        assert_ne!(first_port, second_port);
    }
}
