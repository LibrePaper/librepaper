//! The command line: what `librepaper` accepts, and `main`, which runs it.
//! Every command in the modules below talks to a deployment over HTTP, the
//! way a browser does; deployment administration and `local` are the
//! exceptions and live in their own modules.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use librepaper_base::config::Configuration;
use librepaper_base::http::{detail_of, get_as, get_with_token, post_json, text, Credentials};
use librepaper_base::util::die;
use librepaper_companion::local::cli::{LaunchArgs, LocalAgentCommand, LocalArgs, LocalCommand};

mod agent;

mod documents;
pub mod export;
mod history;
pub(crate) mod server_config;
mod tokens;

pub use documents::*;
pub use tokens::*;

#[derive(Parser)]
#[command(name = "librepaper", version = crate::VERSION, about = "launch the companion to connect local tools to documents; host HTML, markdown and typst documents", long_about = None, args_conflicts_with_subcommands = true)]
#[command(
    after_help = "Run `librepaper` or `librepaper start` to launch the companion, then connect it in Settings → Companion.
On supported desktops, the companion also shows a tray icon. Choose Settings in its menu to open the Companion section in the main LibrePaper app. Run `librepaper install-desktop` to install a desktop launcher that starts the companion.

To sign in from this terminal, set the deployment and run `librepaper login`:

    export LIBREPAPER_SERVER=https://librepaper.example.org
    librepaper login"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Companion launch options. With no command, `librepaper` launches it.
    #[command(flatten)]
    launch: LaunchArgs,
}

/// Deployment credentials: server URL and optional authentication token.
#[derive(Args, Clone, Debug)]
pub(crate) struct Deployment {
    /// Deployment to talk to
    #[arg(
        long,
        env = "LIBREPAPER_SERVER",
        value_name = "URL",
        help_heading = "Deployment"
    )]
    pub(crate) server: Option<String>,
    /// A credential to use instead of the one `login` stored
    #[arg(
        long,
        env = "LIBREPAPER_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true,
        help_heading = "Deployment"
    )]
    pub(crate) token: Option<String>,
}

// Clap parses commands once, so an extra indirection would not improve runtime.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Start the companion (in the background by default)
    Start(LaunchArgs),
    /// Ask the companion to stop cleanly
    Stop,
    /// Show companion status and available tools
    Status {
        #[arg(
            long,
            value_name = "DIRS",
            env = "LIBREPAPER_TOOL_PATH",
            value_delimiter = ':'
        )]
        tool_path: Vec<PathBuf>,
    },
    /// Install a native launcher for starting the companion.
    InstallDesktop,
    /// Manage agents this computer offers to the document sidebar
    Agent {
        #[command(subcommand)]
        command: LocalAgentCommand,
    },
    /// Sign in through a deployment, in a browser
    Login {
        #[command(flatten)]
        deployment: Deployment,
    },
    /// Forget the stored sign-in
    Logout,
    /// Deployment administration and operator commands.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// List your documents
    List {
        #[command(flatten)]
        deployment: Deployment,
    },
    /// Export a complete independent copy of the project
    Export {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// A new directory to create
        dir: String,
        /// Export the project as it stood at this label instead of its live
        /// state; requesting a historical export waits for the server to build
        /// it (SPEC-server-is-a-log §8.5)
        #[arg(long, value_name = "LABEL")]
        at: Option<String>,
        /// A share link, or the key from one: read as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
        #[command(flatten)]
        deployment: Deployment,
    },
    // The one command the sidebar assistant's own agent spawns. Not listed,
    // because nobody types it; see `agent`.
    #[command(hide = true)]
    Mcp(agent::McpArgs),
    /// Approve requests and disconnect paired websites
    Local {
        #[command(subcommand)]
        command: LocalCommand,
    },
}

