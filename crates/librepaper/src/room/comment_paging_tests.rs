//! The comment traversal: that it reaches everything, repeats nothing, and
//! stays bounded doing it.
//!
//! Two generations of bug are covered here. The first was a read that
//! stopped at one SQL page -- `annotations(document_id, None, 500)`, with
//! `find_annotation` scanning that same page for one row -- so comment 501
//! was written, acknowledged, and then invisible to every view and refused
//! as "unknown comment" by resolve, delete, refine and accept; and a reply
//! read with a fixed `LIMIT 5001` that failed the *entire* load rather than
//! one thread. The second was the whole-snapshot protocol that replaced it:
//! SQL paged, but every consumer then collected the result, so a 16 MiB
//! read guard refused collections that were larger than one process wanted
//! to hold. There is no admission limit on how many comments a document holds,
//! which is why the paging contract has to hold for an arbitrary count.
//!
//! What is here now is a bounded keyset traversal
//! (`docs/protocol/comments-v1.md`), and these tests are about its
//! boundaries: page edges, tied timestamps, the byte budget, cursors
//! presented where they do not belong, and concurrent writes. They seed in
//! bulk through the catalogue rather than one sequencer command per comment
//! (a 5,001-reply document is not reachable in a test otherwise), and then
//! read and mutate through the real paths: `Room::comment_page`,
//! `Room::reply_page`, `Room::comment_by_id` and
//! `room.command(&authority, &mut cmd)`.
//!
//! They need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage.

use std::collections::HashSet;

use super::*;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::log::Registry;
use crate::room::annotation::CommentTarget;
use crate::storage::blob::FsStore;
use crate::storage::postgres::annotations::{
    ANNOTATION_PAGE_MAX, REPLY_LOOKUP_MAX, REPLY_PAGE_MAX,
};
use crate::storage::postgres::{
    AnnotationRecord, Authority, MutationAuthorization, NewAccount, NewAnnotation, NewReply,
    PostgresCatalog,
};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n";

/// One more than a whole page, so the first comment past the old ceiling is
/// the one the assertions name.
const PAST_THE_PAGE: usize = ANNOTATION_PAGE_MAX as usize + 2;

