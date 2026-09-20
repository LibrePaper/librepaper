//! A proposal's whole life against a real room and a real catalogue.
//!
//! SPEC-loro.md §8, and SPEC-server-is-a-log §7 for the command shape. Every
//! piece of this was already tested apart -- hunk grouping in
//! `document::hunks`, the frontier encoding in `room::proposals::tests`, the
//! rows in `storage::postgres::proposals`, the panel in
//! `web/tests/browser/changes-browser.mjs` -- and the seam between them was
//! not tested at all. That seam is where `proposal-open` quietly dropped the
//! base its author forked at: each side was right about its own half, and no
//! test held both halves at once.
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it, like
//! the rest of the catalogue coverage. Run them with `--test-threads=1`: they
//! TRUNCATE the database they are pointed at.

use super::*;
use crate::document::session;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::room::proposals::{DecideProposalHunk, OpenProposal, ProposalDecided, UpdateProposal};
use crate::storage::blob::FsStore;
use crate::storage::postgres::{Authority, PostgresCatalog, PostgresOptions, StoredProposal};
use loro::Frontiers;
use uuid::Uuid;

struct Deployment {
    rooms: Rooms,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    authority: Authority,
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
        "TRUNCATE document_proposal_hunks,document_proposals,document_labels,\
         document_updates,document_bases,document_assets,replies,annotations,\
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
    let authority = Authority {
        principal_key: account.id.to_string(),
        account_id: Some(account.id),
        link_hash: None,
    };
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let writer = catalog.claim_writer().await.unwrap();
    let registry = crate::log::Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "test-deployment-peer".to_string(),
    );
    let store = Store::open_with_catalog(
        blobs.clone(),
        config.clone(),
        catalog.clone(),
        registry.clone(),
    )
    .await
    .unwrap();
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
        rooms: Rooms::new(catalog.clone(), blobs, config, registry),
        catalog,
        slug: slug.into(),
        authority,
        _writer: writer,
        _objects: objects,
    })
}

/// The room's own frontier, and the text of the main file.
async fn frontier(room: &Room) -> Frontiers {
    room.log()
        .with_head(|doc| doc.state_frontiers())
        .await
        .unwrap()
}

async fn text(room: &Room, path: &str) -> String {
    room.log()
        .with_head(|doc| {
            session::texts_of(doc)
                .get(path)
                .cloned()
                .unwrap_or_default()
        })
        .await
        .unwrap()
}

/// Writes into the room as somebody who is not the proposal's author, using
/// an ordinary ingest rather than a semantic command: this is a concurrent
/// editor typing, and typing is the sequencer's typing path (§5), not §7.
async fn other_writer_types(room: &Room, path: &str, body: &str) {
    let before = room.log().with_head(session::encode_vector).await.unwrap();
    let update = room
        .log()
        .with_fork_at(&frontier(room).await, |doc| {
            session::put_text(doc, path, body);
            doc.commit();
            session::encode_diff(doc, &before).unwrap()
        })
        .await
        .unwrap();
    match room
        .ingest(9_999, "other-writer", "other-writer", 1, update)
        .await
    {
        crate::log::Ingested::Accepted => {}
        other => panic!("expected the concurrent write to be accepted, got {other:?}"),
    }
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

/// Opens a proposal the way a socket handler does: run the command, then
/// flush so the next `with_head`/`with_fork_at` in the test sees it, since
/// nothing here is holding a live connection to relay through.
async fn open(
    room: &Room,
    authority: &Authority,
    author: &str,
    base: &Frontiers,
) -> StoredProposal {
    let mut command = OpenProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: Uuid::new_v4(),
        author: author.to_string(),
        base: base.clone(),
    };
    room.command(authority, &mut command).await.unwrap()
}

async fn update(
    room: &Room,
    authority: &Authority,
    stored: &StoredProposal,
    tip: &Frontiers,
    branch: &[u8],
) -> StoredProposal {
    let mut command = UpdateProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: stored.id,
        expected_version: stored.version,
        tip: tip.clone(),
        branch: branch.to_vec(),
    };
    room.command(authority, &mut command).await.unwrap()
}

/// Decides one hunk the way a socket handler does: read the current row and
/// what has been decided so far, then run the command. The command rereads
/// both under the sequencer lock; handing in a snapshot is what a caller
/// does, not what the merge is computed from.
async fn decide(
    room: &Room,
    authority: &Authority,
    proposal_id: Uuid,
    hunk_index: i32,
    accepted: bool,
    against: &Frontiers,
    reviewer: loro::PeerID,
) -> Result<ProposalDecided, crate::log::CommandError> {
    let stored = room.catalog().proposal(proposal_id).await.unwrap().unwrap();
    let decided = room.catalog().decisions(proposal_id).await.unwrap();
    decide_with(
        room, authority, stored, decided, hunk_index, accepted, against, reviewer,
    )
    .await
}

