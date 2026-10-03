//! The command line: what `librepaper` accepts, and `main`, which runs it.
//! Every command in the modules below talks to a deployment over HTTP, the
//! way a browser does; deployment administration and `local` are the
//! exceptions and live in their own modules.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::config::Configuration;
use crate::http::{detail_of, get_as, get_with_token, post_json, text, Credentials};
use crate::storage::StorageFlags;
use crate::util::die;

mod agent;

mod documents;
pub mod export;
mod history;
mod tokens;

pub use documents::*;
pub use tokens::*;

#[derive(Parser)]
#[command(name = "librepaper", version = crate::VERSION, about = "launch the companion to connect local tools to documents; host HTML, markdown and typst documents", long_about = None, args_conflicts_with_subcommands = true)]
#[command(
    after_help = "Run `librepaper` or `librepaper start` to launch the companion, then connect it in Settings → Local app.

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

/// The options that describe a running service rather than where it runs:
/// who may publish and comment, how large a document may be, and when
/// documents expire.
#[derive(Args, Clone, Debug, Default)]
pub(crate) struct ServiceFlags {
    /// GitHub OAuth app client id
    #[arg(long, env = "LIBREPAPER_GITHUB_CLIENT_ID", value_name = "ID")]
    github_client_id: Option<String>,
    /// Google OAuth client id
    #[arg(long, env = "LIBREPAPER_GOOGLE_CLIENT_ID", value_name = "ID")]
    google_client_id: Option<String>,
    /// Who may publish: a GitHub login, a comma-separated list, or 'any'
    #[arg(long, env = "LIBREPAPER_PUBLISHERS", value_name = "WHO")]
    publishers: Option<String>,
    /// Who may comment: 'anyone' (default), 'any' signed-in account, or a list of accounts
    #[arg(long, env = "LIBREPAPER_COMMENTERS", value_name = "WHO")]
    commenters: Option<String>,
    /// Most one publisher may store across their documents, in megabytes (default 50)
    #[arg(
        long = "publisher-storage-limit",
        env = "LIBREPAPER_QUOTA",
        value_name = "MB"
    )]
    quota: Option<usize>,
    /// Most the whole deployment will store, in megabytes (default 5120)
    #[arg(
        long = "deployment-storage-limit",
        env = "LIBREPAPER_STORAGE",
        value_name = "MB"
    )]
    storage: Option<usize>,
    /// Most uploads one publisher may make in an hour (default 30)
    #[arg(
        long = "publisher-upload-limit",
        env = "LIBREPAPER_UPLOADS_PER_HOUR",
        value_name = "N"
    )]
    uploads_per_hour: Option<usize>,
    /// Write each new account's starter documents as though they had been
    /// typed over this many days, so a demonstration deployment has a history
    /// panel with something in it. The operations are real; only the clock is
    /// invented. See `crate::seed::activity`.
    #[arg(
        long = "simulate-activity",
        env = "LIBREPAPER_SIMULATE_ACTIVITY",
        value_name = "DAYS",
        hide = true
    )]
    simulate_activity: Option<u32>,
    /// Delete documents after this duration, for example 24h or 30d (default never)
    #[arg(
        long = "document-expire-after",
        env = "LIBREPAPER_EXPIRE_AFTER",
        value_name = "DURATION"
    )]
    expire_after: Option<String>,
    /// Start expiry at 'updated' (default; last write) or 'created'
    #[arg(
        long = "document-expire-from",
        env = "LIBREPAPER_EXPIRE_FROM",
        value_name = "FROM"
    )]
    expire_from: Option<String>,
    /// The origin browsers reach this deployment on, for example
    /// https://paper.example. Documents are served from a second origin, by
    /// default the same host behind "docs."; both must be configured for a
    /// deployment that is not loopback-only, and the server answers on no
    /// other name.
    #[arg(long, env = "LIBREPAPER_ORIGIN", value_name = "URL")]
    origin: Option<String>,
    /// The origin published documents are served from, when it is not the
    /// reader's host behind "docs.". A document is hostile code, so this must
    /// be a different host from --origin, never merely a different port.
    #[arg(
        long = "docs-origin",
        env = "LIBREPAPER_DOCS_ORIGIN",
        value_name = "URL",
        requires = "origin"
    )]
    docs_origin: Option<String>,
    /// Where this deployment's marketing site lives, for example
    /// https://paper.example. Signing out goes there. Without it, signing out
    /// goes to this deployment's own front page, which is what a deployment
    /// with no separate site in front of it wants.
    #[arg(
        long = "site-origin",
        env = "LIBREPAPER_SITE_ORIGIN",
        value_name = "URL"
    )]
    site_origin: Option<String>,
    /// HTTPS static mirror from which browsers fetch the wasm renderers
    /// (`wasm/<sha256>/`) and the pinned LaTeX release (`latex/<sha256>/`).
    #[arg(
        long,
        env = "LIBREPAPER_ASSET_MIRROR",
        value_name = "URL",
        default_value = crate::config::DEFAULT_ASSET_MIRROR
    )]
    asset_mirror: String,
    /// Serve the font files in this directory to typst documents that name a
    /// family the compiler does not embed; `publish` fetches the same fonts.
    /// Without it, such a document is set in the compiler's default faces.
    #[arg(long, env = "LIBREPAPER_TYPST_FONTS", value_name = "DIR")]
    typst_fonts: Option<String>,
    /// Do not run the local app for this machine. By default `serve` also
    /// starts the loopback service that lets an editor whose browser is on
    /// this host render Quarto documents with the tools installed here, with
    /// no pairing code and no project grant.
    #[arg(long, env = "LIBREPAPER_NO_LOCAL")]
    no_local: bool,
    /// Optional advanced configuration file. This is intentionally hidden from
    /// the daily flag surface; use it only for guardrail overrides such as
    /// `trusted_proxies`.
    #[arg(
        long = "config",
        env = "LIBREPAPER_CONFIG",
        value_name = "PATH",
        hide = true
    )]
    advanced_config: Option<PathBuf>,
}

