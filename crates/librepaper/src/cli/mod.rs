//! The command line: what `librepaper` accepts, and `main`, which runs it.
//! Every command in the modules below talks to a deployment over HTTP, the
//! way a browser does; deployment administration and `local` are the
//! exceptions and live in their own modules.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::config::{parse_budget_transfer, Configuration};
use crate::document::render::{
    counted, is_html, is_markdown, is_typst, report, title_from_html, title_from_markdown,
    title_from_typst,
};
use crate::http::{
    detail_of, get_as, get_json, get_with_token, post_directory, post_json, text, Credentials,
};
use crate::storage::StorageFlags;
use crate::util::{die, is_terminal_stdout, new_id};

/// Removed deployment settings must fail loudly even when they arrive through
/// the process environment. Clap can reject removed arguments, but it cannot
/// distinguish an explicitly set legacy environment variable from an absent
/// one, so this check runs before parsing any command.
fn reject_removed_settings() {
    let removed = [
        (
            "LIBREPAPER_MAX_ASSETS",
            "--document-assets-limit / LIBREPAPER_BUDGET_DOCUMENT_ASSETS",
        ),
        (
            "LIBREPAPER_LATEX",
            "--latex-mirror / LIBREPAPER_LATEX_MIRROR",
        ),
        ("LIBREPAPER_FONTS", "--typst-fonts / LIBREPAPER_TYPST_FONTS"),
        (
            "LIBREPAPER_BIBER_VM",
            "Biber WASM from the configured LaTeX mirror",
        ),
    ];
    for (name, replacement) in removed {
        if std::env::var_os(name).is_some() {
            die(format!("{name} was removed; use {replacement} instead"));
        }
    }
    for argument in std::env::args_os().skip(1) {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        let (name, replacement) = if argument == "--latex" || argument.starts_with("--latex=") {
            ("--latex", "--latex-mirror")
        } else if argument == "--fonts" || argument.starts_with("--fonts=") {
            ("--fonts", "--typst-fonts")
        } else if argument == "--max-assets" || argument.starts_with("--max-assets=") {
            ("--max-assets", "--document-assets-limit")
        } else if argument == "--biber-vm" || argument.starts_with("--biber-vm=") {
            ("--biber-vm", "Biber WASM from the configured LaTeX mirror")
        } else {
            continue;
        };
        die(format!("{name} was removed; use {replacement} instead"));
    }
}

fn parse_publishers(value: &str) -> Result<String, String> {
    if value.trim().eq_ignore_ascii_case("anyone") {
        return Err(
            "--publishers anyone was removed; use --publishers any for any authenticated account"
                .into(),
        );
    }
    Ok(value.to_string())
}

mod documents;
pub mod export;
mod history;
pub(crate) mod mcp;
pub mod peer;
mod publish;
mod quarto;
mod runner;
pub(crate) mod runner_context;
pub(crate) mod runner_journal;
mod runner_lifecycle;
pub(crate) mod runner_preview;
mod runner_transport;
mod skills;
pub mod sync;
mod tokens;