/// [`decide`], but with the pre-read snapshot handed in rather than taken
/// here, so a test can hold two reviewers on the same snapshot.
#[allow(clippy::too_many_arguments)]
async fn decide_with(
    room: &Room,
    authority: &Authority,
    stored: StoredProposal,
    decided: Vec<crate::storage::postgres::StoredDecision>,
    hunk_index: i32,
    accepted: bool,
    against: &Frontiers,
    reviewer: loro::PeerID,
) -> Result<ProposalDecided, crate::log::CommandError> {
    let proposal_id = stored.id;
    let mut command = DecideProposalHunk {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        proposal_id,
        stored,
        decided,
        hunk_index,
        accepted,
        decided_by: "Bob".to_string(),
        note: None,
        against: against.encode(),
        reviewer,
        request_id: Uuid::now_v7(),
        total_hunks: 0,
    };
    room.command(authority, &mut command).await
}

/// The regression guard for the base `OpenProposal` must not throw away.
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
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();

    // Where the author forked.
    let base = frontier(&room).await;

    // Somebody else types before the open is handled. This is the whole race.
    other_writer_types(&room, "paper.md", "The cat sat. It purred.\n").await;
    let moved = frontier(&room).await;
    assert_ne!(
        base, moved,
        "the room has to have moved, or this proves nothing"
    );

    let stored = open(&room, &deployment.authority, "Ada", &base).await;

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
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();

    // A frontier from a document this room shares no history with.
    let stranger = session::new_doc();
    stranger.set_peer_id(987_654_321).unwrap();
    session::put_text(&stranger, "paper.md", "Elsewhere.");
    stranger.commit();
    let elsewhere = stranger.state_frontiers();

    let mut command = OpenProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: Uuid::new_v4(),
        author: "Ada".to_string(),
        base: elsewhere,
    };
    let refused = room.command(&deployment.authority, &mut command).await;
    assert!(
        matches!(refused, Err(crate::log::CommandError::Conflict(_))),
        "got {refused:?}"
    );
    assert!(
        deployment
            .catalog
            .open_proposals(room.document_id)
            .await
            .unwrap()
            .is_empty(),
        "a refused open leaves no row behind"
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
/// holds the fix; this one guards the decline path itself.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn declining_a_proposal_does_not_revert_a_concurrent_writer() {
    let Some(deployment) = deployment("decline-keeps-others").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let path = "paper.md";

    let base = frontier(&room).await;
    let (branch_bytes, tip) = room
        .log()
        .with_fork_at(&base, |at| author_forks(at, 4242, path, "The tabby sat.\n"))
        .await
        .unwrap();
    other_writer_types(&room, path, "The cat sat. It purred.\n").await;

    let stored = open(&room, &deployment.authority, "Ada", &base).await;
    let stored = update(&room, &deployment.authority, &stored, &tip, &branch_bytes).await;

    // One hunk, because the author made one change. Anything more is the
    // room's own history being counted as theirs.
    let proposal = crate::room::proposals::Proposal {
        base: Frontiers::decode(&stored.base_frontiers).unwrap(),
        tip: Frontiers::decode(&stored.tip_frontiers).unwrap(),
    };
    let found = room
        .log()
        .with_head(|doc| {
            crate::room::proposals::hunks(doc, &proposal, &stored.branch_bytes).unwrap()
        })
        .await
        .unwrap();
    assert_eq!(
        found.len(),
        1,
        "the proposal is the author's one change and nothing else, got {found:?}"
    );

    let outcome = decide(&room, &deployment.authority, stored.id, 0, false, &tip, 7)
        .await
        .unwrap();
    assert!(outcome.resolved, "the only hunk decided resolves it");

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

/// Open, update, decide -- the sequence a browser actually performs, across
/// the socket's own entry points, with a concurrent writer throughout.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_proposal_goes_open_update_decide_resolve() {
    let Some(deployment) = deployment("round-trip").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let path = "paper.md";

    // Where the author forks. Somebody else then types before the open is
    // handled -- deliberately in that order, so that a base taken from the
    // room rather than from the author would carry the other writer's
    // sentence into the diff, and accepting would delete it. The assertion
    // that it survives, at the bottom, is that fault as a reader would meet
    // it.
    let base = frontier(&room).await;
    let (branch_bytes, tip) = room
        .log()
        .with_fork_at(&base, |at| author_forks(at, 4242, path, "The tabby sat.\n"))
        .await
        .unwrap();
    other_writer_types(&room, path, "The cat sat. It purred.\n").await;

    let stored = open(&room, &deployment.authority, "Ada", &base).await;
    update(&room, &deployment.authority, &stored, &tip, &branch_bytes).await;

    // A decision against a tip the proposal has moved past is refused (§7.1).
    let stale = decide(&room, &deployment.authority, stored.id, 0, true, &base, 7).await;
    assert!(
        matches!(stale, Err(crate::log::CommandError::Conflict(_))),
        "a decision against the wrong tip is refused, got {stale:?}"
    );

    // The real decision, against the tip the reviewer was shown.
    let outcome = decide(&room, &deployment.authority, stored.id, 0, true, &tip, 7)
        .await
        .unwrap();
    assert!(
        outcome.resolved,
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

    assert!(
        deployment
            .catalog
            .open_proposals(room.document_id)
            .await
            .unwrap()
            .is_empty(),
        "a resolved proposal is no longer open"
    );

    // A retry with the same request id, after the merge already landed, is
    // handled without re-resolving: the hunk row is idempotent and `resolve`
    // finds head already reflecting the decision, so `Head::prepare` exports
    // an empty batch and no second merge happens.
    let replay = decide(&room, &deployment.authority, stored.id, 0, true, &tip, 7)
        .await
        .unwrap();
    assert!(
        replay.resolved,
        "a replayed completing decision still reports complete"
    );
    let body_after_replay = text(&room, path).await;
    assert_eq!(
        body_after_replay, body,
        "a replay of the same decision must not change the text a second time"
    );
}

/// Two reviewers decide the last two hunks at the same time.
///
/// §7's guarantee covers what a command reads from head, not what its caller
/// read from Postgres before the command started. The hunks already decided
/// are read out here (see `decide`), so two decisions prepared from the same
/// snapshot each see one answer short of the set and prepare no merge -- yet
/// the second one to reach `transact` counts both rows under the proposal's
/// `FOR UPDATE` lock, finds the review complete and resolves the proposal.
/// The reviewers are told the proposal resolved, and the text they accepted
/// was never applied.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn two_reviewers_deciding_at_once_still_apply_what_they_accepted() {
    let Some(deployment) = deployment("decide-race").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let path = "paper.md";

    // Far enough apart to be two hunks rather than one.
    other_writer_types(&room, path, "alpha\nbravo\ncharlie\ndelta\necho\n").await;

    let base = frontier(&room).await;
    let (branch_bytes, tip) = room
        .log()
        .with_fork_at(&base, |at| {
            author_forks(at, 4242, path, "ALPHA\nbravo\ncharlie\ndelta\nECHO\n")
        })
        .await
        .unwrap();

    let stored = open(&room, &deployment.authority, "Ada", &base).await;
    let stored = update(&room, &deployment.authority, &stored, &tip, &branch_bytes).await;

    let proposal = crate::room::proposals::Proposal {
        base: Frontiers::decode(&stored.base_frontiers).unwrap(),
        tip: Frontiers::decode(&stored.tip_frontiers).unwrap(),
    };
    let found = room
        .log()
        .with_head(|doc| {
            crate::room::proposals::hunks(doc, &proposal, &stored.branch_bytes).unwrap()
        })
        .await
        .unwrap();
    assert_eq!(found.len(), 2, "two separated edits are two hunks");

    // Both reviewers read the proposal and its decisions before either of
    // them answers. This is the race: nothing else about the interleaving
    // matters, because the sequencer serializes the commands themselves.
    let snapshot = deployment.catalog.decisions(stored.id).await.unwrap();
    assert!(snapshot.is_empty(), "nobody has decided anything yet");

    let first = decide_with(
        &room,
        &deployment.authority,
        stored.clone(),
        snapshot.clone(),
        0,
        true,
        &tip,
        7,
    )
    .await
    .unwrap();
    assert!(!first.resolved, "one of two hunks does not resolve it");

    let second = decide_with(
        &room,
        &deployment.authority,
        stored.clone(),
        snapshot,
        1,
        true,
        &tip,
        8,
    )
    .await
    .unwrap();
    assert!(second.resolved, "the second answer completes the review");

    let body = text(&room, path).await;
    assert!(
        body.contains("ALPHA") && body.contains("ECHO"),
        "a resolved proposal's accepted hunks are in the document, got {body:?}"
    );
    assert!(
        deployment
            .catalog
            .open_proposals(room.document_id)
            .await
            .unwrap()
            .is_empty(),
        "and the proposal is closed"
    );
}
