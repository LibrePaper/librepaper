//! What opening a document costs its history, against a real catalogue.
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage. They are here rather than with the
//! repositories because what they check is a room's decision, not a row's
//! shape: that a version is written when somebody asks for one and at no
//! other time, and that the past stays reachable without one.

use super::*;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::storage::blob::FsStore;
use crate::storage::postgres::{PostgresCatalog, PostgresOptions};

struct Deployment {
    rooms: RoomSet,
    catalog: Arc<PostgresCatalog>,
    document: uuid::Uuid,
    slug: String,
    authority: crate::storage::postgres::Authority,
    _writer: crate::storage::postgres::WriterLease,
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
        "TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,bundle_files,
         bundles,document_checkpoints,document_versions,document_assets,replies,annotations,share_links,
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
    let writer = catalog.claim_writer().await.unwrap();
    let store = Arc::new(
        Store::open_with_catalog(blobs, config, catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put_directory_as_actor(
            DocumentInput {
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
    let authority = crate::storage::postgres::Authority {
        principal_key: account.id.to_string(),
        account_id: Some(account.id),
        link_hash: None,
    };
    Some(Deployment {
        rooms: RoomSet::new(store),
        catalog,
        document,
        slug: slug.into(),
        authority,
        _writer: writer,
        _objects: objects,
    })
}

async fn versions(catalog: &PostgresCatalog, document: uuid::Uuid) -> Vec<String> {
    let mut rows = catalog
        .checkpoint_page_records(document, None, 100)
        .await
        .unwrap();
    rows.reverse();
    rows.iter().map(|row| row.reason.clone()).collect()
}

async fn materialize_checkpoints(deployment: &Deployment) {
    let worker = crate::storage::worker::Worker::new(
        deployment.catalog.clone(),
        deployment.rooms.blobs.clone(),
    );
    let claims = deployment
        .catalog
        .claim_jobs("room-test", 100)
        .await
        .unwrap();
    for claim in claims
        .iter()
        .filter(|claim| claim.job.kind == "checkpoint_archive")
    {
        worker.execute(claim).await.unwrap();
        deployment
            .catalog
            .complete_job(claim, serde_json::json!({"ok": true}))
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn startup_repair_is_committed_before_the_room_is_served() {
    let deployment = deployment("startup-repair-paper").await.unwrap();
    sqlx::query("DELETE FROM document_bases WHERE document_id=$1")
        .bind(deployment.document)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM document_updates WHERE document_id=$1")
        .bind(deployment.document)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM document_source_revisions WHERE document_id=$1")
        .bind(deployment.document)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE documents SET update_sequence=0,commit_sequence=0,source_revision=0,\
                              project_generation=0,uncompacted_update_count=0,\
                              uncompacted_update_bytes=0 WHERE id=$1",
    )
    .bind(deployment.document)
    .execute(deployment.catalog.pool())
    .await
    .unwrap();

    let partial = session::new_doc();
    session::put_text(&partial, "paper.tex", "\\documentclass{article}\n");
    let body = session::encode_state(&partial);
    deployment
        .catalog
        .append_update(
            deployment.document,
            &body,
            &partial.state_frontiers().encode(),
            body.len() as i64,
        )
        .await
        .unwrap();

    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let state = room.command_owner.state().await;
    assert_eq!(
        session::texts_of(&state.session.doc)["references.bib"],
        "@book{a,title={A}}\n"
    );
    assert!(!state.session.dirty);
    assert_eq!(state.session.durable_sequence, 2);
    drop(state);
    let document = deployment
        .catalog
        .document(deployment.document)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(document.update_sequence, 2);
    let revisions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_source_revisions WHERE document_id=$1")
            .bind(deployment.document)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert_eq!(revisions, 1);
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn legacy_incremental_saves_recover_history() {
    let deployment = deployment("incremental-paper").await.unwrap();
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let large = (0..300)
        .map(|n| format!("Line {n}: text for the recovery test.\n"))
        .collect::<String>();
    {
        let mut state = room.command_owner.state().await;
        session::put_text(&state.session.doc, "paper.tex", &large);
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
    }
    room.persist().await.unwrap();
    {
        let mut state = room.command_owner.state().await;
        session::put_text(
            &state.session.doc,
            "references.bib",
            "@book{new,title={New}}\n",
        );
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
    }
    room.persist().await.unwrap();
    let updates = deployment
        .catalog
        .updates_after(deployment.document, 0, 100)
        .await
        .unwrap();
    assert_eq!(updates.len(), 3);
    assert!(updates[2].update_bytes.len() * 10 < updates[1].update_bytes.len());
    let recovered = session::new_doc();
    for update in &updates {
        session::apply_update(&recovered, &update.update_bytes).unwrap();
    }
    assert_eq!(
        session::texts_of(&recovered),
        session::texts_of(&room.command_owner.state().await.session.doc)
    );
    assert_eq!(
        deployment
            .catalog
            .updates_after(deployment.document, 0, 100)
            .await
            .unwrap()
            .len(),
        3
    );
    let buckets = deployment
        .catalog
        .document_activity(deployment.document, None)
        .await
        .unwrap();
    assert_eq!(
        buckets.iter().map(|b| b.state_bytes).max().unwrap(),
        session::encode_state(&recovered).len() as i64
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn compacted_remote_edits_do_not_mark_local_edits_durable() {
    let deployment = deployment("compacted-paper").await.unwrap();
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    room.checkpoint("label", "Owner", &deployment.authority)
        .await
        .unwrap();
    let remote = session::new_doc();
    session::apply_update(
        &remote,
        &session::encode_state(&room.command_owner.state().await.session.doc),
    )
    .unwrap();
    let before = session::encode_vector(&remote);
    session::put_text(&remote, "remote.tex", "remote work");
    let update = session::encode_diff(&remote, &before).unwrap();
    let storage = crate::storage::collaboration::CollaborationStorage::new(
        deployment.catalog.clone(),
        room.blobs.clone(),
    );
    let through = storage
        .append(
            deployment.document,
            &update,
            &remote.state_frontiers().encode(),
            session::encode_state(&remote).len() as i64,
        )
        .await
        .unwrap();
    storage
        .compact(
            deployment.document,
            through,
            0,
            &session::encode_state(&remote),
        )
        .await
        .unwrap();
    {
        let mut state = room.command_owner.state().await;
        session::put_text(&state.session.doc, "local.tex", "unsaved local work");
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
    }
    room.persist().await.unwrap();
    let recovered = storage.recover(deployment.document).await.unwrap();
    let doc = session::new_doc();
    session::apply_update(&doc, &recovered.base.unwrap()).unwrap();
    assert_eq!(recovered.updates.len(), 1);
    session::apply_update(&doc, &recovered.updates[0].update_bytes).unwrap();
    let texts = session::texts_of(&doc);
    assert_eq!(texts["remote.tex"], "remote work");
    assert_eq!(texts["local.tex"], "unsaved local work");
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn timer_never_persists_test_only_dirty_state() {
    let deployment = deployment("cadence-paper").await.unwrap();
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    {
        let mut state = room.command_owner.state().await;
        session::put_text(&state.session.doc, "paper.tex", "updated");
        state.session.generation += 1;
        state.session.mark_dirty(now_unix() - 20);
        state.session.updated_at = now_unix() - 20;
        state.session.last_persist_at = now_unix() - 20;
    }
    room.tick().await;
    assert_eq!(
        deployment
            .catalog
            .updates_after(deployment.document, 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    {
        let mut state = room.command_owner.state().await;
        state.session.dirty_since = now_unix() - 31;
        state.session.updated_at = now_unix();
        state.session.last_persist_at = now_unix() - 31;
    }
    room.tick().await;
    assert_eq!(
        deployment
            .catalog
            .updates_after(deployment.document, 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    deployment.catalog.close().await;
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
    assert!(before.is_empty());

    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    for _ in 0..3 {
        room.tick().await;
    }
    assert_eq!(
        versions(&deployment.catalog, deployment.document).await,
        before,
        "an unedited document must not be checkpointed"
    );
    deployment.catalog.close().await;
}

/// Each explicit request is a timeline event, including after a room reload.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reloaded_room_recognises_its_own_newest_version() {
    let Some(deployment) = deployment("reopened-paper").await else {
        return;
    };
    // One real edit, so there is a checkpoint of this room's own making
    // to be recognised on the way back in.
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    room.commit_edit(
        &deployment.authority,
        "Owner".into(),
        false,
        |candidate, _, _| {
            session::replace_text(candidate, "\\documentclass{book}\n", "paper.tex");
            Ok::<_, WriteError>(())
        },
    )
    .await
    .unwrap();
    room.checkpoint("label", "Owner", &deployment.authority)
        .await
        .unwrap();
    let after_edit = versions(&deployment.catalog, deployment.document).await;
    assert_eq!(after_edit, vec!["label".to_string()]);

    for _ in 0..3 {
        let rooms = RoomSet::new(deployment.rooms.store.get().unwrap().clone());
        let room = rooms.try_get(&deployment.slug).await.unwrap();
        for _ in 0..3 {
            room.tick().await;
        }
        room.checkpoint("label", "Owner", &deployment.authority)
            .await
            .unwrap();
    }
    assert_eq!(
        versions(&deployment.catalog, deployment.document).await,
        vec!["label".to_string(); 4],
        "each deliberate checkpoint remains a distinct event"
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
    room.commit_edit(
        &deployment.authority,
        "Owner".into(),
        false,
        |candidate, _, _| {
            session::put_text(candidate, "references.bib", "@book{b,title={B}}\n");
            Ok::<_, WriteError>(())
        },
    )
    .await
    .unwrap();
    let sha = room
        .checkpoint("label", "Owner", &deployment.authority)
        .await
        .unwrap()
        .unwrap();
    let point = room.checkpoint_by_sha(&sha).await.unwrap().unwrap();
    assert_eq!(
        point.changed,
        Some(vec!["paper.tex".to_string(), "references.bib".to_string()])
    );
    assert!(
        !point.tree_sha.is_empty(),
        "a checkpoint must name the tree it holds"
    );
    deployment.catalog.close().await;
}

/// The eviction timer never turns in-memory test mutations into semantic
/// history. Production mutation paths commit before installing state.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_session_of_edits_leaves_no_versions() {
    let Some(deployment) = deployment("probe-paper").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    for round in 0..5 {
        {
            let mut state = room.command_owner.state().await;
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
            let mut state = room.command_owner.state().await;
            let long_ago = now_unix() - 120;
            state.session.updated_at = long_ago;
            state.session.dirty_since = long_ago;
            state.session.last_persist_at = long_ago;
        }
        room.tick().await;
    }
    let seen = versions(&deployment.catalog, deployment.document).await;
    assert!(
        seen.is_empty(),
        "the sweeper writes no checkpoints: {seen:?}"
    );
    assert_eq!(
        deployment
            .catalog
            .updates_after(deployment.document, 0, 100)
            .await
            .unwrap()
            .len(),
        1,
        "the timer must not append source"
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
    room.checkpoint("label", "Owner", &deployment.authority)
        .await
        .unwrap();
    materialize_checkpoints(&deployment).await;
    let published = room.manifest().await.checkpoints[0].clone();
    let proposal_base = room
        .command_owner
        .state()
        .await
        .session
        .doc
        .state_frontiers();
    let proposal_id = room
        .open_proposal(
            "Ada",
            &proposal_base,
            &deployment.authority,
            uuid::Uuid::new_v4(),
        )
        .await
        .unwrap();
    room.commit_edit(
        &deployment.authority,
        "Owner".into(),
        false,
        |candidate, _, _| {
            session::replace_text(candidate, "\\documentclass{book}\n", "paper.tex");
            Ok::<_, WriteError>(())
        },
    )
    .await
    .unwrap();
    room.checkpoint("label", "Owner", &deployment.authority)
        .await
        .unwrap();
    // And one more edit nobody has checkpointed, which is the work a restore
    // would otherwise discard without trace.
    room.commit_edit(
        &deployment.authority,
        "Owner".into(),
        false,
        |candidate, _, _| {
            session::replace_text(candidate, "\\documentclass{report}\n", "paper.tex");
            Ok::<_, WriteError>(())
        },
    )
    .await
    .unwrap();

    let restored = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        room.restore_and_checkpoint(&published, "Owner", &deployment.authority),
    )
    .await
    .expect("restoring must not hang");
    let (_update, sha) = restored.expect("the restore must succeed");
    assert!(!sha.is_empty(), "a restore writes a version of its own");
    assert_eq!(
        deployment
            .catalog
            .proposal(uuid::Uuid::parse_str(&proposal_id).unwrap())
            .await
            .unwrap()
            .unwrap()
            .status,
        "superseded",
        "restore supersedes review branches in its source transaction"
    );

    assert_eq!(
        versions(&deployment.catalog, deployment.document).await,
        vec![
            "label".to_string(),
            "label".to_string(),
            "superseded".to_string(),
            "restore".to_string()
        ],
    );
    let texts = {
        let state = room.command_owner.state().await;
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
            let mut state = room.command_owner.state().await;
            session::replace_text(&state.session.doc, body, "paper.tex");
            state.session.generation += 1;
            state.session.mark_dirty(now_unix());
            state.session.doc.state_frontiers().encode()
        }
    };

    let first = write("\\documentclass{article}\n% the first draft\n").await;
    let second = write("\\documentclass{article}\n% a second thought\n").await;
    write("\\documentclass{article}\n% and a third\n").await;

    let at_first = room.project_at_frontier(&first).await.unwrap();
    assert_eq!(at_first.main, "paper.tex");
    assert!(
        at_first.text_ids.contains_key("paper.tex"),
        "a moment names which id held which path, for a comment to be read against"
    );
    assert_eq!(
        at_first.texts.get("paper.tex").map(String::as_str),
        Some("\\documentclass{article}\n% the first draft\n"),
        "a frontier must reproduce the document as it stood, not as it stands"
    );
    let later = room.project_at_frontier(&second).await.unwrap();
    assert_eq!(
        later.texts.get("paper.tex").map(String::as_str),
        Some("\\documentclass{article}\n% a second thought\n"),
    );
    // The live document is untouched by having been read from: a fork is a
    // copy, and the room keeps writing where it was.
    let now = {
        let state = room.command_owner.state().await;
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
