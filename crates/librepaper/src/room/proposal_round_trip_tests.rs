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
use crate::room::proposals::{
    DecideProposalHunk, DiscardProposal, OpenProposal, ProposalDecided, UpdateProposal,
};
use crate::storage::blob::FsStore;
use crate::storage::postgres::{Authority, PostgresCatalog, StoredProposal};
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
    let catalog = crate::tests::catalog().await?;
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
        session_generation: Some(account.session_generation),
        policy_edit: true,
        policy_comment: true,
        automation: false,
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
                policy_comment: true,
                automation: false,
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
        owner_key: authority.principal_key.clone(),
        base: base.clone(),
        resume: false,
        stored: None,
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
        owner_key: authority.principal_key.clone(),
        expected_version: stored.version,
        tip: tip.clone(),
        base: None,
        branch: branch.to_vec(),
        stored: None,
        decided: Vec::new(),
        carried: Vec::new(),
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
) -> Result<ProposalDecided, crate::log::CommandError> {
    let stored = room.catalog().proposal(proposal_id).await.unwrap().unwrap();
    let decided = room.catalog().decisions(proposal_id).await.unwrap();
    decide_with(
        room, authority, stored, decided, hunk_index, accepted, against,
    )
    .await
}

/// [`decide`], but with the pre-read snapshot handed in rather than taken here.
async fn decide_with(
    room: &Room,
    authority: &Authority,
    stored: StoredProposal,
    decided: Vec<crate::storage::postgres::StoredDecision>,
    hunk_index: i32,
    accepted: bool,
    against: &Frontiers,
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
        all: false,
        decided_by: "Bob".to_string(),
        note: None,
        against: against.encode(),
        request_id: Uuid::now_v7(),
        total_hunks: 0,
        final_decisions: Vec::new(),
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
        owner_key: deployment.authority.principal_key.clone(),
        base: elsewhere,
        resume: false,
        stored: None,
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

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn proposal_updates_require_the_owner_and_acknowledged_version_but_allow_exact_retry() {
    let Some(deployment) = deployment("proposal-update-guards").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let base = frontier(&room).await;
    let (first_branch, first_tip) = room
        .log()
        .with_fork_at(&base, |at| {
            author_forks(at, 4242, "paper.md", "The tabby sat.\n")
        })
        .await
        .unwrap();
    let opened = open(&room, &deployment.authority, "Ada", &base).await;
    let acknowledged = update(
        &room,
        &deployment.authority,
        &opened,
        &first_tip,
        &first_branch,
    )
    .await;
    let original_bytes = acknowledged.branch_bytes.clone();

    // A resent whole snapshot is safe even if its acknowledgement was lost.
    let mut exact_retry = UpdateProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: opened.id,
        owner_key: deployment.authority.principal_key.clone(),
        expected_version: opened.version,
        tip: first_tip.clone(),
        base: None,
        branch: first_branch.clone(),
        stored: None,
        decided: Vec::new(),
        carried: Vec::new(),
    };
    let replayed = room
        .command(&deployment.authority, &mut exact_retry)
        .await
        .expect("an exact retry is idempotent");
    assert_eq!(replayed.version, acknowledged.version);
    assert_eq!(replayed.branch_bytes, original_bytes);

    let (different_branch, different_tip) = room
        .log()
        .with_fork_at(&base, |at| {
            author_forks(at, 4343, "paper.md", "The tabby purred.\n")
        })
        .await
        .unwrap();
    let mut wrong_owner = UpdateProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: opened.id,
        owner_key: "another-author".into(),
        expected_version: acknowledged.version,
        tip: different_tip.clone(),
        base: None,
        branch: different_branch.clone(),
        stored: None,
        decided: Vec::new(),
        carried: Vec::new(),
    };
    let refused_owner = room
        .command(&deployment.authority, &mut wrong_owner)
        .await
        .expect_err("another author cannot update the branch");
    assert!(matches!(
        refused_owner,
        crate::log::CommandError::Conflict(_)
    ));
    assert_eq!(
        deployment
            .catalog
            .proposal(opened.id)
            .await
            .unwrap()
            .unwrap()
            .branch_bytes,
        original_bytes
    );

    let mut stale_version = UpdateProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: opened.id,
        owner_key: deployment.authority.principal_key.clone(),
        expected_version: opened.version,
        tip: different_tip,
        base: None,
        branch: different_branch,
        stored: None,
        decided: Vec::new(),
        carried: Vec::new(),
    };
    let refused_stale = room
        .command(&deployment.authority, &mut stale_version)
        .await
        .expect_err("an older acknowledged version cannot replace a newer branch");
    assert!(matches!(
        refused_stale,
        crate::log::CommandError::Storage(crate::storage::postgres::Error::Conflict(_))
    ));
    let still_original = deployment
        .catalog
        .proposal(opened.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still_original.version, acknowledged.version);
    assert_eq!(still_original.branch_bytes, original_bytes);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn resuming_an_unknown_proposal_id_does_not_create_a_row() {
    let Some(deployment) = deployment("proposal-resume-unknown").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let id = Uuid::new_v4();
    let mut command = OpenProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id,
        author: "Ada".into(),
        owner_key: deployment.authority.principal_key.clone(),
        base: frontier(&room).await,
        resume: true,
        stored: None,
    };
    let unknown = room.command(&deployment.authority, &mut command).await;
    assert!(
        matches!(&unknown, Err(crate::log::CommandError::Conflict(message)) if message.starts_with("proposal status is unknown")),
        "got {unknown:?}"
    );
    assert!(deployment.catalog.proposal(id).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn whole_suggestion_resolution_replaces_legacy_mixed_hunk_answers() {
    let Some(deployment) = deployment("suggestion-whole-resolution").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    // Two changes separated by more than the hunk bridge threshold.
    other_writer_types(&room, "paper.md", "alpha 0123456789abcdefghij omega\n").await;
    let base = frontier(&room).await;
    let (branch, tip) = room
        .log()
        .with_fork_at(&base, |at| {
            author_forks(at, 4242, "paper.md", "aleph 0123456789abcdefghij omicron\n")
        })
        .await
        .unwrap();
    let proposal = open(&room, &deployment.authority, "Ada", &base).await;
    let proposal = update(&room, &deployment.authority, &proposal, &tip, &branch).await;

    // Rows left by the old per-hunk browser path disagree. MCP acceptance and
    // the socket's all:true mode both call resolve_suggestion, which must
    // replace that set with one accepted answer for every actual hunk.
    let mut tx = deployment.catalog.pool().begin().await.unwrap();
    deployment
        .catalog
        .decide_proposal_hunk(
            &mut tx,
            room.document_id,
            proposal.id,
            0,
            false,
            "legacy-reviewer",
            &proposal.tip_frontiers,
            None,
            2,
        )
        .await
        .unwrap();
    deployment
        .catalog
        .decide_proposal_hunk(
            &mut tx,
            room.document_id,
            proposal.id,
            1,
            true,
            "legacy-reviewer",
            &proposal.tip_frontiers,
            None,
            2,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let request_id = Uuid::new_v4();
    let mut tx = deployment.catalog.pool().begin().await.unwrap();
    deployment
        .catalog
        .resolve_suggestion(
            &mut tx,
            room.document_id,
            proposal.id,
            &proposal.tip_frontiers,
            true,
            Some(2),
            "Ada",
            Some(request_id),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let receipt = deployment
        .catalog
        .proposal_outcome(room.document_id, proposal.id)
        .await
        .unwrap()
        .expect("whole decision records its final result");
    assert_eq!(receipt.decisions.0, vec![(0, true), (1, true)]);
    assert!(!receipt.discarded);
    assert!(deployment
        .catalog
        .proposal(proposal.id)
        .await
        .unwrap()
        .is_none());
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

    let outcome = decide(&room, &deployment.authority, stored.id, 0, false, &tip)
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
    let stored = update(&room, &deployment.authority, &stored, &tip, &branch_bytes).await;
    let mut replay_open = OpenProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: stored.id,
        author: "Ada".into(),
        owner_key: deployment.authority.principal_key.clone(),
        base: base.clone(),
        resume: true,
        stored: None,
    };
    let replayed = room
        .command(&deployment.authority, &mut replay_open)
        .await
        .unwrap();
    assert_eq!(replayed.version, stored.version);
    assert_eq!(replayed.base_frontiers, stored.base_frontiers);

    // A decision against a tip the proposal has moved past is refused (§7.1).
    let stale = decide(&room, &deployment.authority, stored.id, 0, true, &base).await;
    assert!(
        matches!(stale, Err(crate::log::CommandError::Conflict(_))),
        "a decision against the wrong tip is refused, got {stale:?}"
    );

    // The real decision, against the tip the reviewer was shown.
    let outcome = decide(&room, &deployment.authority, stored.id, 0, true, &tip)
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
    assert!(
        deployment
            .catalog
            .proposal(stored.id)
            .await
            .unwrap()
            .is_none(),
        "a resolved proposal is deleted, not kept"
    );
    let receipt = deployment
        .catalog
        .proposal_outcome(room.document_id, stored.id)
        .await
        .unwrap()
        .expect("a deleted proposal retains its reconnect result");
    assert_eq!(receipt.base_frontiers, stored.base_frontiers);
    assert_eq!(receipt.tip_frontiers, stored.tip_frontiers);
    assert_eq!(receipt.decisions.0, vec![(0, true)]);
    assert!(!receipt.discarded);

    let mut resumed_closed = OpenProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: stored.id,
        author: "Ada".into(),
        owner_key: deployment.authority.principal_key.clone(),
        base: tip.clone(),
        resume: true,
        stored: None,
    };
    let closed_retry = room
        .command(&deployment.authority, &mut resumed_closed)
        .await;
    assert!(
        matches!(closed_retry, Err(crate::log::CommandError::Conflict(_))),
        "a resumed id with a retained outcome is replayed by the socket, never reopened: {closed_retry:?}"
    );
    assert!(deployment
        .catalog
        .proposal(stored.id)
        .await
        .unwrap()
        .is_none());
    let mut delayed_initial_retry = OpenProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: stored.id,
        author: "Ada".into(),
        owner_key: deployment.authority.principal_key.clone(),
        base: tip.clone(),
        resume: false,
        stored: None,
    };
    let delayed_retry = room
        .command(&deployment.authority, &mut delayed_initial_retry)
        .await;
    assert!(
        matches!(delayed_retry, Err(crate::log::CommandError::Storage(_))),
        "a delayed initial-open retry cannot recreate an id with a receipt: {delayed_retry:?}"
    );
    assert!(deployment
        .catalog
        .proposal(stored.id)
        .await
        .unwrap()
        .is_none());

    // A second decision is refused and changes nothing: the proposal is
    // gone. The socket can answer a retry from the retained proposal outcome.
    let again = decide_with(
        &room,
        &deployment.authority,
        stored.clone(),
        Vec::new(),
        0,
        true,
        &tip,
    )
    .await;
    assert!(
        again.is_err(),
        "a decided proposal takes no second decision"
    );
    assert_eq!(
        text(&room, path).await,
        body,
        "a refused second decision must not change the text"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_discard_keeps_a_reconnect_receipt_without_rebuilding_the_branch() {
    let Some(deployment) = deployment("discard-receipt").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let base = frontier(&room).await;
    let stored = open(&room, &deployment.authority, "Ada", &base).await;
    let mut command = DiscardProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: stored.id,
    };
    let discarded = room
        .command(&deployment.authority, &mut command)
        .await
        .unwrap();
    assert!(discarded.applied);

    let receipt = deployment
        .catalog
        .proposal_outcome(room.document_id, stored.id)
        .await
        .unwrap()
        .expect("discarded proposals retain reconnect status");
    assert!(receipt.discarded);
    assert!(receipt.decisions.0.is_empty());
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
    assert!(
        deployment
            .catalog
            .proposal(stored.id)
            .await
            .unwrap()
            .is_none(),
        "and deleted, with its hunk decisions"
    );
}

/// A decision survives the author absorbing a coauthor's edit.
///
/// The author's branch takes in a word somebody else changed and moves its
/// base past it, which moves the tip and so renumbers the hunks. The decision
/// already made has to follow its change: kept, it must still count toward
/// resolving, and the answer applied at the end must be the one given to that
/// change and no other.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_decision_follows_its_hunk_when_the_base_moves() {
    let Some(deployment) = deployment("carry").await else {
        return;
    };
    let room = deployment.rooms.get(&deployment.slug).await.unwrap();
    let path = "paper.md";
    let middle = " It was a long and quiet afternoon in the old town.";
    other_writer_types(
        &room,
        path,
        &format!("The cat sat on the mat.{middle} Far away, a dog ran.\n"),
    )
    .await;

    let base = frontier(&room).await;
    let (branch_bytes, tip) = room
        .log()
        .with_fork_at(&base, |at| {
            author_forks(
                at,
                4242,
                path,
                &format!("The tabby sat on the mat.{middle} Far away, a dog sprinted.\n"),
            )
        })
        .await
        .unwrap();
    let stored = open(&room, &deployment.authority, "Ada", &base).await;
    let stored = update(&room, &deployment.authority, &stored, &tip, &branch_bytes).await;

    let first = decide(&room, &deployment.authority, stored.id, 0, true, &tip)
        .await
        .unwrap();
    assert!(!first.resolved, "one of two hunks answered");

    // A coauthor changes a word between the two hunks, and the author's
    // branch absorbs it: the base moves to the room's frontier and the tip to
    // the branch's, with the author's own operations unchanged.
    other_writer_types(&room, path, "The cat sat on the mat. It was a long and sleepy afternoon in the old town. Far away, a dog ran.\n").await;
    let new_base = frontier(&room).await;
    let new_tip = room
        .log()
        .with_fork_at(&new_base, |at| {
            session::apply_update(at, &branch_bytes).unwrap();
            at.state_frontiers()
        })
        .await
        .unwrap();
    let mut command = UpdateProposal {
        document_id: room.document_id,
        catalog: room.catalog().clone(),
        id: stored.id,
        owner_key: deployment.authority.principal_key.clone(),
        expected_version: stored.version,
        tip: new_tip.clone(),
        base: Some(new_base),
        branch: branch_bytes.clone(),
        stored: None,
        decided: Vec::new(),
        carried: Vec::new(),
    };
    room.command(&deployment.authority, &mut command)
        .await
        .unwrap();

    let decided = room.catalog().decisions(stored.id).await.unwrap();
    assert_eq!(
        decided
            .iter()
            .map(|d| (d.hunk_index, d.accepted))
            .collect::<Vec<_>>(),
        vec![(0, true)],
        "the accepted hunk keeps its answer under the new numbering"
    );

    let last = decide(&room, &deployment.authority, stored.id, 1, false, &new_tip)
        .await
        .unwrap();
    assert!(last.resolved, "the second answer completes the review");
    let body = text(&room, path).await;
    assert!(
        body.contains("tabby"),
        "the accepted change landed, got {body:?}"
    );
    assert!(
        body.contains("sleepy"),
        "the coauthor's word is kept, got {body:?}"
    );
    assert!(
        body.contains("dog ran."),
        "the declined change did not land, got {body:?}"
    );
}
