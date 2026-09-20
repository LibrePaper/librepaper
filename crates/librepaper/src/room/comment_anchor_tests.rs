//! A comment, from the words a reader selected to the range it is about.
//!
//! The pieces are covered on their own -- `locate` finds a passage, `resolve`
//! follows one, [`AddComment`] and its siblings write it down -- and what is
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
use crate::document::session;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::log::Registry;
use crate::storage::blob::FsStore;
use crate::storage::postgres::{Authority, PostgresCatalog, PostgresOptions};
use serde_json::json;

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    rooms: Rooms,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    account_id: Uuid,
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
    sqlx::query!(
        "TRUNCATE document_updates,document_bases,document_proposal_hunks,document_proposals,\
         replies,annotations,document_labels,document_assets,share_links,grants,documents,\
         accounts CASCADE",
    )
    .execute(catalog.pool())
    .await
    .unwrap();
    let writer = catalog.claim_writer().await.unwrap();
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
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );
    let store = Arc::new(
        Store::open_with_catalog(
            blobs.clone(),
            config.clone(),
            catalog.clone(),
            registry.clone(),
        )
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
        }
    }

    /// The owner, at editor rung, which is also more than enough for a
    /// commenter-gated command -- `authorize_annotation_mutation` admits an
    /// owner at either rung.
    fn mutation_authorization(&self) -> crate::storage::postgres::MutationAuthorization {
        crate::storage::postgres::MutationAuthorization {
            principal_key: self.account_id.to_string(),
            account_id: Some(self.account_id),
            session_generation: None,
            token_hash: None,
            policy_editor: true,
        }
    }

    fn add_comment(
        &self,
        room: &Room,
        exact: &str,
        prefix: &str,
        suffix: &str,
        motivation: &str,
        proposed: Option<&str>,
    ) -> AddComment {
        let config = Configuration::default();
        AddComment::new(
            self.catalog.clone(),
            room.document_id,
            Uuid::new_v4(),
            &config,
            motivation,
            "A remark.",
            "Reviewer",
            Some(self.account_id),
            format!("account:{}", self.account_id),
            self.mutation_authorization(),
            exact,
            prefix,
            suffix,
            Some(0),
            false,
            None,
            proposed,
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
        None,
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
    // made durable, not an archive: a comment writes no source of its own.
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
    let mut cmd = deployment.add_comment(&room, "", "", "", "commenting", None);
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
        "Reviewer",
        Some(deployment.account_id),
        format!("account:{}", deployment.account_id),
        deployment.mutation_authorization(),
        "",
        "",
        "",
        None,
        true,
        None,
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
        None,
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
        None,
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let anchor_before = comment.original_anchor.clone().unwrap();
    let started_at = anchor_before.target.source().unwrap().start_utf16;

    // `reattach_comments` only ever moves what is in the room's warm cache
    // (`self.comments`): under §8.3 there is no `annotation_live_state` to
    // fall back to, and a cache nobody has loaded is a cache nobody is
    // watching, so it is deliberately left alone rather than rebuilt for no
    // reader. `Room::comments` is what a real caller loads it through --
    // and it also runs the first `reattach_comments` itself, right after
    // the load, per its own doc comment -- so a caller of the direct
    // method below expects that same priming.
    let _ = room.comments().await.unwrap();

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
        matches!(ingested, crate::log::Ingested::Accepted),
        "{ingested:?}"
    );

    let moved = room.reattach_comments().await.unwrap();
    assert_eq!(moved.len(), 1, "the one comment on this document moved");
    let comments = room.comments().await.unwrap();
    let found = comments.iter().find(|item| item.id == comment.id).unwrap();
    assert_eq!(
        found.original_anchor.as_ref().unwrap(),
        &anchor_before,
        "what a comment is about does not move",
    );
    let attachment = found.attachment.as_ref().expect("an attachment");
    assert_eq!(attachment.status, AnchorStatus::Exact);
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
        None,
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();

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
        matches!(ingested, crate::log::Ingested::Accepted),
        "{ingested:?}"
    );

    room.reattach_comments().await.unwrap();
    let comments = room.comments().await.unwrap();
    let found = comments.iter().find(|item| item.id == comment.id).unwrap();
    assert!(found.source().is_some());
    assert_eq!(
        found.attachment.as_ref().expect("an attachment").status,
        AnchorStatus::Deleted
    );
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
        super::outgoing::Sender::from_raw(editor_tx.clone()),
        None,
    )
    .await
    .unwrap();
    room.join(
        2,
        true,
        "editor-2",
        super::outgoing::Sender::from_raw(editor_tx),
        None,
    )
    .await
    .unwrap();
    room.join(
        3,
        false,
        "reader-3",
        super::outgoing::Sender::from_raw(reader_tx),
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
        None,
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    let payload = json!({"type": "comment", "comment": comment});
    room.broadcast_comment_event(Some(1), &payload).await;

    let relayed = received(&mut editor_rx);
    assert_eq!(
        relayed.len(),
        1,
        "the submitting socket is not sent it twice"
    );
    assert_eq!(relayed[0]["comment"]["id"], comment.id);

    let public = received(&mut reader_rx);
    assert_eq!(public.len(), 1);
    assert_eq!(public[0]["comment"]["body"], "A remark.");
    assert!(
        public[0]["comment"]["original_anchor"].is_null(),
        "a reader is never sent a range in a source file: {}",
        public[0],
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
        super::outgoing::Sender::from_raw(editor_tx),
        None,
    )
    .await
    .unwrap();
    room.join(
        2,
        false,
        "reader-2",
        super::outgoing::Sender::from_raw(reader_tx),
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
        None,
    );
    room.command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    room.forget_comments().await;
    room.broadcast_comment_snapshot(7).await;

    let editors = received(&mut editor_rx);
    assert_eq!(editors.len(), 1);
    assert_eq!(editors[0]["comment_digest"], 7);
    assert_eq!(
        editors[0]["comments"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1
    );

    let readers = received(&mut reader_rx);
    assert_eq!(readers.len(), 1);
    let public = readers[0]["comments"].as_array().expect("a list");
    assert_eq!(public.len(), 1);
    assert!(public[0]["original_anchor"].is_null());
    deployment.catalog.close().await;
}

/// A suggestion is the one annotation a reader never receives: it is an edit
/// to the document, not a remark about it.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_suggestion_stays_out_of_the_public_channel() {
    let Some(deployment) = deployment("anchor-suggestion").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let (editor_tx, mut editor_rx) = tokio::sync::mpsc::channel(8);
    let (reader_tx, mut reader_rx) = tokio::sync::mpsc::channel(8);
    room.join(
        1,
        true,
        "editor-1",
        super::outgoing::Sender::from_raw(editor_tx),
        None,
    )
    .await
    .unwrap();
    room.join(
        2,
        false,
        "reader-2",
        super::outgoing::Sender::from_raw(reader_tx),
        None,
    )
    .await
    .unwrap();

    let mut cmd = deployment.add_comment(
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
        "editing",
        Some("*interval* contains the mean"),
    );
    let comment = room
        .command(&deployment.authority(), &mut cmd)
        .await
        .unwrap();
    assert!(
        !comment.proposal.is_empty(),
        "a suggestion opens a proposal"
    );
    let payload = json!({"type": "comment", "comment": comment});
    room.broadcast_comment_event(None, &payload).await;

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