struct Deployment {
    rooms: Rooms,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    account_id: Uuid,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn deployment(slug: &str) -> Option<Deployment> {
    let catalog = crate::tests::catalog().await?;
    let writer = catalog.claim_writer().await.unwrap();
    let account = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("paging-owner".into()),
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
    let store = Store::open_with_catalog(
        blobs.clone(),
        config.clone(),
        catalog.clone(),
        registry.clone(),
    );
    let actor = MutationActor {
        account_id: account.id.to_string(),
        owner_key: "owner".into(),
        session_generation: account.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        unowned_publisher: false,
    };
    for name in [slug, &format!("{slug}-other")] {
        store
            .put_directory_as_actor(
                DocumentInput {
                    slug: name.into(),
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
    }
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

    /// Who this deployment writes as: the owner, at editor rung.
    fn writer(&self) -> crate::room::CommentAuthor {
        crate::room::CommentAuthor::new(
            "Reviewer",
            Some(self.account_id),
            format!("account:{}", self.account_id),
            self.mutation_authorization(),
            true,
        )
    }

    fn mutation_authorization(&self) -> MutationAuthorization {
        MutationAuthorization {
            principal_key: self.account_id.to_string(),
            account_id: Some(self.account_id),
            session_generation: None,
            token_hash: None,
            policy_editor: true,
        }
    }

    fn author_key(&self) -> String {
        format!("account:{}", self.account_id)
    }

    fn whole_document_comment(&self, document_id: Uuid, body: &str) -> NewAnnotation {
        NewAnnotation {
            document_id,
            kind: "comment".into(),
            body: body.into(),
            author_account_id: Some(self.account_id),
            author_key: self.author_key(),
            author_label: "Reviewer".into(),
            color: None,
            proposal_id: None,
            original_anchor: OriginalAnchor {
                source_sequence: 1,
                frontier: vec![1, 2, 3],
                target: CommentTarget::Document,
            },
            presentation: Default::default(),
            render_digest: None,
            attachment: None,
        }
    }

    /// `count` whole-document comments on `document_id`, written straight to
    /// the catalogue in one transaction. They share a `created_at` -- `now()`
    /// inside a transaction is the transaction's timestamp -- which is
    /// exactly the tie the `(created_at, id)` cursor has to break, and which
    /// a slower seeding path would never produce.
    async fn seed_comments(&self, document_id: Uuid, count: usize) -> Vec<Uuid> {
        let mut tx = self.catalog.pool().begin().await.unwrap();
        let mut ids = Vec::with_capacity(count);
        for index in 0..count {
            let id = Uuid::new_v4();
            self.catalog
                .put_annotation_authorized(
                    &mut tx,
                    id,
                    self.whole_document_comment(document_id, &format!("Remark {index}")),
                    &self.mutation_authorization(),
                    false,
                )
                .await
                .unwrap();
            ids.push(id);
        }
        tx.commit().await.unwrap();
        ids
    }

    /// `count` suggestions, which is the one kind a reader may not see.
    async fn seed_suggestions(&self, document_id: Uuid, count: usize) -> Vec<Uuid> {
        let mut tx = self.catalog.pool().begin().await.unwrap();
        let mut ids = Vec::with_capacity(count);
        for index in 0..count {
            let id = Uuid::new_v4();
            // A suggestion needs a proposal row of its own:
            // `annotations_suggestion_proposal` refuses one without it,
            // which is what makes "a suggestion is an edit" true in the
            // schema and not only in the code.
            let proposal = self
                .catalog
                .open_proposal(
                    &mut tx,
                    crate::storage::postgres::NewProposal {
                        document_id,
                        id: Uuid::new_v4(),
                        author: self.author_key(),
                        author_peer: (index as i64) | 1,
                        base_frontiers: Vec::new(),
                        tip_frontiers: Vec::new(),
                        branch_bytes: Vec::new(),
                    },
                )
                .await
                .unwrap();
            let mut input = self.whole_document_comment(document_id, &format!("Edit {index}"));
            input.kind = "suggestion".into();
            input.proposal_id = Some(proposal.id);
            self.catalog
                .put_annotation_authorized(
                    &mut tx,
                    id,
                    input,
                    &self.mutation_authorization(),
                    false,
                )
                .await
                .unwrap();
            ids.push(id);
        }
        tx.commit().await.unwrap();
        ids
    }

    async fn seed_replies(&self, document_id: Uuid, comment_id: Uuid, count: usize) {
        let mut tx = self.catalog.pool().begin().await.unwrap();
        for index in 0..count {
            self.catalog
                .create_reply_authorized(
                    &mut tx,
                    document_id,
                    NewReply {
                        id: Uuid::new_v4(),
                        annotation_id: comment_id,
                        author_account_id: Some(self.account_id),
                        author_key: self.author_key(),
                        author_label: "Reviewer".into(),
                        body: format!("Reply {index}"),
                    },
                    &self.mutation_authorization(),
                )
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
    }

    async fn room(&self) -> Arc<Room> {
        self.rooms.get(&self.slug).await.unwrap()
    }

    async fn other_room(&self) -> Arc<Room> {
        self.rooms
            .get(&format!("{}-other", self.slug))
            .await
            .unwrap()
    }
}

/// The ids `load` returns, in the order it returns them.
fn ids_of(comments: &[Comment]) -> Vec<String> {
    comments.iter().map(|comment| comment.id.clone()).collect()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn every_comment_past_the_page_size_is_still_read_back() {
    let Some(deployment) = deployment("paging-read").await else {
        return;
    };
    let room = deployment.room().await;
    let written = deployment
        .seed_comments(room.document_id, PAST_THE_PAGE)
        .await;

    room.forget_comments().await;
    let comments = room.all_comments(true).await.unwrap();
    assert_eq!(
        comments.len(),
        PAST_THE_PAGE,
        "a document with more comments than one page still loads all of them",
    );
    let seen: HashSet<String> = ids_of(&comments).into_iter().collect();
    for id in &written {
        assert!(
            seen.contains(&id.to_string()),
            "comment {id} was accepted and must be readable",
        );
    }
    assert_eq!(
        seen.len(),
        PAST_THE_PAGE,
        "the page walk must not repeat a row at a page boundary",
    );

    // The same must be true of what a browser is actually handed: the
    // snapshot every socket `hello` and document GET is built from.
    // And the same must be true of what a browser is actually handed: a
    // page, plus the authoritative count of everything behind it.
    let state = room.comment_state(true).await.unwrap();
    assert_eq!(state.total, PAST_THE_PAGE as i64);
    let (views, first) = room
        .comment_page(
            None,
            comments::COMMENT_PAGE_DEFAULT,
            &deployment.author_key(),
            true,
        )
        .await
        .unwrap();
    assert_eq!(views.len(), comments::COMMENT_PAGE_DEFAULT);
    assert!(!first.complete, "a page is a page, not the collection");
    assert!(first.next.is_some());

    deployment.catalog.close().await;
}

/// The boundary itself: a document of exactly one page must not cost a
/// second query's worth of rows, and one row past it must not be lost.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_page_boundary_is_exact_in_both_directions() {
    let Some(deployment) = deployment("paging-boundary").await else {
        return;
    };
    let room = deployment.room().await;
    deployment
        .seed_comments(room.document_id, ANNOTATION_PAGE_MAX as usize)
        .await;
    room.forget_comments().await;
    assert_eq!(
        room.all_comments(true).await.unwrap().len(),
        ANNOTATION_PAGE_MAX as usize,
        "exactly one full page",
    );

    // A comment appended after the first page -- the row that the old
    // single-page read dropped.
    let late = deployment.seed_comments(room.document_id, 1).await[0];
    room.forget_comments().await;
    let comments = room.all_comments(true).await.unwrap();
    assert_eq!(comments.len(), ANNOTATION_PAGE_MAX as usize + 1);
    assert!(
        ids_of(&comments).contains(&late.to_string()),
        "the row one past the page size is the one that used to disappear",
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn resolve_reply_and_delete_reach_a_comment_past_the_page_size() {
    let Some(deployment) = deployment("paging-mutate").await else {
        return;
    };
    let room = deployment.room().await;
    let written = deployment
        .seed_comments(room.document_id, PAST_THE_PAGE)
        .await;
    room.forget_comments().await;
    let comments = room.all_comments(true).await.unwrap();
    // Whichever row sorts last is past the first page by construction, and
    // it is the one `find_annotation`'s old first-page scan could not see.
    let last: Uuid = comments.last().unwrap().id.parse().unwrap();
    assert!(written.contains(&last));

    let mut resolve = ResolveComment::new(
        deployment.catalog.clone(),
        room.document_id,
        last,
        true,
        &deployment.writer(),
    );
    let outcome = room
        .command(&deployment.authority(), &mut resolve)
        .await
        .expect("a comment past the first page can be resolved");
    assert!(
        outcome.resolved,
        "the command reports the row it just wrote, read back inside its own \
         transaction rather than from a second connection that cannot see it",
    );
    assert!(outcome.resolved_at.is_some());

    room.forget_comments().await;
    let reloaded = room.all_comments(true).await.unwrap();
    let stored = reloaded
        .iter()
        .find(|comment| comment.id == last.to_string())
        .expect("still there");
    assert!(stored.resolved, "and the row really is resolved");

    // A reply reaches it too -- `AddReply` looks the parent up the same way.
    let mut reply = AddReply::new(
        deployment.catalog.clone(),
        room.document_id,
        Uuid::new_v4(),
        last,
        &Configuration::default(),
        "A reply to a late comment.",
        &deployment.writer(),
    )
    .unwrap();
    room.command(&deployment.authority(), &mut reply)
        .await
        .expect("a comment past the first page can be replied to");

    let mut delete = DeleteComment::new(
        deployment.catalog.clone(),
        room.document_id,
        last,
        &deployment.writer(),
    );
    room.command(&deployment.authority(), &mut delete)
        .await
        .expect("a comment past the first page can be deleted");
    room.forget_comments().await;
    assert_eq!(
        room.all_comments(true).await.unwrap().len(),
        PAST_THE_PAGE - 1,
        "and the delete really removed it",
    );
    deployment.catalog.close().await;
}

/// The reply read used to refuse above 5,000 rows across the requested
/// annotations, which took the whole document's comments down with it.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_document_past_the_aggregate_reply_ceiling_still_loads() {
    let Some(deployment) = deployment("paging-replies").await else {
        return;
    };
    let room = deployment.room().await;
    let comments = deployment.seed_comments(room.document_id, 6).await;
    // Spread over several threads, and more than 5,000 in total: the old
    // ceiling was aggregate, so no per-thread limit would have kept a
    // document under it.
    let per_thread = 900;
    for comment in &comments {
        deployment
            .seed_replies(room.document_id, *comment, per_thread)
            .await;
    }
    let total = per_thread * comments.len();
    assert!(total > 5_000, "the ceiling this test is about");

    room.forget_comments().await;
    let loaded = room.all_comments(true).await.unwrap();
    assert_eq!(loaded.len(), comments.len());
    assert_eq!(
        loaded
            .iter()
            .map(|comment| comment.replies.len())
            .sum::<usize>(),
        total,
        "every reply is read back, in pages, rather than the read failing",
    );
    for comment in &loaded {
        assert_eq!(
            comment.replies.len(),
            per_thread,
            "and each thread keeps its own replies -- the page walk must not \
             spill one thread's rows into another",
        );
    }
    deployment.catalog.close().await;
}

/// One thread alone can exceed a reply page, which is a different boundary
/// from the aggregate one above.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn one_thread_longer_than_a_reply_page_is_read_whole() {
    let Some(deployment) = deployment("paging-one-thread").await else {
        return;
    };
    let room = deployment.room().await;
    let comment = deployment.seed_comments(room.document_id, 1).await[0];
    let count = REPLY_PAGE_MAX as usize + 1;
    deployment
        .seed_replies(room.document_id, comment, count)
        .await;
    room.forget_comments().await;
    let loaded = room.all_comments(true).await.unwrap();
    assert_eq!(loaded[0].replies.len(), count);
    let unique: HashSet<&str> = loaded[0]
        .replies
        .iter()
        .map(|reply| reply.id.as_str())
        .collect();
    assert_eq!(unique.len(), count, "no reply is returned twice");
    deployment.catalog.close().await;
}

/// A comment id is not a capability. The by-id lookup that replaced the
/// first-page scan puts `document_id` in the predicate rather than checking
/// it afterwards, so naming another document's comment is "unknown comment"
/// and not a read of someone else's row.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comment_id_from_another_document_is_not_reachable() {
    let Some(deployment) = deployment("paging-isolation").await else {
        return;
    };
    let room = deployment.room().await;
    let other = deployment.other_room().await;
    let elsewhere = deployment.seed_comments(other.document_id, 1).await[0];

    let mut resolve = ResolveComment::new(
        deployment.catalog.clone(),
        room.document_id,
        elsewhere,
        true,
        &deployment.writer(),
    );
    let error = match room.command(&deployment.authority(), &mut resolve).await {
        Err(error) => error,
        Ok(_) => panic!("another document's comment is not this document's to resolve"),
    };
    assert!(
        matches!(&error, CommandError::Conflict(message) if message == "unknown comment"),
        "{error:?}",
    );

    // And the write really did not happen, on either side.
    other.forget_comments().await;
    assert!(
        !other.all_comments(true).await.unwrap()[0].resolved,
        "a refused command leaves no partial change behind",
    );
    assert!(
        deployment
            .catalog
            .annotation(room.document_id, elsewhere)
            .await
            .unwrap()
            .is_none(),
        "the document-scoped lookup does not see it either",
    );
    assert!(
        deployment
            .catalog
            .annotation(other.document_id, elsewhere)
            .await
            .unwrap()
            .is_some(),
        "though its own document still has it",
    );
    deployment.catalog.close().await;
}

/// The retry mechanism §7.2 relies on, at the boundary: a create replayed
/// with the same id returns the row it already wrote rather than a second
/// one, and neither copy is lost to the page walk.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_retried_create_past_the_page_size_stays_one_comment() {
    let Some(deployment) = deployment("paging-retry").await else {
        return;
    };
    let room = deployment.room().await;
    deployment
        .seed_comments(room.document_id, ANNOTATION_PAGE_MAX as usize)
        .await;

    let id = Uuid::new_v4();
    let input = deployment.whole_document_comment(room.document_id, "A retried remark");
    let mut written: Vec<AnnotationRecord> = Vec::new();
    for _ in 0..2 {
        let mut tx = deployment.catalog.pool().begin().await.unwrap();
        written.push(
            deployment
                .catalog
                .put_annotation_authorized(
                    &mut tx,
                    id,
                    input.clone(),
                    &deployment.mutation_authorization(),
                    false,
                )
                .await
                .unwrap(),
        );
        tx.commit().await.unwrap();
    }
    assert_eq!(written[0].id, written[1].id);
    assert_eq!(written[0].created_at, written[1].created_at);

    room.forget_comments().await;
    let comments = room.all_comments(true).await.unwrap();
    assert_eq!(
        comments.len(),
        ANNOTATION_PAGE_MAX as usize + 1,
        "the retry wrote one row, not two",
    );
    assert!(ids_of(&comments).contains(&id.to_string()));
    deployment.catalog.close().await;
}

/// Comments created while the walk is in flight. The walk is not a snapshot
/// -- each page is its own statement -- so the contract is the weaker and
/// honest one: it never loses a row that existed before it started, and it
/// never returns a row twice, whatever arrives meanwhile.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_concurrent_create_neither_duplicates_nor_drops_a_row() {
    let Some(deployment) = deployment("paging-concurrent").await else {
        return;
    };
    let room = deployment.room().await;
    let before = deployment
        .seed_comments(room.document_id, PAST_THE_PAGE)
        .await;

    let catalog = deployment.catalog.clone();
    let document_id = room.document_id;
    let input = deployment.whole_document_comment(document_id, "Arriving mid-walk");
    let actor = deployment.mutation_authorization();
    let writer = tokio::spawn(async move {
        let mut tx = catalog.pool().begin().await.unwrap();
        let row = catalog
            .put_annotation_authorized(&mut tx, Uuid::new_v4(), input, &actor, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        row.id
    });
    let loaded = comments::walk_all(&deployment.catalog, document_id, true)
        .await
        .unwrap();
    let arrived = writer.await.unwrap();

    let ids = ids_of(&loaded);
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "no row is returned twice");
    for id in &before {
        assert!(
            unique.contains(&id.to_string()),
            "a row that existed before the walk started is never lost",
        );
    }
    assert!(
        ids.len() == before.len() || ids.len() == before.len() + 1,
        "the only row that may or may not appear is the one written during \
         the walk: {} of {}",
        ids.len(),
        before.len(),
    );

    // And once the walk is over, it is certainly there.
    let after = comments::walk_all(&deployment.catalog, document_id, true)
        .await
        .unwrap();
    assert!(ids_of(&after).contains(&arrived.to_string()));
    deployment.catalog.close().await;
}

/// A collection larger than the old 16 MiB whole-snapshot budget.
///
/// That budget refused the read outright: the comments were written,
/// acknowledged, and then unreadable by every consumer. There is no such
/// budget now, because no consumer reads the collection -- a page is
/// bounded by its own byte budget, and the pages behind it are still
/// reachable.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_collection_past_the_old_snapshot_budget_is_still_readable() {
    let Some(deployment) = deployment("paging-past-budget").await else {
        return;
    };
    let room = deployment.room().await;
    // Forty rows of a megabyte each: 40 MiB of bodies, well past the 16 MiB
    // a whole snapshot was allowed to estimate at.
    let seeded = deployment.seed_comments(room.document_id, 40).await;
    sqlx::query("UPDATE annotations SET body=repeat('x', $2) WHERE document_id=$1")
        .bind(room.document_id)
        .bind(1024 * 1024_i32)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();

