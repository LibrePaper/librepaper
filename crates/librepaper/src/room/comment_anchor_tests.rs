//! A comment, from the words a reader selected to the range it is about.
//!
//! The pieces are covered on their own -- `locate` finds a passage, `resolve`
//! follows one, the storage layer keeps the two apart -- and what is left is
//! the join: that a comment arriving as a quotation acquires a source range it
//! never sent, that the range is the checkpoint's own text, that nothing
//! afterwards edits it, and that editing the document moves where the comment
//! points without moving what it is about.
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage.

use super::*;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::storage::blob::FsStore;
use crate::storage::postgres::{PostgresCatalog, PostgresOptions};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    rooms: RoomSet,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    actor: MutationActor,
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
        "TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,bundle_files,
         bundles,document_versions,document_assets,replies,annotation_live_state,annotations,
         share_links,grants,documents,accounts CASCADE",
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
    let actor = MutationActor {
        account_id: account.id.to_string(),
        owner_key: "owner".into(),
        session_generation: account.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        unowned_publisher: false,
    };
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "A Paper".into(),
                source: PAPER.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
            actor.clone(),
        )
        .await
        .unwrap();
    Some(Deployment {
        rooms: RoomSet::new(store),
        catalog,
        slug: slug.into(),
        actor,
        _objects: objects,
    })
}

/// A comment as a browser sends one: the words it showed, and the words on
/// either side of them. No file, no offsets, no checkpoint.
fn selection(exact: &str, prefix: &str, suffix: &str) -> Command {
    Command::Comment {
        motivation: "commenting".into(),
        bundle_id: String::new(),
        body: "A remark.".into(),
        creator: "Reviewer".into(),
        exact: exact.into(),
        prefix: prefix.into(),
        suffix: suffix.into(),
        position: Some(0),
        document: false,
        color: None,
        proposed: None,
        temp_id: crate::storage::postgres::new_id().to_string(),
        request_id: crate::storage::postgres::new_id().to_string(),
    }
}

