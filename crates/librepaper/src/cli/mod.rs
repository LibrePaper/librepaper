//! The command line: what `librepaper` accepts, and `main`, which runs it.
//! Every command in the modules below talks to a deployment over HTTP, the
//! way a browser does; `serve`, `seed` and `local` are the exceptions and
//! live in their own modules.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::config::Configuration;
use crate::document::render::{
    counted, is_html, is_markdown, is_typst, pdf_of, report, title_from_html, title_from_markdown,
    title_from_typst,
};
use crate::http::{
    detail_of, get_as, get_json, get_with_token, post_directory, post_json, post_json_as,
    put_current_bytes, text, Credentials,
};
use crate::storage::StorageFlags;
use crate::util::{die, is_terminal_stdin, is_terminal_stdout, new_id, read_line};

mod documents;
pub mod export;
mod history;
pub mod peer;
mod publish;
mod suggest;
pub mod sync;
mod tokens;

pub use documents::*;
pub use history::*;
pub use publish::*;
pub use suggest::*;
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
    /// Who may publish: a GitHub login, a comma-separated list, 'any', or 'anyone'
    #[arg(long, env = "LIBREPAPER_PUBLISHERS", value_name = "WHO")]
    publishers: Option<String>,
    /// Who may comment: 'anyone' (default), 'any' signed-in account, or a list of accounts
    #[arg(long, env = "LIBREPAPER_COMMENTERS", value_name = "WHO")]
    commenters: Option<String>,
    /// No public front page: the examples are listed only to their owner
    #[arg(long, env = "LIBREPAPER_NO_LISTING")]
    no_listing: bool,
    /// Largest document accepted, in megabytes (default 4, maximum 8)
    #[arg(long, env = "LIBREPAPER_MAX_SIZE", value_name = "MB")]
    max_size: Option<usize>,
    /// Most the figures of one document may come to, in megabytes (default 32)
    #[arg(long, env = "LIBREPAPER_MAX_ASSETS", value_name = "MB")]
    max_assets: Option<usize>,
    /// Most one publisher may store across their documents, in megabytes (default 100)
    #[arg(long, env = "LIBREPAPER_QUOTA", value_name = "MB")]
    quota: Option<usize>,
    /// Most the whole deployment will store, in megabytes (default 5120)
    #[arg(long, env = "LIBREPAPER_STORAGE", value_name = "MB")]
    storage: Option<usize>,
    /// Most documents one publisher may hold (default 50)
    #[arg(long, env = "LIBREPAPER_MAX_DOCUMENTS", value_name = "N")]
    max_documents: Option<usize>,
    /// Most uploads one publisher may make in an hour (default 30)
    #[arg(long, env = "LIBREPAPER_UPLOADS_PER_HOUR", value_name = "N")]
    uploads_per_hour: Option<usize>,
    /// Minutes of quiet before a document is checkpointed (default 5)
    #[arg(long, env = "LIBREPAPER_CHECKPOINT", value_name = "MINUTES")]
    checkpoint: Option<usize>,
    /// Most checkpoints one document keeps; 0 keeps only the current text
    #[arg(long, env = "LIBREPAPER_HISTORY", value_name = "N")]
    history: Option<usize>,
    /// Delete documents after this duration, for example 24h or 30d (default never)
    #[arg(long, env = "LIBREPAPER_EXPIRE_AFTER", value_name = "DURATION")]
    expire_after: Option<String>,
    /// Start expiry at 'updated' (default; last publication) or 'created'
    #[arg(long, env = "LIBREPAPER_EXPIRE_FROM", value_name = "FROM")]
    expire_from: Option<String>,
    /// Serve LaTeX distributions from this https bucket or directory.
    /// LibrePaper always serves LaTeX: without this, it uses the project's
    /// own mirror.
    #[arg(
        long,
        env = "LIBREPAPER_LATEX",
        value_name = "URL-OR-DIR",
        default_value = crate::server::latex::DEFAULT_MIRROR
    )]
    latex: String,
    /// Serve the font files in this directory to typst documents that name a
    /// family the compiler does not embed; `publish` fetches the same fonts.
    /// Without it, such a document is set in the compiler's default faces.
    #[arg(long, env = "LIBREPAPER_FONTS", value_name = "DIR")]
    fonts: Option<String>,
    /// The browser bibliography VM's descriptor (vm.json), as
    /// <url>#<sha256-of-vm.json>: the LaTeX mirror carries no VM image of its
    /// own any more, so this is the only way a deployment offers one. The
    /// sha256 is required, since it is what `vm.js` verifies vm.json against
    /// before trusting anything it names, and is checked to be 64 hex
    /// characters at startup. Without this flag, a document that needs Biber
    /// and has no local LibrePaper reachable simply stops with "no
    /// bibliography VM is configured" once browser TeX itself has run.
    #[arg(long, env = "LIBREPAPER_BIBER_VM", value_name = "URL#SHA256")]
    biber_vm: Option<String>,
}