    let state = room.comment_state(true).await.unwrap();
    assert_eq!(state.total, 40);

    // Each page stays inside the byte budget, and the traversal reaches
    // every row.
    let mut seen: Vec<String> = Vec::new();
    let mut after = None;
    let mut pages = 0;
    loop {
        let page = comments::page(&deployment.catalog, room.document_id, after, 200, true)
            .await
            .unwrap();
        pages += 1;
        let bytes: usize = page
            .comments
            .iter()
            .map(|comment| {
                serde_json::to_vec(comment)
                    .map(|raw| raw.len())
                    .unwrap_or(0)
            })
            .sum();
        assert!(
            bytes <= comments::PAGE_BYTES_MAX + 2 * 1024 * 1024,
            "page {pages} carried {bytes} bytes",
        );
        assert!(
            page.comments.len() < 40,
            "the byte budget, not the row limit, is what cut page {pages} short",
        );
        seen.extend(page.comments.iter().map(|comment| comment.id.clone()));
        if page.complete {
            break;
        }
        after = page.next;
        assert!(
            after.is_some(),
            "an incomplete page always says where to continue"
        );
        assert!(pages < 200, "the traversal must terminate");
    }
    assert!(pages > 1, "40 MiB cannot have been one page");
    let unique: HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), seen.len(), "no row twice");
    assert_eq!(unique.len(), 40, "and none missing");
    for id in &seeded {
        assert!(unique.contains(&id.to_string()));
    }
    deployment.catalog.close().await;
}