impl ServiceFlags {
    fn configuration(&self) -> Configuration {
        let mut config = Configuration::default();
        if let Some(path) = &self.advanced_config {
            let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
                die(format!(
                    "could not read advanced configuration {}: {error}",
                    path.display()
                ))
            });
            let file: AdvancedConfigFile = serde_yaml::from_str(&text).unwrap_or_else(|error| {
                die(format!(
                    "could not parse advanced configuration {}: {error}",
                    path.display()
                ))
            });
            if let Err(error) = config.apply_backup_overrides(file.backup) {
                die(format!("invalid advanced backup policy: {error}"));
            }
            if let Some(proxies) = file.trusted_proxies {
                config.cost.trusted_proxies = proxies;
            }
            if let Err(error) = config.set_peer_queue(file.session_peer_queue) {
                die(format!("invalid advanced session policy: {error}"));
            }
            if let Err(error) = config.set_pending(file.pending_mb, file.pending_scratch_mb) {
                die(format!("invalid advanced pending policy: {error}"));
            }
            if let Err(error) = config.set_log_quota(file.log_quota_mb) {
                die(format!("invalid advanced log quota: {error}"));
            }
            if let Err(error) = config.set_memory_budget(file.memory_budget_mb) {
                die(format!("invalid advanced memory budget: {error}"));
            }
        }
        if let Err(err) = config.set_storage(self.quota, self.storage) {
            die(err);
        }
        if let Err(err) = config.set_uploads_per_hour(self.uploads_per_hour) {
            die(err);
        }
        if let Err(err) = config.cost.validate() {
            die(err);
        }
        // Checked after all configuration overrides are applied.
        if let Err(err) = config.validate_pending() {
            die(err);
        }
        if let Err(err) = config.validate_budgets() {
            die(err);
        }
        config
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AdvancedConfigFile {
    /// The explicit peer trust boundary.
    trusted_proxies: Option<Vec<String>>,
    /// How many frames one socket's outbound queue may hold before the
    /// subscriber is closed. The byte budget is this limit times 64 KiB.
    session_peer_queue: Option<usize>,
    /// How much unsaved source the whole deployment will hold, in megabytes,
    /// and how much temporary memory the persistence path may hold while
    /// writing it. Advanced configuration only; an integration test lowers
    /// them to reach pressure without buffering sixty-four megabytes first.
    pending_mb: Option<u64>,
    pending_scratch_mb: Option<u64>,
    /// The per-document log ceiling, in megabytes. Advanced configuration only;
    /// the memory budget must be able to build one document this large, see
    /// `memory_budget_mb`.
    log_quota_mb: Option<u64>,
    /// The process-wide budget for decoded documents, in megabytes. Advanced
    /// configuration only. It must be able to build one document at the log
    /// quota, so a raised quota needs a raised budget with it; the pair is
    /// checked at startup.
    memory_budget_mb: Option<u64>,
    #[serde(default)]
    backup: crate::config::BackupPolicyOverrides,
}