async fn send(room: &Arc<Room>, command: Command, actor: &MutationActor) -> Value {
    let (response, _) = room
        .apply_command_with_actor(
            command,
            "127.0.0.1",
            "github:owner",
            "",
            None,
            true,
            actor.clone(),
        )
        .await;
    response
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_quotation_becomes_a_range_the_browser_never_sent() {
    let Some(deployment) = deployment("anchor-paper").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    // The page says "interval covers the mean"; the source says
    // "*interval* covers the mean".
    let response = send(
        &room,
        selection(
            "interval covers the mean",
            "Interval estimates The ",
            " of the posterior.",
        ),
        &deployment.actor,
    )
    .await;
    assert_eq!(response["type"], "comment", "{response}");
    let anchor = &response["comment"]["original_anchor"];
    assert_eq!(anchor["kind"], "source_text", "{response}");
    let target = &anchor["target"];
    let start = target["start_utf16"].as_u64().unwrap() as usize;
    let end = target["end_utf16"].as_u64().unwrap() as usize;
    let units: Vec<u16> = PAPER.encode_utf16().collect();
    assert_eq!(
        String::from_utf16_lossy(&units[start..end]),
        "*interval* covers the mean",
        "the range is the source's own, markup and all",
    );
    assert_eq!(target["exact"], "*interval* covers the mean");
    // And what it was made against is the position in the editing history the
    // room was at, not a checkpoint: a comment writes no source archive, and
    // the frontier names the state exactly rather than naming the nearest
    // state somebody happened to save.
    let made_on = anchor["checkpoint_id"].as_str().unwrap_or_default();
    let recorded = made_on
        .strip_prefix(crate::room::annotation::MOMENT)
        .expect("a comment on the live draft is anchored to a frontier");
    let frontier = crate::room::decode_update(recorded).expect("the frontier is base64");
    let at = room
        .project_at_frontier(&frontier)
        .await
        .expect("the document is readable at the frontier a comment names");
    assert!(
        at.texts
            .values()
            .any(|text| text.contains(target["exact"].as_str().unwrap())),
        "the state a comment names holds the passage it is about",
    );

    // Nothing was written to say so. The harness truncates the catalogue and
    // creates one document, so the only version in it is the one that document
    // arrived with.
    // Asked at runtime rather than through the checked macro: this is the only
    // place that counts versions, and one test is not worth an entry in the
    // offline query cache.
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM document_versions")
        .fetch_one(deployment.catalog.pool())
        .await
        .unwrap();
    assert_eq!(versions, 1, "a comment writes no source archive");
    // The words the page had are kept beside it, and are not the anchor.
    assert_eq!(
        response["comment"]["presentation"]["rendered_exact"],
        "interval covers the mean"
    );
    assert_eq!(response["comment"]["attachment"]["status"], "exact");
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_remark_about_the_whole_document_needs_no_passage() {
    let Some(deployment) = deployment("anchor-whole").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let mut command = selection("", "", "");
    if let Command::Comment { document, .. } = &mut command {
        *document = true;
    }
    let response = send(&room, command, &deployment.actor).await;
    assert_eq!(response["type"], "comment", "{response}");
    assert_eq!(response["comment"]["original_anchor"]["kind"], "document");
    assert!(response["comment"]["original_anchor"]["target"].is_null());
    // It quotes nothing because there was nothing to quote, which is not the
    // same as a passage being kept back: a reader's card should read it as the
    // general remark it is.
    let public = room.snapshot_for("", false).await;
    let value = serde_json::to_value(&public[0]).unwrap();
    assert_eq!(
        value["passage_withheld"],
        serde_json::Value::Null,
        "a remark about the document withholds no passage: {value}",
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn words_in_two_places_are_refused_rather_than_placed_in_one() {
    let Some(deployment) = deployment("anchor-ambiguous").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    // "paragraph" appears once, but a phrase that appears nowhere cannot be
    // placed at all, and neither answer is a guess.
    let response = send(
        &room,
        selection("a sentence from another paper entirely", "", ""),
        &deployment.actor,
    )
    .await;
    assert_eq!(response["type"], "error", "{response}");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("not in this document's source"),
        "{response}"
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn editing_the_document_moves_where_a_comment_points_and_not_what_it_is_about() {
    let Some(deployment) = deployment("anchor-edited").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let response = send(
        &room,
        selection(
            "interval covers the mean",
            "Interval estimates The ",
            " of the posterior.",
        ),
        &deployment.actor,
    )
    .await;
    let id = response["comment"]["id"].as_str().unwrap().to_string();
    let anchor_before = response["comment"]["original_anchor"].clone();
    let started_at = anchor_before["target"]["start_utf16"].as_u64().unwrap();
    let (editor_tx, _editor_rx) = tokio::sync::mpsc::channel(8);
    room.attach(999, editor_tx, true).await;

    // Somebody writes a paragraph above it. Nothing about the comment has
    // changed; everything about where it sits has.
    let update = {
        let state = room.state.lock().await;
        let edited = state.session.doc.fork();
        let vector = session::encode_vector(&edited);
        session::put_text(
            &edited,
            "paper.md",
            &format!(
                "# Interval estimates\n\nA new opening paragraph.\n{}",
                &PAPER[21..]
            ),
        );
        session::encode_diff(&edited, &vector).unwrap()
    };
    assert!(matches!(
        room.receive_update(999, &update, 1, "Owner").await,
        Applied::Relay
    ));
    assert!(room.state.lock().await.pending_attachments.contains(&id));
    room.persist().await.unwrap();
    assert!(room.state.lock().await.pending_attachments.is_empty());

    let comment = room.agent_comment(&id).await.expect("comment");
    assert_eq!(
        serde_json::to_value(comment.original_anchor.clone()).unwrap(),
        anchor_before,
        "what a comment is about does not move",
    );
    let attachment = comment.attachment.expect("an attachment");
    assert_eq!(attachment.status, AnchorStatus::Exact);
    let (start, _) = attachment.resolved_range_utf16.expect("a resolved range");
    assert!(
        u64::from(start) > started_at,
        "the passage moved down the file: {start} is not after {started_at}",
    );
    let document = deployment
        .catalog
        .document_by_slug(&deployment.slug)
        .await
        .unwrap()
        .unwrap();
    let stored = deployment
        .catalog
        .annotations(document.id, None, None, 10)
        .await
        .unwrap();
    let persisted = crate::storage::postgres::attachment_from_record(&stored[0])
        .unwrap()
        .unwrap()
        .resolved_range_utf16
        .unwrap();
    assert_eq!(persisted.0, start);
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_passage_that_is_deleted_leaves_a_comment_that_says_so() {
    let Some(deployment) = deployment("anchor-deleted").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let response = send(
        &room,
        selection(
            "interval covers the mean",
            "Interval estimates The ",
            " of the posterior.",
        ),
        &deployment.actor,
    )
    .await;
    let id = response["comment"]["id"].as_str().unwrap().to_string();
    {
        let state = room.state.lock().await;
        session::put_text(
            &state.session.doc,
            "paper.md",
            "# Interval estimates\n\nA second paragraph, for company.\n",
        );
    }
    room.reattach_comments().await;
    let comment = room.agent_comment(&id).await.expect("comment");
    // The comment is still here, still about what it was about, and says what
    // became of it.
    assert!(comment.source().is_some());
    assert_eq!(
        comment.attachment.expect("an attachment").status,
        AnchorStatus::Deleted,
    );
    deployment.catalog.close().await;
}

/// Two people editing one document must see each other's comments as they are
/// made, not on the next reload -- and so must the reader who is waiting on an
/// answer.
///
/// The frame is not the same for both. An editor peer is sent the project's
/// own annotation, source anchor and all; a reader peer is sent the remark
/// with the anchor taken out. One view used to be computed for everybody, with
/// `is_owner: false`, so an editor's own colleague was handed the public
/// projection and the comment appeared only when the socket reconnected.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_editing_comment_reaches_the_other_editor_live() {
    let Some(deployment) = deployment("anchor-relay").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let (editor_tx, mut editor_rx) = tokio::sync::mpsc::channel(8);
    let (reader_tx, mut reader_rx) = tokio::sync::mpsc::channel(8);
    // The socket that submitted it (1) is skipped: it gets its own frame, with
    // its own `mine` and delete control, straight back down its connection.
    room.attach(1, editor_tx.clone(), true).await;
    room.attach(2, editor_tx, true).await;
    room.attach(3, reader_tx, false).await;

    let response = send(
        &room,
        selection(
            "interval covers the mean",
            "Interval estimates The ",
            " of the posterior.",
        ),
        &deployment.actor,
    )
    .await;
    assert_eq!(response["type"], "comment", "{response}");
    room.broadcast_comment_event(Some(1), &response).await;

    let relayed = received(&mut editor_rx);
    assert_eq!(
        relayed.len(),
        1,
        "the submitting socket is not sent it twice"
    );
    assert_eq!(relayed[0]["type"], "comment", "{}", relayed[0]);
    assert_eq!(relayed[0]["comment"]["id"], response["comment"]["id"]);
    assert_eq!(
        relayed[0]["comment"]["mine"],
        serde_json::json!(false),
        "a shared frame says nothing about whose comment it is",
    );

    // The reader who is being answered gets the answer, in the view the
    // public channel may carry: the remark, and not what it is about.
    let public = received(&mut reader_rx);
    assert_eq!(public.len(), 1);
    assert_eq!(public[0]["type"], "comment", "{}", public[0]);
    assert_eq!(public[0]["comment"]["id"], response["comment"]["id"]);
    assert_eq!(public[0]["comment"]["body"], "A remark.");
    assert!(
        public[0]["comment"]["original_anchor"].is_null(),
        "a reader is never sent a range in a source file: {}",
        public[0],
    );
    assert!(
        public[0]["comment"]["attachment"].is_null(),
        "nor where that range has got to: {}",
        public[0],
    );
    // This one was written in the editor, so the words it quotes are the
    // draft's. They stay there; the card is told why it has none.
    assert!(
        public[0]["comment"]["presentation"]["rendered_exact"].is_null(),
        "the draft's words do not cross: {}",
        public[0],
    );
    // The editor peer keeps them: it is reading the draft they were taken
    // from.
    assert_eq!(
        relayed[0]["comment"]["presentation"]["rendered_exact"],
        "interval covers the mean",
    );
    assert_eq!(
        public[0]["comment"]["passage_withheld"],
        serde_json::json!(true),
        "and the reader is told this is a passage rather than the document: {}",
        public[0],
    );
    // The editor peer's view of the same comment is the project's own.
    assert!(
        !relayed[0]["comment"]["original_anchor"].is_null(),
        "an editor peer keeps the anchor: {}",
        relayed[0],
    );

    deployment.catalog.close().await;
}

/// An agent edits annotations in batches, so what the room states is the whole
/// list rather than each step it took. It carries the same two views: the
/// editing project's own annotations are the editor peers', and the public
/// channel gets only what it may carry -- which for a batch of editing
/// comments is an empty list, not the editors' one.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_agents_batch_states_the_list_each_peer_may_see() {
    let Some(deployment) = deployment("anchor-snapshot").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let (editor_tx, mut editor_rx) = tokio::sync::mpsc::channel(8);
    let (reader_tx, mut reader_rx) = tokio::sync::mpsc::channel(8);
    room.attach(1, editor_tx, true).await;
    room.attach(2, reader_tx, false).await;

    let made = send(
        &room,
        selection(
            "interval covers the mean",
            "Interval estimates The ",
            " of the posterior.",
        ),
        &deployment.actor,
    )
    .await;
    assert_eq!(made["type"], "comment", "{made}");
    room.broadcast_comment_snapshot(7).await;

    let editors = received(&mut editor_rx);
    assert_eq!(editors.len(), 1);
    assert_eq!(editors[0]["type"], "comments");
    assert_eq!(editors[0]["annotation_revision"], 7);
    assert_eq!(
        editors[0]["comments"]
            .as_array()
            .map(|list| list.len())
            .unwrap_or_default(),
        1,
        "an editor peer is stated the project's own annotations",
    );

    let readers = received(&mut reader_rx);
    assert_eq!(readers.len(), 1);
    assert_eq!(readers[0]["type"], "comments");
    let public = readers[0]["comments"].as_array().expect("a list");
    assert_eq!(
        public.len(),
        1,
        "and a reader peer the ones the public channel may carry",
    );
    assert!(
        public[0]["original_anchor"].is_null(),
        "carried without the source it is anchored in: {}",
        readers[0],
    );

    deployment.catalog.close().await;
}

/// A suggestion is the one annotation a reader never receives.
///
/// It is not a remark about the document, it is an edit to it: a source anchor
/// and the words proposed for that source. Both are the editable project, and
/// the public annotation channel is not where the project goes.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_suggestion_stays_out_of_the_public_channel() {
    let Some(deployment) = deployment("anchor-suggestion").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let (editor_tx, mut editor_rx) = tokio::sync::mpsc::channel(8);
    let (reader_tx, mut reader_rx) = tokio::sync::mpsc::channel(8);
    room.attach(1, editor_tx, true).await;
    room.attach(2, reader_tx, false).await;

    let mut command = selection(
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
    );
    if let Command::Comment {
        motivation,
        proposed,
        ..
    } = &mut command
    {
        *motivation = "editing".into();
        *proposed = Some("*interval* contains the mean".into());
    }
    let response = send(&room, command, &deployment.actor).await;
    assert_eq!(response["type"], "comment", "{response}");
    room.broadcast_comment_event(None, &response).await;

    let editors = received(&mut editor_rx);
    assert_eq!(editors.len(), 1);
    assert_eq!(editors[0]["type"], "comment", "{}", editors[0]);

    let readers = received(&mut reader_rx);
    assert_eq!(readers.len(), 1);
    assert_eq!(
        readers[0]["type"], "annotation-redacted",
        "a proposed edit is not a remark a reader may have: {}",
        readers[0],
    );

    deployment.catalog.close().await;
}

/// Whatever is already queued for a peer, as JSON.
fn received(rx: &mut tokio::sync::mpsc::Receiver<crate::room::outgoing::Outgoing>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        let text = match frame {
            crate::room::outgoing::Outgoing::Text(value) => value,
            crate::room::outgoing::Outgoing::SharedText(value) => value.to_string(),
            crate::room::outgoing::Outgoing::Close(_) => continue,
        };
        out.push(serde_json::from_str(&text).expect("every frame is JSON"));
    }
    out
}