/// Commands used to operate a deployment rather than work with documents.
// `Serve` is a deployment's whole configuration and is much the largest
// variant. One is parsed once on the way into `main`; clap cannot flatten its
// flag groups through a box.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub(crate) enum AdminCommand {
    /// Run the service on this machine
    Serve {
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
    },
    /// Inspect or validate the effective deployment configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Create a snapshot-consistent PostgreSQL and immutable-object recovery point.
    Backup {
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
        /// New directory to create
        directory: String,
        /// Identifier to record for this backup; generated when omitted
        #[arg(long, value_name = "ID")]
        id: Option<String>,
    },
    /// Restore a verified backup into a new deployment directory.
    Restore {
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
        /// Backup directory to read
        backup: String,
        /// New deployment directory to create
        directory: String,
    },
    /// Block accounts or hide and restore projects. This command connects
    /// directly to the deployment database and requires database access.
    Moderate {
        #[command(subcommand)]
        command: ModerationCommand,
    },
    /// Delete objects no catalogue row names any more.
    ///
    /// Every blob a deployment keeps on purpose is named by an asset, a
    /// compaction base or a label archive (§8.5); anything else under the
    /// object store's prefixes is left over from a crash between writing
    /// bytes and committing the row that would have named them. Reclaiming
    /// it means listing the store. The service reclaims old orphan objects
    /// in bounded periodic sweeps; this command lets an operator request
    /// cleanup directly.
    #[command(hide = true)]
    Sweep {
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
        /// How many objects to examine per prefix. The sweep remembers where
        /// it stopped, so running it again continues from there.
        #[arg(
            long,
            default_value_t = 500,
            value_parser = clap::value_parser!(u32).range(1..=1000),
            value_name = "COUNT"
        )]
        batch: u32,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Validate the configuration without opening storage or binding ports.
    Check {
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
    },
    /// Print effective configuration with secrets redacted.
    Show {
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
    },
}

const DEFAULT_SERVER_CONFIG: &str = "/etc/librepaper/config.toml";

#[derive(Subcommand, Clone, Debug)]
pub(crate) enum ModerationCommand {
    /// Revoke an account's sessions and deny future access and writes.
    BlockAccount {
        /// Account UUID or unique provider handle.
        account: String,
        #[arg(long, value_name = "ACTOR", required = true)]
        actor: String,
        #[arg(long, value_name = "REASON", required = true)]
        reason: String,
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
    },
    /// Restore an account blocked by moderation.
    UnblockAccount {
        /// Account UUID or unique provider handle.
        account: String,
        #[arg(long, value_name = "ACTOR", required = true)]
        actor: String,
        #[arg(long, value_name = "REASON", required = true)]
        reason: String,
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
    },
    /// Hide a project from every document and asset route without deleting it.
    HideProject {
        /// Project slug.
        project: String,
        #[arg(long, value_name = "ACTOR", required = true)]
        actor: String,
        #[arg(long, value_name = "REASON", required = true)]
        reason: String,
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
    },
    /// Restore public access to a project hidden by moderation.
    UnhideProject {
        /// Project slug.
        project: String,
        #[arg(long, value_name = "ACTOR", required = true)]
        actor: String,
        #[arg(long, value_name = "REASON", required = true)]
        reason: String,
        #[arg(long, value_name = "PATH", default_value = DEFAULT_SERVER_CONFIG)]
        config: PathBuf,
    },
}

/// Installs the one process-wide subscriber. Companion commands accept `RUST_LOG`;
/// deployment administration keeps its logging filter independent of the environment.
fn install_subscriber(server_filter: Option<&str>) {
    use std::io::IsTerminal;
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(if let Some(filter) = server_filter {
            EnvFilter::try_new(filter).unwrap_or_else(|error| {
                die(format!("logging.filter is invalid: {error}"));
            })
        } else {
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
        })
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}

#[tokio::main]
pub async fn main() {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Start(cli.launch));
    if !matches!(&command, Command::Admin { .. }) {
        install_subscriber(None);
    }
    match command {
        Command::Start(args) => {
            librepaper_companion::local::cli::run(LocalArgs {
                command: LocalCommand::Start(args),
            })
            .await
        }
        Command::Stop => {
            librepaper_companion::local::cli::run(LocalArgs {
                command: LocalCommand::Stop,
            })
            .await
        }
        Command::Status { tool_path } => {
            librepaper_companion::local::cli::run(LocalArgs {
                command: LocalCommand::Status { tool_path },
            })
            .await
        }
        Command::InstallDesktop => {
            match librepaper_companion::local::lifecycle::install_desktop_shortcut() {
                Ok(path) => println!("LibrePaper launcher installed at {}", path.display()),
                Err(error) => die(error),
            }
        }
        Command::Agent { command } => {
            librepaper_companion::local::cli::run(LocalArgs {
                command: LocalCommand::Agent { command },
            })
            .await
        }
        Command::Login { deployment } => login(deployment.server).await,
        Command::Logout => logout(),
        Command::Admin { command } => run_admin(command).await,
        Command::List { deployment } => list_documents(deployment.server, deployment.token).await,
        Command::Export {
            id,
            dir,
            at,
            key,
            deployment,
        } => {
            crate::cli::export::export_project(
                &id,
                deployment.server,
                deployment.token,
                &dir,
                key.unwrap_or_default(),
                at.unwrap_or_default(),
            )
            .await
        }
        Command::Mcp(args) => {
            if let Err(err) = agent::run_mcp(args).await {
                die(err);
            }
        }
        Command::Local { command } => {
            librepaper_companion::local::cli::run(LocalArgs { command }).await
        }
    }
}