// The nested `AdminCommand::Serve` flags determine this enum's size too; clap
// parses one command once, so an extra indirection would not improve runtime.
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
        #[arg(
            long,
            env = "LIBREPAPER_BIND",
            value_name = "ADDRESS",
            default_value = "0.0.0.0"
        )]
        bind: std::net::IpAddr,
        #[arg(
            long,
            env = "LIBREPAPER_PORT",
            value_name = "PORT",
            default_value_t = 0,
            hide_default_value = true
        )]
        port: u16,
        #[command(flatten)]
        service: ServiceFlags,
        #[command(flatten)]
        storage: StorageFlags,
    },
    /// Create a snapshot-consistent PostgreSQL and immutable-object recovery point.
    Backup {
        #[command(flatten)]
        storage: StorageFlags,
        /// New directory to create
        directory: String,
        /// Identifier to record for this backup; generated when omitted
        #[arg(long, value_name = "ID")]
        id: Option<String>,
    },
    /// Restore a verified backup into a new deployment directory.
    Restore {
        #[command(flatten)]
        storage: StorageFlags,
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
        #[command(flatten)]
        storage: StorageFlags,
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

#[derive(Args, Clone, Debug)]
pub(crate) struct ModerationDatabase {
    /// PostgreSQL URL for the deployment being moderated.
    #[arg(
        long,
        env = "LIBREPAPER_DATABASE_URL",
        hide_env_values = true,
        default_value = "postgresql:///librepaper",
        value_name = "URL"
    )]
    database_url: String,
}

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
        #[command(flatten)]
        database: ModerationDatabase,
    },
    /// Restore an account blocked by moderation.
    UnblockAccount {
        /// Account UUID or unique provider handle.
        account: String,
        #[arg(long, value_name = "ACTOR", required = true)]
        actor: String,
        #[arg(long, value_name = "REASON", required = true)]
        reason: String,
        #[command(flatten)]
        database: ModerationDatabase,
    },
    /// Hide a project from every document and asset route without deleting it.
    HideProject {
        /// Project slug.
        project: String,
        #[arg(long, value_name = "ACTOR", required = true)]
        actor: String,
        #[arg(long, value_name = "REASON", required = true)]
        reason: String,
        #[command(flatten)]
        database: ModerationDatabase,
    },
    /// Restore public access to a project hidden by moderation.
    UnhideProject {
        /// Project slug.
        project: String,
        #[arg(long, value_name = "ACTOR", required = true)]
        actor: String,
        #[arg(long, value_name = "REASON", required = true)]
        reason: String,
        #[command(flatten)]
        database: ModerationDatabase,
    },
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

