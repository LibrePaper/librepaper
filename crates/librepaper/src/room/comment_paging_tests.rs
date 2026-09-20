//! Read and mutation regressions beyond the old SQL page ceilings, plus
//! explicit refusal at the whole-snapshot memory budget.
//!
//! `comments::load` used to read one page -- `annotations(document_id, None,
//! 500)` -- and `find_annotation` used to scan that same page for one row.
//! Nothing enforces `config.max_comments` at admission, so a document could
//! and did grow past it: comment 501 was written, acknowledged, and then
//! invisible to every view and refused as "unknown comment" by resolve,
//! delete, refine and accept. The reply read had the same shape of bug in a
//! harsher form -- a fixed `LIMIT 5001` and a refusal above 5,000 rows across
//! the whole document, which failed the *entire* load rather than one thread.
//!
//! These tests are about the boundary and what lies past it, so they seed in
//! bulk through the catalogue rather than one sequencer command per comment
//! (a 5,001-reply document is not reachable in a test otherwise), and then
//! read and mutate through the real paths: `Room::comments`,
//! `Room::snapshot_for` and `room.command(&authority, &mut cmd)`.
//!
//! They need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage.

use std::collections::HashSet;

use super::*;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::log::Registry;
use crate::room::annotation::CommentTarget;
use crate::storage::blob::FsStore;
use crate::storage::postgres::{
    AnnotationRecord, Authority, MutationAuthorization, NewAccount, NewAnnotation, NewReply,
    PostgresCatalog, PostgresOptions, ANNOTATION_PAGE_MAX, REPLY_PAGE_MAX,
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
    )
    .await
    .unwrap();
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
    let comments = room.comments().await.unwrap();
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
    let snapshot = room
        .snapshot_for(&deployment.author_key(), true)
        .await
        .unwrap();
    assert_eq!(snapshot.len(), PAST_THE_PAGE);

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
        room.comments().await.unwrap().len(),
        ANNOTATION_PAGE_MAX as usize,
        "exactly one full page",
    );

    // A comment appended after the first page -- the row that the old
    // single-page read dropped.
    let late = deployment.seed_comments(room.document_id, 1).await[0];
    room.forget_comments().await;
    let comments = room.comments().await.unwrap();
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
    let comments = room.comments().await.unwrap();
    // Whichever row sorts last is past the first page by construction, and
    // it is the one `find_annotation`'s old first-page scan could not see.
    let last: Uuid = comments.last().unwrap().id.parse().unwrap();
    assert!(written.contains(&last));

    let mut resolve = ResolveComment::new(
        deployment.catalog.clone(),
        room.document_id,
        last,
        true,
        deployment.mutation_authorization(),
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
    let reloaded = room.comments().await.unwrap();
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
        "Reviewer",
        Some(deployment.account_id),
        deployment.author_key(),
        deployment.mutation_authorization(),
    )
    .unwrap();
    room.command(&deployment.authority(), &mut reply)
        .await
        .expect("a comment past the first page can be replied to");

    let mut delete = DeleteComment::new(
        deployment.catalog.clone(),
        room.document_id,
        last,
        deployment.author_key(),
        true,
        deployment.mutation_authorization(),
    );
    room.command(&deployment.authority(), &mut delete)
        .await
        .expect("a comment past the first page can be deleted");
    room.forget_comments().await;
    assert_eq!(
        room.comments().await.unwrap().len(),
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
    let loaded = room.comments().await.unwrap();
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
    let loaded = room.comments().await.unwrap();
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
        deployment.mutation_authorization(),
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
        !other.comments().await.unwrap()[0].resolved,
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
    let comments = room.comments().await.unwrap();
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
    let loaded = comments::load(&deployment.catalog, document_id)
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
    let after = comments::load(&deployment.catalog, document_id)
        .await
        .unwrap();
    assert!(ids_of(&after).contains(&arrived.to_string()));
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn aggregate_read_budget_refuses_without_caching_an_empty_snapshot() {
    let Some(deployment) = deployment("paging-budget").await else {
        return;
    };
    let room = deployment.room().await;
    deployment.seed_comments(room.document_id, 1).await;
    assert!(matches!(
        comments::load_with_budget(&deployment.catalog, room.document_id, 0).await,
        Err(WriteError::Unreadable(_))
    ));
    // Model already-stored data larger than the whole-snapshot transport can
    // carry. This is a read guard, so the stored row must remain untouched.
    sqlx::query("UPDATE annotations SET body=repeat('x', $2) WHERE document_id=$1")
        .bind(room.document_id)
        .bind((comments::COMMENT_SNAPSHOT_BYTES_MAX / 2 + 1) as i32)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    let result = room.snapshot_for("", true).await;
    assert!(matches!(result, Err(WriteError::Unreadable(_))));
    assert!(room.comments.read().await.is_none());
    assert_eq!(room.counts().await.unwrap(), (1, 1));
    assert_eq!(
        deployment
            .catalog
            .annotation_count(room.document_id)
            .await
            .unwrap(),
        1
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn invalidation_cannot_be_overwritten_by_an_in_flight_load() {
    let Some(deployment) = deployment("paging-cache-race").await else {
        return;
    };
    let room = deployment.room().await;
    deployment.seed_comments(room.document_id, 1).await;
    let mut lock = deployment.catalog.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE annotations IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let loading = {
        let room = room.clone();
        tokio::spawn(async move { room.comments().await })
    };
    // The fill lock is held while the catalogue read is blocked. Previously
    // the load did not hold it, and an invalidation could finish too early.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if room.comments.try_write().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let invalidating = {
        let room = room.clone();
        tokio::spawn(async move { room.forget_comments().await })
    };
    lock.commit().await.unwrap();
    loading.await.unwrap().unwrap();
    invalidating.await.unwrap();
    assert!(
        room.comments.read().await.is_none(),
        "the later invalidation must win"
    );
    deployment.catalog.close().await;
}
