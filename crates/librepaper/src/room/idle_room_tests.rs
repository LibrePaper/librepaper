//! What opening a document costs its history, against a real catalogue.
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage. They are here rather than with the
//! repositories because what they check is a room's decision, not a row's
//! shape: the sweeper's, when it is asked whether anything has changed.

use super::*;
use crate::document::store::{MutationActor, Publication, Store};
use crate::storage::blob::FsStore;
use crate::storage::postgres::{PostgresCatalog, PostgresOptions};

struct Deployment {
    rooms: RoomSet,
    catalog: Arc<PostgresCatalog>,
    document: uuid::Uuid,
    slug: String,
    _objects: tempfile::TempDir,
}

async fn deployment(slug: &str) -> Option<Deployment> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap(),
    );
    catalog.migrate().await.unwrap();
    sqlx::query(
        "TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,publication_files,
         publications,document_versions,document_assets,replies,annotations,share_links,
         grants,documents,accounts CASCADE",
    )
    .execute(catalog.pool())
    .await
    .unwrap();
    let account = catalog
        .create_account(crate::storage::postgres::NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("one".into()),
            handle: "owner".into(),
            display_name: "Owner".into(),
            email: None,
        })
        .await
        .unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        Store::open_with_catalog(blobs, config, catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put_directory_as_actor(
            Publication {
                slug: slug.into(),
                title: "A Paper".into(),
                source: "\\documentclass{article}\n".into(),
                source_format: "latex".into(),
                main: "paper.tex".into(),
            },
            vec![("references.bib".into(), b"@book{a,title={A}}\n".to_vec())],
            MutationActor {
                account_id: account.id.to_string(),
                owner_key: "owner".into(),
                session_generation: account.session_generation.to_string(),
                link_hash: String::new(),
                policy_editor: true,
                unowned_publisher: false,
            },
        )
        .await
        .unwrap();
    let document = catalog.document_by_slug(slug).await.unwrap().unwrap().id;
    Some(Deployment {
        rooms: RoomSet::new(store),
        catalog,
        document,
        slug: slug.into(),
        _objects: objects,
    })
}

async fn versions(catalog: &PostgresCatalog, document: uuid::Uuid) -> Vec<String> {
    let mut rows = catalog.versions(document, 100).await.unwrap();
    rows.reverse();
    rows.iter().map(|row| row.reason.clone()).collect()
}

/// A reader opens the tutorial, reads it, and closes the tab. The sweeper
/// runs while the room is resident. Nothing was written, so the history
/// must be exactly what it was: no version, and no archive behind it.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn opening_a_document_nobody_edits_writes_no_version() {
    let Some(deployment) = deployment("idle-paper").await else {
        return;
    };
    let before = versions(&deployment.catalog, deployment.document).await;
    assert_eq!(before, vec!["publish".to_string()]);

    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    // Before anything is asked of it: a room that recovered the document
    // its newest version already holds has nothing waiting to be marked,
    // and must not tell its reader that it has.
    assert!(
        !room.history_durability().await.checkpoint_pending,
        "a document nobody has edited has no checkpoint pending"
    );
    for _ in 0..3 {
        room.tick().await;
    }
    // And what the last editor leaving asks for, which is the other way a
    // document that was only read used to acquire a version.
    let _ = room.checkpoint("left", "Owner").await;
    assert_eq!(
        versions(&deployment.catalog, deployment.document).await,
        before,
        "an unedited document must not be checkpointed"
    );
    deployment.catalog.close().await;
}

/// The same, across the room being let go of and loaded again -- which is
/// what a deployment does all day, and what used to write one duplicate
/// archive per cycle because the newest version's tree digest lived only
/// in the memory of the process that wrote it.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reloaded_room_recognises_its_own_newest_version() {
    let Some(deployment) = deployment("reopened-paper").await else {
        return;
    };
    // One real edit, so there is a checkpoint of this room's own making
    // to be recognised on the way back in.
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    {
        let mut state = room.state.lock().await;
        session::replace_text(&state.session.doc, "\\documentclass{book}\n", "paper.tex");
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
    }
    room.checkpoint("quiet", "Owner").await.unwrap();
    let after_edit = versions(&deployment.catalog, deployment.document).await;
    assert_eq!(after_edit, vec!["publish".to_string(), "quiet".to_string()]);

    for _ in 0..3 {
        let rooms = RoomSet::new(deployment.rooms.store.get().unwrap().clone());
        let room = rooms.try_get(&deployment.slug).await.unwrap();
        // A recovered session begins with a generation taken from its
        // update sequence. Without a content comparison at load, that
        // alone would read as unmarked edits, and the reader would be
        // told a checkpoint was owing from the moment the page opened.
        assert!(
            !room.history_durability().await.checkpoint_pending,
            "a reloaded room must not invent a pending checkpoint"
        );
        for _ in 0..3 {
            room.tick().await;
        }
        let _ = room.checkpoint("left", "Owner").await;
    }
    assert_eq!(
        versions(&deployment.catalog, deployment.document).await,
        after_edit,
        "reopening a document must not duplicate the version it already has"
    );
    deployment.catalog.close().await;
}

/// A checkpoint records which files it moved, so the panel beside an open
/// file can show that file's own history.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_checkpoint_records_the_paths_it_moved() {
    let Some(deployment) = deployment("scoped-paper").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    {
        let mut state = room.state.lock().await;
        session::put_text(&state.session.doc, "references.bib", "@book{b,title={B}}\n");
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
    }
    let sha = room.checkpoint("quiet", "Owner").await.unwrap().unwrap();
    let point = room.checkpoint_by_sha(&sha).await.unwrap().unwrap();
    assert_eq!(point.changed, vec!["references.bib".to_string()]);
    assert!(
        !point.tree_sha.is_empty(),
        "a checkpoint must name the tree it holds"
    );
    deployment.catalog.close().await;
}
