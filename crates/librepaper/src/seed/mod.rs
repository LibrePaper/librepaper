//! Seeding fills an empty data directory with the example documents and a
//! handful of annotations on each, so there is something to look at without
//! signing in and uploading by hand.
//!
//! It writes to the store directly rather than over HTTP: it is a development
//! command, run against a directory rather than a deployment, and going
//! through the API would mean holding a GitHub token to talk to your own
//! laptop.

pub mod examples;

pub(crate) const ACCOUNT_EXAMPLE_COUNT: usize = 5;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::{json, Value};

use crate::auth::{link_sealing_keyring_file, session_key_file};
use crate::cli::{server_or_die, stored_token_for};
use crate::config::Configuration;
use crate::document::render::{is_latex, is_markdown, is_typst, render_markdown_document};
use crate::document::store::{example_suffix, slugify, Publication, Store};
use crate::http::{detail_of, get_json, post_directory, post_json, text};
use crate::room::{Comment, Message, Region, Reply, Room, RoomSet, SourceAnchor};
use crate::storage::backup::verify_local_backup;
use crate::storage::blob::{clear_storage_checked, release_room_locks};
use crate::storage::{open_storage, StorageOptions};
#[cfg(not(test))]
use crate::util::die;
use crate::util::new_id;
use crate::util::timestamp;

#[cfg(test)]
fn die(message: impl std::fmt::Display) -> ! {
    panic!("{message}");
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SeedAnnotation {
    pub motivation: &'static str,
    /// The passage to anchor to. It has to appear in the rendered document,
    /// and it has to appear once: prefix and suffix are computed from wherever
    /// it is found.
    pub exact: &'static str,
    pub body: &'static str,
    pub creator: &'static str,
    pub resolved: bool,
    pub replies: Vec<&'static str>,
    /// Annotates part of a figure instead of a passage, given as percentages
    /// of the image.
    pub region: Option<Region>,
}

#[derive(Clone, Debug)]
pub struct SeedDocument {
    pub file: String,
    pub files: Vec<String>,
    pub assets: Vec<String>,
    pub title: &'static str,
    pub annotations: Vec<SeedAnnotation>,
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

/// What to anchor the annotations against, the source to store, and its
/// format. Rendered text exists only long enough to place seed annotations;
/// generated output is never uploaded or stored.
pub fn read_seed_document(document: &SeedDocument) -> (String, String, String) {
    let raw = std::fs::read_to_string(&document.file).unwrap_or_else(|err| {
        die(format!(
            "could not read {}: {err}\n\n  Run `make examples` first, which renders them.",
            document.file
        ))
    });
    if is_markdown(&document.file) {
        return (
            render_markdown_document(&raw, document.title),
            raw,
            "markdown".into(),
        );
    }
    if is_typst(&document.file) {
        return (raw.clone(), raw, "typst".into());
    }
    if is_latex(&document.file) {
        return (latex_prose(&raw), raw, "latex".into());
    }
    // An HTML example is its own source, through the identity renderer, and is
    // as editable as the other two.
    (raw.clone(), raw, "html".into())
}

/// The words of a LaTeX source, near enough: comments dropped, and whitespace
/// collapsed the way `visible_text` collapses it. Macros stay, so a seeded
/// passage has to be plain prose -- a run of words with no `\cite` or `\emph`
/// inside it -- which is also what survives the trip through pdf.js on the
/// other side.
pub fn latex_prose(source: &str) -> String {
    let comment = regex::Regex::new(r"(?m)(^|[^\\])%.*$").expect("pattern");
    let space = regex::Regex::new(r"\s+").expect("pattern");
    let text = comment.replace_all(source, "$1");
    space.replace_all(&text, " ").to_string()
}

pub async fn seed(options: StorageOptions, owner: &str, documents: &[SeedDocument]) {
    seed_with_backup(options, owner, documents, None).await;
}

pub async fn seed_with_backup(
    options: StorageOptions,
    owner: &str,
    documents: &[SeedDocument],
    backup: Option<&std::path::Path>,
) {
    let paths = options.paths().unwrap_or_else(|err| die(err));
    let blobs = open_storage(options.clone())
        .await
        .unwrap_or_else(|err| die(err));
    let config = Arc::new(Configuration::default());
    let lock = crate::server::serve::acquire_writer_lock(&paths.writer_lock)
        .unwrap_or_else(|err| die(err));
    let catalog_path = &paths.catalog;
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open_with(catalog_path, options.fsync)
            .unwrap_or_else(|err| die(format!("could not open catalogue: {err}"))),
    );
    let catalog_nonempty = catalog
        .totals()
        .map(|(_, documents)| documents != 0)
        .unwrap_or_else(|err| die(format!("could not inspect catalogue: {err}")));
    let deployment_id = paths
        .ensure_deployment_identity(catalog_nonempty)
        .unwrap_or_else(|err| die(err));
    if catalog_nonempty {
        let backup = backup.unwrap_or_else(|| {
            die("refusing to reset a nonempty deployment without --backup <verified-point>")
        });
        let manifest = verify_local_backup(backup)
            .unwrap_or_else(|err| die(format!("seed backup verification failed: {err}")));
        if manifest.deployment_id != deployment_id {
            die("seed backup belongs to a different deployment identity");
        }
        let current_schema: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get(0))
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap_or_else(|err| die(format!("could not read catalogue schema: {err}")));
        let current_revision: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT catalog_revision FROM server_state WHERE id=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap_or_else(|err| die(format!("could not read catalogue revision: {err}")));
        if manifest.schema_version != current_schema || manifest.head_revision != current_revision {
            die("seed backup is not an exact verified point for this deployment");
        }
        let current_catalog_digest =
            crate::storage::backup::catalog_snapshot_digest(&catalog, catalog_path)
                .unwrap_or_else(|err| die(format!("could not verify current catalogue: {err}")));
        if manifest.catalog.digest != current_catalog_digest {
            die("seed backup is not fresh for the current catalogue state");
        }
    }
    let secrets = &paths.secrets;
    let link_keys = link_sealing_keyring_file(&secrets.join("links.key"), catalog_nonempty)
        .unwrap_or_else(|err| die(err));
    session_key_file(&secrets.join("session.key"), catalog_nonempty).unwrap_or_else(|err| die(err));
    catalog
        .set_link_sealing_key(&link_keys[0])
        .unwrap_or_else(|err| die(format!("could not configure link sealing: {err}")));
    let marker = paths.state.join("seed-reset.json");
    let marker_body = serde_json::json!({
        "deployment_id": deployment_id,
        "backup": backup.map(|path| path.display().to_string()),
        "stage": "prepared",
    });
    write_seed_marker(&marker, &marker_body)
        .unwrap_or_else(|err| die(format!("could not write seed reset marker: {err}")));
    seed_into_catalog(blobs, config, owner, documents, catalog, &marker).await;
    update_seed_marker(&marker, "seeded")
        .unwrap_or_else(|err| die(format!("could not update seed reset marker: {err}")));
    let _ = std::fs::remove_file(marker);
    drop(lock);
}

