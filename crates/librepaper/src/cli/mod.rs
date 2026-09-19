//! The command line: what `librepaper` accepts, and `main`, which runs it.
//! Every command in the modules below talks to a deployment over HTTP, the
//! way a browser does; deployment administration and `local` are the
//! exceptions and live in their own modules.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::config::{parse_budget_transfer, Configuration};
use crate::http::{detail_of, get_as, get_with_token, post_json, text, Credentials};
use crate::storage::StorageFlags;
use crate::util::{die, new_id};

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
    /// Deployment to talk to
    #[arg(
        long,
        global = true,
        env = "LIBREPAPER_SERVER",
        value_name = "URL",
        help_heading = "Deployment"
    )]
    server: Option<String>,
    /// A credential to use instead of the one `login` stored
    #[arg(
        long,
        global = true,
        env = "LIBREPAPER_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true,
        help_heading = "Deployment"
    )]
    token: Option<String>,
    #[command(subcommand)]
    command: Command,
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
    /// Largest document accepted, in megabytes (default 4, maximum 8)
    #[arg(
        long = "document-size-limit",
        env = "LIBREPAPER_MAX_SIZE",
        value_name = "MB"
    )]
    max_size: Option<usize>,
    /// Combined input assets one document may hold, in MiB (default 32).
    #[arg(
        long = "document-assets-limit",
        env = "LIBREPAPER_BUDGET_DOCUMENT_ASSETS",
        value_name = "MIB"
    )]
    budget_document_assets: Option<usize>,
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
    /// Most documents one publisher may hold (default 50)
    #[arg(
        long = "publisher-document-limit",
        env = "LIBREPAPER_MAX_DOCUMENTS",
        value_name = "N"
    )]
    max_documents: Option<usize>,
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
        value_name = "DAYS"
    )]
    simulate_activity: Option<u32>,
    /// Delete documents after this duration, for example 24h or 30d (default never)
    #[arg(
        long = "document-expire-after",
        env = "LIBREPAPER_EXPIRE_AFTER",
        value_name = "DURATION"
    )]
    expire_after: Option<String>,
    /// Start expiry at 'updated' (default; last bundle) or 'created'
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
    /// Daily origin response allowance. Bare integers mean bytes; binary
    /// suffixes such as 10GiB are accepted. Omit for unlimited transfer.
    #[arg(long = "transfer-budget", value_parser = parse_budget_transfer, env = "LIBREPAPER_BUDGET_TRANSFER", value_name = "BYTES")]
    budget_transfer: Option<u64>,
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
            config.apply_cost_overrides(file.cost);
            if let Err(error) = config.apply_backup_overrides(file.backup) {
                die(format!("invalid advanced backup policy: {error}"));
            }
            if let Some(proxies) = file.trusted_proxies {
                config.cost.trusted_proxies = proxies;
                config
                    .policy_origins
                    .insert("cost.trusted_proxies".into(), "configuration file".into());
            }
            for (name, value) in file.session {
                let signed = i64::try_from(value)
                    .unwrap_or_else(|_| die(format!("session.{name} is too large")));
                match name.as_str() {
                    "rooms_max" => config.session.rooms_max = value,
                    "rooms_bytes_max" => config.session.rooms_bytes_max = value,
                    "peer_queue" if value > 0 => config.session.peer_queue = value,
                    "inline_state_max" => config.session.inline_state_max = value,
                    "updates_per_minute" if value > 0 => config.session.updates_per_minute = signed,
                    "checkpoint_owner_per_hour" => {
                        config.session.checkpoint_owner_per_hour = signed
                    }
                    "checkpoint_deployment_per_hour" => {
                        config.session.checkpoint_deployment_per_hour = signed
                    }
                    "write_after_seconds" if value > 0 => {
                        config.session.write_after_seconds = signed
                    }
                    _ => die(format!("unknown or invalid advanced session limit: {name}")),
                }
                config
                    .policy_origins
                    .insert(format!("session.{name}"), "configuration file".into());
            }
            for (name, value) in file.sockets {
                if value == 0 {
                    die(format!("sockets.{name} must be positive"));
                }
                let count = usize::try_from(value)
                    .unwrap_or_else(|_| die(format!("sockets.{name} is too large")));
                match name.as_str() {
                    "deployment_max" => config.sockets.deployment_max = count,
                    "network_max" => config.sockets.network_max = count,
                    "principal_max" => config.sockets.principal_max = count,
                    "document_max" => config.sockets.document_max = count,
                    "document_readers_max" => config.sockets.document_readers_max = count,
                    "document_commenters_max" => config.sockets.document_commenters_max = count,
                    "document_editors_max" => config.sockets.document_editors_max = count,
                    "queue_bytes_max" => config.sockets.queue_bytes_max = count,
                    "state_network_bytes" => config.sockets.state_network_bytes = value,
                    "state_deployment_bytes" => config.sockets.state_deployment_bytes = value,
                    "idle_seconds" => config.sockets.idle_seconds = value,
                    _ => die(format!("unknown advanced socket limit: {name}")),
                }
                config
                    .policy_origins
                    .insert(format!("sockets.{name}"), "configuration file".into());
            }
            for (name, value) in file.persistence {
                match name.as_str() {
                    "max_encoded_snapshot_bytes" => {
                        config.persistence.max_encoded_snapshot_bytes = value
                    }
                    "max_staging_bytes" => config.persistence.max_staging_bytes = value,
                    _ => die(format!("unknown advanced persistence limit: {name}")),
                }
                config
                    .policy_origins
                    .insert(format!("persistence.{name}"), "configuration file".into());
            }
        }
        if let Err(err) = config.set_budget_document_assets(self.budget_document_assets) {
            die(err);
        }
        if let Err(err) = config.set_max_document(self.max_size) {
            die(err);
        }
        if let Err(err) = config.set_storage(self.quota, self.storage) {
            die(err);
        }
        if let Err(err) = config.set_counts(self.max_documents, self.uploads_per_hour) {
            die(err);
        }
        if let Some(transfer) = self.budget_transfer {
            config.cost.transfer_bytes = Some(transfer);
        }
        if let Err(err) = config.cost.validate() {
            die(err);
        }
        // The one place a deployment learns that its ceilings cannot be
        // durably saved: before anything opens a socket, not at the first
        // oversized document.
        if let Err(err) = config.persistence().validate() {
            die(err);
        }
        for (key, flag, environment, present) in [
            (
                "cost.transfer_bytes",
                "transfer-budget",
                "LIBREPAPER_BUDGET_TRANSFER",
                self.budget_transfer.is_some(),
            ),
            (
                "max_assets",
                "document-assets-limit",
                "LIBREPAPER_BUDGET_DOCUMENT_ASSETS",
                self.budget_document_assets.is_some(),
            ),
            (
                "max_document",
                "document-size-limit",
                "LIBREPAPER_MAX_SIZE",
                self.max_size.is_some(),
            ),
            (
                "storage.total",
                "deployment-storage-limit",
                "LIBREPAPER_STORAGE",
                self.storage.is_some(),
            ),
            (
                "storage.per_owner",
                "publisher-storage-limit",
                "LIBREPAPER_QUOTA",
                self.quota.is_some(),
            ),
            (
                "storage.documents_per_owner",
                "publisher-document-limit",
                "LIBREPAPER_MAX_DOCUMENTS",
                self.max_documents.is_some(),
            ),
            (
                "storage.uploads_per_hour",
                "publisher-upload-limit",
                "LIBREPAPER_UPLOADS_PER_HOUR",
                self.uploads_per_hour.is_some(),
            ),
        ] {
            if present {
                let spelling = format!("--{flag}");
                let cli = std::env::args()
                    .take_while(|arg| arg != "--")
                    .any(|arg| arg == spelling || arg.starts_with(&format!("{spelling}=")));
                let source = if cli {
                    "CLI"
                } else if std::env::var_os(environment).is_some() {
                    "environment"
                } else {
                    "CLI"
                };
                config.policy_origins.insert(key.into(), source.into());
            }
        }
        config
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AdvancedConfigFile {
    #[serde(default)]
    cost: crate::config::CostPolicyOverrides,
    /// The explicit peer trust boundary.
    trusted_proxies: Option<Vec<String>>,
    #[serde(default)]
    session: std::collections::BTreeMap<String, usize>,
    #[serde(default)]
    persistence: std::collections::BTreeMap<String, usize>,
    #[serde(default)]
    sockets: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    backup: crate::config::BackupPolicyOverrides,
}