/// One stored row wider than a whole page's byte budget. Nothing ever
/// refused one at write time, so a page that returned nothing would strand
/// every row behind it: the row comes back alone, and the page says so.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_row_wider_than_the_byte_budget_is_served_alone_rather_than_refused() {
    let Some(deployment) = deployment("paging-oversize").await else {
        return;
    };
    let room = deployment.room().await;
    // The wide row goes in its own transaction first, so it is first in
    // `(created_at, id)` order regardless of which uuid it drew. Rows
    // written together share a timestamp and sort by id, which is exactly
    // what the tie-breaker test is about and exactly what this one must
    // not depend on.
    let wide = deployment.seed_comments(room.document_id, 1).await[0];
    sqlx::query("UPDATE annotations SET body=repeat('x', $2) WHERE id=$1")
        .bind(wide)
        .bind((comments::PAGE_BYTES_MAX * 2) as i32)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    deployment.seed_comments(room.document_id, 2).await;

    let first = comments::page(&deployment.catalog, room.document_id, None, 200, true)
        .await
        .unwrap();
    assert_eq!(first.comments.len(), 1, "the wide row comes alone");
    assert_eq!(first.comments[0].id, wide.to_string());
    assert!(first.oversize, "and the page says it was over budget");
    assert!(!first.complete);
    let second = comments::page(&deployment.catalog, room.document_id, first.next, 200, true)
        .await
        .unwrap();
    assert_eq!(second.comments.len(), 2, "the rows behind it are reachable");
    assert!(second.complete);
    assert!(!second.oversize);
    deployment.catalog.close().await;
}

