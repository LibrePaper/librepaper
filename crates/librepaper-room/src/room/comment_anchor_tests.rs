//! A comment, from the words a reader selected to the range it is about.
//!
//! The pieces are covered on their own -- `locate` finds a passage, `derive`
//! reads one back, [`AddComment`] and its siblings write it down -- and what is
//! left is the join: that a comment arriving as a quotation acquires a source
//! range it never sent, that the range is the head's own text, that nothing
//! afterwards edits it, and that editing the document moves where the
//! comment points without moving what it is about.
//!
//! These run the new command path directly -- `room.command(&authority, &mut
//! cmd)` -- rather than through the wire protocol in `command.rs` and
//! `server/mod.rs`, because that is where the guarantees in this file
//! actually live (§7).
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage.

use super::*;
use futures_util::future::BoxFuture;
use librepaper_document::document::{anchors, session};
use librepaper_engine::log::sequencer::{
    Command as SequencerCommand, Evidence, Head, PreparedSource, Rung,
};
use librepaper_engine::log::Registry;
use librepaper_engine::storage::annotation::{AnchorStatus, CommentTarget};
use librepaper_engine::storage::blob::FsStore;
use librepaper_engine::storage::postgres::Authority;
use librepaper_engine::storage::store::{DocumentInput, MutationActor, Store};
use serde_json::json;

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    rooms: Rooms,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    account_id: Uuid,
    _writer: librepaper_engine::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn deployment(slug: &str) -> Option<Deployment> {
    let catalog = librepaper_engine::testing::catalog().await?;
    let writer = catalog.claim_writer().await.unwrap();
    let account = catalog
        .create_account(librepaper_engine::storage::postgres::NewAccount {
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
    let blobs: Arc<dyn librepaper_engine::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );
    let store = Arc::new(Store::open_with_catalog(
        blobs.clone(),
        config.clone(),
        catalog.clone(),
        registry.clone(),
    ));
    let actor = MutationActor {
        account_id: account.id.to_string(),
        owner_key: "owner".into(),
        session_generation: account.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        policy_comment: true,
        automation: false,
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
            actor,
        )
        .await
        .unwrap();
    Some(Deployment {
        rooms: Rooms::new(catalog.clone(), blobs, config, registry),
        catalog,
        slug: slug.into(),
        account_id: account.id,
        _writer: writer,
        _objects: objects,
    })
}

impl Deployment {
    fn authority(&self) -> Authority {
        Authority {
            principal_key: self.account_id.to_string(),
            account_id: Some(self.account_id),
            link_hash: None,
            session_generation: Some(1),
            policy_edit: true,
            policy_comment: true,
            automation: false,
        }
    }

    /// The owner, at editor rung, which is also more than enough for a
    /// commenter-gated command -- `authorize_annotation_mutation` admits an
    /// owner at either rung.
    /// Who this deployment writes as: the owner, at editor rung.
    fn writer(&self) -> CommentAuthor {
        CommentAuthor::new(
            "Reviewer",
            Some(self.account_id),
            format!("account:{}", self.account_id),
            self.mutation_authorization(),
            true,
        )
    }

    fn mutation_authorization(
        &self,
    ) -> librepaper_engine::storage::postgres::MutationAuthorization {
        librepaper_engine::storage::postgres::MutationAuthorization {
            principal_key: self.account_id.to_string(),
            account_id: Some(self.account_id),
            session_generation: Some(1),
            token_hash: None,
            policy_edit: true,
            policy_comment: true,
            automation: false,
        }
    }

    fn add_comment(
        &self,
        room: &Room,
        exact: &str,
        prefix: &str,
        suffix: &str,
        motivation: &str,
    ) -> AddComment {
        let config = Configuration::default();
        AddComment::new(
            self.catalog.clone(),
            room.document_id,
            Uuid::new_v4(),
            &config,
            motivation,
            "A remark.",
            &self.writer(),
            exact,
            prefix,
            suffix,
            Some(0),
            false,
            None,
            None,
        )
        .expect("a well-formed comment")
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_quotation_becomes_a_range_the_browser_never_sent() {
    let Some(deployment) = deployment("anchor-paper").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    // The page says "interval covers the mean"; the source says
    // "*interval* covers the mean".
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let target = comment.source().expect("a source-text anchor");
    let units: Vec<u16> = PAPER.encode_utf16().collect();
    assert_eq!(
        String::from_utf16_lossy(&units[target.start_utf16 as usize..target.end_utf16 as usize]),
        "*interval* covers the mean",
        "the range is the source's own, markup and all",
    );
    assert_eq!(target.exact, "*interval* covers the mean");
    // What it was made against is a row in the log and the frontier that row
    // made durable, not an archive: a comment writes its anchor record and
    // nothing else of the source.
    let anchor = comment.original_anchor.as_ref().unwrap();
    assert!(anchor.source_sequence >= 0);
    assert!(!anchor.frontier.is_empty());
    // The document's own creation is `store::ReplaceProject`, a
    // source-producing command in its own right (§7.2), so it always leaves
    // one label behind before this test's comment is ever made. The claim
    // under test is that a comment adds none of its own, not that the table
    // is empty -- so this checks for exactly the creation label and nothing
    // else, rather than for no labels at all.
    let labels = deployment
        .catalog
        .label_page(room.document_id, None, 10)
        .await
        .unwrap();
    assert_eq!(
        labels.iter().map(|l| l.reason.as_str()).collect::<Vec<_>>(),
        vec!["whole-project replacement"],
        "a comment writes no label of its own",
    );
    // The words the page had are kept beside it, and are not the anchor.
    assert_eq!(
        comment.presentation.rendered_exact,
        "interval covers the mean"
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_remark_about_the_whole_document_needs_no_passage() {
    let Some(deployment) = deployment("anchor-whole").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(&room, "", "", "", "commenting");
    // `AddComment::new` treats an empty selection as a document-level remark
    // only when the caller asked for one explicitly; do so here.
    let config = Configuration::default();
    let mut cmd2 = AddComment::new(
        deployment.catalog.clone(),
        room.document_id,
        Uuid::new_v4(),
        &config,
        "commenting",
        "A remark.",
        &deployment.writer(),
        "",
        "",
        "",
        None,
        true,
        None,
        None,
    )
    .unwrap();
    let _ = &mut cmd; // the plain-selection variant above is unused; kept for its type only.
    let comment = room
        .command(&deployment.authority(), &mut cmd2)
        .await
        .unwrap();
    assert!(matches!(
        comment.original_anchor.as_ref().unwrap().target,
        CommentTarget::Document
    ));
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn words_nowhere_in_the_document_are_refused_rather_than_placed() {
    let Some(deployment) = deployment("anchor-notfound").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "a sentence from another paper entirely",
        "",
        "",
        "commenting",
    );
    let error = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap_err();
    match error {
        CommandError::Conflict(message) => {
            assert!(
                message.contains("not in this document's source"),
                "{message}"
            );
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn editing_the_document_moves_where_a_comment_points_and_not_what_it_is_about() {
    let Some(deployment) = deployment("anchor-edited").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let anchor_before = comment.original_anchor.clone().unwrap();
    let started_at = anchor_before.target.source().unwrap().start_utf16;
    let file_before = anchor_before.target.source().unwrap().file_id.clone();

    // Somebody writes a paragraph above it. Nothing about the comment has
    // changed; everything about where it sits has.
    let (vector, edited) = room
        .log()
        .with_head(|doc| (session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    session::put_text(
        &edited,
        "paper.md",
        &format!(
            "# Interval estimates\n\nA new opening paragraph.\n{}",
            &PAPER[21..]
        ),
    );
    let update = session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(
        matches!(ingested, librepaper_engine::log::Ingested::Accepted),
        "{ingested:?}"
    );

    let comments = room.all_comments().await.unwrap();
    let found = comments.iter().find(|item| item.id == comment.id).unwrap();
    assert_eq!(
        found.original_anchor.as_ref().unwrap(),
        &anchor_before,
        "what a comment is about does not move",
    );
    let attachment = found.attachment.as_ref().expect("an attachment");
    assert_eq!(attachment.status, AnchorStatus::Exact);
    assert_eq!(
        attachment.resolved_file_id.as_ref(),
        Some(&file_before),
        "the passage is still in the file it was made in",
    );
    let (start, _) = attachment.resolved_range_utf16.expect("a resolved range");
    assert!(
        start > started_at,
        "the passage moved down the file: {start} is not after {started_at}"
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_passage_that_is_deleted_leaves_a_comment_that_says_so() {
    let Some(deployment) = deployment("anchor-deleted").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let file_id = comment
        .source()
        .expect("a source-text anchor")
        .file_id
        .0
        .clone();

    let (vector, edited) = room
        .log()
        .with_head(|doc| (session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    session::put_text(
        &edited,
        "paper.md",
        "# Interval estimates\n\nA second paragraph, for company.\n",
    );
    let update = session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(
        matches!(ingested, librepaper_engine::log::Ingested::Accepted),
        "{ingested:?}"
    );

    let comments = room.all_comments().await.unwrap();
    let found = comments.iter().find(|item| item.id == comment.id).unwrap();
    assert!(found.source().is_some());
    let attachment = found.attachment.as_ref().expect("an attachment");
    assert_eq!(attachment.status, AnchorStatus::Deleted);
    assert_eq!(attachment.resolved_range_utf16, None);
    // Reading writes nothing: the record is still attached, with its cursors
    // collapsed. The next browser that edits the document is what detaches it.
    let records = records_of(&room, &file_id).await;
    assert_eq!(records.len(), 1);
    assert!(records[0].0.collapsed && !records[0].0.detached);
    deployment.catalog.close().await;
}

/// Two people editing one document must see each other's comments as they
/// are made, and so must the reader who is waiting on an answer.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comment_reaches_the_other_editor_and_the_public_channel_live() {
    let Some(deployment) = deployment("anchor-relay").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let (editor_tx, mut editor_rx) = tokio::sync::mpsc::channel(8);
    let (reader_tx, mut reader_rx) = tokio::sync::mpsc::channel(8);
    room.join(
        1,
        true,
        "editor-1",
        librepaper_engine::storage::outgoing::Sender::from_raw(editor_tx.clone()),
        None,
    )
    .await
    .unwrap();
    room.join(
        2,
        true,
        "editor-2",
        librepaper_engine::storage::outgoing::Sender::from_raw(editor_tx),
        None,
    )
    .await
    .unwrap();
    room.join(
        3,
        false,
        "reader-3",
        librepaper_engine::storage::outgoing::Sender::from_raw(reader_tx),
        None,
    )
    .await
    .unwrap();

    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let payload = json!({"type": "comment", "comment": comment});
    room.broadcast_comment_event(Some(1), &payload).await;
    assert_eq!(
        room.event_comment_read_count(),
        1,
        "editor and reader views share one enriched row read"
    );

    // The editor channel also carries the comment's own source batch, which
    // holds its anchor record, and the durable mark after it. Those are not
    // comments, so the comment checks select by type.
    let editor_frames = received(&mut editor_rx);
    let relayed = frames_of(&editor_frames, "comment");
    assert_eq!(
        relayed.len(),
        1,
        "the submitting socket is not sent its own comment twice"
    );
    assert_eq!(relayed[0]["comment"]["id"], comment.id);
    assert!(relayed[0]["comment"]["original_anchor"].is_object());
    assert!(
        !frames_of(&editor_frames, "doc-update").is_empty(),
        "the comment's source batch is relayed to editors"
    );

    // A reader also gets a source-changed frame for the batch. Only the
    // comment frame is checked here, and no source batch reaches a reader.
    let public_frames = received(&mut reader_rx);
    let public = frames_of(&public_frames, "comment");
    assert_eq!(public.len(), 1);
    assert_eq!(public[0]["comment"]["body"], "A remark.");
    assert!(
        public[0]["comment"]["original_anchor"].is_null(),
        "a reader is never sent a range in a source file: {}",
        public[0],
    );
    assert!(
        frames_of(&public_frames, "doc-update").is_empty(),
        "a reader is never sent a source batch"
    );
    deployment.catalog.close().await;
}

/// A change no single event describes -- here, a document nobody has
/// commented on gaining one -- states the whole list to every peer, in the
/// view each is entitled to.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_snapshot_states_the_list_each_peer_may_see() {
    let Some(deployment) = deployment("anchor-snapshot").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let (editor_tx, mut editor_rx) = tokio::sync::mpsc::channel(8);
    let (reader_tx, mut reader_rx) = tokio::sync::mpsc::channel(8);
    room.join(
        1,
        true,
        "editor-1",
        librepaper_engine::storage::outgoing::Sender::from_raw(editor_tx),
        None,
    )
    .await
    .unwrap();
    room.join(
        2,
        false,
        "reader-2",
        librepaper_engine::storage::outgoing::Sender::from_raw(reader_tx),
        None,
    )
    .await
    .unwrap();

    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    room.command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    room.broadcast_comments_changed().await;

    // The frame says the collection moved and how large it is. It carries
    // no rows at all -- that is the whole point of it -- so what both
    // peers get is a count and a revision, and each re-reads the pages it
    // is actually holding.
    // The comment's own source batch and durable mark reach an editor too, and
    // a reader gets a source-changed frame. Those are not the list, so only the
    // list frames are checked.
    let editors = frames_of(&received(&mut editor_rx), "comments-changed");
    assert_eq!(editors.len(), 1);
    assert!(editors[0].get("comments").is_none(), "no rows on the wire");
    assert_eq!(editors[0]["state"]["total"], 1);
    assert_eq!(editors[0]["state"]["open"], 1);
    assert!(editors[0]["state"]["revision"]
        .as_str()
        .is_some_and(|r| !r.is_empty()));

    let readers = frames_of(&received(&mut reader_rx), "comments-changed");
    assert_eq!(readers.len(), 1);
    assert!(readers[0].get("comments").is_none());
    assert_eq!(readers[0]["state"]["total"], 1);
    deployment.catalog.close().await;
}

/// The frames of one type, in the order they were queued.
fn frames_of(frames: &[Value], kind: &str) -> Vec<Value> {
    frames
        .iter()
        .filter(|frame| frame["type"] == kind)
        .cloned()
        .collect()
}

/// Whatever is already queued for a peer, as JSON.
fn received(
    rx: &mut tokio::sync::mpsc::Receiver<librepaper_engine::storage::outgoing::Outgoing>,
) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        let text = match frame {
            librepaper_engine::storage::outgoing::Outgoing::Text(value) => value,
            librepaper_engine::storage::outgoing::Outgoing::SharedText(value) => value.to_string(),
            librepaper_engine::storage::outgoing::Outgoing::Close(_) => continue,
        };
        out.push(serde_json::from_str(&text).expect("every frame is JSON"));
    }
    out
}

/// The MCP action gate, against a comment that really was stored.
///
/// `validate_existing_action` reads the value `comment_by_id` hands MCP, so
/// this drives it with exactly that value rather than with one built by hand.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn mcp_validates_a_comment_as_it_is_actually_served() {
    use crate::room::comments::validate_existing_action;

    let Some(deployment) = deployment("anchor-mcp-gate").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let created = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();

    let served = room
        .comment_by_id(&created.id)
        .await
        .unwrap()
        .expect("the comment is readable");

    let author = format!("account:{}", deployment.account_id);
    let version = served.version();
    let check = |action: &str, caller: &str, editor: bool| {
        validate_existing_action(
            action,
            &served.id,
            Some(&served),
            &version,
            Some(&version),
            caller,
            editor,
        )
    };

    check("resolve", &author, false).expect("the comment's own author may resolve it");
    check("delete", "an-editor", true).expect("an editor may delete it");
    check("delete", &author, false).expect("the author may delete it");

    assert_eq!(
        check("delete", "someone-else", false)
            .expect_err("an unrelated commenter may not delete")
            .code,
        "permission_changed",
    );
    assert_eq!(
        validate_existing_action(
            "resolve",
            &served.id,
            Some(&served),
            "a-version-read-two-edits-ago",
            Some(&version),
            &author,
            false,
        )
        .expect_err("a stale expected_version is refused")
        .code,
        "conflict",
    );
    deployment.catalog.close().await;
}

/// The anchor records of one file, each with the status a reader would see.
async fn records_of(room: &Room, file_id: &str) -> Vec<(anchors::Anchor, anchors::Status)> {
    room.log()
        .with_head(|doc| {
            anchors::resolve_anchors(doc, file_id)
                .into_iter()
                .map(|record| {
                    let status = anchors::status_of(doc, &record);
                    (record, status)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap()
}

/// Deletes a comment's anchor record, as a room from before records existed
/// would have none.
struct DropAnchorRecord {
    id: String,
}

impl SequencerCommand for DropAnchorRecord {
    type Output = ();

    fn name(&self) -> &'static str {
        "test-drop-anchor"
    }

    fn authority(&self) -> Rung {
        Rung::Editor
    }

    fn evaluate(&mut self, head: &Head<'_>) -> Result<Option<PreparedSource>, CommandError> {
        let id = self.id.clone();
        head.prepare(1, |draft| {
            draft
                .get_map(anchors::ANCHORS)
                .delete(&id)
                .map_err(|error| error.to_string())
        })
        .map_err(CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<(), CommandError>> {
        Box::pin(async move { Ok(()) })
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comment_writes_its_anchor_record() {
    let Some(deployment) = deployment("anchor-record").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let target = comment.source().expect("a source-text anchor");
    let found = records_of(&room, &target.file_id.0).await;
    assert_eq!(found.len(), 1, "the comment's record is the only one");
    let (record, status) = &found[0];
    assert_eq!(record.id, comment.id);
    assert!(!record.detached);
    assert_eq!(
        (record.from, record.to),
        (target.start_utf16 as usize, target.end_utf16 as usize),
    );
    assert_eq!(record.exact, target.exact);
    assert_eq!(*status, anchors::Status::Exact);
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn deleting_a_comment_deletes_its_record() {
    let Some(deployment) = deployment("anchor-delete").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let file_id = comment
        .source()
        .expect("a source-text anchor")
        .file_id
        .0
        .clone();
    assert_eq!(records_of(&room, &file_id).await.len(), 1);

    let mut delete = DeleteComment::new(
        deployment.catalog.clone(),
        room.document_id,
        Uuid::parse_str(&comment.id).unwrap(),
        &deployment.writer(),
    );
    room.command(&deployment.authority(), &mut delete)
        .await
        .unwrap();
    assert!(
        records_of(&room, &file_id).await.is_empty(),
        "the record went with its comment"
    );
    assert!(room.comment_by_id(&comment.id).await.unwrap().is_none());
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_room_opening_backfills_old_comments() {
    let Some(deployment) = deployment("anchor-backfill").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let file_id = comment
        .source()
        .expect("a source-text anchor")
        .file_id
        .0
        .clone();

    // A comment from before records existed: its record is gone.
    let mut drop_record = DropAnchorRecord {
        id: comment.id.clone(),
    };
    room.command(&deployment.authority(), &mut drop_record)
        .await
        .unwrap();
    assert!(records_of(&room, &file_id).await.is_empty());

    deployment.rooms.close(&deployment.slug).await;
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let found = records_of(&room, &file_id).await;
    assert_eq!(found.len(), 1, "opening the room wrote the record back");
    let (record, status) = &found[0];
    assert_eq!(record.id, comment.id);
    assert!(!record.detached, "its passage is still there");
    assert_eq!(*status, anchors::Status::Exact);
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_room_opening_detaches_a_backfilled_comment_whose_passage_is_gone() {
    let Some(deployment) = deployment("anchor-backfill-gone").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "commenting",
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let source = comment.source().expect("a source-text anchor");
    let (file_id, exact) = (source.file_id.0.clone(), source.exact.clone());

    let mut drop_record = DropAnchorRecord {
        id: comment.id.clone(),
    };
    room.command(&deployment.authority(), &mut drop_record)
        .await
        .unwrap();

    // Edited while the comment has no record: the passage is deleted.
    let (vector, edited) = room
        .log()
        .with_head(|doc| (session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    session::put_text(
        &edited,
        "paper.md",
        "# Interval estimates\n\nA second paragraph, for company.\n",
    );
    let update = session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(
        matches!(ingested, librepaper_engine::log::Ingested::Accepted),
        "{ingested:?}"
    );

    deployment.rooms.close(&deployment.slug).await;
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let found = records_of(&room, &file_id).await;
    assert_eq!(found.len(), 1, "opening the room wrote the record back");
    let (record, status) = &found[0];
    assert_eq!(record.id, comment.id);
    assert!(
        record.detached,
        "the passage is gone, so the record is detached"
    );
    assert_eq!(record.exact, exact, "the quote is kept");
    assert_eq!(*status, anchors::Status::Deleted);
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_whole_document_comment_has_no_record() {
    let Some(deployment) = deployment("anchor-whole-record").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let config = Configuration::default();
    let mut cmd = AddComment::new(
        deployment.catalog.clone(),
        room.document_id,
        Uuid::new_v4(),
        &config,
        "commenting",
        "A remark.",
        &deployment.writer(),
        "",
        "",
        "",
        None,
        true,
        None,
        None,
    )
    .unwrap();
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    assert!(matches!(
        comment.original_anchor.as_ref().unwrap().target,
        CommentTarget::Document
    ));
    let has_record = room
        .log()
        .with_head(|doc| doc.get_map(anchors::ANCHORS).get(&comment.id).is_some())
        .await
        .unwrap();
    assert!(
        !has_record,
        "a remark about the whole document has no passage"
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_retried_passage_comment_answers_from_its_row_and_writes_no_second_record() {
    let Some(deployment) = deployment("anchor-retry").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let config = Configuration::default();
    // One request, sent twice with the same id, as a client that lost the
    // first answer would send it.
    let id = Uuid::new_v4();
    let request = || {
        AddComment::new(
            deployment.catalog.clone(),
            room.document_id,
            id,
            &config,
            "commenting",
            "A remark.",
            &deployment.writer(),
            "interval covers the mean",
            "Interval estimates The ",
            " of the posterior.",
            Some(0),
            false,
            None,
            None,
        )
        .expect("a well-formed comment")
    };

    let before = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    let mut first = request();
    let (comment, replayed) = room
        .command_reporting_replay(&deployment.authority(), &mut first)
        .await
        .unwrap();
    assert!(!replayed, "the first attempt is a fresh commit");
    let after_first = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    assert_ne!(after_first, before, "the passage's anchor record moved the frontier");

    let mut retry = request();
    let (again, replayed) = room
        .command_reporting_replay(&deployment.authority(), &mut retry)
        .await
        .unwrap();
    assert!(replayed, "the retry is answered from its row");
    assert_eq!(again.id, comment.id, "the retry returns the first comment");
    assert_eq!(
        again.original_anchor,
        comment.original_anchor,
        "the retry answers with the first record's anchor",
    );
    let after_retry = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    assert_eq!(after_retry, after_first, "the retry wrote no second anchor record");

    let comments = room.all_comments().await.unwrap();
    assert_eq!(comments.len(), 1, "the retry added no second comment");
    assert_eq!(comments[0].id, comment.id);
    deployment.catalog.close().await;
}