impl ServiceFlags {
    fn configuration(&self) -> Configuration {
        let mut config = Configuration::default();
        if let Err(err) = config.set_max_assets(self.max_assets) {
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
        // The one place a deployment learns that its ceilings cannot be
        // durably saved: before anything opens a socket, not at the first
        // oversized document.
        if let Err(err) = config.persistence().validate() {
            die(err);
        }
        config
    }
}

// `Serve` is a deployment's whole configuration and is much the largest
// variant, which is what the lint is about. One of these is parsed, once, on
// the way into `main`; boxing it would buy a few hundred bytes at the cost of
// an indirection through every flag, and `clap` cannot flatten through a
// `Box` anyway.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub(crate) enum Command {
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
    /// Run the service on this machine
    Serve {
        /// Interface address; use 127.0.0.1 to accept only local connections
        #[arg(
            long,
            env = "LIBREPAPER_BIND",
            value_name = "ADDRESS",
            default_value = "0.0.0.0"
        )]
        bind: std::net::IpAddr,
        /// Port to listen on; default is the first free one from 8080 to 8099
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
    /// Rotate the local deployment's sealed-link key, retaining the old key
    /// until every catalogue envelope has been resealed.
    RotateLinkKey {
        /// Local deployment directory containing catalog.db and secrets/.
        dir: String,
    },
    /// List your documents
    List,
    /// Open a document for commenting
    Comment {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// A share link, or the key from one: act as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Edit a markdown or typst document, with live preview
    Edit {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// A share link, or the key from one: act as its holder rather than
        /// as your sign-in
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
    },
    /// Show or change how a document is shared
    Share {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// Mint or rotate the link carrying this role: 'read', 'comment', or
        /// 'edit' (also accepted as 'reader', 'commenter', 'editor'). Minting
        /// a role that already has a link replaces it, so the old one stops
        /// working
        #[arg(long, value_name = "ROLE")]
        link: Option<String>,
        /// When the new link stops working, such as 30d; 'never' for no
        /// expiry
        #[arg(long, value_name = "DURATION")]
        until: Option<String>,
        /// A memo describing what this role's link is used for, such as 'CI'.
        /// It survives later rotations unless another label is supplied
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
        /// Comment actions this link may make per clock hour. Omit to use the
        /// deployment's ordinary comment limit
        #[arg(long, value_name = "COUNT")]
        budget: Option<i64>,
        /// Turn off a role's link ('read', 'comment', or 'edit'), or take
        /// away a legacy login
        #[arg(long, value_name = "ROLE")]
        revoke: Option<String>,
    },
    /// Hand a document, its history and its quota to another account
    Transfer {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The GitHub login to hand it to
        to: String,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// What a document used to say, and when
    History {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// A share link, or the key from one: read as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Print the source diff between two checkpoints
    Diff {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The older checkpoint, as the digest `history` prints or its prefix
        from: String,
        /// The newer checkpoint, as the digest `history` prints or its prefix
        to: String,
        /// A share link, or the key from one
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Restore a checkpoint into the live document
    Restore {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The checkpoint, as the digest `history` prints or its prefix
        sha: String,
        /// An editor share link, or the key from one
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Name a checkpoint, so it stands out in the timeline
    Label {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The checkpoint, as the digest `history` prints or the start of it
        sha: String,
        /// What to call it; omit to take an existing name away
        text: Option<String>,
    },
    /// Propose a replacement for a passage, for an editor to accept or reject
    Suggest {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The passage to replace, found once in the file
        #[arg(long, value_name = "TEXT", conflicts_with_all = ["anchor", "batch"])]
        find: Option<String>,
        /// A source anchor copied from an assistant context, as JSON. The
        /// object is sent unchanged to the comments endpoint.
        #[arg(long, value_name = "JSON", conflicts_with_all = ["find", "batch"], requires = "revision")]
        anchor: Option<String>,
        /// The revision on which the anchor was captured.
        #[arg(long, value_name = "SHA", value_parser = crate::cli::revision_value)]
        revision: Option<String>,
        /// JSON file containing a batch of anchored proposals.
        #[arg(long, value_name = "FILE", conflicts_with_all = ["find", "anchor", "replace", "note"])]
        batch: Option<String>,
        /// What to put in its place; empty proposes deleting the passage
        #[arg(long, value_name = "TEXT", default_value = "")]
        replace: String,
        /// Which file the passage is in; defaults to the document's main file
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        /// An optional note explaining the suggestion
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// A comment or editor share link, or the key from one
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Apply a suggestion to the live document
    Accept {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The suggestion's comment id, as `suggest` printed it
        comment_id: String,
        /// An editor share link, or the key from one
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Resolve a suggestion without applying it
    Reject {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The suggestion's comment id, as `suggest` printed it
        comment_id: String,
        /// An editor share link, or the key from one
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Annotations as W3C JSON-LD, markdown, or a response to reviewers
    Export {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// jsonld (W3C Web Annotation), markdown, or response
        #[arg(long, value_name = "FORMAT", default_value = "jsonld")]
        format: String,
        /// Only comments made at or after this checkpoint, as `history` prints it
        #[arg(long, value_name = "SHA")]
        since: Option<String>,
        /// File to write; defaults to standard output
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
        /// A share link, or the key from one: read as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
    },
    /// Replace local or remote data with the example documents
    Seed {
        #[command(flatten)]
        storage: StorageFlags,
        /// The account that owns the local examples, as a GitHub login or a
        /// Google address; without one nobody can edit or manage their sharing
        #[arg(long, value_name = "ACCOUNT")]
        owner: Option<String>,
        /// Verified local backup required before replacing a nonempty catalog.
        #[arg(long, value_name = "DIRECTORY")]
        backup: Option<String>,
    },
    /// Create a verified offline SQLite/object/secrets recovery point.
    Backup {
        #[command(flatten)]
        storage: StorageFlags,
        /// Directory in which the named backup directory is created.
        #[arg(long, value_name = "DIRECTORY")]
        output: String,
        /// Backup name; defaults to a timestamped id.
        #[arg(long, value_name = "ID")]
        id: Option<String>,
    },
    /// Restore a verified local backup into a new deployment directory.
    RestoreBackup {
        /// Backup directory containing manifest.json.
        #[arg(long, value_name = "DIRECTORY")]
        backup: String,
        /// New deployment directory; it must not already exist.
        #[arg(long, value_name = "DIRECTORY")]
        directory: String,
    },
    /// The local compilation service: run native TeX on this machine for
    /// the browser editor when its own compiler cannot
    Local {
        #[command(subcommand)]
        command: LocalCommand,
    },
    /// Run a provider-neutral automation operation against a document link.
    Agent {
        #[command(subcommand)]
        command: crate::cli::peer::AgentCommand,
    },
    /// Delete one document and its comments
    Destroy {
        /// The document to delete, by ID or slug
        #[arg(long, value_name = "ID")]
        document: String,
        /// Skip the confirmation prompt (dangerous)
        #[arg(long)]
        yes: bool,
    },
}

/// `librepaper local <command>`. See `crate::local::cli`.
#[derive(Subcommand, Clone, Debug)]
pub enum LocalCommand {
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
        /// Stay attached to the terminal rather than detaching
        #[arg(long)]
        foreground: bool,
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
        Command::Serve {
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
                latex: service.latex,
                fonts: service.fonts,
                biber_vm: service.biber_vm,
                config,
            })
            .await
        }
        Command::RotateLinkKey { dir } => {
            let root = std::path::PathBuf::from(dir);
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
            // If the file was updated before a process died, its first key is
            // the durable destination even though SQLite still names the old
            // primary. Reuse it; otherwise create a new destination. This
            // makes rerunning the command resume instead of starting a second
            // rotation with an undecryptable source.
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
        Command::List => list_documents(server, token).await,
        Command::Comment { id, key } => {
            comment_document(&id, server, token, key.unwrap_or_default()).await
        }
        Command::Edit { id, key } => {
            edit_document(&id, server, token, key.unwrap_or_default()).await
        }
        Command::Sync {
            id,
            file,
            interval,
            key,
        } => {
            crate::cli::sync::sync_document(
                &id,
                &file,
                server,
                token,
                interval.unwrap_or_default(),
                key.unwrap_or_default(),
            )
            .await
        }
        Command::Share {
            id,
            link,
            until,
            label,
            budget,
            revoke,
        } => {
            share_document(
                &id,
                server,
                token,
                link.unwrap_or_default(),
                until.unwrap_or_default(),
                label,
                budget,
                revoke.unwrap_or_default(),
            )
            .await
        }
        Command::Transfer { id, to, yes } => transfer_document(&id, &to, server, token, yes).await,
        Command::History { id, key } => {
            history_document(&id, server, token, key.unwrap_or_default()).await
        }
        Command::Diff { id, from, to, key } => {
            diff_document(&id, &from, &to, server, token, key.unwrap_or_default()).await
        }
        Command::Restore { id, sha, key } => {
            restore_document(&id, &sha, server, token, key.unwrap_or_default()).await
        }
        Command::Label { id, sha, text } => {
            label_checkpoint(&id, &sha, text.unwrap_or_default(), server, token).await
        }
        Command::Suggest {
            id,
            find,
            anchor,
            revision,
            batch,
            replace,
            path,
            note,
            key,
        } => match batch {
            Some(batch) => {
                suggest_batch(
                    &id,
                    &batch,
                    revision.unwrap_or_default(),
                    server,
                    token,
                    key.unwrap_or_default(),
                )
                .await
            }
            None => match (find, anchor) {
                (Some(find), None) => {
                    let path = path.unwrap_or_default();
                    let note = note.unwrap_or_default();
                    let key = key.unwrap_or_default();
                    match revision.unwrap_or_default() {
                        revision if revision.is_empty() => {
                            suggest_passage(&id, &find, &replace, path, note, server, token, key)
                                .await
                        }
                        revision => {
                            suggest_passage_with_revision(
                                &id, &find, &replace, path, note, revision, server, token, key,
                            )
                            .await
                        }
                    }
                }
                (None, Some(anchor)) => {
                    suggest_anchor(
                        &id,
                        &anchor,
                        &replace,
                        note.unwrap_or_default(),
                        revision.unwrap_or_default(),
                        server,
                        token,
                        key.unwrap_or_default(),
                    )
                    .await
                }
                _ => die("provide exactly one of --find, --anchor, or --batch"),
            },
        },
        Command::Accept {
            id,
            comment_id,
            key,
        } => accept_suggestion(&id, &comment_id, server, token, key.unwrap_or_default()).await,
        Command::Reject {
            id,
            comment_id,
            key,
        } => reject_suggestion(&id, &comment_id, server, token, key.unwrap_or_default()).await,
        Command::Export {
            id,
            format,
            since,
            out,
            key,
        } => {
            crate::cli::export::export_document(
                &id,
                server,
                token,
                &format,
                out.unwrap_or_default(),
                since.unwrap_or_default(),
                key.unwrap_or_default(),
            )
            .await
        }
        Command::Seed {
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
        Command::Backup {
            storage,
            output,
            id,
        } => {
            crate::storage::backup::backup_cli(storage.options(), output, id.unwrap_or_default())
                .await
        }
        Command::RestoreBackup { backup, directory } => {
            crate::storage::backup::restore_cli(backup, directory).await
        }
        Command::Local { command } => crate::local::cli::run(LocalArgs { command }).await,
        Command::Agent { command } => {
            if let Err(err) = crate::cli::peer::run_cli(command, server, token).await {
                die(err);
            }
        }
        Command::Destroy { document, yes } => destroy_document(&document, server, token, yes).await,
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