/// Ties on `created_at`. Every row seeded here shares one transaction
/// timestamp, so the id tie-breaker is the only thing keeping the traversal
/// from repeating or skipping at a page boundary.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn tied_timestamps_traverse_exactly_once_at_every_page_size() {
    let Some(deployment) = deployment("paging-ties").await else {
        return;
    };
    let room = deployment.room().await;
    let written = deployment.seed_comments(room.document_id, 137).await;
    for limit in [1usize, 2, 7, 136, 137, 138] {
        let mut seen: Vec<String> = Vec::new();
        let mut after = None;
        loop {
            let page = comments::page(&deployment.catalog, room.document_id, after, limit, true)
                .await
                .unwrap();
            seen.extend(page.comments.iter().map(|comment| comment.id.clone()));
            if page.complete {
                break;
            }
            after = page.next;
            assert!(after.is_some());
        }
        let unique: HashSet<&String> = seen.iter().collect();
        assert_eq!(unique.len(), seen.len(), "limit {limit} repeated a row");
        assert_eq!(unique.len(), written.len(), "limit {limit} lost a row");
    }
    deployment.catalog.close().await;
}

/// A thread of more than a thousand replies, walked on its own cursor.
/// The annotation page it belongs to is not enlarged by it: that is the
/// point of paging the two independently.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn one_thread_past_a_thousand_replies_does_not_widen_its_page() {
    let Some(deployment) = deployment("paging-deep-thread").await else {
        return;
    };
    let room = deployment.room().await;
    let ids = deployment.seed_comments(room.document_id, 3).await;
    let deep = ids[0];
    let count = 1_001;
    deployment.seed_replies(room.document_id, deep, count).await;

    let page = comments::page(&deployment.catalog, room.document_id, None, 200, true)
        .await
        .unwrap();
    assert_eq!(
        page.comments.len(),
        3,
        "three comments, however deep one is"
    );
    let thread = page
        .comments
        .iter()
        .find(|comment| comment.id == deep.to_string())
        .unwrap();
    assert_eq!(
        thread.reply_total, count as i64,
        "the count is authoritative"
    );
    assert!(
        thread.replies.len() <= comments::REPLY_PREVIEW,
        "a page carries a preview, not the thread",
    );
    assert!(thread.reply_cursor.is_some(), "and says where the rest is");

    // The rest of it, on its own traversal.
    let mut seen: Vec<String> = thread.replies.iter().map(|r| r.id.clone()).collect();
    let mut after = thread.replies.last().and_then(|reply| reply.at);
    loop {
        let more = comments::thread_page(&deployment.catalog, deep, after, 200)
            .await
            .unwrap();
        assert_eq!(more.total, count as i64);
        seen.extend(more.replies.iter().map(|reply| reply.id.clone()));
        if more.complete {
            break;
        }
        after = more.next;
        assert!(after.is_some());
    }
    let unique: HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), count, "every reply exactly once");
    deployment.catalog.close().await;
}