async fn run_admin(command: AdminCommand) {
    match command {
        AdminCommand::Serve { config } => {
            let resolved = server_config::load(&config).unwrap_or_else(|error| die(error));
            install_subscriber(Some(&resolved.log_filter));
            librepaper_server::server::serve::serve(serve_options(resolved)).await
        }
        AdminCommand::Config { command } => match command {
            ConfigCommand::Check { config } => {
                let resolved = server_config::load(&config).unwrap_or_else(|error| die(error));
                install_subscriber(Some(&resolved.log_filter));
                let options = serve_options(resolved);
                if let Err(error) = librepaper_server::server::serve::validate_serve_options(&options) {
                    die(error);
                }
                println!("configuration is valid");
            }
            ConfigCommand::Show { config } => {
                let resolved = server_config::load(&config).unwrap_or_else(|error| die(error));
                install_subscriber(Some(&resolved.log_filter));
                let rendered = server_config::show_resolved(&resolved);
                print!("{rendered}");
            }
        },
        AdminCommand::Backup {
            config,
            directory,
            id,
        } => {
            let (storage, filter) = server_config::load_storage_with_log_filter(&config)
                .unwrap_or_else(|error| die(error));
            install_subscriber(Some(&filter));
            librepaper_engine::storage::backup::backup_cli(
                storage,
                directory,
                id.unwrap_or_default(),
            )
            .await
        }
        AdminCommand::Restore {
            config,
            backup,
            directory,
        } => {
            let (storage, filter) = server_config::load_storage_with_log_filter(&config)
                .unwrap_or_else(|error| die(error));
            install_subscriber(Some(&filter));
            librepaper_engine::storage::backup::restore_cli(storage, backup, directory).await
        }
        AdminCommand::Moderate { command } => moderate(command).await,
        AdminCommand::Sweep { config, batch } => {
            let (storage, filter) = server_config::load_storage_with_log_filter(&config)
                .unwrap_or_else(|error| die(error));
            install_subscriber(Some(&filter));
            sweep(storage, batch as usize).await
        }
    }
}

fn serve_options(
    resolved: server_config::ResolvedConfig,
) -> librepaper_server::server::serve::ServeOptions {
    let start_local = resolved.local_companion.then(|| {
        Box::new(|base| {
            Box::pin(async move {
                let app = librepaper_companion::local::embedded::start(&base, Vec::new()).await?;
                let stopper = app.clone();
                Ok(librepaper_server::server::serve::LocalApp {
                    address: app.address.clone(),
                    stop: Box::new(move || Box::pin(async move { stopper.stop().await })),
                })
            })
        }) as librepaper_server::server::serve::StartLocal
    });
    librepaper_server::server::serve::ServeOptions {
        bind: resolved.bind,
        port: resolved.port,
        storage: resolved.storage,
        github_client_id: resolved.github_client_id,
        github_client_secret: resolved.github_client_secret,
        google_client_id: resolved.google_client_id,
        google_client_secret: resolved.google_client_secret,
        publishers: Some(resolved.publishers.join(",")),
        commenters: Some(resolved.commenters.join(",")),
        simulate_activity: resolved.simulate_activity,
        origin: resolved.origin,
        docs_origin: resolved.docs_origin,
        site_origin: resolved.site_origin,
        expire_after: resolved.expire_after,
        expire_from: resolved.expire_from,
        asset_mirror: resolved.asset_mirror,
        typst_fonts: resolved.typst_fonts,
        start_local,
        load_shell: librepaper_shell::load_shell,
        latex_release: librepaper_shell::latex_release(),
        config: resolved.config,
        metrics_address: resolved.metrics_address,
    }
}