fn reset_catalog(catalog: &crate::storage::catalog::Catalog) -> Result<(), String> {
    catalog
        .with_connection(|connection| {
            // Foreign-key-safe order.  The seed command is explicitly a
            // destructive replacement of the demonstration deployment.
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     UPDATE documents SET current_checkpoint_id=NULL,
                         journal_base_object_id=NULL, publication_object_id=NULL,
                         publication_id=NULL, published_at=NULL;
                     DELETE FROM object_leases;
                     DELETE FROM checkpoint_objects;
                     DELETE FROM replies;
                     DELETE FROM annotations;
                     DELETE FROM checkpoints;
                     DELETE FROM grants;
                     DELETE FROM links;
                     DELETE FROM objects;
                     DELETE FROM operations;
                     DELETE FROM documents;
                     DELETE FROM accounts;
                     UPDATE server_state SET catalog_revision=0,
                         stored_bytes=0, reserved_bytes=0, document_count=0,
                         agent_payload_bytes=0, agent_payload_count=0,
                         checkpoint_ref_count=0, updated_at=0 WHERE id=1;
                     COMMIT;",
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .map_err(|err| format!("could not reset catalogue before seeding: {err}"))
}

/// Seeds a blob store directly, with no catalogue in front of it. Starts
/// from nothing: seeding is for looking at the result, not for adding to
/// whatever was there. Test-only, for cases that want the example documents
/// without paying for a catalogue and a writer lock; `seed_with_backup` is
/// what `librepaper admin seed` actually runs.
///
/// `owner` is the account handle or visitor key the examples belong to, or
/// "" for nobody. Ownerless examples can be read and commented on, but nobody
/// can edit or share them. Account onboarding creates separately owned copies
/// through the ordinary sign-in flow.
#[cfg(test)]
pub async fn seed_into(
    blobs: Arc<dyn crate::storage::blob::BlobStore>,
    config: Arc<Configuration>,
    owner: &str,
    documents: &[SeedDocument],
) {
    clear_storage_checked(blobs.as_ref())
        .await
        .unwrap_or_else(|err| {
            die(format!(
                "could not clear object storage before seeding: {err}"
            ))
        });
    seed_with_store(blobs, config, owner, documents, None).await;
}

async fn seed_into_catalog(
    blobs: Arc<dyn crate::storage::blob::BlobStore>,
    config: Arc<Configuration>,
    owner: &str,
    documents: &[SeedDocument],
    catalog: Arc<crate::storage::catalog::Catalog>,
    marker: &Path,
) {
    update_seed_marker(marker, "clearing-objects")
        .unwrap_or_else(|err| die(format!("could not update seed reset marker: {err}")));
    clear_storage_checked(blobs.as_ref())
        .await
        .unwrap_or_else(|err| {
            die(format!(
                "could not clear object storage before seeding: {err}"
            ))
        });
    update_seed_marker(marker, "objects-cleared")
        .unwrap_or_else(|err| die(format!("could not update seed reset marker: {err}")));
    // Physical cleanup is the first half of the destructive reset. Only
    // after every object has a confirmed absent outcome do we remove its
    // catalogue row and release its counters. This covers allocated PUTs as
    // well as acknowledged objects.
    reset_catalog(&catalog).unwrap_or_else(|err| die(err));
    update_seed_marker(marker, "catalog-reset")
        .unwrap_or_else(|err| die(format!("could not update seed reset marker: {err}")));
    seed_with_store(blobs, config, owner, documents, Some(catalog)).await;
}

/// The local examples are administrative products.  Give them the durable
/// v2 system owner rather than creating an anonymous publishing account for
/// every fresh seed.  An explicit `--owner` remains the legacy visitor/account
/// ownership mode and is intentionally left alone.
async fn ensure_system_seed_account(
    catalog: &Arc<crate::storage::catalog::Catalog>,
) -> Result<String, String> {
    const ID: &str = "system:examples";
    let kind = catalog
        .with_connection(|connection| {
            connection
                .query_row("SELECT kind FROM accounts WHERE id=?1", [ID], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .map_err(|error| error.to_string())?;
    if let Some(kind) = kind {
        if kind != "system" {
            return Err(format!("reserved seed account {ID} has kind {kind:?}"));
        }
        return Ok(ID.to_string());
    }
    let preferences = serde_json::to_string(&crate::document::quota::QuotaPreferences::default())
        .map_err(|error| error.to_string())?;
    catalog
        .create_v2_account(
            &crate::storage::catalog::V2AccountInput {
                id: ID.to_string(),
                kind: crate::storage::catalog::AccountKind::System,
                provider: None,
                provider_subject: None,
                handle: "examples".into(),
                display_name: "Examples".into(),
                email: None,
                plan: "default".into(),
                session_generation: hex::encode(crate::auth::random_bytes(16)),
                preferences_json: preferences,
                bookmarks_json: "{\"version\":1,\"items\":[]}".into(),
                onboarding_json: "{\"version\":1,\"items\":[]}".into(),
            },
            crate::storage::catalog::UnixMillis::now(),
        )
        .map_err(|error| error.to_string())?;
    Ok(ID.to_string())
}

fn write_seed_marker(path: &Path, body: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "seed reset marker has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(body).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    let file = std::fs::File::open(&temporary).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    crate::config::DeploymentPaths::protect_file(path)?;
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())
}

fn update_seed_marker(path: &Path, stage: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut body: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    body["stage"] = Value::String(stage.to_string());
    write_seed_marker(path, &body)
}

async fn seed_with_store(
    blobs: Arc<dyn crate::storage::blob::BlobStore>,
    config: Arc<Configuration>,
    owner: &str,
    documents: &[SeedDocument],
    catalog: Option<Arc<crate::storage::catalog::Catalog>>,
) {
    let system_owner_id = match &catalog {
        Some(catalog) if owner.trim().is_empty() => Some(
            ensure_system_seed_account(catalog)
                .await
                .unwrap_or_else(|error| die(format!("could not initialize seed owner: {error}"))),
        ),
        _ => None,
    };
    let store = match catalog.clone() {
        Some(catalog) => Store::open_with_catalog(blobs.clone(), config.clone(), catalog)
            .await
            .unwrap_or_else(|err| die(err)),
        None => die("catalog-backed seed requires the deployment catalogue"),
    };
    let store = Arc::new(store);
    let seed_actor = if let Some(owner_id) = &system_owner_id {
        let catalog = catalog
            .as_ref()
            .expect("system seed ownership requires a catalogue");
        let account = catalog
            .account(owner_id)
            .unwrap_or_else(|error| die(format!("could not read seed owner: {error}")))
            .unwrap_or_else(|| die("seed owner disappeared"));
        crate::document::store::MutationActor {
            account_id: account.id,
            owner_key: String::new(),
            session_generation: account.session_generation,
            link_hash: String::new(),
            policy_editor: true,
            automation: false,
            unowned_publisher: false,
        }
    } else {
        crate::document::store::MutationActor {
            account_id: String::new(),
            owner_key: owner.trim().to_lowercase(),
            session_generation: String::new(),
            link_hash: String::new(),
            policy_editor: true,
            automation: false,
            unowned_publisher: owner.trim().is_empty(),
        }
    };
    let rooms = RoomSet::new(blobs.clone(), config.clone());
    rooms.attach_store(store.clone());
    if let Some(catalog) = &store.catalog {
        let journal_catalog = Arc::new(
            crate::storage::v2_catalog::V2JournalCatalogAdapter::with_limits_and_quota(
                catalog.clone(),
                config.persistence(),
                config.storage.per_owner,
                config.storage.total,
            ),
        );
        let journal = Arc::new(
            crate::storage::journal::V2JournalRuntime::with_persistence(
                journal_catalog,
                blobs.clone(),
                config.persistence(),
            )
            .unwrap_or_else(|err| die(format!("could not initialize v2 journal: {err}"))),
        );
        rooms.attach_journal(journal);
    }
    let mut seeded = Vec::new();

    for document in documents {
        // Rendered in memory, and not stored: the rendering is here only to
        // anchor the example annotations against the text a browser will
        // show, exactly as `visible_text` did when the HTML was stored.
        let (raw, source, format) = read_seed_document(document);
        let base = slugify(document.title, &config);
        let slug = format!("{base}-{}", example_suffix(&base, &config));
        let main = seed_main(document);
        let entry = store
            .put_as_actor(
                Publication {
                    slug: slug.clone(),
                    title: document.title.to_string(),
                    source: source.clone(),
                    source_format: format.clone(),
                    main: main.clone(),
                    owner: owner.trim().to_lowercase(),
                    owner_id: String::new(),
                    owner_name: system_owner_id
                        .as_ref()
                        .map(|_| "Examples".to_string())
                        .unwrap_or_else(|| owner.trim().to_string()),
                    ..Publication::default()
                },
                seed_actor.clone(),
            )
            .await
            .unwrap_or_else(|err| die(format!("could not store {}: {err}", document.file)));

        // A named seed owner is anonymous at the request boundary, but v2
        // persists that stable owner key as an anonymous account. Resolve the
        // account and its live generation before any catalog-backed asset or
        // checkpoint mutation. Carrying the original owner key as authority
        // would make those writes look like an unauthenticated caller.
        let catalog_actor = if store.catalog.is_some() && seed_actor.account_id.is_empty() {
            let catalog = store
                .catalog
                .as_ref()
                .expect("catalog actor resolution requires a catalogue");
            let owner_id = catalog
                .document(&slug)
                .unwrap_or_else(|error| die(format!("could not read seeded owner: {error}")))
                .and_then(|document| document.owner_id)
                .unwrap_or_else(|| die("seeded anonymous document has no owner account"));
            let account = catalog
                .account(&owner_id)
                .unwrap_or_else(|error| {
                    die(format!("could not read seeded owner account: {error}"))
                })
                .unwrap_or_else(|| die("seeded anonymous owner account disappeared"));
            crate::document::store::MutationActor {
                account_id: account.id,
                owner_key: String::new(),
                session_generation: account.session_generation,
                link_hash: String::new(),
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
            }
        } else {
            seed_actor.clone()
        };

        let text = visible_text(&raw);
        let room = rooms.get(&slug).await;
        // A seed that could not write its source has nothing to checkpoint;
        // stopping here is what keeps a half-seeded document out of the
        // catalogue.
        if let Err(error) = room.set_main_file(&source, &format, &main).await {
            die(format!("could not store {}: {error}", document.file));
        }
        for (path, bytes) in seed_text_files(document) {
            let body = std::str::from_utf8(&bytes)
                .unwrap_or_else(|err| die(format!("{path} is not UTF-8: {err}")));
            room.add_text(&path, body)
                .await
                .unwrap_or_else(|err| die(format!("could not store {path}: {err}")));
        }
        let assets = seed_assets(document);
        for (path, bytes) in &assets {
            let (sha, _) = room
                .put_asset_authorized(
                    bytes.clone(),
                    (config.max_asset, config.max_assets),
                    &catalog_actor,
                )
                .await
                .unwrap_or_else(|err| die(format!("could not store {path}: {err}")));
            room.name_asset(&path, &sha)
                .await
                .unwrap_or_else(|err| die(format!("could not name {path}: {err}")));
        }
        // Seeded and imported documents have no authenticated caller behind
        // them: the operator ran a command. There is no account to record,
        // and the empty display name is the one this path has always written.
        let checkpoint_authority = crate::storage::catalog::MutationAuthority {
            account_id: &catalog_actor.account_id,
            owner_key: &catalog_actor.owner_key,
            generation: &catalog_actor.session_generation,
            link_hash: &catalog_actor.link_hash,
            policy_editor: catalog_actor.policy_editor,
            automation: catalog_actor.automation,
            unowned_publisher: catalog_actor.unowned_publisher,
            execution_epoch: "",
            agent_checkpoint: None,
        };
        let checkpoint_display = if seed_actor.account_id.is_empty() {
            owner.trim()
        } else if system_owner_id.is_some() {
            "Examples"
        } else {
            owner.trim()
        };
        let _checkpoint = match room
            .checkpoint_now_with_authority(
                "cli",
                crate::room::Attribution::account(&catalog_actor.account_id, checkpoint_display),
                checkpoint_authority,
            )
            .await
        {
            Ok(Some(sha)) => sha,
            Ok(None) => die(format!(
                "could not store {}: checkpoint deferred",
                document.file
            )),
            Err(err) => die(format!("could not store {}: {err}", document.file)),
        };
        if store.catalog.is_some() {
            publish_seed_display(
                &store,
                &entry.storage_id,
                &catalog_actor,
                &source,
                &format,
                &raw,
                &assets,
            )
            .await
            .unwrap_or_else(|error| die(format!("could not publish {}: {error}", document.file)));
        }
        let (placed, missed) =
            seed_annotations(&room, &document.annotations, &text, &source, &main).await;
        seeded.push(slug.clone());

        println!("  {:<28} {}", entry.slug, document.title);
        print!("      {placed} annotation(s)");
        if missed > 0 {
            // Worth saying out loud: a phrase that is not in the rendered
            // document anchors nowhere, and the seed is meant to look right.
            print!(", {missed} could not be anchored");
        }
        println!();
    }
    // The seeding is done, so nothing here is writing these rooms any more.
    release_room_locks(blobs.as_ref(), &seeded).await;
}

/// Gives a deployment the same curated titles and annotations as the local
/// seed. It deliberately replaces everything already there: a seed is a known
/// demonstration state, not an additive publishing operation.
pub async fn seed_remote(server_flag: String, token: Option<&str>, documents: &[SeedDocument]) {
    let server = server_or_die(Some(server_flag));
    let token = stored_token_for(&server, token);
    let examples_enabled =
        match get_json(&format!("{server}/api/me"), Duration::from_secs(30)).await {
            Ok((200, capabilities)) => capabilities
                .get("examples_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            _ => false,
        };

    let (status, listing) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &token,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "could not list the deployment before seeding ({status}): {}",
            detail_of(&listing)
        ));
    }
    if !examples_enabled {
        for document in listing
            .get("documents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let slug = text(&document, "slug");
            let (status, result) = post_json(
                &format!("{server}/api/documents/{slug}/delete"),
                &json!({}),
                &token,
                Duration::from_secs(120),
            )
            .await
            .unwrap_or_else(|err| die(err));
            if status != 200 {
                die(format!(
                    "could not remove {slug} before seeding ({status}): {}",
                    detail_of(&result)
                ));
            }
        }
    }

    println!("seeding {server}");
    let config = Configuration::default();
    for document in documents {
        let (raw, source, _format) = read_seed_document(document);
        let main = seed_main(document);
        let mut files = vec![(main.clone(), source.clone().into_bytes())];
        files.extend(seed_text_files(document));
        files.extend(seed_assets(document));
        let (status, uploaded) = post_directory(
            &format!("{server}/api/documents"),
            document.title,
            &slugify(document.title, &config),
            &main,
            files,
            &token,
            Duration::from_secs(300),
        )
        .await
        .unwrap_or_else(|err| die(err));
        if status != 201 {
            die(format!(
                "could not seed {} ({status}): {}",
                document.file,
                detail_of(&uploaded)
            ));
        }

        let slug = text(&uploaded, "slug");
        let (placed, missed) = if uploaded
            .get("example")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let visible = visible_text(&raw);
            let missed = document
                .annotations
                .iter()
                .filter(|a| a.region.is_none() && !visible.contains(a.exact))
                .count();
            (document.annotations.len() - missed, missed)
        } else {
            seed_remote_annotations(
                &server,
                &token,
                &slug,
                &document.annotations,
                &visible_text(&raw),
                &source,
                &main,
            )
            .await
        };
        println!("  {slug:<28} {}", document.title);
        print!("      {placed} annotation(s)");
        if missed > 0 {
            print!(", {missed} could not be anchored");
        }
        println!();
    }
}

async fn seed_remote_annotations(
    server: &str,
    token: &str,
    slug: &str,
    annotations: &[SeedAnnotation],
    visible: &str,
    source: &str,
    main_path: &str,
) -> (usize, usize) {
    let url = format!("{server}/api/documents/{slug}/comments");
    let (mut placed, mut missed) = (0, 0);
    for item in annotations {
        let Some(spot) = anchor(item, visible) else {
            missed += 1;
            continue;
        };
        let incoming = Message {
            kind: "comment".into(),
            motivation: item.motivation.into(),
            body: item.body.into(),
            creator: item.creator.into(),
            exact: item.exact.into(),
            prefix: spot.prefix,
            suffix: spot.suffix,
            position: spot.position,
            source: source_anchor(item, source, main_path),
            region: item.region.clone(),
            ..Message::default()
        };
        let (status, result) = post_json(&url, &json!(incoming), token, Duration::from_secs(60))
            .await
            .unwrap_or_else(|err| die(err));
        if status != 200 {
            die(format!(
                "could not seed an annotation on {slug} ({status}): {}",
                detail_of(&result)
            ));
        }
        let comment_id = result
            .get("comment")
            .map(|c| text(c, "id"))
            .unwrap_or_default();
        for body in &item.replies {
            let (status, reply) = post_json(
                &url,
                &json!({"type": "reply", "comment_id": comment_id, "body": body, "creator": "Reviewer"}),
                token,
                Duration::from_secs(60),
            )
            .await
            .unwrap_or_else(|err| die(err));
            if status != 200 {
                die(format!(
                    "could not seed a reply on {slug} ({status}): {}",
                    detail_of(&reply)
                ));
            }
        }
        if item.resolved {
            let (status, resolved) = post_json(
                &url,
                &json!({"type": "resolve", "comment_id": comment_id, "resolved": true}),
                token,
                Duration::from_secs(60),
            )
            .await
            .unwrap_or_else(|err| die(err));
            if status != 200 {
                die(format!(
                    "could not resolve a seeded annotation on {slug} ({status}): {}",
                    detail_of(&resolved)
                ));
            }
        }
        placed += 1;
    }
    (placed, missed)
}

/// Where a seeded annotation's passage sits in the document's visible text,
/// and the context stored either side of it. A region annotation is anchored
/// to the image instead and needs no passage; anything else whose passage is
/// not in the document cannot be placed.
pub struct SeedAnchor {
    pub prefix: String,
    pub suffix: String,
    pub position: Option<i64>,
}

pub fn anchor(item: &SeedAnnotation, text: &str) -> Option<SeedAnchor> {
    if item.region.is_some() {
        return Some(SeedAnchor {
            prefix: String::new(),
            suffix: String::new(),
            position: None,
        });
    }
    let at = text.find(item.exact)?;
    let context = Configuration::default().caps.context;
    Some(SeedAnchor {
        prefix: tail(&text[..at], context),
        suffix: head(&text[at + item.exact.len()..], context),
        position: Some(at as i64),
    })
}

/// Where a seeded annotation's passage sits in the source it was published
/// from, as opposed to the rendered page `anchor` above locates it in. Kept
/// only when the passage names one spot unambiguously: several matches would
/// leave no way to say which one the comment is about, and that is the same
/// rule the reader itself follows when a comment arrives with no position of
/// its own. A region annotation has no passage to look for.
pub fn source_anchor(item: &SeedAnnotation, source: &str, path: &str) -> Option<SourceAnchor> {
    if item.region.is_some() || source.matches(item.exact).count() != 1 {
        return None;
    }
    let at = source.find(item.exact)?;
    let context = Configuration::default().caps.context;
    Some(SourceAnchor {
        path: path.to_string(),
        exact: item.exact.to_string(),
        prefix: tail(&source[..at], context),
        suffix: head(&source[at + item.exact.len()..], context),
        position: Some(at as i64),
    })
}

fn seed_publication_mime(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "css" => "text/css",
        "csv" => "text/csv",
        "gif" => "image/gif",
        "html" => "text/html",
        "jpeg" | "jpg" => "image/jpeg",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "txt" | "md" => "text/plain",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn seed_display_html(raw: &str, format: &str) -> Vec<u8> {
    if matches!(format, "html" | "markdown") {
        raw.as_bytes().to_vec()
    } else {
        format!("<pre>{}</pre>", html_escape::encode_text(raw)).into_bytes()
    }
}

#[derive(Serialize)]
struct SeedBundleAsset<'a> {
    path: &'a str,
    sha256: &'a str,
    bytes: usize,
    mime: &'a str,
}

#[derive(Serialize)]
struct SeedBundle<'a> {
    html: &'a str,
    assets: Vec<SeedBundleAsset<'a>>,
}

