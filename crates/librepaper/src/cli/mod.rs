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
#[command(name = "librepaper", version = crate::VERSION, about = "host HTML, markdown and typst documents that readers can annotate", long_about = None)]
#[command(
    after_help = "Configure sign-in providers and publishing permissions on the server.
To sign in from this terminal:

    export LIBREPAPER_SERVER=https://librepaper.example.org
    librepaper login"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Deployment credentials: server URL and optional authentication token.
#[derive(Args, Clone, Debug)]
struct Deployment {
    /// Deployment to talk to
    #[arg(
        long,
        env = "LIBREPAPER_SERVER",
        value_name = "URL",
        help_heading = "Deployment"
    )]
    server: Option<String>,
    /// A credential to use instead of the one `login` stored
    #[arg(
        long,
        env = "LIBREPAPER_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true,
        help_heading = "Deployment"
    )]
    token: Option<String>,
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
    /// No public front page: the examples are listed only to their owner
    #[arg(long, env = "LIBREPAPER_NO_LISTING")]
    no_listing: bool,
    /// Most one publisher may store across their documents, in megabytes (default 100)
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
    /// HTTPS static mirror from which browsers fetch LaTeX distributions.
    #[arg(
        long,
        env = "LIBREPAPER_LATEX_MIRROR",
        value_name = "URL",
        default_value = crate::config::DEFAULT_LATEX_MIRROR
    )]
    latex_mirror: String,
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
    fn configuration(&self) -> (Configuration, Option<u32>) {
        let mut config = Configuration::default();
        let mut simulate_activity = None;
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
            simulate_activity = file.simulate_activity_days;
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
        (config, simulate_activity)
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
    /// Write each new account's starter documents as though they had been
    /// typed over this many days, so a demonstration deployment has a history
    /// panel with something in it. The operations are real; only the clock is
    /// invented. See `crate::seed::activity`. Advanced configuration only.
    simulate_activity_days: Option<u32>,
    #[serde(default)]
    backup: crate::config::BackupPolicyOverrides,
}


