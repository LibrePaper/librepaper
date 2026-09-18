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
use crate::document::store::{MutationActor, Publication, Store};
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
        "TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,publication_files,
         publications,document_versions,document_assets,replies,annotation_live_state,annotations,
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
            Publication {
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
        publication_id: String::new(),
        body: "A remark.".into(),
        creator: "Reviewer".into(),
        exact: exact.into(),
        prefix: prefix.into(),
        suffix: suffix.into(),
        position: Some(0),
        point: false,
        document: false,
        color: None,
        proposed: None,
        temp_id: crate::storage::postgres::new_id().to_string(),
        request_id: crate::storage::postgres::new_id().to_string(),
    }
}

/// A note left between two words: no words of its own, and the text on either
/// side of the caret as the whole of the evidence for where it is.
fn point(prefix: &str, suffix: &str) -> Command {
    match selection("", prefix, suffix) {
        Command::Comment {
            motivation,
            publication_id,
            body,
            creator,
            temp_id,
            request_id,
            ..
        } => Command::Comment {
            motivation,
            publication_id,
            body,
            creator,
            exact: String::new(),
            prefix: prefix.into(),
            suffix: suffix.into(),
            position: Some(prefix.chars().count() as i64),
            point: true,
            document: false,
            color: None,
            proposed: None,
            temp_id,
            request_id,
        },
        other => other,
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
    // And what it was made against is the checkpoint the room is on.
    let current = {
        let state = room.state.lock().await;
        state
            .manifest
            .latest()
            .map(|point| point.sha.clone())
            .unwrap_or_default()
    };
    assert_eq!(anchor["checkpoint_id"], json!(current));
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

    // Somebody writes a paragraph above it. Nothing about the comment has
    // changed; everything about where it sits has.
    {
        let state = room.state.lock().await;
        session::put_text(
            &state.session.doc,
            "paper.md",
            &format!(
                "# Interval estimates\n\nA new opening paragraph.\n{}",
                &PAPER[21..]
            ),
        );
    }
    room.reattach_comments().await;

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
    deployment.catalog.close().await;
}

/// A point is an empty range, and an empty range is what a deleted passage
/// collapses to. Telling them apart is the difference between a point note
/// that follows the text and one that reports itself removed the moment
/// anybody types -- which is what the whole join looks like from the sidebar.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_point_note_follows_the_text_instead_of_reporting_itself_deleted() {
    let Some(deployment) = deployment("anchor-point").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let response = send(
        &room,
        point(
            "Interval estimates The interval covers",
            " the mean of the posterior.",
        ),
        &deployment.actor,
    )
    .await;
    assert_eq!(response["type"], "comment", "{response}");
    let id = response["comment"]["id"].as_str().unwrap().to_string();
    let placed = room.agent_comment(&id).await.expect("comment");
    let at = placed.source().expect("a source range").start_utf16;
    assert_eq!(
        placed.attachment.as_ref().expect("an attachment").status,
        AnchorStatus::Exact,
    );

    // Somebody writes a paragraph above it.
    {
        let state = room.state.lock().await;
        session::put_text(
            &state.session.doc,
            "paper.md",
            &format!(
                "# Interval estimates\n\nA new opening paragraph.\n{}",
                &PAPER[21..]
            ),
        );
    }
    room.reattach_comments().await;

    let comment = room.agent_comment(&id).await.expect("comment");
    let attachment = comment.attachment.expect("an attachment");
    assert_eq!(
        attachment.status,
        AnchorStatus::Exact,
        "a place between two characters is not content that can be deleted",
    );
    let (start, end) = attachment.resolved_range_utf16.expect("a resolved range");
    assert_eq!(start, end, "a point stays a point");
    assert!(
        start > at,
        "the point moved down the file: {start} is not after {at}",
    );
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
