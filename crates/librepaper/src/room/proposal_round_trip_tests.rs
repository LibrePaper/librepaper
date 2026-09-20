//! A proposal's whole life against a real room and a real catalogue.
//!
//! SPEC-loro.md §8. Every piece of this was already tested apart -- hunk
//! grouping in `document::hunks`, the frontier encoding in
//! `room::proposals::tests`, the rows in `storage::postgres::proposals`, the
//! panel in `web/tests/browser/changes-browser.mjs` -- and the seam between
//! them was not tested at all. That seam is where `proposal-open` quietly
//! dropped the base its author forked at: each side was right about its own
//! half, and no test held both halves at once.
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage. Run them with `--test-threads=1`: they
//! TRUNCATE the database they are pointed at.

use super::*;
use crate::document::session;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::room::proposals::{Decision, ProposalError};
use crate::storage::blob::FsStore;
use crate::storage::postgres::{PostgresCatalog, PostgresOptions};
use loro::Frontiers;
use uuid::Uuid;

struct Deployment {
    rooms: RoomSet,
    catalog: Arc<PostgresCatalog>,
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
        "TRUNCATE document_proposal_hunks,document_proposals,maintenance_cursors,jobs,
         document_updates,document_bases,bundle_files,bundles,document_versions,
         document_assets,replies,annotations,share_links,grants,documents,accounts CASCADE",
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
    let authority = crate::storage::postgres::Authority {
        principal_key: account.id.to_string(),
        account_id: Some(account.id),
        link_hash: None,
    };
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
                source: "The cat sat.\n".into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            vec![],
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
    Some(Deployment {
        rooms: RoomSet::new(store),
        catalog,
        slug: slug.into(),
        authority,
        _writer: writer,
        _objects: objects,
    })
}

/// The room's own frontier, and the text of the main file.
async fn frontier(room: &Room) -> Frontiers {
    let state = room.command_owner.state().await;
    state.session.doc.state_frontiers()
}

async fn text(room: &Room, path: &str) -> String {
    let state = room.command_owner.state().await;
    session::texts_of(&state.session.doc)
        .get(path)
        .cloned()
        .unwrap_or_default()
}

/// Writes into the room as somebody who is not the proposal's author.
async fn other_writer_types(room: &Room, path: &str, body: &str) {
    let state = room.command_owner.state().await;
    session::put_text(&state.session.doc, path, body);
    state.session.doc.commit();
}

/// Forks the room the way a browser does: at the frontier it can see, into a
/// branch with a peer of its own, and exports the branch's own operations.
fn author_forks(
    at: &loro::LoroDoc,
    peer: loro::PeerID,
    path: &str,
    body: &str,
) -> (Vec<u8>, Frontiers) {
    let before = session::encode_vector(at);
    let branch = at.fork();
    branch.set_peer_id(peer).unwrap();
    session::put_text(&branch, path, body);
    branch.commit();
    let tip = branch.state_frontiers();
    (session::encode_diff(&branch, &before).unwrap(), tip)
}