fn backup_policy_from_config(path: Option<&std::path::Path>) -> crate::config::BackupPolicy {
    let Some(path) = path else {
        return crate::config::BackupPolicy::default();
    };
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
    let mut config = crate::config::Configuration::default();
    config
        .apply_backup_overrides(file.backup)
        .unwrap_or_else(|error| die(format!("invalid advanced backup policy: {error}")));
    config.backup
}

// The nested `AdminCommand::Serve` flags determine this enum's size too; clap
// parses one command once, so an extra indirection would not improve runtime.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Sign in through a deployment, in a browser
    Login,
    /// Forget the stored sign-in
    Logout,
    /// Deployment administration and operator commands.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// List your documents
    List,
    /// Export comments or an independent copy of the complete project
    Export {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// jsonld (W3C Web Annotation), markdown, or response
        #[arg(long, value_name = "FORMAT")]
        format: Option<String>,
        /// Only comments made at or after this checkpoint, using its history digest
        #[arg(long, value_name = "SHA")]
        since: Option<String>,
        /// File for comments, or a new directory for a project
        #[arg(long, value_name = "PATH")]
        output: Option<String>,
        /// A share link, or the key from one: read as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
        /// Export every project source file and owned binary asset instead
        /// of the comments
        #[arg(long)]
        project: bool,
    },
    /// The local app: run native tools on this machine for the jobs the
    /// browser editor cannot do itself
    Local {
        #[command(subcommand)]
        command: LocalCommand,
    },
    /// Serve the document MCP tools to an agent on this computer
    Agent {
        #[command(subcommand)]
        command: agent::AgentCommand,
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
    /// Create or restore verified deployment backups.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum BackupCommand {
    /// Create a snapshot-consistent PostgreSQL and immutable-object recovery point.
    Create {
        #[command(flatten)]
        storage: StorageFlags,
        #[arg(
            long = "config",
            env = "LIBREPAPER_CONFIG",
            value_name = "PATH",
            hide = true
        )]
        advanced_config: Option<PathBuf>,
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
}