// The nested `AdminCommand::Serve` flags determine this enum's size too; clap
// parses one command once, so an extra indirection would not improve runtime.
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
        dir: PathBuf,
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
    /// Serve the document MCP tools to an agent on this computer
    Mcp {
        #[arg(
            required_unless_present = "connection",
            conflicts_with = "connection",
            default_value = ""
        )]
        link: String,
        /// A connection registered by the browser on this computer.
        #[arg(long, value_name = "NAME")]
        connection: Option<String>,
        #[command(flatten)]
        deployment: Deployment,
    },
    /// Drive a local ACP agent against a private sidebar conversation. The
    /// local app starts this; it is not listed because nobody types it.
    #[command(hide = true)]
    RunAgent {
        link: String,
        /// Private conversation identifier from the LibrePaper sidebar.
        conversation: String,
        /// Conversation credential.
        #[arg(
            long = "chat-token",
            env = "LIBREPAPER_CHAT_TOKEN",
            hide_env_values = true
        )]
        chat_token: Option<String>,
        /// Directory for the local thread id and completed task ids.
        #[arg(
            long = "state-directory",
            env = "LIBREPAPER_ASSISTANT_STATE_DIR",
            value_name = "DIRECTORY"
        )]
        state_dir: Option<std::path::PathBuf>,
        /// Command line for an Agent Client Protocol implementation.
        /// `allow_hyphen_values` because an adapter's arguments are flags of
        /// its own: `gemini --experimental-acp`, `npx -y pi-acp`. Without it
        /// clap claims the adapter's flag as one of ours and exits 2.
        #[arg(
            long = "agent",
            env = "LIBREPAPER_AGENT",
            value_delimiter = ' ',
            allow_hyphen_values = true,
            required = true,
            value_name = "COMMAND"
        )]
        agent: Vec<String>,
        /// Start a detached runner and wait until it is ready.
        #[arg(long)]
        background: bool,
        #[command(flatten)]
        deployment: Deployment,
    },
    /// The local app: run native tools on this machine for the jobs the
    /// browser editor cannot do itself
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
    /// Replace the contents of a data directory with the example documents
    Seed {
        #[command(flatten)]
        storage: StorageFlags,
        #[arg(long, value_name = "ACCOUNT")]
        owner: Option<String>,
        #[arg(long, value_name = "DIRECTORY")]
        backup: Option<String>,
        /// Write each example as though it had been typed over this many
        /// days -- drafted, cut and rewritten, in sittings -- so a fresh
        /// deployment has a calendar of versions to look at. The operations
        /// are real and reachable; only the times are invented. Capped at the
        /// four weeks the store keeps versions for.
        #[arg(long, value_name = "DAYS")]
        simulate_activity: Option<u32>,
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
    /// Delete objects no catalogue row names any more.
    ///
    /// Every blob a deployment keeps on purpose is named by an asset, a
    /// compaction base or a label archive (§8.5); anything else under the
    /// object store's prefixes is left over from a crash between writing
    /// bytes and committing the row that would have named them. Reclaiming
    /// it means listing the store, which is why this is a command an
    /// operator runs after a bug rather than something a boot does.
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

/// `librepaper local <command>`. See `crate::local::cli`.
#[derive(Subcommand, Clone, Debug)]
pub enum LocalCommand {
    /// Start the companion (in the background by default, or in this process with --foreground)
    Start {
        /// Run in the foreground of this process instead of in the background
        #[arg(long)]
        foreground: bool,
        /// Port to listen on (default 8763)
        #[arg(
            long,
            value_name = "PORT",
            default_value_t = 0,
            hide_default_value = true,
            env = "LIBREPAPER_LOCAL_PORT"
        )]
        port: u16,
        /// Fixed pairing code to use instead of a random one each run
        #[arg(
            long,
            value_name = "CODE",
            env = "LIBREPAPER_LOCAL_CODE",
            hide_env_values = true
        )]
        code: Option<String>,
        /// Extra directories to search for TeX tools, colon-separated
        #[arg(
            long,
            value_name = "DIRS",
            env = "LIBREPAPER_TEX_PATH",
            value_delimiter = ':'
        )]
        tex_path: Vec<PathBuf>,
    },
    /// Open the local companion settings, starting it if needed.
    Settings,
    /// Ask a running companion to stop cleanly.
    Stop,
    /// Launch the companion and open a validated local connection link.
    /// The operating system runs this for `librepaper://` links; it is not
    /// listed because nobody types it.
    #[command(hide = true)]
    Open { url: String },
    /// Service running state, address, code, pairings, native tools, and agent connections
    Status {
        /// Extra directories to search for TeX tools, colon-separated
        #[arg(
            long,
            value_name = "DIRS",
            env = "LIBREPAPER_TEX_PATH",
            value_delimiter = ':'
        )]
        tex_path: Vec<PathBuf>,
    },
    /// Teach this computer an ACP agent the sidebar can drive
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
    /// `librepaper local agent add opencode --label opencode -- opencode acp`
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
    match cli.command {
        Command::Login { deployment } => login(deployment.server).await,
        Command::Logout => logout(),
        Command::Admin { command } => run_admin(command).await,
        Command::List { deployment } => {
            list_documents(deployment.server, deployment.token).await
        }
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
                dir.to_str().unwrap_or_else(|| die("directory path is not valid UTF-8")),
                key.unwrap_or_default(),
                at.unwrap_or_default(),
            )
            .await
        }
        Command::Mcp {
            link,
            connection,
            deployment,
        } => {
            if let Err(err) = agent::run_mcp(link, connection, deployment.server, deployment.token).await {
                die(err);
            }
        }
        Command::RunAgent {
            link,
            conversation,
            chat_token,
            state_dir,
            agent,
            background,
            deployment,
        } => {
            if let Err(err) = agent::run_connect(
                link,
                conversation,
                chat_token,
                state_dir,
                agent,
                background,
                deployment.server,
                deployment.token,
            )
            .await
            {
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
            let (config, simulate_activity) = service.configuration();
            crate::server::serve::serve(crate::server::serve::ServeOptions {
                bind,
                port,
                storage: storage.options(),
                github_client_id: service.github_client_id,
                google_client_id: service.google_client_id,
                publishers: service.publishers,
                commenters: service.commenters,
                no_listing: service.no_listing,
                simulate_activity,
                origin: service.origin,
                docs_origin: service.docs_origin,
                site_origin: service.site_origin,
                expire_after: service.expire_after,
                expire_from: service.expire_from,
                latex_mirror: service.latex_mirror,
                typst_fonts: service.typst_fonts,
                no_local: service.no_local,
                config,
            })
            .await
        }
        AdminCommand::Seed {
            storage,
            owner,
            backup,
            simulate_activity,
        } => {
            let documents = crate::seed::examples::seed_documents();
            crate::seed::seed_with_backup(
                storage.options(),
                &owner.unwrap_or_default(),
                &documents,
                backup.as_deref().map(std::path::Path::new),
                simulate_activity,
            )
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
        AdminCommand::Sweep { storage, batch } => sweep(storage, batch as usize).await,
    }
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

    #[test]
    fn export_project_parse() {
        let export = Cli::try_parse_from(["librepaper", "export", "paper", "copy"]);
        assert!(export.is_ok());
        let with_at = Cli::try_parse_from([
            "librepaper", "export", "paper", "copy", "--at", "label",
        ]);
        assert!(with_at.is_ok());
        let old_format = Cli::try_parse_from([
            "librepaper", "export", "paper", "--project", "--output", "copy",
        ]);
        assert!(old_format.is_err());
    }
}