async fn publish_seed_display(
    store: &Arc<Store>,
    storage_id: &str,
    actor: &crate::document::store::MutationActor,
    source: &str,
    format: &str,
    rendered: &str,
    assets: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let mut publication_actor = actor.clone();
    if publication_actor.account_id.is_empty() && !publication_actor.owner_key.is_empty() {
        let catalog = store
            .catalog
            .as_ref()
            .ok_or_else(|| "anonymous seed publication requires a catalogue".to_string())?;
        let storage = storage_id.to_owned();
        let slug = catalog
            .execute_catalog(storage.len(), move |catalog| {
                catalog.slug_by_storage_id(&storage)
            })
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "seed publication document disappeared".to_string())?;
        let slug_for_document = slug.clone();
        let document = catalog
            .execute_catalog(slug.len(), move |catalog| {
                catalog.document(&slug_for_document)
            })
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "seed publication document disappeared".to_string())?;
        let owner_id = document
            .owner_id
            .ok_or_else(|| "anonymous seed publication has no owner account".to_string())?;
        let owner = owner_id.clone();
        let account = catalog
            .execute_catalog(owner_id.len(), move |catalog| catalog.account(&owner))
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "anonymous seed publication owner disappeared".to_string())?;
        publication_actor.account_id = account.id;
        publication_actor.owner_key.clear();
        publication_actor.session_generation = account.session_generation;
    }
    let html = seed_display_html(rendered, format);
    let html_sha256 = crate::document::store::digest_of_bytes(&html);
    let mut publication_assets = assets
        .iter()
        .map(
            |(path, bytes)| crate::server::publication::PublicationAsset {
                path: path.clone(),
                object: crate::server::publication::PublicationObject {
                    sha256: crate::document::store::digest_of_bytes(bytes),
                    bytes: bytes.len(),
                    mime: seed_publication_mime(path).into(),
                },
            },
        )
        .collect::<Vec<_>>();
    publication_assets.sort_by(|left, right| left.path.cmp(&right.path));
    let bundle_assets = publication_assets
        .iter()
        .map(|asset| SeedBundleAsset {
            path: &asset.path,
            sha256: &asset.object.sha256,
            bytes: asset.object.bytes,
            mime: &asset.object.mime,
        })
        .collect::<Vec<_>>();
    let bundle = serde_json::to_vec(&SeedBundle {
        html: &html_sha256,
        assets: bundle_assets,
    })
    .map_err(|error| error.to_string())?;
    let manifest = crate::server::publication::PublicationManifest {
        publication_id: crate::storage::catalog::ObjectId::new(hex::encode(
            crate::auth::random_bytes(16),
        ))
        .expect("random publication id is valid")
        .to_string(),
        bundle_sha256: crate::document::store::digest_of_bytes(&bundle),
        source_sha256: crate::document::store::digest_of(source),
        render_config_sha256: crate::document::store::digest_of(format),
        published_at: timestamp(),
        publisher: if actor.account_id == "system:examples" {
            "Examples".into()
        } else if actor.owner_key.is_empty() {
            "Anonymous".into()
        } else {
            actor.owner_key.clone()
        },
        previous_publication_id: String::new(),
        html: crate::server::publication::PublicationObject {
            sha256: html_sha256,
            bytes: html.len(),
            mime: "text/html".into(),
        },
        assets: publication_assets,
    };
    let publication = crate::server::publication::PublicationStore::for_store(store.clone())
        .with_actor(publication_actor);
    let request_id = crate::util::new_request_key();
    let missing = publication
        .prepare(
            storage_id,
            &request_id,
            "",
            &manifest,
            crate::server::publication::MAX_STAGED_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?;
    for hash in missing.hashes {
        if hash == manifest.html.sha256 {
            publication
                .stage_object(
                    storage_id,
                    &request_id,
                    &hash,
                    false,
                    &html,
                    &manifest.html.mime,
                )
                .await
                .map_err(|error| error.to_string())?;
            continue;
        }
        let Some((asset, (_, bytes))) = manifest
            .assets
            .iter()
            .filter(|asset| asset.object.sha256 == hash)
            .find_map(|asset| {
                assets
                    .iter()
                    .find(|(_, bytes)| {
                        crate::document::store::digest_of_bytes(bytes) == asset.object.sha256
                    })
                    .map(|entry| (asset, entry))
            })
        else {
            return Err(format!("publication asset {hash} has no source bytes"));
        };
        publication
            .stage_object(
                storage_id,
                &request_id,
                &hash,
                false,
                bytes,
                &asset.object.mime,
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    publication
        .activate(storage_id, &request_id, "", &manifest)
        .await
        .map_err(|error| error.to_string())
}

/// Writes one document's annotations, anchoring each to where its passage
/// actually appears.
pub async fn seed_annotations(
    room: &Room,
    annotations: &[SeedAnnotation],
    text: &str,
    source: &str,
    main_path: &str,
) -> (usize, usize) {
    let (mut placed, mut missed) = (0, 0);
    for item in annotations {
        let Some(spot) = anchor(item, text) else {
            missed += 1;
            continue;
        };
        let mut written = Comment {
            id: new_id(),
            motivation: item.motivation.into(),
            exact: item.exact.into(),
            prefix: spot.prefix,
            suffix: spot.suffix,
            position: spot.position,
            source: source_anchor(item, source, main_path),
            region: item.region.clone(),
            body: item.body.into(),
            creator: item.creator.into(),
            created: timestamp(),
            ..Comment::default()
        };
        if item.resolved {
            written.resolved = true;
            written.resolved_at = Some(timestamp());
        }
        for answer in &item.replies {
            written.replies.push(Reply {
                id: new_id(),
                body: answer.to_string(),
                creator: "Reviewer".into(),
                created: timestamp(),
                author: String::new(),
            });
        }
        if let Err(err) = room.append_seed_comment(written).await {
            die(format!(
                "could not write the seeded comments for {}: {err}",
                room.slug
            ));
        }
        placed += 1;
    }
    (placed, missed)
}

#[cfg(test)]
mod catalog_seed_tests {
    use super::*;

    fn document(file: &std::path::Path) -> SeedDocument {
        SeedDocument {
            file: file.display().to_string(),
            files: Vec::new(),
            assets: vec!["asset.txt".into(), "style.css".into()],
            title: "Catalog seed fixture",
            annotations: vec![SeedAnnotation {
                motivation: "commenting",
                exact: "seed phrase",
                body: "seed comment",
                creator: "Seed display",
                replies: vec!["seed reply"],
                ..SeedAnnotation::default()
            }],
        }
    }

    #[tokio::test]
    async fn catalog_seed_uses_system_owner_and_publishes_typed_annotations() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("example.md");
        std::fs::write(&source, "seed phrase").unwrap();
        std::fs::write(root.path().join("asset.txt"), "seed asset").unwrap();
        std::fs::write(root.path().join("style.css"), "body { color: red; }").unwrap();
        let catalog = Arc::new(
            crate::storage::catalog::Catalog::open(root.path().join("catalog.db")).unwrap(),
        );
        let blobs = Arc::new(crate::storage::blob::FsStore::new(root.path(), true));
        seed_with_store(
            blobs.clone(),
            Arc::new(Configuration::default()),
            "",
            &[document(&source)],
            Some(catalog.clone()),
        )
        .await;
        let slug = format!(
            "catalog-seed-fixture-{}",
            example_suffix("catalog-seed-fixture", &Configuration::default())
        );
        let row = catalog.document(&slug).unwrap().unwrap();
        assert_eq!(row.owner_id.as_deref(), Some("system:examples"));
        assert!(row.example);
        assert_eq!(row.status, "active");
        let comment = catalog.comments(&slug, None, 10).unwrap().remove(0);
        assert_eq!(comment.creator, "Seed display");
        assert_eq!(catalog.replies(&slug, &comment.id, 10).unwrap().len(), 1);
        let reader = Arc::new(
            Store::open_with_catalog(
                blobs.clone(),
                Arc::new(Configuration::default()),
                catalog.clone(),
            )
            .await
            .unwrap(),
        );
        let publication = crate::server::publication::PublicationStore::for_store(reader);
        let manifest = publication
            .current(&row.storage_id)
            .await
            .unwrap()
            .expect("seed display publication");
        assert!(!manifest.publication_id.is_empty());
        assert_eq!(manifest.html.mime, "text/html");
        assert_eq!(manifest.assets.len(), 2);
        let (_, html) = publication
            .deliver(&row.storage_id, "index.html")
            .await
            .unwrap();
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("seed phrase"));
        assert!(
            html.contains("<p>"),
            "seed served source markdown instead of rendered HTML: {html}"
        );
        let (_, asset) = publication
            .deliver(&row.storage_id, "asset.txt")
            .await
            .unwrap();
        assert_eq!(asset, b"seed asset");
        let (_, stylesheet) = publication
            .deliver(&row.storage_id, "style.css")
            .await
            .unwrap();
        assert_eq!(stylesheet, b"body { color: red; }");
        let kind: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT kind FROM accounts WHERE id='system:examples'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap();
        assert_eq!(kind, "system");
    }

    #[tokio::test]
    async fn catalog_seed_with_owner_keeps_anonymous_owner_kind() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("owned.md");
        std::fs::write(&source, "seed phrase").unwrap();
        std::fs::write(root.path().join("asset.txt"), "seed asset").unwrap();
        std::fs::write(root.path().join("style.css"), "body { color: red; }").unwrap();
        let catalog = Arc::new(
            crate::storage::catalog::Catalog::open(root.path().join("catalog.db")).unwrap(),
        );
        let blobs = Arc::new(crate::storage::blob::FsStore::new(root.path(), true));
        seed_with_store(
            blobs.clone(),
            Arc::new(Configuration::default()),
            "Alice",
            &[document(&source)],
            Some(catalog.clone()),
        )
        .await;
        let slug = format!(
            "catalog-seed-fixture-{}",
            example_suffix("catalog-seed-fixture", &Configuration::default())
        );
        let row = catalog.document(&slug).unwrap().unwrap();
        let owner_id = row.owner_id.clone().unwrap();
        let kind: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT kind FROM accounts WHERE id=?1",
                        [&owner_id],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap();
        assert_eq!(kind, "anonymous");
        assert!(!row.example);
    }
}

/// What the reader would anchor against: the document with its markup,
/// scripts and styles removed. An approximation of what a browser shows,
/// which is enough to locate a phrase and take its surroundings. All
/// whitespace collapses, newlines included: a browser renders a line break
/// inside a paragraph as a single space.
pub fn visible_text(document: &str) -> String {
    let script_or_style =
        regex::Regex::new(r"(?is)<(script|style)\b[^>]*>.*?</(script|style)>").expect("pattern");
    let tag = regex::Regex::new(r"(?s)<[^>]*>").expect("pattern");
    let space = regex::Regex::new(r"\s+").expect("pattern");
    let text = script_or_style.replace_all(document, " ");
    let text = tag.replace_all(&text, "");
    let text = html_escape::decode_html_entities(&text);
    space.replace_all(&text, " ").to_string()
}

fn head(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let mut end = n;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn tail(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let mut start = text.len() - n;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}