/// A page may carry ten previews for every comment. The storage lookup must
/// therefore accept more than its old 1,000-row aggregate ceiling.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn many_short_reply_previews_stay_on_one_comment_page() {
    const { assert!(comments::COMMENT_PAGE_MAX * comments::REPLY_PREVIEW <= REPLY_LOOKUP_MAX) };
    let Some(deployment) = deployment("paging-preview-bound").await else {
        return;
    };
    let room = deployment.room().await;
    let comments = deployment.seed_comments(room.document_id, 110).await;
    for comment in &comments {
        deployment
            .seed_replies(room.document_id, *comment, 10)
            .await;
    }

    let page = comments::page(&deployment.catalog, room.document_id, None, 200, true)
        .await
        .unwrap();
    assert_eq!(page.comments.len(), 110);
    assert_eq!(
        page.comments.iter().map(|c| c.replies.len()).sum::<usize>(),
        1_100
    );
    deployment.catalog.close().await;
}

/// The sizing and fetch passes use one repeatable-read snapshot. A concurrent
/// update, delete, and insert cannot widen a measured row or replace it with a
/// later row between those passes.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn page_sizing_snapshot_does_not_substitute_rows() {
    let Some(deployment) = deployment("paging-sizing-snapshot").await else {
        return;
    };
    let room = deployment.room().await;
    let ids = deployment.seed_comments(room.document_id, 3).await;
    let mut tx = deployment.catalog.pool().begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .unwrap();
    let sizes = deployment
        .catalog
        .annotation_sizes_in_transaction(&mut tx, room.document_id, None, 3, true)
        .await
        .unwrap();
    assert_eq!(sizes.len(), 3);

    sqlx::query("UPDATE annotations SET body=repeat('x', 1000000) WHERE id=$1")
        .bind(ids[0])
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM annotations WHERE id=$1")
        .bind(ids[1])
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    let replacement = deployment.seed_comments(room.document_id, 1).await[0];

    let sized_ids: Vec<Uuid> = sizes.iter().map(|row| row.id).collect();
    let rows = deployment
        .catalog
        .annotations_in_transaction(&mut tx, room.document_id, &sized_ids, true)
        .await
        .unwrap();
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), sized_ids);
    assert!(!rows.iter().any(|row| row.id == replacement));
    assert!(rows.iter().find(|row| row.id == ids[0]).unwrap().body.len() < 1000000);
    tx.rollback().await.unwrap();
    deployment.catalog.close().await;
}

/// A cursor is a position, bound to the document, the thread and the query
/// options it was issued for. Presenting one anywhere else is refused
/// rather than answered from the wrong collection.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_cursor_is_refused_outside_what_it_was_issued_for() {
    let Some(deployment) = deployment("paging-cursors").await else {
        return;
    };
    let room = deployment.room().await;
    let other = deployment.other_room().await;
    let ids = deployment.seed_comments(room.document_id, 4).await;
    let page = comments::page(&deployment.catalog, room.document_id, None, 2, true)
        .await
        .unwrap();
    let cursor = comments::encode_cursor(room.document_id, None, true, page.next.unwrap());

    // Its own request accepts it.
    assert!(comments::decode_cursor(&cursor, room.document_id, None, true).is_ok());
    // Another document's does not.
    assert!(comments::decode_cursor(&cursor, other.document_id, None, true).is_err());
    // Nor a thread's.
    assert!(comments::decode_cursor(&cursor, room.document_id, Some(ids[0]), true).is_err());
    // Nor a reader's, whose eligible rows are a different set.
    assert!(comments::decode_cursor(&cursor, room.document_id, None, false).is_err());
    // And neither does anything that is not a cursor at all.
    for junk in ["", "not-base64!!", "YWJj", "eyJ2Ijo5OTl9"] {
        assert!(
            comments::decode_cursor(junk, room.document_id, None, true).is_err(),
            "{junk:?} was accepted",
        );
    }
    // A thread cursor is refused on the listing, symmetrically.
    let thread_cursor = comments::encode_cursor(
        room.document_id,
        Some(ids[0]),
        true,
        comments::Position::start(),
    );
    assert!(comments::decode_cursor(&thread_cursor, room.document_id, None, true).is_err());
    deployment.catalog.close().await;
}

/// A reader's page is a page of what a reader may see: the suggestion
/// filter is in the query, not applied after the fact, so a document of
/// mostly suggestions does not hand a reader a page of three rows.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reader_pages_only_what_a_reader_may_see() {
    let Some(deployment) = deployment("paging-visibility").await else {
        return;
    };
    let room = deployment.room().await;
    deployment.seed_comments(room.document_id, 10).await;
    deployment.seed_suggestions(room.document_id, 90).await;

    assert_eq!(room.comment_state(true).await.unwrap().total, 100);
    assert_eq!(
        room.comment_state(false).await.unwrap().total,
        10,
        "a reader's count is a reader's count",
    );
    let (views, page) = room.comment_page(None, 10, "", false).await.unwrap();
    assert_eq!(views.len(), 10, "a full page of readable rows");
    assert!(page.complete);
    for comment in &page.comments {
        assert_ne!(comment.motivation, "editing");
    }
    // And a reader cannot reach a suggestion's thread by naming it.
    let suggestion = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM annotations WHERE document_id=$1 AND kind='suggestion' LIMIT 1",
    )
    .bind(room.document_id)
    .fetch_one(deployment.catalog.pool())
    .await
    .unwrap();
    assert!(room
        .reply_page(suggestion, None, 10, false)
        .await
        .unwrap()
        .is_none());
    assert!(room
        .reply_page(suggestion, None, 10, true)
        .await
        .unwrap()
        .is_some());
    deployment.catalog.close().await;
}

