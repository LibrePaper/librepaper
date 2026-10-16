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
use librepaper_companion::local::cli::{LocalArgs, LocalCommand};

mod agent;

mod documents;
pub mod export;
mod history;
pub(crate) mod server_config;
mod tokens;

pub use documents::*;
pub use tokens::*;

#[derive(Parser)]
#[command(name = "librepaper", version = crate::VERSION, about = "sign in, list and export documents, and administer deployments", long_about = None, arg_required_else_help = true)]
#[command(
    after_help = "To sign in from this terminal, set the deployment and run `librepaper login`:

    export LIBREPAPER_SERVER=https://librepaper.example.org
    librepaper login"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
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
        /// it
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
    // because nobody types it.
    #[command(hide = true)]
    Mcp(agent::McpArgs),
    // The desktop app's interface to the background service. Not listed,
    // because nobody types it.
    #[command(hide = true)]
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
    /// Apply pending PostgreSQL schema migrations, then exit.
    Migrate {
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
        /// New directory to create, or - to write a tar archive to stdout
        directory: String,
        /// Identifier to record for this backup; generated when omitted
        #[arg(long, value_name = "ID")]
        id: Option<String>,
        /// Path inside the archive that holds the recovery point (with -)
        #[arg(long, value_name = "PREFIX", default_value = "backup")]
        prefix: String,
        /// Absolute file or directory to add to the archive at its own path (with -)
        #[arg(long = "include", value_name = "PATH")]
        include: Vec<String>,
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

const DEFAULT_SERVER_CONFIG: &str = "/etc/librepaper/librepaper.toml";

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

/// Installs the one process-wide subscriber. Local commands accept `RUST_LOG`;
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
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(server_config::DEFAULT_LOG_FILTER))
        })
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}

/// Parse and isolate deployment administration before creating the Tokio
/// runtime. SQLx reads libpq-style PG* variables while constructing
/// PostgreSQL options, so this must remain a synchronous process-startup path.
pub fn main() {
    if librepaper_companion::local::folder::run_folder_picker_helper() {
        return;
    }
    let command = Cli::parse().command;
    if needs_local_environment(&command) {
        // SAFETY: this runs synchronously before Tokio creates worker threads,
        // and the environment module documents the same process-startup contract.
        unsafe { librepaper_companion::local::environment::initialize() };
    }
    let postgres_env = if matches!(&command, Command::Admin { .. }) {
        let snapshot = server_config::PostgresEnvSnapshot::capture();
        snapshot.scrub_process_environment();
        Some(snapshot)
    } else {
        None
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| die(format!("could not start async runtime: {error}")));
    runtime.block_on(run(command, postgres_env.as_ref()));
}

fn needs_local_environment(command: &Command) -> bool {
    matches!(
        command,
        Command::Local {
            command: LocalCommand::Start(_)
                | LocalCommand::Status
                | LocalCommand::Diagnostics { .. },
        }
    )
}

