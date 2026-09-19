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

/// One stored proposal in the shape the wire uses, so that a client sent the
/// whole list and a client sent a single change are reading the same fields.
fn summarize(p: &crate::storage::postgres::StoredProposal) -> serde_json::Value {
    serde_json::json!({
        "id": p.id.to_string(),
        "author": p.author,
        "base": super::encode_update(&p.base_frontiers),
        "tip": super::encode_update(&p.tip_frontiers),
        "branch": super::encode_update(&p.branch_bytes),
    })
}

/// What went wrong deciding one.
#[derive(Debug)]
pub enum ProposalError {
    /// The decision was computed against a tip the proposal has moved past.
    /// The author edited while the reviewer was reading, so the hunks the
    /// reviewer decided about are not the hunks that are there now.
    Stale,
    /// The base a client forked at is not a frontier this room can reach, so
    /// the branch cannot be rebuilt against it. The author is ahead of what
    /// they have sent: their own operations have to arrive before a proposal
    /// can be opened on top of them.
    UnknownBase,
    /// The branch could not be read, or the room refused the result.
    Failed(String),
}

impl std::fmt::Display for ProposalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => f.write_str("this proposal has changed since it was reviewed"),
            Self::UnknownBase => {
                f.write_str("this proposal forked from work the server has not received yet")
            }
            Self::Failed(text) => f.write_str(text),
        }
    }
}