/// The regression guard for the base `proposal-open` used to throw away.
///
/// The author forks, somebody else types, and only then does the open reach
/// the server. Before the fix the room recorded its own frontier -- the one
/// that already contains the other writer's sentence -- rather than the one
/// the author actually forked at, which is what §5.1 says the message carries
/// and what `rebuild` forks at on every later read of the proposal.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_proposal_opens_at_the_base_its_author_forked_at() {
    let Some(deployment) = deployment("base-is-the-authors").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();

    // Where the author forked.
    let base = frontier(&room).await;

    // Somebody else types before the open is handled. This is the whole race.
    other_writer_types(&room, "paper.md", "The cat sat. It purred.\n").await;
    let moved = frontier(&room).await;
    assert_ne!(
        base, moved,
        "the room has to have moved, or this proves nothing"
    );

    let id = room
        .open_proposal("Ada", &base, &deployment.authority, Uuid::new_v4())
        .await
        .unwrap();

    let stored = deployment
        .catalog
        .proposal(uuid::Uuid::parse_str(&id).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.base_frontiers,
        base.encode(),
        "the proposal is recorded at the author's fork point"
    );
    assert_ne!(
        stored.base_frontiers,
        moved.encode(),
        "and not at wherever the room had got to when the message arrived"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn proposal_open_receipt_replays_without_a_second_commit() {
    let Some(deployment) = deployment("proposal-open-receipt").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let base = frontier(&room).await;
    let request_id = Uuid::new_v4();

    let first = room
        .open_proposal("Ada", &base, &deployment.authority, request_id)
        .await
        .unwrap();
    let replay = room
        .open_proposal("Ada", &base, &deployment.authority, request_id)
        .await
        .unwrap();
    assert_eq!(replay, first);

    let document_id: Uuid = room.storage_id.parse().unwrap();
    let (commits, proposals): (i64, i64) = sqlx::query_as(
        "SELECT d.commit_sequence,count(p.id) FROM documents d \
         LEFT JOIN document_proposals p ON p.document_id=d.id WHERE d.id=$1 \
         GROUP BY d.commit_sequence",
    )
    .bind(document_id)
    .fetch_one(deployment.catalog.pool())
    .await
    .unwrap();
    // Creation commits the initial source revision; opening the proposal is
    // the only additional semantic commit, and replay advances nothing.
    assert_eq!((commits, proposals), (2, 1));

    let changed = room
        .open_proposal("Grace", &base, &deployment.authority, request_id)
        .await;
    assert!(
        changed.is_err(),
        "a request identity cannot change its command"
    );
}

/// Declining, with somebody else writing at the same time.
///
/// Declining is the path that applies an inverse, so it is where a wrong base
/// would have the most room to do damage: the revert is computed from the
/// base, and a base carrying another writer's sentence could put that sentence
/// in the set of things to take back out.
///
/// It does not, as it turns out. This test passes both with the base honoured
/// and with the old behaviour that ignored it -- Loro's merge converges on the
/// same text either way, because the author's operations and the other
/// writer's are concurrent and both stay in the graph. That is worth having
/// written down: the base being wrong was a divergence from §5.1 and from what
/// the client actually did, not a demonstrated case of lost or misattributed
/// text. `a_proposal_opens_at_the_base_its_author_forked_at` is the test that
/// holds the fix; this one guards the decline path itself, which had no
/// end-to-end coverage at all.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn declining_a_proposal_does_not_revert_a_concurrent_writer() {
    let Some(deployment) = deployment("decline-keeps-others").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let path = "paper.md";

    let base = frontier(&room).await;
    let (branch_bytes, tip) = {
        let state = room.command_owner.state().await;
        let at = state.session.doc.fork_at(&base).unwrap();
        author_forks(&at, 4242, path, "The tabby sat.\n")
    };
    other_writer_types(&room, path, "The cat sat. It purred.\n").await;

    let id = room
        .open_proposal("Ada", &base, &deployment.authority, Uuid::new_v4())
        .await
        .unwrap();
    room.update_proposal(
        &id,
        &tip.encode(),
        &branch_bytes,
        &deployment.authority,
        Uuid::new_v4(),
    )
    .await
    .unwrap();

    // One hunk, because the author made one change. Anything more is the
    // room's own history being counted as theirs.
    let stored = deployment
        .catalog
        .proposal(uuid::Uuid::parse_str(&id).unwrap())
        .await
        .unwrap()
        .unwrap();
    let proposal = crate::room::proposals::Proposal {
        base: Frontiers::decode(&stored.base_frontiers).unwrap(),
        tip: Frontiers::decode(&stored.tip_frontiers).unwrap(),
    };
    let found = {
        let state = room.command_owner.state().await;
        crate::room::proposals::hunks(&state.session.doc, &proposal, &stored.branch_bytes).unwrap()
    };
    assert_eq!(
        found.len(),
        1,
        "the proposal is the author's one change and nothing else, got {found:?}"
    );

    room.decide_hunk(
        &id,
        Decision {
            hunk: 0,
            accepted: false,
            by: "Bob",
            against: &tip.encode(),
            note: Some("not this time".into()),
            reviewer: 7,
            request_id: uuid::Uuid::now_v7(),
            authority: &deployment.authority,
        },
    )
    .await
    .unwrap();

    let body = text(&room, path).await;
    assert!(
        !body.contains("tabby"),
        "the declined word is out, got {body:?}"
    );
    assert!(
        body.contains("It purred."),
        "declining takes back only the author's work, never the other writer's, got {body:?}"
    );
}

