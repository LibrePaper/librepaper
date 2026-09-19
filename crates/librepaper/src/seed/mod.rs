//! Seeding fills an empty data directory with the example documents, so
//! there is something to look at without signing in and uploading by hand.
//!
//! It writes to the store directly rather than over HTTP: it is a development
//! command, run against a directory rather than a deployment, and going
//! through the API would mean holding a GitHub token to talk to your own
//! laptop.

pub mod examples;

pub(crate) const ACCOUNT_EXAMPLE_COUNT: usize = 5;

use std::path::Path;
use std::sync::Arc;

use crate::config::Configuration;
use crate::document::render::document_format;
use crate::document::store::{example_suffix, slugify, DocumentInput, Store};

use crate::storage::{open_storage, StorageOptions};
#[cfg(not(test))]
use crate::util::die;

#[cfg(test)]
fn die(message: impl std::fmt::Display) -> ! {
    panic!("{message}");
}

#[derive(Clone, Debug)]
pub struct SeedDocument {
    pub file: String,
    pub files: Vec<String>,
    pub assets: Vec<String>,
    pub title: &'static str,
}

fn seed_main(document: &SeedDocument) -> String {
    std::path::Path::new(&document.file)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| die(format!("{} has no file name", document.file)))
        .to_string()
}

fn seed_text_files(document: &SeedDocument) -> Vec<(String, Vec<u8>)> {
    let root = std::path::Path::new(&document.file)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    document
        .files
        .iter()
        .map(|path| {
            let disk = root.join(path);
            let bytes = std::fs::read(&disk)
                .unwrap_or_else(|err| die(format!("could not read {}: {err}", disk.display())));
            (path.clone(), bytes)
        })
        .collect()
}

fn seed_assets(document: &SeedDocument) -> Vec<(String, Vec<u8>)> {
    let root = std::path::Path::new(&document.file)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    document
        .assets
        .iter()
        .map(|path| {
            let disk = root.join(path);
            let bytes = std::fs::read(&disk)
                .unwrap_or_else(|err| die(format!("could not read {}: {err}", disk.display())));
            (path.clone(), bytes)
        })
        .collect()
}

/// A seed document's source and format, read from the checked-in tutorial.
pub fn read_seed_document(document: &SeedDocument) -> (String, String) {
    let raw = std::fs::read_to_string(&document.file).unwrap_or_else(|err| {
        die(format!(
            "could not read {}: {err}\n\n  Run `make examples` first, which renders them.",
            document.file
        ))
    });
    let format = match document_format(&document.file) {
        Some("markdown") => "markdown",
        Some("typst") => "typst",
        Some("latex") => "latex",
        // HTML and unknown examples are their own source through the identity
        // renderer.
        _ => "html",
    };
    (raw, format.into())
}

pub mod activity;

pub async fn seed_with_backup(
    options: StorageOptions,
    owner: &str,
    documents: &[SeedDocument],
    backup: Option<&std::path::Path>,
    // Days of invented history to write for each example, if the operator
    // asked for any. See `seed::activity`: the operations are real and only
    // the clock is simulated.
    simulate_activity: Option<u32>,
) {
    let blobs = open_storage(options.clone())
        .await
        .unwrap_or_else(|e| die(e));
    let mut pg = crate::storage::postgres::PostgresOptions::new(&options.database_url);
    pg.max_connections = options.database_connections;
    let catalog = Arc::new(
        crate::storage::postgres::PostgresCatalog::connect(pg)
            .await
            .unwrap_or_else(|e| die(e)),
    );
    catalog.migrate().await.unwrap_or_else(|e| die(e));
    let count = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM documents"#)
        .fetch_one(catalog.pool())
        .await
        .unwrap_or_else(|e| die(e));
    if count > 0 && backup.is_none() {
        die("refusing to reset a nonempty deployment without --backup <verified-point>")
    }
    if count > 0 {
        sqlx::query!("TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,bundle_files,bundles,document_versions,document_assets,replies,annotations,share_links,grants,documents,accounts CASCADE").execute(catalog.pool()).await.unwrap_or_else(|e|die(e));
    }
    let handle = if owner.trim().is_empty() {
        "examples"
    } else {
        owner.trim()
    };
    let account = catalog
        .create_account(crate::storage::postgres::NewAccount {
            kind: "system".into(),
            provider: None,
            provider_subject: None,
            handle: handle.into(),
            display_name: if owner.is_empty() {
                "Examples".into()
            } else {
                owner.into()
            },
            email: None,
        })
        .await
        .unwrap_or_else(|e| die(e));
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        Store::open_with_catalog(blobs.clone(), config, catalog.clone())
            .await
            .unwrap_or_else(|e| die(e)),
    );
    for document in documents {
        let (source, format) = read_seed_document(document);
        let main = seed_main(document);
        let mut files = seed_text_files(document);
        files.retain(|(path, _)| path != &main);
        files.extend(seed_assets(document));
        let slug = format!(
            "{}-{}",
            slugify(document.title, &store.config),
            example_suffix(document.title, &store.config)
        );
        let bundle = DocumentInput {
            slug: slug.clone(),
            title: document.title.into(),
            source,
            source_format: format,
            main,
        };
        let actor = crate::document::store::MutationActor {
            account_id: account.id.to_string(),
            owner_key: String::new(),
            session_generation: account.session_generation.to_string(),
            link_hash: String::new(),
            policy_editor: true,
            unowned_publisher: false,
        };
        let entry = store
            .put_directory_as_actor(bundle, files, actor)
            .await
            .unwrap_or_else(|e| die(e));
        println!("  {:<28} {}", slug, document.title);
        if let Some(days) = simulate_activity {
            let id = uuid::Uuid::parse_str(&entry.storage_id).unwrap_or_else(|_| die("bad id"));
            match activity::simulate(catalog.clone(), blobs.clone(), id, &slug, days).await {
                Ok(written) => println!(
                    "  {:<28} written in {} steps and {} versions over {} minutes, from {}",
                    "",
                    written.steps,
                    written.versions,
                    written.buckets,
                    written.first.date()
                ),
                Err(error) => eprintln!("  {:<28} activity not simulated: {error}", ""),
            }
        }
    }
    catalog.close().await;
}