#[tokio::main]
pub async fn main() {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Start(cli.launch));
    match command {
        Command::Start(args) => {
            crate::local::cli::run(LocalArgs {
                command: LocalCommand::Start(args),
            })
            .await
        }
        Command::Stop => {
            crate::local::cli::run(LocalArgs {
                command: LocalCommand::Stop,
            })
            .await
        }
        Command::Status { tool_path } => {
            crate::local::cli::run(LocalArgs {
                command: LocalCommand::Status { tool_path },
            })
            .await
        }
        Command::Agent { command } => {
            crate::local::cli::run(LocalArgs {
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
        Command::Local { command } => crate::local::cli::run(LocalArgs { command }).await,
    }
}

async fn run_admin(command: AdminCommand) {
    match command {
        AdminCommand::Serve {
            bind,
            port,
            service,
            storage,
        } => {
            let config = service.configuration();
            crate::server::serve::serve(crate::server::serve::ServeOptions {
                bind,
                port,
                storage: storage.options(),
                github_client_id: service.github_client_id,
                google_client_id: service.google_client_id,
                publishers: service.publishers,
                commenters: service.commenters,
                simulate_activity: service.simulate_activity,
                origin: service.origin,
                docs_origin: service.docs_origin,
                site_origin: service.site_origin,
                expire_after: service.expire_after,
                expire_from: service.expire_from,
                asset_mirror: service.asset_mirror,
                typst_fonts: service.typst_fonts,
                no_local: service.no_local,
                config,
            })
            .await
        }
        AdminCommand::Backup {
            storage,
            directory,
            id,
        } => {
            crate::storage::backup::backup_cli(storage.options(), directory, id.unwrap_or_default())
                .await
        }
        AdminCommand::Restore {
            storage,
            backup,
            directory,
        } => crate::storage::backup::restore_cli(storage.options(), backup, directory).await,
        AdminCommand::Moderate { command } => moderate(command).await,
        AdminCommand::Sweep { storage, batch } => sweep(storage, batch as usize).await,
    }
}

async fn moderate(command: ModerationCommand) {
    use crate::storage::postgres::{ModerationAction, PostgresCatalog, PostgresOptions};
    let (database, action, target, actor, reason) = match command {
        ModerationCommand::BlockAccount {
            account,
            actor,
            reason,
            database,
        } => (
            database,
            ModerationAction::BlockAccount,
            account,
            actor,
            reason,
        ),
        ModerationCommand::UnblockAccount {
            account,
            actor,
            reason,
            database,
        } => (
            database,
            ModerationAction::UnblockAccount,
            account,
            actor,
            reason,
        ),
        ModerationCommand::HideProject {
            project,
            actor,
            reason,
            database,
        } => (
            database,
            ModerationAction::HideProject,
            project,
            actor,
            reason,
        ),
        ModerationCommand::UnhideProject {
            project,
            actor,
            reason,
            database,
        } => (
            database,
            ModerationAction::UnhideProject,
            project,
            actor,
            reason,
        ),
    };
    let catalog = match PostgresCatalog::connect(PostgresOptions::new(database.database_url)).await
    {
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
async fn sweep(storage: StorageFlags, batch: usize) {
    let options = storage.options();
    let blobs = match crate::storage::open_storage(options.clone()).await {
        Ok(blobs) => blobs,
        Err(error) => die(error),
    };
    let catalog = match crate::storage::postgres::PostgresCatalog::connect(
        crate::storage::postgres::PostgresOptions::new(&options.database_url),
    )
    .await
    {
        Ok(catalog) => std::sync::Arc::new(catalog),
        Err(error) => die(error.to_string()),
    };
    match crate::storage::maintenance::Maintenance::new(catalog, blobs)
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
        assert!(
            Cli::try_parse_from(["librepaper", "local", "open", "librepaper://connect"]).is_ok()
        );
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
        for command in ["start", "stop", "status", "agent", "login", "list", "admin"] {
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