/// The room holds no comment list any more, and the anchors it does hold
/// are bounded. Paging a document far larger than the cache leaves the
/// cache at its ceiling rather than at the document's size.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_room_cache_stays_bounded_while_a_large_document_is_paged() {
    let Some(deployment) = deployment("paging-cache-bound").await else {
        return;
    };
    let room = deployment.room().await;
    deployment
        .seed_comments(room.document_id, crate::room::ATTACHMENT_CACHE_MAX + 200)
        .await;
    let held = room.all_comments(true).await.unwrap();
    assert_eq!(held.len(), crate::room::ATTACHMENT_CACHE_MAX + 200);
    assert!(
        room.attachment_cache_len().await <= crate::room::ATTACHMENT_CACHE_MAX,
        "the anchor cache does not grow with the document",
    );
    // And invalidation still empties it.
    room.forget_comments().await;
    assert_eq!(room.attachment_cache_len().await, 0);
    deployment.catalog.close().await;
}

/// Mutating a comment past the first page still works, and so does reading
/// it back -- the regression that the by-id lookup exists for, re-checked
/// against the paged reads that replaced the whole-collection one.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comment_beyond_the_first_page_is_readable_by_id() {
    let Some(deployment) = deployment("paging-by-id").await else {
        return;
    };
    let room = deployment.room().await;
    let written = deployment
        .seed_comments(room.document_id, PAST_THE_PAGE)
        .await;
    let (_, first) = room
        .comment_page(None, comments::COMMENT_PAGE_DEFAULT, "", true)
        .await
        .unwrap();
    let on_page: HashSet<String> = first
        .comments
        .iter()
        .map(|comment| comment.id.clone())
        .collect();
    let beyond = written
        .iter()
        .find(|id| !on_page.contains(&id.to_string()))
        .expect("most of them are past the first page");
    let comment = room
        .comment_by_id(&beyond.to_string(), true)
        .await
        .expect("the catalogue answers")
        .expect("a comment past the first page reads by id");
    assert_eq!(comment.id, beyond.to_string());
    deployment.catalog.close().await;
}

/// Boundedness, measured rather than asserted from the shape of the code.
///
/// The same traversal is run over collections of very different sizes and
/// the *peak* page cost is compared. A page that grew with the document
/// would show it here; a bounded one shows the same figure whatever it is
/// walking. The numbers are printed so a change in them is visible in a
/// test log rather than only in a failing assertion.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn page_cost_does_not_grow_with_the_collection() {
    let Some(deployment) = deployment("paging-bounded").await else {
        return;
    };
    let room = deployment.room().await;

    /// Peak encoded bytes and peak row count over one whole traversal,
    /// plus how many pages it took.
    async fn walk(deployment: &Deployment, document_id: Uuid) -> (usize, usize, usize) {
        let mut peak_bytes = 0usize;
        let mut peak_rows = 0usize;
        let mut pages = 0usize;
        let mut after = None;
        loop {
            let page = comments::page(&deployment.catalog, document_id, after, 200, true)
                .await
                .unwrap();
            let bytes: usize = page
                .comments
                .iter()
                .map(|comment| {
                    serde_json::to_vec(comment)
                        .map(|raw| raw.len())
                        .unwrap_or(0)
                })
                .sum();
            peak_bytes = peak_bytes.max(bytes);
            peak_rows = peak_rows.max(page.comments.len());
            pages += 1;
            if page.complete {
                break;
            }
            after = page.next;
            assert!(after.is_some());
            assert!(pages < 2_000, "the traversal must terminate");
        }
        (peak_bytes, peak_rows, pages)
    }

    // Every comment is the same width, so the only variable is how many
    // there are.
    let mut measured = Vec::new();
    for target in [50usize, 400, 3_200] {
        let have = deployment
            .catalog
            .annotation_count(room.document_id)
            .await
            .unwrap() as usize;
        deployment
            .seed_comments(room.document_id, target - have)
            .await;
        sqlx::query("UPDATE annotations SET body=repeat('y', $2) WHERE document_id=$1")
            .bind(room.document_id)
            .bind(4_096_i32)
            .execute(deployment.catalog.pool())
            .await
            .unwrap();
        let (bytes, rows, pages) = walk(&deployment, room.document_id).await;
        eprintln!(
            "comment traversal: {target} comments -> {pages} page(s), \
             peak {rows} rows, peak {bytes} encoded bytes per page"
        );
        measured.push((target, bytes, rows));
    }

    for (target, bytes, rows) in &measured {
        assert!(
            *bytes <= comments::PAGE_BYTES_MAX + 64 * 1024,
            "{target} comments produced a {bytes}-byte page",
        );
        assert!(
            *rows <= comments::COMMENT_PAGE_MAX,
            "{target} comments produced a {rows}-row page",
        );
    }
    // The comparison that means something: two collections that both take
    // more than one page. The 50-comment run above is one short page, so
    // its peak is the size of the document rather than the size of a page
    // and comparing against it would only be measuring that.
    let (small_count, small, _) = measured[1];
    let (large_count, large, _) = measured[measured.len() - 1];
    assert!(
        large.abs_diff(small) <= 64 * 1024,
        "{large_count} comments peaked at {large} bytes per page against \
         {small} at {small_count}: the page grows with the collection",
    );

    // And the room's own residency after walking all of it.
    let _ = room.all_comments(true).await.unwrap();
    let held = room.attachment_cache_len().await;
    eprintln!("room attachment cache after a 3,200-comment traversal: {held} entries");
    assert!(held <= crate::room::ATTACHMENT_CACHE_MAX);
    deployment.catalog.close().await;
}