/// `librepaper local <command>`. See `crate::local::cli`.
#[derive(Subcommand, Clone, Debug)]
pub enum LocalCommand {
    /// Manage companion-local build presets and their execution grants.
    Preset {
        #[command(subcommand)]
        command: LocalPresetCommand,
    },
    /// Start the loopback service and print its pairing code
    Start {
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
    /// Start the local companion in the background.
    Launch {
        #[arg(long, default_value_t = 0, hide_default_value = true)]
        port: u16,
    },
    /// Open the local companion settings, starting it if needed.
    Manage,
    /// Ask a running companion to stop cleanly.
    Stop,
    /// Launch the companion and open a validated local connection link.
    /// The operating system runs this for `librepaper://` links; it is not
    /// listed because nobody types it.
    #[command(hide = true)]
    Open { url: String },
    /// Enable or disable starting the companion when you log in.
    Startup {
        #[command(subcommand)]
        command: StartupCommand,
    },
    /// Whether the service is running, its address, code and pairings
    Status,
    /// Which agents on this computer can reach which documents, and revoke one
    Connections {
        /// Remove one connection by name. Agents configured against it stop
        /// resolving immediately.
        #[arg(long, value_name = "NAME")]
        remove: Option<String>,
    },
    /// Teach this computer an ACP agent the sidebar can drive
    Agent {
        #[command(subcommand)]
        command: LocalAgentCommand,
    },
    /// Which native tools were found, and what is missing
    Doctor {
        /// Extra directories to search for TeX tools, colon-separated
        #[arg(
            long,
            value_name = "DIRS",
            env = "LIBREPAPER_TEX_PATH",
            value_delimiter = ':'
        )]
        tex_path: Vec<PathBuf>,
    },
    /// Revoke pairings
    Disconnect {
        /// The browser origin to revoke; all of them with --all
        #[arg(long, value_name = "URL")]
        origin: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum LocalPresetCommand {
    /// List safe metadata for locally configured presets.
    List,
    /// Create a local preset. Options and environment are key=value pairs.
    Create {
        name: String,
        adapter: String,
        #[arg(long, value_delimiter = ',')]
        format: Vec<String>,
        #[arg(long = "option", value_name = "KEY=VALUE")]
        options: Vec<String>,
        #[arg(long = "env", value_name = "KEY=VALUE")]
        environment: Vec<String>,
        #[arg(long)]
        wrapper: Option<String>,
    },
    /// Update a local preset; all grants become invalid.
    Update {
        id: String,
        name: String,
        adapter: String,
        #[arg(long, value_delimiter = ',')]
        format: Vec<String>,
        #[arg(long = "option", value_name = "KEY=VALUE")]
        options: Vec<String>,
        #[arg(long = "env", value_name = "KEY=VALUE")]
        environment: Vec<String>,
        #[arg(long)]
        wrapper: Option<String>,
    },
    Remove {
        id: String,
    },
    Grant {
        preset: String,
        origin: String,
        project: String,
        entrypoint: String,
        #[arg(long, default_value = "snapshot")]
        workspace: String,
        #[arg(long, default_value = "build")]
        operation: String,
    },
    Revoke {
        id: String,
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

#[derive(Subcommand, Clone, Debug)]
pub enum StartupCommand {
    Enable,
    Disable,
}

/// The arguments `librepaper local` hands to `crate::local::run`.
#[derive(Clone, Debug)]
pub struct LocalArgs {
    pub command: LocalCommand,
}

#[tokio::main]
pub async fn main() {
    let cli = Cli::parse();
    let server = cli.server;
    let token = cli.token;
    match cli.command {
        Command::Login => login(server).await,
        Command::Logout => logout(),
        Command::Admin { command } => run_admin(command).await,
        Command::List => list_documents(server, token).await,
        Command::Export {
            id,
            format,
            since,
            output,
            key,
            project,
        } => {
            if project {
                if format.is_some() || since.is_some() {
                    die("--format and --since apply only to comments export");
                }
                let output = output.unwrap_or_else(|| die("--project requires --output DIRECTORY"));
                crate::cli::export::export_project(
                    &id,
                    server,
                    token,
                    &output,
                    key.unwrap_or_default(),
                )
                .await
            } else {
                crate::cli::export::export_document(
                    &id,
                    server,
                    token,
                    format.as_deref().unwrap_or("jsonld"),
                    output.unwrap_or_default(),
                    since.unwrap_or_default(),
                    key.unwrap_or_default(),
                )
                .await
            }
        }
        Command::Local { command } => crate::local::cli::run(LocalArgs { command }).await,
        Command::Agent { command } => {
            if let Err(err) = agent::run(command, server, token).await {
                die(err);
            }
        }
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
                no_listing: service.no_listing,
                simulate_activity: service.simulate_activity,
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
            command:
                BackupCommand::Create {
                    storage,
                    directory,
                    id,
                    advanced_config,
                },
        } => {
            let _backup_policy = backup_policy_from_config(advanced_config.as_deref());
            crate::storage::backup::backup_cli(storage.options(), directory, id.unwrap_or_default())
                .await
        }
        AdminCommand::Backup {
            command:
                BackupCommand::Restore {
                    storage,
                    backup,
                    directory,
                },
        } => crate::storage::backup::restore_cli(storage.options(), backup, directory).await,
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
    fn export_modes_parse() {
        let comments =
            Cli::try_parse_from(["librepaper", "export", "paper", "--format", "markdown"]);
        assert!(comments.is_ok());
        let project = Cli::try_parse_from([
            "librepaper",
            "export",
            "paper",
            "--project",
            "--output",
            "copy",
        ]);
        assert!(project.is_ok());
    }

    #[test]
    fn advanced_configuration_accepts_separate_document_role_caps() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "sockets:\n  document_readers_max: 80\n  document_commenters_max: 12\n  document_editors_max: 5\n").unwrap();
        let config = ServiceFlags {
            advanced_config: Some(file.path().to_path_buf()),
            ..Default::default()
        }
        .configuration();
        assert_eq!(config.sockets.document_readers_max, 80);
        assert_eq!(config.sockets.document_commenters_max, 12);
        assert_eq!(config.sockets.document_editors_max, 5);
        assert_eq!(config.sockets.document_max, 256);
        for name in [
            "document_readers_max",
            "document_commenters_max",
            "document_editors_max",
        ] {
            assert_eq!(
                config.policy_origins[&format!("sockets.{name}")],
                "configuration file"
            );
        }
    }
}
