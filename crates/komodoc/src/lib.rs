//! Komodoc: host HTML, markdown and typst documents that readers can
//! annotate. One binary: the server, and the command line that talks to it.
//!
//! A library only so that `fuzz/` can reach the three modules that read what
//! a peer sends -- the shared document, the path rules, and the configuration
//! that holds those rules. Those are `pub`; everything else stays private, so
//! the dead-code lint still covers it. `main.rs` is one line.

mod assets;
mod auth;
mod blob;
mod checkpoint_cache;
mod cli;
mod clock;
pub mod config;
mod export;
mod history;
mod http;
mod latex;
mod origins;
pub mod paths;
mod pseudonym;
mod render;
mod retention;
mod room;
mod s3;
mod seed;
mod seed_examples;
mod serve;
mod server;
pub mod session;
mod storage;
mod store;
mod sync;
mod util;

#[cfg(test)]
mod tests;

use clap::{Args, Parser, Subcommand};

use crate::config::Configuration;
use crate::storage::StorageFlags;
use crate::util::die;

/// The release version, stamped in at build time. Unreleased builds keep the
/// placeholder.
pub const VERSION: &str = match option_env!("KOMODOC_VERSION") {
    Some(version) => version,
    None => "dev",
};

#[derive(Parser)]
#[command(name = "komodoc", version = VERSION, about = "host HTML, markdown and typst documents that readers can annotate", long_about = None)]
#[command(
    after_help = "Serving needs a GitHub OAuth app (github.com/settings/developers) and
--publishers saying who may publish. Publishing needs neither,
only the server and a sign-in:

    export KOMODOC_SERVER=https://komodoc.example.org
    komodoc login"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The options that describe a running service rather than where it runs:
/// who may publish and comment, how large a document may be, and when
/// documents expire.
#[derive(Args, Clone, Debug, Default)]
struct ServiceFlags {
    /// GitHub OAuth app client id; or $KOMODOC_GITHUB_CLIENT_ID
    #[arg(long, value_name = "ID")]
    client_id: Option<String>,
    /// GitHub OAuth app client secret; or $KOMODOC_GITHUB_CLIENT_SECRET
    #[arg(long, value_name = "SECRET")]
    client_secret: Option<String>,
    /// Who may publish: a GitHub login, a comma-separated list, 'any', or 'anyone'
    #[arg(long, value_name = "WHO")]
    publishers: Option<String>,
    /// Who may comment: 'anyone' (default), 'any' GitHub account, or a list of logins
    #[arg(long, value_name = "WHO")]
    commenters: Option<String>,
    /// No public front page: the examples are listed only to their owner
    #[arg(long)]
    no_listing: bool,
    /// Largest document accepted, in megabytes (default 4)
    #[arg(long, value_name = "MB", default_value_t = 0)]
    max_size: usize,
    /// Most the figures of one document may come to, in megabytes (default 32)
    #[arg(long, value_name = "MB", default_value_t = 0)]
    max_assets: i64,
    /// Most one publisher may store across their documents, in megabytes (default 100)
    #[arg(long, value_name = "MB", default_value_t = 0)]
    quota: i64,
    /// Most the whole deployment will store, in megabytes (default 5120)
    #[arg(long, value_name = "MB", default_value_t = 0)]
    storage: i64,
    /// Most documents one publisher may hold (default 50)
    #[arg(long, value_name = "N", default_value_t = 0)]
    max_documents: i64,
    /// Most uploads one publisher may make in an hour (default 30)
    #[arg(long, value_name = "N", default_value_t = 0)]
    uploads_per_hour: i64,
    /// Minutes of quiet before a document is checkpointed (default 5)
    #[arg(long, value_name = "MINUTES", default_value_t = 0)]
    checkpoint: i64,
    /// Most checkpoints one document keeps; 0 keeps only the current text
    #[arg(long, value_name = "N")]
    history: Option<usize>,
    /// Delete documents after this duration, for example 24h or 30d (default never)
    #[arg(long, value_name = "DURATION")]
    expire_after: Option<String>,
    /// Start expiry at 'updated' (default; last publication) or 'created'
    #[arg(long, value_name = "FROM")]
    expire_from: Option<String>,
    /// Serve LaTeX distributions from this https bucket or directory; bare
    /// --latex uses the project's own mirror. Without it, .tex documents are
    /// stored and read but nothing compiles them.
    #[arg(
        long,
        value_name = "URL-OR-DIR",
        num_args = 0..=1,
        default_missing_value = crate::latex::DEFAULT_MIRROR
    )]
    latex: Option<String>,
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
enum Command {
    /// Sign in through a deployment, in a browser
    Login {
        /// Deployment URL; defaults to $KOMODOC_SERVER
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
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
        /// Deployment URL; defaults to $KOMODOC_SERVER
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Run the service on this machine
    Serve {
        /// Port to listen on; default is the first free one from 8080 to 8099
        #[arg(long, value_name = "PORT", default_value_t = 0)]
        port: u16,
        #[command(flatten)]
        service: ServiceFlags,
        #[command(flatten)]
        storage: StorageFlags,
    },
    /// List your documents
    List {
        /// Deployment URL; defaults to $KOMODOC_SERVER
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Open a document for commenting
    Comment {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// A share link, or the key from one: act as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Edit a markdown or typst document, with live preview
    Edit {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// A share link, or the key from one: act as its holder rather than
        /// as your sign-in
        #[arg(long, value_name = "LINK")]
        key: Option<String>,
        #[arg(long, value_name = "URL")]
        server: Option<String>,
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Hand a document, its history and its quota to another account
    Transfer {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The GitHub login to hand it to
        to: String,
        #[arg(long, value_name = "URL")]
        server: Option<String>,
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Name a checkpoint, so it stands out in the timeline
    Label {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// The checkpoint, as the digest `history` prints or the start of it
        sha: String,
        /// What to call it; omit to take an existing name away
        text: Option<String>,
        #[arg(long, value_name = "URL")]
        server: Option<String>,
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Replace local or remote data with the example documents
    Seed {
        #[command(flatten)]
        storage: StorageFlags,
        /// Deployment URL to wipe and fill instead of local storage
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Delete one document and its comments
    Destroy {
        /// The document to delete, by ID or slug
        #[arg(long, value_name = "ID")]
        document: String,
        #[arg(long, value_name = "URL")]
        server: Option<String>,
        /// Skip the confirmation prompt (dangerous)
        #[arg(long)]
        yes: bool,
    },
}

#[tokio::main]
pub async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Login { server } => cli::login(server.unwrap_or_default()).await,
        Command::Logout => cli::logout(),
        Command::Publish {
            file,
            title,
            slug,
            main,
            server,
        } => {
            cli::publish(
                &file,
                title.unwrap_or_default(),
                slug.unwrap_or_default(),
                server.unwrap_or_default(),
                main.unwrap_or_default(),
            )
            .await
        }
        Command::Serve {
            port,
            service,
            storage,
        } => {
            let config = service.configuration();
            serve::serve(serve::ServeOptions {
                port,
                storage: storage.options(),
                client_id: service.client_id.unwrap_or_default(),
                client_secret: service.client_secret.unwrap_or_default(),
                publishers: service.publishers.unwrap_or_default(),
                commenters: service.commenters.unwrap_or_default(),
                no_listing: service.no_listing,
                expire_after: service.expire_after.unwrap_or_default(),
                expire_from: service.expire_from.unwrap_or_default(),
                latex: service.latex.unwrap_or_default(),
                config,
            })
            .await
        }
        Command::List { server } => cli::list_documents(server.unwrap_or_default()).await,
        Command::Comment { id, key, server } => {
            cli::comment_document(&id, server.unwrap_or_default(), key.unwrap_or_default()).await
        }
        Command::Edit { id, key, server } => {
            cli::edit_document(&id, server.unwrap_or_default(), key.unwrap_or_default()).await
        }
        Command::Sync {
            id,
            file,
            interval,
            key,
            server,
        } => {
            sync::sync_document(
                &id,
                &file,
                server.unwrap_or_default(),
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
            server,
        } => {
            cli::share_document(
                &id,
                server.unwrap_or_default(),
                link.unwrap_or_default(),
                until.unwrap_or_default(),
                label,
                budget,
                revoke.unwrap_or_default(),
            )
            .await
        }
        Command::Transfer {
            id,
            to,
            server,
            yes,
        } => cli::transfer_document(&id, &to, server.unwrap_or_default(), yes).await,
        Command::History { id, key, server } => {
            cli::history_document(&id, server.unwrap_or_default(), key.unwrap_or_default()).await
        }
        Command::Diff {
            id,
            from,
            to,
            key,
            server,
        } => {
            cli::diff_document(
                &id,
                &from,
                &to,
                server.unwrap_or_default(),
                key.unwrap_or_default(),
            )
            .await
        }
        Command::Restore {
            id,
            sha,
            key,
            server,
        } => {
            cli::restore_document(
                &id,
                &sha,
                server.unwrap_or_default(),
                key.unwrap_or_default(),
            )
            .await
        }
        Command::Label {
            id,
            sha,
            text,
            server,
        } => {
            cli::label_checkpoint(
                &id,
                &sha,
                text.unwrap_or_default(),
                server.unwrap_or_default(),
            )
            .await
        }
        Command::Export {
            id,
            format,
            since,
            out,
            key,
            server,
        } => {
            export::export_document(
                &id,
                server.unwrap_or_default(),
                &format,
                out.unwrap_or_default(),
                since.unwrap_or_default(),
                key.unwrap_or_default(),
            )
            .await
        }
        Command::Seed { storage, server } => {
            let documents = seed_examples::seed_documents();
            match server {
                Some(server) if !server.is_empty() => seed::seed_remote(server, &documents).await,
                _ => seed::seed(storage.options(), &documents).await,
            }
        }
        Command::Destroy {
            document,
            server,
            yes,
        } => cli::destroy_document(&document, server.unwrap_or_default(), yes).await,
    }
}