/// Turns a suggestion into a branch: one hunk putting the proposed text where
/// the comment's passage is.
///
/// This is §1.2's consolidation. A comment that suggests a change used to carry
/// both the replacement text and its accept/reject state on itself, which made
/// it a second implementation of "a change awaiting a decision". As a branch it
/// is reviewed and decided exactly like a change anyone typed.
///
/// `at` is a BYTE offset, because `locate_anchor` finds the passage by
/// searching the file as a string. Rather than convert it to the UTF-16 offsets
/// the rest of this module speaks, the replacement is made on the string and
/// the result handed to `put_text`, which diffs it. So no offset crosses from
/// one counting system to the other -- the mistake §3.2 exists to prevent --
/// and the hunk that comes out is whatever actually changed rather than
/// whatever we calculated ought to have.
pub fn from_suggestion(
    doc: &LoroDoc,
    path: &str,
    at: usize,
    exact: &str,
    proposed: &str,
) -> Result<(LoroDoc, Frontiers, Frontiers, PeerID), ProposalError> {
    let body = session::texts_of(doc).get(path).cloned().ok_or_else(|| {
        ProposalError::Failed("that suggestion names a file that is not here".into())
    })?;
    let end = at.saturating_add(exact.len());
    // The anchor was found against some reading of this file. If the file no
    // longer says there what it said then, the passage has moved or changed and
    // the suggestion is about text that is not there -- which is a refusal,
    // not something to place approximately.
    if body.get(at..end) != Some(exact) {
        return Err(ProposalError::Stale);
    }

    let base = doc.state_frontiers();
    let branch = doc.fork();
    let peer = fresh_peer();
    branch
        .set_peer_id(peer)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let mut wanted = body.clone();
    wanted.replace_range(at..end, proposed);
    session::put_text(&branch, path, &wanted);
    branch.commit();
    let tip = branch.state_frontiers();
    Ok((branch, base, tip, peer))
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
    // The branch as it was at the base, so a hunk that bridged a short retain
    // can say what its new side reads as rather than dropping the bridged words
    // (`document/hunks.rs`). These hunks are read by people -- the Changes
    // queue, and `proposed_text` below -- which is what that costs a fork for.
    let at_base = branch
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    Ok(
        hunks_of_batch(&batch, |cid| at_base.get_text(cid.clone()).to_string())
            .into_iter()
            .map(|(_, h)| h)
            .collect(),
    )
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

/// A peer id for a branch that nothing else will use.
///
/// This is not a nicety. Loro names an operation by its peer and a counter that
/// peer allocates, so two branches forked from the same point and claiming the
/// same peer allocate the SAME names for different edits. Importing the second
/// does not conflict and does not error: it is taken for an update already
/// seen, and its operations are discarded in silence. Two people proposing a
/// change to one paragraph would lose one of the proposals with nothing
/// reported anywhere.
///
/// So a proposal mints its own peer when it opens. Borrowing one from the
/// socket was the shape of that bug -- a socket can open more than one
/// proposal, and did.
fn fresh_peer() -> PeerID {
    let bytes = crate::auth::random_bytes(8);
    let mut id = [0u8; 8];
    id.copy_from_slice(&bytes);
    // Peer 0 is Loro's own default, so taking it is the collision above with
    // whoever else did not choose either.
    PeerID::from_le_bytes(id) | 1
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

    /// Opens a proposal at the frontier the author forked their own copy at.
    ///
    /// The base comes from the client because only the client knows it. The
    /// room's frontier at the moment this message is handled is a different
    /// thing entirely: anything committed here since the author forked is not
    /// in their branch, so recording the room's frontier as the base makes
    /// every later read of this proposal -- `rebuild` forks at it, `hunks`
    /// diffs from it -- describe a fork that never happened.
    ///
    /// Recording the wrong one is not known to lose or misattribute text:
    /// `declining_a_proposal_does_not_revert_a_concurrent_writer` reproduces
    /// the race and Loro's merge converges either way. What it does is put the
    /// server's record of a proposal at odds with §5.1 and with where the
    /// author is actually typing, which is a bad thing to be casual about in
    /// the one structure the review model is built on.
    ///
    /// Forking here is also the check that we can fork at all. [`rebuild`]
    /// forks at this frontier on every read of the proposal, so a base this
    /// room cannot reach is refused now, at the one moment there is a client
    /// waiting to be told, rather than on somebody's first attempt to review.
    pub(crate) async fn open_proposal(
        &self,
        author: &str,
        base: &Frontiers,
    ) -> Result<String, ProposalError> {
        let peer = fresh_peer();
        let (catalog, document) = self.catalog_and_document()?;
        {
            let state = self.state.lock().await;
            state
                .session
                .doc
                .fork_at(base)
                .map_err(|_| ProposalError::UnknownBase)?;
        }
        let base = base.clone();
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

    /// Opens a proposal for a suggestion, and returns its id for the comment to
    /// carry.
    ///
    /// A suggestion arrives as a passage and the text somebody wants in its
    /// place. That is a one-hunk branch, so this makes one: the comment ends up
    /// holding an id, and the change it offers is reviewed and decided like any
    /// other (§1.2).
    pub(crate) async fn open_suggestion(
        &self,
        author: &str,
        path: &str,
        at: usize,
        exact: &str,
        proposed: &str,
    ) -> Result<String, ProposalError> {
        let (id, proposal) = self
            .prepare_suggestion(author, path, at, exact, proposed)
            .await?;
        let (catalog, _) = self.catalog_and_document()?;
        catalog
            .open_proposal(proposal)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;
        Ok(id)
    }

    pub(crate) async fn prepare_suggestion(
        &self,
        author: &str,
        path: &str,
        at: usize,
        exact: &str,
        proposed: &str,
    ) -> Result<(String, crate::storage::postgres::NewProposal), ProposalError> {
        let (_, document) = self.catalog_and_document()?;
        let (branch, base, tip, peer, bytes) = {
            let state = self.state.lock().await;
            let (branch, base, tip, peer) =
                from_suggestion(&state.session.doc, path, at, exact, proposed)?;
            // The branch's own operations, which is all that is stored:
            // rebuilding it means forking the room at the base and replaying
            // these.
            let bytes = session::encode_diff(&branch, &session::encode_vector(&state.session.doc))
                .map_err(ProposalError::Failed)?;
            (branch, base, tip, peer, bytes)
        };
        let _ = branch;
        let id = crate::storage::postgres::new_id();
        Ok((
            id.to_string(),
            crate::storage::postgres::NewProposal {
                document_id: document,
                id,
                author: author.to_string(),
                author_peer: peer as i64,
                base_frontiers: base.encode(),
                tip_frontiers: tip.encode(),
                branch_bytes: bytes,
            },
        ))
    }

    /// Writes a branch down as a proposal, and returns its id.
    ///
    /// Split from [`Room::open_suggestion`] because the callers that make a
    /// suggestion while holding the room state cannot take that lock again --
    /// it is not reentrant, and asking for it twice is a deadlock rather than
    /// an error. So they build the branch under the lock they already hold,
    /// where the document is, and store it here after letting it go.
    pub(crate) async fn store_proposal(
        &self,
        author: &str,
        peer: PeerID,
        base: &Frontiers,
        tip: &Frontiers,
        branch: Vec<u8>,
    ) -> Result<String, ProposalError> {
        let (catalog, document) = self.catalog_and_document()?;
        let id = crate::storage::postgres::new_id();
        catalog
            .open_proposal(crate::storage::postgres::NewProposal {
                document_id: document,
                id,
                author: author.to_string(),
                author_peer: peer as i64,
                base_frontiers: base.encode(),
                tip_frontiers: tip.encode(),
                branch_bytes: branch,
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

    /// What a suggestion proposes, read from its branch.
    ///
    /// The text is not stored beside the comment any more, so this is how the
    /// places that show it -- a reader's view, an export, an agent asking what
    /// is pending -- get it. A suggestion is one hunk, and the words it wants
    /// are that hunk's insertion.
    ///
    /// `None` when the comment is not a suggestion. An error only when the
    /// proposal it names cannot be read, which means something is wrong rather
    /// than absent.
    pub(crate) async fn proposed_text(
        &self,
        proposal: &str,
    ) -> Result<Option<String>, ProposalError> {
        if proposal.is_empty() {
            return Ok(None);
        }
        let (catalog, _) = self.catalog_and_document()?;
        let id = uuid::Uuid::parse_str(proposal)
            .map_err(|_| ProposalError::Failed("that is not a proposal".into()))?;
        let Some(stored) = catalog
            .proposal(id)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?
        else {
            return Ok(None);
        };
        let proposal = Proposal {
            base: Frontiers::decode(&stored.base_frontiers)
                .map_err(|error| ProposalError::Failed(error.to_string()))?,
            tip: Frontiers::decode(&stored.tip_frontiers)
                .map_err(|error| ProposalError::Failed(error.to_string()))?,
        };
        let found = {
            let state = self.state.lock().await;
            hunks(&state.session.doc, &proposal, &stored.branch_bytes)?
        };
        // A suggestion is one hunk by construction. More than one means the
        // branch has grown beyond what a suggestion is, and joining them would
        // read as a single replacement that nobody proposed.
        Ok(found.first().map(|hunk| hunk.inserted.clone()))
    }

    pub(crate) async fn hydrate_proposed_comments(
        &self,
        comments: &mut [crate::room::Comment],
    ) -> Result<(), ProposalError> {
        for comment in comments {
            if !comment.proposal.is_empty() {
                comment.proposed = self.proposed_text(&comment.proposal).await?;
            }
        }
        Ok(())
    }

    /// The proposals a joining client needs to know about.
    pub(crate) async fn open_proposals(&self) -> Result<Vec<serde_json::Value>, ProposalError> {
        let (catalog, document) = self.catalog_and_document()?;
        let open = catalog
            .open_proposals(document)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;
        Ok(open.iter().map(summarize).collect())
    }

    /// One proposal, as the list would have described it.
    ///
    /// A branch that has just been opened or added to is news: until somebody
    /// is told, the author is typing into a fork nobody can see -- including
    /// their own browser, which draws the queue from what the server says is
    /// open. Sending the whole list on every flush would carry everyone else's
    /// branch bytes with it, so the one that changed travels alone, in the
    /// same shape, and the client merges it into the list it already has.
    ///
    /// `None` once the proposal is no longer pending: a decided branch is not
    /// something to put back into a reviewer's queue.
    pub(crate) async fn proposal_summary(
        &self,
        id: &str,
    ) -> Result<Option<serde_json::Value>, ProposalError> {
        let (catalog, _) = self.catalog_and_document()?;
        let id = uuid::Uuid::parse_str(id)
            .map_err(|_| ProposalError::Failed("that is not a proposal".into()))?;
        let stored = catalog
            .proposal(id)
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;
        Ok(stored
            .filter(|p| p.status == "pending")
            .as_ref()
            .map(summarize))
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

        let frontier = candidate.state_frontiers().encode();
        let state_bytes = session::encode_state(&candidate).len() as i64;
        let previous_sequence = self.state.lock().await.session.durable_sequence;

        let durable_sequence = catalog
            .resolve_with_update(
                uuid,
                by.to_string(),
                document,
                &update,
                &frontier,
                state_bytes,
            )
            .await
            .map_err(|error| ProposalError::Failed(error.to_string()))?;

        // Durable, so the room may have it.
        {
            let mut state = self.state.lock().await;
            session::apply_update(&state.session.doc, &update).map_err(ProposalError::Failed)?;
            let metadata = LoroDoc::decode_import_blob_meta(&update, true)
                .map_err(|error| ProposalError::Failed(error.to_string()))?;
            state.session.durable_vector.merge(&metadata.partial_end_vv);
            if durable_sequence == previous_sequence + 1 {
                state.session.durable_sequence = durable_sequence;
            }
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

    /// A decision names the tip it was made against, and that tip crosses the
    /// wire: the browser encodes it with loro-crdt's `encodeFrontiers` and the
    /// server decodes it with `Frontiers::decode`. Nothing checks that those
    /// two agree -- they are the same library, but a format change on one side
    /// would make every decision compare unequal and be refused as stale, which
    /// is a confusing way to find out.
    ///
    /// So the bytes are pinned. These were produced by loro-crdt in node and by
    /// this crate, and checked to be identical, including for a peer id too
    /// large to fit in the 16 bits an earlier hand-rolled encoding allowed for.
    #[test]
    fn frontier_bytes_are_what_the_browser_writes() {
        fn hex(bytes: &[u8]) -> String {
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        }

        let doc = session::new_doc();
        doc.set_peer_id(7).unwrap();
        doc.get_text("f").insert_utf16(0, "hello").unwrap();
        doc.commit();
        assert_eq!(hex(&doc.state_frontiers().encode()), "010708");

        let big = session::new_doc();
        big.set_peer_id(123_456_789_012_345).unwrap();
        big.get_text("f").insert_utf16(0, "xyz").unwrap();
        big.commit();
        assert_eq!(hex(&big.state_frontiers().encode()), "01f9beb7b088891c04");
    }

    /// A proposal's base is where its author forked, and nowhere else.
    ///
    /// `open_proposal` used to record the room's frontier at the moment the
    /// `proposal-open` message happened to be handled, ignoring the base the
    /// client sent. The two agree in a quiet room, so this was invisible until
    /// somebody else was typing -- which is the only time proposals matter.
    ///
    /// When they disagree the recorded base is *ahead* of the fork, and the
    /// branch does not contain what the room did in between.
    ///
    /// This pins the property that makes the base worth getting right: given
    /// the author's own fork point, the hunks of a proposal describe only what
    /// its author did, and resolving it leaves a concurrent writer's sentence
    /// alone. The existing `main_moving_on_does_not_disturb_a_proposal` covers
    /// a concurrent edit to a different file; this one is the same file, which
    /// is the case where a diff could actually confuse the two.
    #[test]
    fn a_proposal_holds_only_its_authors_work_when_the_room_moves_under_it() {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();

        // Where the author forked, and what the room had at that moment.
        let base = room.state_frontiers();
        let at_fork = session::encode_vector(&room);

        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat.");
        branch.commit();
        let tip = branch.state_frontiers();
        let bytes = session::encode_diff(&branch, &at_fork).unwrap();

        // Somebody else writes into the room while the proposal is open. This
        // is the operation that must not end up inside it.
        session::put_text(&room, "main.md", "The cat sat. It purred.");
        room.commit();
        assert_ne!(
            room.state_frontiers(),
            base,
            "the room has to have moved, or this test proves nothing"
        );

        let proposal = Proposal {
            base,
            tip: tip.clone(),
        };
        let found = hunks(&room, &proposal, &bytes).unwrap();
        assert_eq!(
            found.len(),
            1,
            "only the author's own change is a hunk, got {found:?}"
        );

        // Accept it whole. The author's word lands; the other writer's
        // sentence is still there, because it was never part of the diff.
        let declined = HashSet::new();
        resolve(&room, &proposal, &bytes, &declined, &tip, REVIEWER).unwrap();
        let text = text_at(&room, "main.md");
        assert!(
            text.contains("tabby"),
            "the accepted word is in, got {text:?}"
        );
        assert!(
            text.contains("It purred."),
            "the concurrent writer's sentence survives an unrelated proposal, got {text:?}"
        );
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

#[cfg(test)]
mod peer_tests {
    use super::*;

    /// Two proposals against the same passage must both survive, intact.
    ///
    /// They will not if they share a peer id. Loro names an operation by its
    /// peer and a counter that peer allocates, so two branches forked from one
    /// point and claiming one peer allocate the same names for different edits.
    /// Importing the second returns `Ok` and does not conflict, because the
    /// names look like ones already seen. What comes out is not one proposal or
    /// the other but a splice of both: text nobody wrote and nobody approved,
    /// arrived at without a single error being reported.
    #[test]
    fn a_shared_peer_would_corrupt_two_proposals_into_neither() {
        let room = session::new_doc();
        room.set_peer_id(1).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();
        let vv = session::encode_vector(&room);

        // What the old code did: take the peer from the socket, so one socket
        // opening two proposals gives both the same one.
        let shared = 42;
        let mut updates = Vec::new();
        for wanted in ["The tabby sat.", "The lion sat."] {
            let branch = room.fork();
            branch.set_peer_id(shared).unwrap();
            session::put_text(&branch, "main.md", wanted);
            branch.commit();
            updates.push(session::encode_diff(&branch, &vv).unwrap());
        }
        for update in &updates {
            session::apply_update(&room, update).expect("neither import errors");
        }
        let got = session::texts_of(&room).get("main.md").cloned().unwrap();
        assert!(
            got != "The tabby sat." && got != "The lion sat.",
            "a shared peer does not merely lose the second proposal -- it corrupts the text \
             into one that nobody proposed. Got {got:?}, and neither author wrote that. \
             Pinned so a change that reintroduces peer sharing fails here and not in a paper."
        );

        // What it does now: each proposal mints its own, so both arrive.
        let fresh = session::new_doc();
        fresh.set_peer_id(1).unwrap();
        session::put_text(&fresh, "main.md", "The cat sat.");
        fresh.commit();
        let vv = session::encode_vector(&fresh);
        let mut peers = Vec::new();
        for wanted in ["The tabby sat.", "The lion sat."] {
            let branch = fresh.fork();
            let peer = fresh_peer();
            peers.push(peer);
            branch.set_peer_id(peer).unwrap();
            session::put_text(&branch, "main.md", wanted);
            branch.commit();
            session::apply_update(&fresh, &session::encode_diff(&branch, &vv).unwrap()).unwrap();
        }
        assert_ne!(peers[0], peers[1], "each proposal mints its own peer");
        let both = fresh.oplog_vv();
        assert!(
            both.get(&peers[0]).is_some() && both.get(&peers[1]).is_some(),
            "both proposals' operations are in the graph"
        );
    }

    #[test]
    fn a_minted_peer_is_never_zero() {
        // Zero is what a document that never chose gets, so taking it is a
        // collision with everyone who did not choose either.
        for _ in 0..64 {
            assert_ne!(fresh_peer(), 0);
        }
    }
}

#[cfg(test)]
mod suggestion_tests {
    use super::*;

    const OWNER: PeerID = 1;
    const REVIEWER: PeerID = 3;

    fn room_with(body: &str) -> LoroDoc {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", body);
        room.commit();
        room
    }

    fn decide(room: &LoroDoc, made: (LoroDoc, Frontiers, Frontiers, PeerID), declined: bool) {
        let (branch, base, tip, _) = made;
        let bytes = session::encode_diff(&branch, &session::encode_vector(room)).unwrap();
        let declined = if declined {
            HashSet::from([0])
        } else {
            HashSet::new()
        };
        let proposal = Proposal {
            base,
            tip: tip.clone(),
        };
        resolve(room, &proposal, &bytes, &declined, &tip, REVIEWER).unwrap();
    }

    fn text_of(room: &LoroDoc) -> String {
        session::texts_of(room).get("main.md").cloned().unwrap()
    }

    #[test]
    fn a_suggestion_is_one_decision() {
        let room = room_with("The cat sat on the mat.");
        let made = from_suggestion(&room, "main.md", 4, "cat", "tabby").unwrap();
        let bytes = session::encode_diff(&made.0, &session::encode_vector(&room)).unwrap();
        let proposal = Proposal {
            base: made.1.clone(),
            tip: made.2.clone(),
        };
        let found = hunks(&room, &proposal, &bytes).unwrap();
        assert_eq!(found.len(), 1, "one passage, one answer, got {found:?}");
    }

    #[test]
    fn accepting_a_suggestion_keeps_it_the_suggesters_words() {
        let room = room_with("The cat sat on the mat.");
        let made = from_suggestion(&room, "main.md", 4, "cat", "tabby").unwrap();
        let suggester = made.3;
        decide(&room, made, false);
        assert_eq!(text_of(&room), "The tabby sat on the mat.");
        assert!(
            room.oplog_vv().get(&suggester).is_some(),
            "the words that stayed are still whoever suggested them"
        );
    }

    #[test]
    fn a_suggestion_proposing_nothing_cuts_the_passage() {
        let room = room_with("The very cat sat.");
        let made = from_suggestion(&room, "main.md", 4, "very ", "").unwrap();
        decide(&room, made, false);
        assert_eq!(
            text_of(&room),
            "The cat sat.",
            "proposing the empty string is how a suggestion says to cut something"
        );
    }

    #[test]
    fn declining_a_suggestion_leaves_the_passage_alone() {
        let room = room_with("The cat sat.");
        let made = from_suggestion(&room, "main.md", 4, "cat", "tabby").unwrap();
        decide(&room, made, true);
        assert_eq!(text_of(&room), "The cat sat.");
    }

    #[test]
    fn a_suggestion_about_text_that_is_not_there_is_refused() {
        let room = room_with("The cat sat.");
        // The anchor was recorded when the file said something else.
        let refused = from_suggestion(&room, "main.md", 4, "dog", "tabby");
        assert!(
            matches!(refused, Err(ProposalError::Stale)),
            "placing it approximately would edit a passage nobody reviewed, got {refused:?}"
        );
    }

    #[test]
    fn a_suggestion_against_a_file_that_is_gone_is_refused() {
        let room = room_with("The cat sat.");
        let refused = from_suggestion(&room, "chapters/two.md", 0, "x", "y");
        assert!(
            matches!(refused, Err(ProposalError::Failed(_))),
            "got {refused:?}"
        );
    }
}
