//! Proposals: a change someone has offered, and what happens when it is decided.
//!
//! A proposal is a branch (SPEC-loro.md §3.3). It forks the room document at a
//! frontier, collects ordinary edits with no protocol of its own, and is
//! reviewed as the diff between where it forked and where it has reached.
//! Nothing about it lives inside the shared document: the branch is a blob of
//! operations, and whether each of its hunks was accepted is a row in Postgres
//! that only the server writes.
//!
//! ## Accepting part of one
//!
//! The obvious implementation — filter the diff to the accepted hunks and apply
//! it — produces the right text over the wrong authorship, because
//! `apply_diff` re-authors everything it applies as the applying peer. The
//! reviewer would appear to have written the prose they approved.
//!
//! So a proposal is merged whole and the declined hunks are then reverted, as
//! the reviewer. Every one of the author's operations enters the graph and
//! stays there, so the accepted text is still theirs; the removal is the
//! reviewer's, which is what actually happened. [`resolve`] is that sequence,
//! and it is one atomic step: the intermediate state holds the declined text,
//! and no peer ever sees it.
//!
//! ## Why it happens once, at the end
//!
//! Decisions stream, but the document changes when the proposal resolves
//! (§5.1a). The revert has to be computed against the tip, so merging on the
//! first accept would leave a later accept with nothing to do but re-apply a
//! hunk it had already reverted — as the reviewer, which is the re-authoring
//! this design exists to avoid. For a tracked edit, which is usually one hunk,
//! deciding it resolves the proposal and the difference is invisible.

use std::collections::HashSet;

use loro::{Frontiers, LoroDoc, PeerID};

use crate::document::hunks::{hunks_of_batch, keep_declined_batch, Hunk};
use crate::document::session;

/// Where a branch forked and where it has reached.
///
/// Everything else a proposal has -- who opened it, what it is called, whether
/// its hunks were taken -- belongs to the row in Postgres. What the document
/// layer needs is these two frontiers, because between them is the diff, and
/// the diff is the proposal.
#[derive(Clone, Debug)]
pub struct Proposal {
    pub base: Frontiers,
    pub tip: Frontiers,
}

/// What went wrong deciding one.
#[derive(Debug)]
pub enum ProposalError {
    /// The decision was computed against a tip the proposal has moved past.
    /// The author edited while the reviewer was reading, so the hunks the
    /// reviewer decided about are not the hunks that are there now.
    Stale,
    /// The branch could not be read, or the room refused the result.
    Failed(String),
}

impl std::fmt::Display for ProposalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => f.write_str("this proposal has changed since it was reviewed"),
            Self::Failed(text) => f.write_str(text),
        }
    }
}

/// Rebuilds a branch from the room document and the operations it added.
///
/// A branch is stored as its own operations only, so reading it back means
/// forking the room at the proposal's base and replaying them. Forking at the
/// base rather than at the room's tip is what keeps the diff below a diff of
/// the proposal, rather than of the proposal plus everything else the room did
/// while it was open.
fn rebuild(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
) -> Result<LoroDoc, ProposalError> {
    let branch = doc
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    session::apply_update(&branch, branch_bytes).map_err(ProposalError::Failed)?;
    Ok(branch)
}