/// An agent's continuation across pages, and the version token it echoes
/// back to mutate safely.
///
/// The token used to be a digest of the whole serialized comment, replies
/// included, so it changed with whatever page had been loaded. It is now
/// the row plus the thread's shape, which is what makes it the same token
/// from a page, from a single-row read and from an agent window.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_agent_walks_threads_and_its_version_token_survives_paging() {
    let Some(deployment) = deployment("paging-agent").await else {
        return;
    };
    let room = deployment.room().await;
    let ids = deployment.seed_comments(room.document_id, 60).await;
    deployment.seed_replies(room.document_id, ids[0], 40).await;

    // Three windows of 24 reach every comment exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut after = None;
    loop {
        let window = room
            .thread_query_page(&deployment.author_key(), true, after, 24)
            .await
            .unwrap();
        let last = window
            .items
            .last()
            .and_then(|item| item.get("cursor").and_then(|c| c.as_str()))
            .map(|raw| {
                comments::decode_cursor(raw, room.document_id, None, true).expect("its own cursor")
            });
        seen.extend(
            window
                .items
                .iter()
                .map(|item| item["id"].as_str().unwrap_or_default().to_string()),
        );
        if window.complete {
            break;
        }
        after = last;
        assert!(after.is_some());
    }
    let unique: HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), 60, "every comment once");

    // The same comment's version token, from three different reads.
    let deep = ids[0].to_string();
    let by_id = room
        .comment_by_id(&deep, true)
        .await
        .expect("the catalogue answers")
        .expect("by id");
    let (_, page) = room
        .comment_page(None, 200, &deployment.author_key(), true)
        .await
        .unwrap();
    let on_page = page
        .comments
        .iter()
        .find(|comment| comment.id == deep)
        .expect("on the first page");
    let window = room
        .thread_reply_window(ids[0], &deployment.author_key(), true, None, 24)
        .await
        .unwrap();
    let from_window = window.thread.expect("the thread itself");
    let version = crate::room::agent_comments::comment_version(&by_id);
    assert_eq!(
        version,
        crate::room::agent_comments::comment_version(on_page),
        "a page's reply preview must not change the token",
    );
    assert_eq!(
        version,
        from_window["comment_version"].as_str().unwrap_or_default(),
        "and neither must an agent window's",
    );
    assert_eq!(by_id.reply_total, 40, "the count is authoritative");
    assert!(
        by_id.replies.len() <= comments::REPLY_PREVIEW,
        "even though only a preview was loaded",
    );

    // A reply changes it, which is what an `expected_version` check is for.
    deployment.seed_replies(room.document_id, ids[0], 1).await;
    let after_reply = room
        .comment_by_id(&deep, true)
        .await
        .expect("the catalogue answers")
        .expect("still there");
    assert_ne!(
        version,
        crate::room::agent_comments::comment_version(&after_reply),
        "a reply to the thread moves the token",
    );
    deployment.catalog.close().await;
}

/// An event reuses the enriched row, but its reply cursor belongs to the
/// audience receiving it, just like a cursor from a normal comment page.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn event_reply_cursors_continue_for_their_own_audience() {
    let Some(deployment) = deployment("event-reply-cursors").await else {
        return;
    };
    let room = deployment.room().await;
    let id = deployment.seed_comments(room.document_id, 1).await[0];
    deployment
        .seed_replies(room.document_id, id, comments::REPLY_PREVIEW + 3)
        .await;
    let payload = serde_json::json!({"type": "reply", "comment_id": id.to_string()});
    let event = room.prepare_comment_event(&payload).await;
    for suggestions in [true, false] {
        let view = event.view_for("", suggestions);
        let cursor = view["comment"]["reply_cursor"].as_str().unwrap();
        let at = comments::decode_cursor(cursor, room.document_id, Some(id), suggestions).unwrap();
        assert!(comments::decode_cursor(cursor, room.document_id, Some(id), !suggestions).is_err());
        let page = room
            .reply_page(id, Some(at), 10, suggestions)
            .await
            .unwrap()
            .unwrap();
        assert!(page.complete);
        assert_eq!(page.replies.len(), 3);
        let prefix = view["comment"]["replies"].as_array().unwrap();
        assert_eq!(prefix.len(), comments::REPLY_PREVIEW);
        for reply in &page.replies {
            assert!(prefix
                .iter()
                .all(|old| old["id"].as_str() != Some(reply.id.as_str())));
        }
    }
    assert_eq!(room.event_comment_read_count(), 1);
    deployment.catalog.close().await;
}