async fn moderate(command: ModerationCommand) {
    use librepaper_engine::storage::postgres::{
        ModerationAction, PostgresCatalog, PostgresOptions,
    };
    let (config, action, target, actor, reason) = match command {
        ModerationCommand::BlockAccount {
            account,
            actor,
            reason,
            config,
        } => (
            config,
            ModerationAction::BlockAccount,
            account,
            actor,
            reason,
        ),
        ModerationCommand::UnblockAccount {
            account,
            actor,
            reason,
            config,
        } => (
            config,
            ModerationAction::UnblockAccount,
            account,
            actor,
            reason,
        ),
        ModerationCommand::HideProject {
            project,
            actor,
            reason,
            config,
        } => (
            config,
            ModerationAction::HideProject,
            project,
            actor,
            reason,
        ),
        ModerationCommand::UnhideProject {
            project,
            actor,
            reason,
            config,
        } => (
            config,
            ModerationAction::UnhideProject,
            project,
            actor,
            reason,
        ),
    };
    let (storage, filter) = server_config::load_storage_with_log_filter(&config)
        .unwrap_or_else(|error| die(error));
    install_subscriber(Some(&filter));
    let mut postgres = PostgresOptions::new(storage.database_url);
    postgres.max_connections = storage.database_connections;
    let catalog = match PostgresCatalog::connect(postgres).await {
        Ok(catalog) => catalog,
        Err(error) => die(error.to_string()),
    };
    if let Err(error) = catalog.migrate().await {
        die(error.to_string());
    }
    let outcome = match action {
        ModerationAction::BlockAccount | ModerationAction::UnblockAccount => {
            let blocked = matches!(action, ModerationAction::BlockAccount);
            catalog
                .moderate_account(&target, blocked, &actor, &reason)
                .await
                .map(|(id, label)| {
                    format!(
                        "{} account {label} ({id})",
                        if blocked { "blocked" } else { "unblocked" }
                    )
                })
        }
        ModerationAction::HideProject | ModerationAction::UnhideProject => {
            let hidden = matches!(action, ModerationAction::HideProject);
            catalog
                .moderate_project(&target, hidden, &actor, &reason)
                .await
                .map(|id| {
                    format!(
                        "{} project {target} ({id})",
                        if hidden { "hidden" } else { "restored" }
                    )
                })
        }
    };
    match outcome {
        Ok(message) => println!("{message}"),
        Err(error) => die(error.to_string()),
    }
    catalog.close().await;
}

/// `librepaper admin sweep`. The seven-day floor is the orphan sweeper's
/// own: a blob younger than that may belong to a write still in flight, and
/// `delete_orphans` refuses a shorter grace rather than trusting a flag.
async fn sweep(options: librepaper_engine::storage::StorageOptions, batch: usize) {
    let blobs = match librepaper_engine::storage::open_storage(options.clone()).await {
        Ok(blobs) => blobs,
        Err(error) => die(error),
    };
    let catalog = match librepaper_engine::storage::postgres::PostgresCatalog::connect(
        librepaper_engine::storage::postgres::PostgresOptions::new(&options.database_url),
    )
    .await
    {
        Ok(catalog) => std::sync::Arc::new(catalog),
        Err(error) => die(error.to_string()),
    };
    match librepaper_engine::storage::maintenance::Maintenance::new(catalog, blobs)
        .delete_orphans(batch, time::Duration::days(7))
        .await
    {
        Ok(0) => println!("no unreferenced objects were due"),
        Ok(removed) => println!("deleted {removed} unreferenced object(s)"),
        Err(error) => die(error),
    }
}

/// The server this command talks to: the global `--server` flag, or the
/// `$LIBREPAPER_SERVER` clap merges into it. Every command below needs one,
/// and this is the one place that says so, rather than each repeating the
/// same check.
pub fn server_or_die(server: Option<String>) -> String {
    let server = server.unwrap_or_default();
    if server.trim().is_empty() {
        die("set --server or $LIBREPAPER_SERVER");
    }
    server.trim_end_matches('/').to_string()
}