async fn run(command: Command, postgres_env: Option<&server_config::PostgresEnvSnapshot>) {
    if !matches!(&command, Command::Admin { .. }) {
        install_subscriber(None);
    }
    match command {
        Command::Login { deployment } => login(deployment.server).await,
        Command::Logout => logout(),
        Command::Admin { command } => run_admin(command, postgres_env).await,
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

async fn run_admin(
    command: AdminCommand,
    postgres_env: Option<&server_config::PostgresEnvSnapshot>,
) {
    match command {
        AdminCommand::Serve { config } => {
            let resolved = server_config::load_with_postgres_env(&config, postgres_env)
                .unwrap_or_else(|error| die(error));
            install_subscriber(Some(&resolved.log_filter));
            librepaper_server::server::serve::serve(serve_options(resolved)).await
        }
        AdminCommand::Migrate { config } => migrate(config, postgres_env).await,
        AdminCommand::Config { command } => match command {
            ConfigCommand::Check { config } => {
                let resolved = server_config::load_with_postgres_env(&config, postgres_env)
                    .unwrap_or_else(|error| die(error));
                install_subscriber(Some(&resolved.log_filter));
                let options = serve_options(resolved);
                if let Err(error) =
                    librepaper_server::server::serve::validate_serve_options(&options)
                {
                    die(error);
                }
                println!("configuration is valid");
            }
            ConfigCommand::Show { config } => {
                let resolved = server_config::load_with_postgres_env(&config, postgres_env)
                    .unwrap_or_else(|error| die(error));
                install_subscriber(Some(&resolved.log_filter));
                let rendered = server_config::show_resolved(&resolved);
                print!("{rendered}");
            }
        },
        AdminCommand::Backup {
            config,
            directory,
            id,
            prefix,
            include,
        } => {
            let (storage, filter) =
                server_config::load_storage_with_log_filter_and_env(&config, postgres_env)
                    .unwrap_or_else(|error| die(error));
            install_subscriber(Some(&filter));
            if directory == "-" {
                librepaper_engine::storage::backup::backup_archive_cli(
                    storage,
                    id.unwrap_or_default(),
                    prefix,
                    include,
                )
                .await
            } else {
                if !include.is_empty() || prefix != "backup" {
                    die("--prefix and --include are only valid with - (tar archive to stdout)");
                }
                librepaper_engine::storage::backup::backup_cli(
                    storage,
                    directory,
                    id.unwrap_or_default(),
                )
                .await
            }
        }
        AdminCommand::Restore {
            config,
            backup,
            directory,
        } => {
            let (storage, filter) =
                server_config::load_storage_with_log_filter_and_env(&config, postgres_env)
                    .unwrap_or_else(|error| die(error));
            install_subscriber(Some(&filter));
            librepaper_engine::storage::backup::restore_cli(storage, backup, directory).await
        }
        AdminCommand::Moderate { command } => moderate(command, postgres_env).await,
        AdminCommand::Sweep { config, batch } => {
            let (storage, filter) =
                server_config::load_storage_with_log_filter_and_env(&config, postgres_env)
                    .unwrap_or_else(|error| die(error));
            install_subscriber(Some(&filter));
            sweep(storage, batch as usize).await
        }
    }
}

fn serve_options(
    resolved: server_config::ResolvedConfig,
) -> librepaper_server::server::serve::ServeOptions {
    librepaper_server::server::serve::ServeOptions {
        address: resolved.address,
        storage: resolved.storage,
        migrate: resolved.migrate,
        github_client_id: resolved.github_client_id,
        github_client_secret: resolved.github_client_secret,
        google_client_id: resolved.google_client_id,
        google_client_secret: resolved.google_client_secret,
        oidc_issuer: resolved.oidc_issuer,
        oidc_client_id: resolved.oidc_client_id,
        oidc_client_secret: resolved.oidc_client_secret,
        oidc_name: resolved.oidc_name,
        oidc_scopes: resolved.oidc_scopes,
        publishers: Some(resolved.publishers.join(",")),
        commenters: Some(resolved.commenters.join(",")),
        simulate_activity: resolved.simulate_activity,
        app_origin: resolved.app_origin,
        docs_origin: resolved.docs_origin,
        site_origin: resolved.site_origin,
        admin_origin: resolved.admin_origin,
        admin_github_ids: resolved.admin_github_ids,
        admin_oidc_subjects: resolved.admin_oidc_subjects,
        expire_after: resolved.expire_after,
        expire_from: resolved.expire_from,
        erase_inactive_after: resolved.erase_inactive_after,
        asset_mirror: resolved.asset_mirror,
        typst_fonts: resolved.typst_fonts,
        load_shell: librepaper_shell::load_shell,
        latex_release: librepaper_shell::latex_release(),
        config: resolved.config,
        metrics_address: resolved.metrics_address,
    }
}

async fn moderate(
    command: ModerationCommand,
    postgres_env: Option<&server_config::PostgresEnvSnapshot>,
) {
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
    let (storage, filter) =
        server_config::load_storage_with_log_filter_and_env(&config, postgres_env)
            .unwrap_or_else(|error| die(error));
    install_subscriber(Some(&filter));
    let mut postgres = PostgresOptions::new(storage.database_url);
    postgres.max_connections = storage.database_connections;
    let catalog = match PostgresCatalog::connect(postgres).await {
        Ok(catalog) => catalog,
        Err(error) => die(error.to_string()),
    };
    if let Err(error) = catalog.validate_schema().await {
        die(format!("PostgreSQL schema is not ready: {error}"));
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
        // Names are refused before sign-in on the admin page, never here.
        ModerationAction::BanHandle | ModerationAction::UnbanHandle => unreachable!(),
    };
    match outcome {
        Ok(message) => println!("{message}"),
        Err(error) => die(error.to_string()),
    }
    catalog.close().await;
}

/// Apply migrations with the storage credentials resolved from the selected
/// config. This intentionally avoids resolving auth, origins, object storage,
/// or listener settings: migration is a one-shot database operation.
async fn migrate(config: PathBuf, postgres_env: Option<&server_config::PostgresEnvSnapshot>) {
    use librepaper_engine::storage::postgres::{PostgresCatalog, PostgresOptions};

    let database = server_config::load_database_with_env(&config, postgres_env)
        .unwrap_or_else(|error| die(error));
    let mut postgres = PostgresOptions::new(database.database_url);
    postgres.max_connections = database.database_connections;
    let catalog = PostgresCatalog::connect(postgres)
        .await
        .unwrap_or_else(|error| die(format!("could not connect to PostgreSQL: {error}")));
    catalog
        .migrate()
        .await
        .unwrap_or_else(|error| die(format!("could not migrate PostgreSQL: {error}")));
    catalog
        .validate_schema()
        .await
        .unwrap_or_else(|error| die(format!("PostgreSQL schema validation failed: {error}")));
    println!("PostgreSQL schema is current");
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
    fn the_bare_command_has_no_launch_and_the_old_root_controls_are_gone() {
        assert!(Cli::try_parse_from(["librepaper"]).is_err());
        for args in [
            vec!["librepaper", "start"],
            vec!["librepaper", "stop"],
            vec!["librepaper", "status"],
            vec!["librepaper", "install-desktop"],
            vec!["librepaper", "--foreground"],
            vec!["librepaper", "--at-login"],
            vec!["librepaper", "--port", "9123", "list"],
            vec!["librepaper", "local", "approve", "123456"],
            vec![
                "librepaper",
                "local",
                "open",
                "librepaper://open?url=https://app.librepaper.org/",
            ],
            vec!["librepaper", "local", "tray"],
            vec!["librepaper", "local", "agent", "list"],
            vec!["librepaper", "agent", "list"],
            vec!["librepaper", "desktop"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn local_commands_parse_for_the_desktop_app() {
        for args in [
            vec!["librepaper", "local", "start"],
            vec![
                "librepaper",
                "local",
                "start",
                "--foreground",
                "--port",
                "9123",
            ],
            vec!["librepaper", "local", "stop"],
            vec!["librepaper", "local", "status"],
            vec!["librepaper", "local", "endpoint"],
            vec!["librepaper", "local", "endpoint", "--port", "9123"],
            vec!["librepaper", "local", "diagnostics"],
            vec!["librepaper", "local", "diagnostics", "--log-dir"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_ok(), "{args:?}");
        }
        assert!(
            Cli::try_parse_from(["librepaper", "local", "start", "--tool-path", "/opt"]).is_err()
        );
        assert!(Cli::try_parse_from(["librepaper", "local", "start", "--at-login"]).is_err());
    }

    #[test]
    fn help_lists_the_public_commands_and_hides_local() {
        let help = Cli::command().render_help().to_string();
        for command in ["login", "logout", "list", "export", "admin"] {
            assert!(help.contains(command), "missing {command} in {help}");
        }
        assert!(!help.to_lowercase().contains("companion"), "{help}");
        assert!(Cli::command()
            .find_subcommand("local")
            .unwrap()
            .is_hide_set());
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
