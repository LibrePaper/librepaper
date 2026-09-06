//! Komodoc: host HTML, markdown and typst documents that readers can
//! annotate. One binary: the server, and the command line that talks to it.

mod assets;
mod auth;
mod blob;
mod cli;
mod clock;
mod config;
mod export;
mod history;
mod http;
mod latex;
mod origins;
mod paths;
mod render;
mod retention;
mod room;
mod s3;
mod seed;
mod seed_examples;
mod serve;
mod server;
mod session;
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
    /// Never list documents on the front page, whatever a document asks for
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
        if let Err(err) = config.set_max_html(self.max_size) {
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Edit a markdown or typst document, with live preview
    Edit {
        /// A full slug, or one of the short handles `list` prints
        id: String,
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
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Show or change who a document is shared with
    Share {
        /// A full slug, or one of the short handles `list` prints
        id: String,
        /// Name a GitHub account as an editor: they may edit the source and
        /// the history, and delete any comment
        #[arg(long, value_name = "LOGIN")]
        editor: Option<String>,
        /// Name a GitHub account as a commenter
        #[arg(long, value_name = "LOGIN")]
        commenter: Option<String>,
        /// Mint a link carrying a role: 'commenter' or 'editor'. Its key is
        /// printed once and never again
        #[arg(long, value_name = "ROLE")]
        link: Option<String>,
        /// What to call the new link, for telling reviewers apart
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
        /// When the new link stops working, such as 180d; 'never' for no expiry
        #[arg(long, value_name = "DURATION")]
        until: Option<String>,
        /// Who may read: 'link' (default), 'private', or 'listed'
        #[arg(long, value_name = "WHO")]
        visibility: Option<String>,
        /// Take away a grant: a GitHub login, or a link's id
        #[arg(long, value_name = "WHO")]
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
async fn main() {
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
        Command::Comment { id, server } => {
            cli::comment_document(&id, server.unwrap_or_default()).await
        }
        Command::Edit { id, server } => cli::edit_document(&id, server.unwrap_or_default()).await,
        Command::Sync {
            id,
            file,
            interval,
            server,
        } => {
            sync::sync_document(
                &id,
                &file,
                server.unwrap_or_default(),
                interval.unwrap_or_default(),
            )
            .await
        }
        Command::Share {
            id,
            editor,
            commenter,
            link,
            label,
            until,
            visibility,
            revoke,
            server,
        } => {
            cli::share_document(
                &id,
                server.unwrap_or_default(),
                editor.unwrap_or_default(),
                commenter.unwrap_or_default(),
                link.unwrap_or_default(),
                label.unwrap_or_default(),
                until.unwrap_or_default(),
                visibility.unwrap_or_default(),
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
        Command::History { id, server } => {
            cli::history_document(&id, server.unwrap_or_default()).await
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
            server,
        } => {
            export::export_document(
                &id,
                server.unwrap_or_default(),
                &format,
                out.unwrap_or_default(),
                since.unwrap_or_default(),
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