// The CLI signs in through the deployment, not through a provider: it asks the
// server for a code, you open the URL it prints and approve there with
// whichever provider that deployment offers, and the token lands here. No
// callback URL and no local web server, so it works over SSH and on a machine
// with no browser of its own -- and adding a provider to a deployment adds it
// to `login` with no new flag and no new release of this binary.

#[cfg(test)]
mod socket_policy_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn companion_launch_default_flags_and_explicit_start_parse() {
        let bare = Cli::try_parse_from(["librepaper"]).unwrap();
        assert!(bare.command.is_none());
        assert!(!bare.launch.foreground);
        assert!(!bare.launch.at_login);
        assert!(bare.launch.port.is_none());

        let configured = Cli::try_parse_from([
            "librepaper",
            "--foreground",
            "--at-login",
            "--port",
            "9123",
            "--tool-path",
            "/opt/tools",
        ])
        .unwrap();
        assert!(configured.launch.foreground);
        assert!(configured.launch.at_login);
        assert_eq!(configured.launch.port, Some(9123));
        assert_eq!(configured.launch.tool_path, [PathBuf::from("/opt/tools")]);

        assert!(
            Cli::try_parse_from(["librepaper", "start", "--foreground", "--port", "9123"]).is_ok()
        );
    }

    #[test]
    fn companion_commands_and_hidden_compatibility_paths_parse() {
        for args in [
            vec!["librepaper", "stop"],
            vec!["librepaper", "status"],
            vec!["librepaper", "install-desktop"],
            vec!["librepaper", "agent", "list"],
            vec!["librepaper", "local", "start", "--foreground"],
            vec!["librepaper", "local", "stop"],
            vec!["librepaper", "local", "status"],
            vec!["librepaper", "local", "agent", "list"],
            vec!["librepaper", "local", "approve", "123456"],
            vec!["librepaper", "local", "disconnect", "https://paper.example"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        assert!(Cli::try_parse_from(["librepaper", "desktop"]).is_err());
        assert!(
            Cli::try_parse_from(["librepaper", "local", "open", "librepaper://connect"]).is_ok()
        );
        assert!(Cli::try_parse_from(["librepaper", "local", "tray"]).is_ok());
    }

    #[test]
    fn launch_options_are_isolated_to_bare_launch_and_help_hides_legacy_controls() {
        for args in [
            ["librepaper", "--port", "9123", "list"],
            ["librepaper", "--foreground", "admin", "serve"],
            ["librepaper", "--at-login", "agent", "list"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        let help = Cli::command().render_help().to_string();
        for command in [
            "start",
            "stop",
            "status",
            "install-desktop",
            "agent",
            "login",
            "list",
            "admin",
        ] {
            assert!(help.contains(command), "missing {command} in {help}");
        }
        assert!(!help.contains("local start"));
        let mut root = Cli::command();
        let local_help = root
            .find_subcommand_mut("local")
            .unwrap()
            .render_help()
            .to_string();
        assert!(local_help.contains("approve"));
        assert!(local_help.contains("disconnect"));
        assert!(!local_help.contains("start"));
    }

    #[test]
    fn moderation_cli_requires_actor_and_reason_for_reversible_actions() {
        let incomplete = Cli::try_parse_from([
            "librepaper",
            "admin",
            "moderate",
            "hide-project",
            "abusive-paper",
            "--actor",
            "operator@example.org",
        ]);
        assert!(incomplete.is_err());
        let complete = Cli::try_parse_from([
            "librepaper",
            "admin",
            "moderate",
            "hide-project",
            "abusive-paper",
            "--actor",
            "operator@example.org",
            "--reason",
            "abuse review",
        ]);
        assert!(complete.is_ok());
    }

    #[test]
    fn export_project_parse() {
        let export = Cli::try_parse_from(["librepaper", "export", "paper", "copy"]);
        assert!(export.is_ok());
        let with_at =
            Cli::try_parse_from(["librepaper", "export", "paper", "copy", "--at", "label"]);
        assert!(with_at.is_ok());
        let old_format = Cli::try_parse_from([
            "librepaper",
            "export",
            "paper",
            "--project",
            "--output",
            "copy",
        ]);
        assert!(old_format.is_err());
    }
}