/// What a reviewer is deciding about: the hunks between the branch's base and
/// its tip, numbered across every file it touches.
pub fn hunks(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
) -> Result<Vec<Hunk>, ProposalError> {
    let branch = rebuild(doc, proposal, branch_bytes)?;
    let batch = branch
        .diff(&proposal.base, &proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    Ok(hunks_of_batch(&batch).into_iter().map(|(_, h)| h).collect())
}

/// Carries a decided proposal into the document, and returns the single update
/// that did it.
///
/// `declined` names hunks by the index [`hunks`] gave them. `against` is the
/// tip the reviewer's decisions were computed from: if the proposal has moved
/// since, the decisions describe a diff that no longer exists and are refused
/// rather than applied to text the reviewer never saw.
///
/// The returned update covers the merge and the revert together. That is not a
/// convenience: between them the document holds the declined text, and §3.4
/// requires that no peer ever receives it and that it is never persisted as a
/// version of its own.
pub fn resolve(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
    declined: &HashSet<usize>,
    against: &Frontiers,
    reviewer: PeerID,
) -> Result<Vec<u8>, ProposalError> {
    if against != &proposal.tip {
        return Err(ProposalError::Stale);
    }
    let branch = rebuild(doc, proposal, branch_bytes)?;

    // Everything the room has already, so the update returned below is exactly
    // what this resolution added and nothing else.
    let before = session::encode_vector(doc);

    // Step one: the whole branch, so that every operation the author made --
    // and their authorship of it -- is in the graph before anything is taken
    // back out.
    session::apply_update(doc, branch_bytes).map_err(ProposalError::Failed)?;

    // Steps two and three: what it would take to undo the branch entirely,
    // narrowed to the hunks the reviewer declined.
    let inverse = branch
        .diff(&proposal.tip, &proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let reverts = keep_declined_batch(&inverse, |hunk| declined.contains(&hunk));

    // Step four: apply them as the reviewer. This is the whole of contract 4 --
    // the text that stays is the author's because their operations are still
    // here, and the text that goes was taken out by whoever declined it.
    let was = doc.peer_id();
    doc.set_peer_id(reviewer)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let applied = doc.apply_diff(reverts);
    doc.commit();
    let _ = doc.set_peer_id(was);
    applied.map_err(|error| ProposalError::Failed(error.to_string()))?;

    session::encode_diff(doc, &before).map_err(ProposalError::Failed)
}

/// One reviewer's answer about one hunk.
///
/// Gathered into a struct because these travel together and mean nothing
/// apart: `against` is only meaningful beside `hunk`, and `reviewer` is the
/// peer that `by` writes as.
pub(crate) struct Decision<'a> {
    pub hunk: usize,
    pub accepted: bool,
    pub by: &'a str,
    /// The tip the reviewer was looking at when they answered.
    pub against: &'a [u8],
    pub note: Option<String>,
    /// The peer the revert is written as, so that declining attributes the
    /// removal to whoever declined it.
    pub reviewer: PeerID,
}

/// What the room does with a proposal, as opposed to what a document does with
/// one.
///
/// The split matters: everything above this point is pure and testable against
/// two `LoroDoc`s, and everything below it is the part that has to agree with
/// Postgres and with everyone connected. The rule that keeps them apart is that
/// a proposal never reaches the shared document until Postgres has accepted the
/// resolution, so a crash cannot leave text that no proposal accounts for.
impl super::Room {
    fn catalog_and_document(
        &self,
    ) -> Result<
        (
            std::sync::Arc<crate::storage::postgres::PostgresCatalog>,
            uuid::Uuid,
        ),
        ProposalError,
    > {
        let catalog = self
            .catalog
            .as_ref()
            .get()
            .ok_or_else(|| ProposalError::Failed("this deployment has no catalog".into()))?;
        let document = uuid::Uuid::parse_str(&self.storage_id)
            .map_err(|_| ProposalError::Failed("this room has no storage identity".into()))?;
        Ok((catalog.clone(), document))
    }

    /// Opens a proposal against the room's current state.
    pub(crate) async fn open_proposal(
        &self,
        author: &str,
        peer: PeerID,
    ) -> Result<String, ProposalError> {
        let (catalog, document) = self.catalog_and_document()?;
        let base = {
            let state = self.state.lock().await;
            state.session.doc.state_frontiers()
        };
        let id = crate::storage::postgres::new_id();
        // A proposal opens empty. The author's first keystroke arrives as an
        // update, so there is nothing to store yet and the tip is the base.
        catalog
            .open_proposal(crate::storage::postgres::NewProposal {
                document_id: document,
                id,
                author: author.to_string(),
                author_peer: peer as i64,
                base_frontiers: base.encode(),
                tip_frontiers: base.encode(),
                branch_bytes: Vec::new(),
            })
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;
        Ok(id.to_string())
    }

    /// Records what the author has added to their branch.
    ///
    /// The whole branch is stored each time rather than appended to, because a
    /// branch is small -- it is one person's edits since they started -- and
    /// because replacing it means the stored blob and the stored tip can never
    /// disagree about what the branch contains.
    pub(crate) async fn update_proposal(
        &self,
        id: &str,
        tip: &[u8],
        branch: &[u8],
    ) -> Result<(), ProposalError> {
        let (catalog, _) = self.catalog_and_document()?;
        let id = uuid::Uuid::parse_str(id)
            .map_err(|_| ProposalError::Failed("that is not a proposal".into()))?;
        catalog
            .update_proposal_branch(id, tip.to_vec(), branch.to_vec())
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))
    }

    /// The proposals a joining client needs to know about.
    pub(crate) async fn open_proposals(&self) -> Result<Vec<serde_json::Value>, ProposalError> {
        let (catalog, document) = self.catalog_and_document()?;
        let open = catalog
            .open_proposals(document)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;
        Ok(open
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id.to_string(),
                    "author": p.author,
                    "base": super::encode_update(&p.base_frontiers),
                    "tip": super::encode_update(&p.tip_frontiers),
                    "branch": super::encode_update(&p.branch_bytes),
                })
            })
            .collect())
    }

    /// Records one hunk decision, and resolves the proposal once every hunk has
    /// one.
    ///
    /// Returns the update the room should broadcast, if this decision was the
    /// last one. Until then the document does not move: §5.1a explains why the
    /// merge waits for the whole answer rather than landing a hunk at a time.
    pub(crate) async fn decide_hunk(
        &self,
        id: &str,
        decision: Decision<'_>,
    ) -> Result<Option<Vec<u8>>, ProposalError> {
        let Decision {
            hunk,
            accepted,
            by,
            against,
            note,
            reviewer,
        } = decision;
        let (catalog, document) = self.catalog_and_document()?;
        let uuid = uuid::Uuid::parse_str(id)
            .map_err(|_| ProposalError::Failed("that is not a proposal".into()))?;
        let stored = catalog
            .proposal(uuid)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?
            .ok_or_else(|| ProposalError::Failed("that proposal is not open".into()))?;

        // The reviewer decided about a diff. If the author has typed since,
        // that diff no longer exists and the decision is about hunks that are
        // not there -- so it is refused rather than guessed at.
        if stored.tip_frontiers != against {
            return Err(ProposalError::Stale);
        }

        let proposal = Proposal {
            base: Frontiers::decode(&stored.base_frontiers)
                .map_err(|error| ProposalError::Failed(error.to_string()))?,
            tip: Frontiers::decode(&stored.tip_frontiers)
                .map_err(|error| ProposalError::Failed(error.to_string()))?,
        };

        catalog
            .decide_hunk(
                uuid,
                hunk as i32,
                accepted,
                by.to_string(),
                against.to_vec(),
                note,
            )
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;

        let decided = catalog
            .decisions(uuid)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;

        // Resolve on a fork, so that a Postgres failure below leaves the room's
        // own document exactly where it was. The shared document moves only
        // after the decision is durable.
        let (candidate, total) = {
            let state = self.state.lock().await;
            let candidate = state.session.doc.fork();
            let total = hunks(&candidate, &proposal, &stored.branch_bytes)?.len();
            (candidate, total)
        };
        if decided.len() < total {
            return Ok(None);
        }

        let declined: HashSet<usize> = decided
            .iter()
            .filter(|d| !d.accepted)
            .map(|d| d.hunk_index as usize)
            .collect();
        let update = resolve(
            &candidate,
            &proposal,
            &stored.branch_bytes,
            &declined,
            &proposal.tip,
            reviewer,
        )?;

        catalog
            .resolve_with_update(uuid, by.to_string(), document, &update)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;

        // Durable, so the room may have it.
        {
            let state = self.state.lock().await;
            session::apply_update(&state.session.doc, &update).map_err(ProposalError::Failed)?;
        }
        Ok(Some(update))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: PeerID = 1;
    const AUTHOR: PeerID = 2;
    const REVIEWER: PeerID = 3;

    /// A room with two files, and a proposal that rewrites a word in each.
    fn room_and_proposal() -> (LoroDoc, Proposal, Vec<u8>) {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        session::put_text(&room, "notes.md", "The dog ran.");
        room.commit();

        let base = room.state_frontiers();
        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat.");
        session::put_text(&branch, "notes.md", "The dog sprinted.");
        branch.commit();
        let tip = branch.state_frontiers();
        let bytes = session::encode_diff(&branch, &session::encode_vector(&room)).unwrap();

        let proposal = Proposal { base, tip };
        (room, proposal, bytes)
    }

    fn text_at(doc: &LoroDoc, path: &str) -> String {
        session::texts_of(doc)
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn a_proposal_across_two_files_numbers_its_hunks_in_one_list() {
        let (room, proposal, bytes) = room_and_proposal();
        let found = hunks(&room, &proposal, &bytes).unwrap();
        assert_eq!(found.len(), 2, "one decision per file, got {found:?}");
        assert_eq!(found[0].index, 0);
        assert_eq!(
            found[1].index, 1,
            "hunks are numbered across the whole proposal, not restarted per file"
        );
    }

    #[test]
    fn accepting_one_file_and_declining_the_other_keeps_both_authorships() {
        let (room, proposal, bytes) = room_and_proposal();
        let declined = HashSet::from([1]);
        let update = resolve(&room, &proposal, &bytes, &declined, &proposal.tip, REVIEWER).unwrap();

        assert_eq!(
            text_at(&room, "main.md"),
            "The tabby sat.",
            "the accepted file changed"
        );
        assert_eq!(
            text_at(&room, "notes.md"),
            "The dog ran.",
            "the declined file did not"
        );

        // Contract 4. The author's operations are still in the graph, so the
        // prose that stayed is still theirs, and the reviewer appears as the
        // peer that removed the rest rather than as the author of what remains.
        let vv = room.oplog_vv();
        assert!(
            vv.get(&AUTHOR).is_some(),
            "the author survives a partial accept"
        );
        assert!(
            vv.get(&REVIEWER).is_some(),
            "the reviewer is recorded as having declined"
        );
        assert!(
            vv.get(&OWNER).is_some(),
            "so does whoever wrote the base text"
        );

        // A peer that applies this one update lands on the same text, which is
        // what makes the intermediate state -- which held the declined prose --
        // unobservable rather than merely brief.
        let peer = session::new_doc();
        session::apply_update(&peer, &session::encode_state(&room)).unwrap();
        assert_eq!(text_at(&peer, "notes.md"), "The dog ran.");
        assert!(
            !update.is_empty(),
            "resolving produces an update to broadcast"
        );
    }

    #[test]
    fn declining_everything_leaves_the_document_where_it_was() {
        let (room, proposal, bytes) = room_and_proposal();
        let declined = HashSet::from([0, 1]);
        resolve(&room, &proposal, &bytes, &declined, &proposal.tip, REVIEWER).unwrap();
        assert_eq!(text_at(&room, "main.md"), "The cat sat.");
        assert_eq!(text_at(&room, "notes.md"), "The dog ran.");
    }

    #[test]
    fn accepting_everything_is_the_merge_alone() {
        let (room, proposal, bytes) = room_and_proposal();
        resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::new(),
            &proposal.tip,
            REVIEWER,
        )
        .unwrap();
        assert_eq!(text_at(&room, "main.md"), "The tabby sat.");
        assert_eq!(text_at(&room, "notes.md"), "The dog sprinted.");
        // Nothing was taken back out, so the reviewer never wrote anything.
        assert!(
            room.oplog_vv().get(&REVIEWER).is_none(),
            "accepting everything should not attribute anything to the reviewer"
        );
    }

    #[test]
    fn a_decision_made_against_a_stale_tip_is_refused() {
        let (room, mut proposal, bytes) = room_and_proposal();
        let reviewed = proposal.tip.clone();
        // The author types again while the reviewer is reading, so the hunks
        // the reviewer decided about are no longer the hunks that are there.
        proposal.tip = {
            let branch = rebuild(&room, &proposal, &bytes).unwrap();
            session::put_text(&branch, "main.md", "The tabby dozed.");
            branch.commit();
            branch.state_frontiers()
        };
        let refused = resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::new(),
            &reviewed,
            REVIEWER,
        );
        assert!(
            matches!(refused, Err(ProposalError::Stale)),
            "got {refused:?}"
        );
    }

    #[test]
    fn main_moving_on_does_not_disturb_a_proposal() {
        let (room, proposal, bytes) = room_and_proposal();
        // Somebody else edits a file the proposal does not touch.
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "extra.md", "Meanwhile.");
        room.commit();

        resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::from([1]),
            &proposal.tip,
            REVIEWER,
        )
        .unwrap();
        assert_eq!(text_at(&room, "main.md"), "The tabby sat.");
        assert_eq!(text_at(&room, "notes.md"), "The dog ran.");
        assert_eq!(
            text_at(&room, "extra.md"),
            "Meanwhile.",
            "the concurrent edit survives"
        );
    }
}
