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
    sqlx::query!(
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
    assert_eq!(point.changed, Some(vec!["references.bib".to_string()]));
    assert!(
        !point.tree_sha.is_empty(),
        "a checkpoint must name the tree it holds"
    );
    deployment.catalog.close().await;
}

/// The other direction from the two above, and the one nothing covered: a
/// working session -- several edits, each left long enough for the sweeper to
/// mark -- must leave one version for each. The tests beside this one all
/// check that no spurious version is written, which a room that had stopped
/// checkpointing altogether would also pass.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_session_of_edits_leaves_a_version_for_each() {
    let Some(deployment) = deployment("probe-paper").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    for round in 0..5 {
        {
            let mut state = room.state.lock().await;
            session::replace_text(
                &state.session.doc,
                &format!("\\documentclass{{article}}\n% round {round}\n"),
                "paper.tex",
            );
            state.session.generation += 1;
            state.session.mark_dirty(now_unix());
        }
        // The sweeper's own path: pretend the author paused long enough.
        {
            let mut state = room.state.lock().await;
            let long_ago = now_unix() - 120;
            state.session.updated_at = long_ago;
            state.session.dirty_since = long_ago;
            state.session.last_persist_at = long_ago;
            if state.session.pending_checkpoint_since > 0 {
                state.session.pending_checkpoint_since = long_ago;
            }
        }
        room.tick().await;
    }
    let seen = versions(&deployment.catalog, deployment.document).await;
    assert_eq!(
        seen.len(),
        6,
        "one version per edit, plus the publish: {seen:?}"
    );
    deployment.catalog.close().await;
}

/// Restoring an earlier version, which is what the History panel's button
/// asks the server for. Nothing covered this: the room harness had no
/// catalogue until the deployment above, and `restore_and_checkpoint` needs
/// one to write the version it supersedes.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_restore_puts_an_earlier_version_back() {
    let Some(deployment) = deployment("restored-paper").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let published = room.manifest().await.checkpoints[0].clone();
    {
        let mut state = room.state.lock().await;
        session::replace_text(&state.session.doc, "\\documentclass{book}\n", "paper.tex");
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
    }
    room.checkpoint("quiet", "Owner").await.unwrap();
    // And one more edit nobody has checkpointed, which is the work a restore
    // would otherwise discard without trace.
    {
        let mut state = room.state.lock().await;
        session::replace_text(&state.session.doc, "\\documentclass{report}\n", "paper.tex");
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
    }

    let restored = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        room.restore_and_checkpoint(&published, "Owner"),
    )
    .await
    .expect("restoring must not hang");
    let (_update, sha) = restored.expect("the restore must succeed");
    assert!(!sha.is_empty(), "a restore writes a version of its own");

    assert_eq!(
        versions(&deployment.catalog, deployment.document).await,
        vec![
            "publish".to_string(),
            "quiet".to_string(),
            "superseded".to_string(),
            "restore".to_string()
        ],
    );
    let texts = {
        let state = room.state.lock().await;
        session::texts_of(&state.session.doc)
    };
    assert_eq!(
        texts.get("paper.tex").map(String::as_str),
        Some("\\documentclass{article}\n"),
        "the restored version's source must be what the room now holds"
    );
    deployment.catalog.close().await;
}

/// Going to a moment nobody checkpointed, which is what the activity
/// timeline's anchors are for. The frontier is read where the room records
/// one -- after a write, under the state lock -- and the document is asked for
/// it after two more edits have moved on.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_frontier_reproduces_the_document_as_it_stood() {
    let Some(deployment) = deployment("anchored-paper").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let write = |body: &'static str| {
        let room = room.clone();
        async move {
            let mut state = room.state.lock().await;
            session::replace_text(&state.session.doc, body, "paper.tex");
            state.session.generation += 1;
            state.session.mark_dirty(now_unix());
            state.session.doc.state_frontiers().encode()
        }
    };

    let first = write("\\documentclass{article}\n% the first draft\n").await;
    let second = write("\\documentclass{article}\n% a second thought\n").await;
    write("\\documentclass{article}\n% and a third\n").await;

    let (main, texts, _assets) = room.project_at_frontier(&first).await.unwrap();
    assert_eq!(main, "paper.tex");
    assert_eq!(
        texts.get("paper.tex").map(String::as_str),
        Some("\\documentclass{article}\n% the first draft\n"),
        "a frontier must reproduce the document as it stood, not as it stands"
    );
    let (_, later, _) = room.project_at_frontier(&second).await.unwrap();
    assert_eq!(
        later.get("paper.tex").map(String::as_str),
        Some("\\documentclass{article}\n% a second thought\n"),
    );
    // The live document is untouched by having been read from: a fork is a
    // copy, and the room keeps writing where it was.
    let now = {
        let state = room.state.lock().await;
        session::texts_of(&state.session.doc)
    };
    assert_eq!(
        now.get("paper.tex").map(String::as_str),
        Some("\\documentclass{article}\n% and a third\n"),
    );

    // A version this document never had is refused rather than approximated.
    assert!(room.project_at_frontier(b"not a frontier").await.is_err());
    deployment.catalog.close().await;
}