/// A base the room cannot reach is refused, rather than silently replaced by
/// one it can. This is the other half of honouring the client's base: taking
/// it on trust would mean `rebuild` failing later, on somebody's first
/// attempt to review, with nothing to point at.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_base_the_room_has_never_seen_is_refused() {
    let Some(deployment) = deployment("base-unknown").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();

    // A frontier from a document this room shares no history with.
    let stranger = session::new_doc();
    stranger.set_peer_id(987_654_321).unwrap();
    session::put_text(&stranger, "paper.md", "Elsewhere.");
    stranger.commit();
    let elsewhere = stranger.state_frontiers();

    let refused = room
        .open_proposal("Ada", &elsewhere, &deployment.authority, Uuid::new_v4())
        .await;
    assert!(
        matches!(refused, Err(ProposalError::UnknownBase)),
        "got {refused:?}"
    );
    assert!(
        deployment
            .catalog
            .open_proposals(
                deployment
                    .catalog
                    .document_by_slug(&deployment.slug)
                    .await
                    .unwrap()
                    .unwrap()
                    .id
            )
            .await
            .unwrap()
            .is_empty(),
        "a refused open leaves no row behind"
    );
}

/// Open, update, decide, resolve -- the sequence a browser actually performs,
/// across the socket's own entry points, with a concurrent writer throughout.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_proposal_goes_open_update_decide_resolve() {
    let Some(deployment) = deployment("round-trip").await else {
        return;
    };
    let room = deployment.rooms.try_get(&deployment.slug).await.unwrap();
    let path = "paper.md";

    // Where the author forks. Somebody else then types before the open is
    // handled -- deliberately in that order, so that a base taken from the
    // room rather than from the author would carry the other writer's
    // sentence into the diff, and accepting would delete it. The assertion
    // that it survives, at the bottom, is that fault as a reader would meet
    // it.
    let base = frontier(&room).await;
    let (branch_bytes, tip) = {
        let state = room.command_owner.state().await;
        let at = state.session.doc.fork_at(&base).unwrap();
        author_forks(&at, 4242, path, "The tabby sat.\n")
    };
    other_writer_types(&room, path, "The cat sat. It purred.\n").await;

    let id = room
        .open_proposal("Ada", &base, &deployment.authority, Uuid::new_v4())
        .await
        .unwrap();
    room.update_proposal(
        &id,
        &tip.encode(),
        &branch_bytes,
        &deployment.authority,
        Uuid::new_v4(),
    )
    .await
    .unwrap();

    // A decision against a tip the proposal has moved past is refused (§5.1).
    let stale = room
        .decide_hunk(
            &id,
            Decision {
                hunk: 0,
                accepted: true,
                by: "Bob",
                against: &base.encode(),
                note: None,
                reviewer: 7,
                request_id: uuid::Uuid::now_v7(),
                authority: &deployment.authority,
            },
        )
        .await;
    assert!(
        matches!(stale, Err(ProposalError::Stale)),
        "a decision against the wrong tip is refused, got {stale:?}"
    );

    // The real decision, against the tip the reviewer was shown.
    let update = room
        .decide_hunk(
            &id,
            Decision {
                hunk: 0,
                accepted: true,
                by: "Bob",
                against: &tip.encode(),
                note: None,
                reviewer: 7,
                request_id: uuid::Uuid::now_v7(),
                authority: &deployment.authority,
            },
        )
        .await
        .unwrap();
    assert!(
        update.is_some(),
        "the last hunk resolves the proposal and carries it into the document"
    );

    let body = text(&room, path).await;
    assert!(
        body.contains("tabby"),
        "the accepted word landed, got {body:?}"
    );
    assert!(
        body.contains("It purred."),
        "the concurrent writer's sentence is untouched, got {body:?}"
    );

    let document = deployment
        .catalog
        .document_by_slug(&deployment.slug)
        .await
        .unwrap()
        .unwrap()
        .id;
    assert!(
        deployment
            .catalog
            .open_proposals(document)
            .await
            .unwrap()
            .is_empty(),
        "a resolved proposal is no longer open"
    );
}