pub use documents::*;
pub use publish::*;
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
    #[arg(long, env = "LIBREPAPER_PUBLISHERS", value_name = "WHO", value_parser = parse_publishers)]
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
    /// Minutes of quiet before a document is checkpointed (default: 30 seconds)
    #[arg(
        long = "history-checkpoint-minutes",
        env = "LIBREPAPER_CHECKPOINT",
        value_name = "MINUTES"
    )]
    checkpoint: Option<usize>,
    /// Most checkpoints one document keeps (default unlimited); 0 keeps only the current text
    #[arg(long = "history-limit", env = "LIBREPAPER_HISTORY", value_name = "N")]
    history: Option<usize>,
    /// Delete documents after this duration, for example 24h or 30d (default never)
    #[arg(
        long = "document-expire-after",
        env = "LIBREPAPER_EXPIRE_AFTER",
        value_name = "DURATION"
    )]
    expire_after: Option<String>,
    /// Start expiry at 'updated' (default; last publication) or 'created'
    #[arg(
        long = "document-expire-from",
        env = "LIBREPAPER_EXPIRE_FROM",
        value_name = "FROM"
    )]
    expire_from: Option<String>,
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
                    "history_max" => config.session.history_max = value,
                    "checkpoint_seconds" => config.session.checkpoint_seconds = signed,
                    "write_after_seconds" if value > 0 => {
                        config.session.write_after_seconds = signed
                    }
                    "history_interval_seconds" if value > 0 => {
                        config.session.history_interval_seconds = signed
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
                    "max_queued_payload_bytes" => {
                        config.persistence.max_queued_payload_bytes = value
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
        if let Err(err) = config.set_history(self.checkpoint, self.history) {
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
            (
                "session.history_max",
                "history-limit",
                "LIBREPAPER_HISTORY",
                self.history.is_some(),
            ),
            (
                "session.checkpoint_seconds",
                "history-checkpoint-minutes",
                "LIBREPAPER_CHECKPOINT",
                self.checkpoint.is_some(),
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
    /// Read or export the agent skills bundled with this version (offline)
    Skills {
        #[command(subcommand)]
        command: skills::SkillsCommand,
    },
    /// Sign in through a deployment, in a browser
    Login,
    /// Forget the stored sign-in
    Logout,
    /// Publish a document and print its link
    Publish {
        /// The HTML, markdown or typst file to publish, or a directory holding one
        file: String,
        /// Display title; defaults to the first heading, then the filename
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// Full existing slug to replace, keeping link and comments
        #[arg(long, value_name = "SLUG")]
        slug: Option<String>,
        /// Which file in a directory is the document
        #[arg(long, value_name = "PATH")]
        main: Option<String>,
    },
    /// Deployment administration and operator commands.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// List your documents
    List,
    /// Open a document in the browser
    Open {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// A share link, or the key from one
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Keep a local file and a document in step, both ways, until interrupted
    Sync {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The file to keep in step with the document's main file
        file: String,
        /// How long either side stays quiet before it is acted on (default 250ms)
        #[arg(long, value_name = "DURATION")]
        interval: Option<String>,
        /// An edit link, or the key from one: join as its holder, with no
        /// sign-in needed where the deployment asks for none
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
        /// Show the project files that would be synchronized, then exit
        #[arg(long)]
        dry_run: bool,
    },
    /// Annotations as W3C JSON-LD, markdown, or a response to reviewers
    Export {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// jsonld (W3C Web Annotation), markdown, or response
        #[arg(long, value_name = "FORMAT", default_value = "jsonld")]
        format: String,
        /// Only comments made at or after this checkpoint, using its history digest
        #[arg(long, value_name = "SHA")]
        since: Option<String>,
        /// File to write; defaults to standard output
        #[arg(long, value_name = "FILE")]
        output: Option<String>,
        /// A share link, or the key from one: read as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// The local compilation service: run native TeX on this machine for
    /// the browser editor when its own compiler cannot
    Local {
        #[command(subcommand)]
        command: LocalCommand,
    },
    /// Import, publish, and inspect saved Quarto results without executing code
    Quarto {
        #[command(subcommand)]
        command: quarto::QuartoCommand,
    },
    /// Run the MCP adapter or manage the local assistant runner.
    Agent {
        #[command(subcommand)]
        command: crate::cli::peer::AgentCommand,
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
    /// Print the operator status surface from the local server.
    Status {
        #[command(flatten)]
        storage: StorageFlags,
        #[arg(long, value_name = "URL")]
        endpoint: Option<String>,
        #[arg(long, default_value_t = 8080, value_name = "PORT")]
        port: u16,
    },
    /// Manage the keys used by a deployment.
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
    /// Replace local or remote data with the example documents
    Seed {
        #[command(flatten)]
        storage: StorageFlags,
        #[arg(long, value_name = "ACCOUNT")]
        owner: Option<String>,
        #[arg(long, value_name = "DIRECTORY")]
        backup: Option<String>,
    },
    /// Create or restore verified deployment backups.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum KeyCommand {
    /// Rotate the local deployment's sealed-link key.
    Rotate {
        /// Deployment data directory
        directory: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum BackupCommand {
    /// Create a verified offline SQLite/object/secrets recovery point.
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
        /// Backup directory to read
        backup: String,
        /// New deployment directory to create
        directory: String,
    },
}

/// `librepaper local <command>`. See `crate::local::cli`.
#[derive(Subcommand, Clone, Debug)]
pub enum LocalCommand {
    /// Manage local Quarto execution permissions.
    Quarto {
        #[command(subcommand)]
        command: LocalQuartoCommand,
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
    /// Restart the background companion.
    Restart {
        #[arg(long, default_value_t = 0, hide_default_value = true)]
        port: u16,
    },
    /// Launch the companion and open a validated local connection link.
    Open { url: String },
    /// Enable or disable starting the companion when you log in.
    Startup {
        #[command(subcommand)]
        command: StartupCommand,
    },
    /// Whether the service is running, its address, code and pairings
    Status,
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
    /// Refresh the discovered tools
    Rescan {
        /// Extra directories to search for TeX tools, colon-separated
        #[arg(
            long,
            value_name = "DIRS",
            env = "LIBREPAPER_TEX_PATH",
            value_delimiter = ':'
        )]
        tex_path: Vec<PathBuf>,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum LocalQuartoCommand {
    /// Grant this machine permission to render a linked Quarto project
    Bind {
        origin: String,
        project: String,
        #[arg(long, default_value = ".", value_name = "DIRECTORY")]
        root: String,
        #[arg(long, default_value = "main.qmd", value_name = "FILE")]
        main: String,
    },
    /// Revoke a local Quarto execution binding
    Unbind { binding: String },
    /// List bindings for an origin and document
    List { origin: String, project: String },
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
    reject_removed_settings();
    let cli = Cli::parse();
    let server = cli.server;
    let token = cli.token;
    match cli.command {
        Command::Skills { command } => {
            if let Err(error) = skills::run(command) {
                die(error);
            }
        }
        Command::Login => login(server).await,
        Command::Logout => logout(),
        Command::Publish {
            file,
            title,
            slug,
            main,
        } => {
            publish(
                &file,
                title.unwrap_or_default(),
                slug.unwrap_or_default(),
                server,
                token,
                main.unwrap_or_default(),
            )
            .await
        }
        Command::Admin { command } => run_admin(command, server, token).await,
        Command::List => list_documents(server, token).await,
        Command::Open { id, key } => {
            open_document(&id, server, token, key.unwrap_or_default()).await
        }
        Command::Sync {
            id,
            file,
            interval,
            key,
            dry_run,
        } => {
            crate::cli::sync::sync_document(
                &id,
                &file,
                server,
                token,
                interval.unwrap_or_default(),
                key.unwrap_or_default(),
                dry_run,
            )
            .await
        }
        Command::Export {
            id,
            format,
            since,
            output,
            key,
        } => {
            crate::cli::export::export_document(
                &id,
                server,
                token,
                &format,
                output.unwrap_or_default(),
                since.unwrap_or_default(),
                key.unwrap_or_default(),
            )
            .await
        }
        Command::Local { command } => crate::local::cli::run(LocalArgs { command }).await,
        Command::Quarto { command } => {
            if let Err(error) = quarto::run(command, server, token).await {
                die(error);
            }
        }
        Command::Agent { command } => {
            if let Err(err) = crate::cli::peer::run_cli(command, server, token).await {
                die(err);
            }
        }
    }
}

async fn run_admin(command: AdminCommand, server: Option<String>, token: Option<String>) {
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
                expire_after: service.expire_after,
                expire_from: service.expire_from,
                latex_mirror: service.latex_mirror,
                typst_fonts: service.typst_fonts,
                no_local: service.no_local,
                config,
            })
            .await
        }
        AdminCommand::Status {
            endpoint,
            port,
            storage,
        } => {
            let endpoint = endpoint
                .or(server)
                .unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
            let url = format!("{}/api/status", endpoint.trim_end_matches('/'));
            let parsed = url::Url::parse(&url)
                .unwrap_or_else(|error| die(format!("invalid status URL: {error}")));
            let loopback = match parsed.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                Some(url::Host::Domain("localhost")) => true,
                _ => false,
            };
            if !loopback {
                die("operator status requires a loopback endpoint");
            }
            let (status, payload) = match get_json(&url, Duration::from_secs(10)).await {
                Ok(response) => response,
                Err(error) => {
                    let paths = storage.options().paths().unwrap_or_else(|error| die(error));
                    let payload = crate::server::cost::offline_status(&paths.catalog)
                        .unwrap_or_else(|offline| die(format!("could not query operator status: {error}; could not read durable counters: {offline}")));
                    (200, payload)
                }
            };
            if status != 200 {
                die(format!(
                    "operator status returned {status}: {}",
                    detail_of(&payload)
                ));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|error| die(format!(
                    "could not format operator status: {error}"
                )))
            );
        }
        AdminCommand::Key {
            command: KeyCommand::Rotate { directory },
        } => {
            let root = directory;
            let _writer_lock =
                crate::server::serve::acquire_writer_lock(&root.join("state/writer.lock"))
                    .unwrap_or_else(|error| die(error));
            let catalog = crate::storage::catalog::Catalog::open(root.join("catalog.db"))
                .unwrap_or_else(|error| die(format!("could not open catalogue: {error}")));
            let key_path = root.join("secrets/links.key");
            let keys = crate::auth::link_sealing_keyring_file(&key_path, true)
                .unwrap_or_else(|error| die(error));
            let key_id = |key: &[u8]| {
                use sha2::Digest;
                hex::encode(sha2::Sha256::digest(key))[..16].to_string()
            };
            let durable_primary = catalog
                .link_keyring_primary_id()
                .unwrap_or_else(|error| die(error.to_string()));
            let source_index = durable_primary
                .as_deref()
                .and_then(|primary| keys.iter().position(|key| key_id(key) == primary))
                .unwrap_or(0);
            let source = keys[source_index].clone();
            let destination = if key_id(&keys[0]) != key_id(&source) {
                keys[0].clone()
            } else {
                crate::auth::random_bytes(32)
            };
            let mut persisted = Vec::with_capacity(keys.len() + 1);
            persisted.push(destination.clone());
            persisted.push(source.clone());
            for key in keys {
                if key_id(&key) != key_id(&destination) && key_id(&key) != key_id(&source) {
                    persisted.push(key);
                }
            }
            crate::auth::write_link_sealing_keyring(&key_path, &persisted)
                .unwrap_or_else(|error| die(error));
            catalog
                .set_link_sealing_key(&source)
                .unwrap_or_else(|error| die(error.to_string()));
            for old in persisted.iter().skip(1) {
                catalog
                    .add_link_decryption_key(old)
                    .unwrap_or_else(|error| die(error.to_string()));
            }
            let changed = catalog
                .rotate_link_sealing_key(&destination)
                .unwrap_or_else(|error| die(error.to_string()));
            println!("resealed {changed} links");
        }
        AdminCommand::Seed {
            storage,
            owner,
            backup,
        } => {
            let documents = crate::seed::examples::seed_documents();
            match server {
                Some(server) if !server.is_empty() => {
                    crate::seed::seed_remote(server, token.as_deref(), &documents).await
                }
                _ => match backup {
                    Some(backup) => {
                        crate::seed::seed_with_backup(
                            storage.options(),
                            &owner.unwrap_or_default(),
                            &documents,
                            Some(std::path::Path::new(&backup)),
                        )
                        .await
                    }
                    None => {
                        crate::seed::seed(storage.options(), &owner.unwrap_or_default(), &documents)
                            .await
                    }
                },
            }
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
            let backup_policy = backup_policy_from_config(advanced_config.as_deref());
            crate::storage::backup::backup_cli(
                storage.options(),
                directory,
                id.unwrap_or_default(),
                backup_policy,
            )
            .await
        }
        AdminCommand::Backup {
            command: BackupCommand::Restore { backup, directory },
        } => crate::storage::backup::restore_cli(backup, directory).await,
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

/// Normalizes a server into the origin its cached token is filed under:
/// scheme, host and port, with the scheme's default port made explicit so an
/// address with and without an explicit `:443` resolve to the same entry. A
/// string that does not parse as a URL is lowercased and trimmed instead of
/// failing -- every value handed to `--server` needs a cache key, valid URL
/// or not.
fn origin_of(server: &str) -> String {
    match url::Url::parse(server) {
        Ok(url) if url.host_str().is_some() => format!(
            "{}://{}:{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.port_or_known_default().unwrap_or(0)
        ),
        _ => server.trim().trim_end_matches('/').to_lowercase(),
    }
}

#[cfg(test)]
mod socket_policy_tests {
    use super::*;

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
