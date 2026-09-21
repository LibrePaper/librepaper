//! Proposals: a change someone has offered, and what happens when it is decided.
//!
//! A proposal is a branch (SPEC-loro.md §3.3). It forks the room document at a
//! frontier, collects ordinary edits with no protocol of its own, and is
//! reviewed as the diff between where it forked and where it has reached.
//! Nothing about it lives inside the shared document: the branch is a blob of
//! operations, and whether each of its hunks was accepted is a row in Postgres
//! that only the server writes.
//!
//! Under SPEC-server-is-a-log §7, opening a proposal, moving its tip and
//! deciding a hunk are each a semantic command: `OpenProposal`, `UpdateProposal`
//! and `DecideProposalHunk` below. What used to be a Room method that acquired
//! `command_owner` by hand is now a command the sequencer runs -- head is
//! materialized, `evaluate` reads what it needs from it and (for a decision
//! that completes the review) prepares the merge, and `transact` writes the
//! proposal's own rows in the same transaction as the log row that made that
//! head durable.
//!
//! ## Accepting part of one
//!
//! The obvious implementation -- filter the diff to the accepted hunks and apply
//! it -- produces the right text over the wrong authorship, because
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
//! Decisions stream, but the document changes when the proposal resolves.
//! The revert has to be computed against the tip, so merging on the first
//! accept would leave a later accept with nothing to do but re-apply a hunk
//! it had already reverted -- as the reviewer, which is the re-authoring this
//! design exists to avoid. For a tracked edit, which is usually one hunk,
//! deciding it resolves the proposal and the difference is invisible.

use std::collections::HashSet;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use loro::{Frontiers, LoroDoc, PeerID};
use uuid::Uuid;

use crate::document::hunks::{hunks_of_batch, keep_declined_batch, Hunk};
use crate::document::session;
use crate::log::{Command, CommandError, Evidence, Head, PreparedSource};
use crate::storage::postgres::{
    self, NewLabel, NewProposal, PostgresCatalog, StoredDecision, StoredProposal,
};

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
///
/// Takes a plain `&LoroDoc` rather than a room, so it is called both from a
/// command's `evaluate` -- against `head.doc()` -- and from the pure tests
/// below.
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

/// The branch's two sides as documents: the room as it was at the proposal's
/// base, and the same with the proposal's operations applied.
///
/// This is what lets a caller ask "does this suggestion still propose what I
/// last read?" without a diff heuristic. [`from_suggestion`] builds a tip by
/// replacing one passage in one file's text, so the question is answered by
/// making that same replacement again and comparing the strings: hunks are
/// for review, and their bridging makes them the wrong tool for an identity
/// check.
pub fn sides(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
) -> Result<(LoroDoc, LoroDoc), ProposalError> {
    let branch = rebuild(doc, proposal, branch_bytes)?;
    let at_base = branch
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_tip = branch
        .fork_at(&proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    Ok((at_base, at_tip))
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
    // queue, and a suggestion's proposed text -- which is what that costs a
    // fork for.
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
/// Called both directly, by the tests below, and from `DecideProposalHunk`'s
/// `evaluate`, where `doc` is the draft `Head::prepare` forked -- so this
/// function's own return value is not what makes it into the log row there;
/// `prepare` exports its own diff of what this leaves on `doc`. What matters
/// is that between the merge and the revert `doc` holds the declined text,
/// and no peer ever sees that intermediate state: it is the caller's job,
/// same as ever, to make sure `doc` is a fork nobody else can read.
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

/// A command's request id, cast to the number a batch's `client_seq` column
/// holds (§7.3). Nothing compares this to another peer's own sequence, so the
/// exact mapping does not matter -- only that it is stable across a retry
/// that reuses the same request id.
fn client_seq_of(id: Uuid) -> i64 {
    i64::from_be_bytes(id.as_bytes()[..8].try_into().expect("a uuid is 16 bytes"))
}

// -- §7: the three semantic commands ------------------------------------

/// Opens a proposal at the frontier the author forked their own copy at.
///
/// The id is the client's, so a retry after a lost response finds the row it
/// already made rather than opening a second one (§7.2). The base comes from
/// the client because only the client knows it: the room's frontier at the
/// moment this command happens to run is a different thing entirely, and
/// recording that instead would make every later read of this proposal --
/// `rebuild` forks at it, `hunks` diffs from it -- describe a fork that never
/// happened.
pub struct OpenProposal {
    pub document_id: Uuid,
    pub catalog: Arc<PostgresCatalog>,
    pub id: Uuid,
    pub author: String,
    pub base: Frontiers,
}

impl Command for OpenProposal {
    type Output = StoredProposal;

    fn name(&self) -> &'static str {
        "proposal-open"
    }

    /// Forking here is also the check that the room can reach this base at
    /// all: `rebuild` forks at it on every later read of the proposal, so a
    /// base this room cannot reach is refused now, at the one moment there is
    /// a client waiting to be told, rather than on somebody's first attempt
    /// to review.
    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        head.doc()
            .fork_at(&self.base)
            .map_err(|_| CommandError::Conflict(ProposalError::UnknownBase.to_string()))?;
        // A proposal opens empty. The author's first keystroke arrives as an
        // `UpdateProposal`, so there is nothing to store yet and the tip is
        // the base; opening never moves the document.
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let peer = fresh_peer();
            let base = self.base.encode();
            self.catalog
                .open_proposal(
                    tx,
                    NewProposal {
                        document_id: self.document_id,
                        id: self.id,
                        author: self.author.clone(),
                        author_peer: peer as i64,
                        base_frontiers: base.clone(),
                        tip_frontiers: base,
                        branch_bytes: Vec::new(),
                    },
                )
                .await
                .map_err(CommandError::from)
        })
    }
}

/// Moves a proposal's branch to a new tip, conditional on the version the
/// caller read (§7.2). The author typing again is what calls this.
///
/// The whole branch is sent each time rather than appended to, because a
/// branch is small -- it is one person's edits since they started -- and
/// because replacing it means the stored blob and the stored tip can never
/// disagree about what the branch contains. It produces no source: the
/// document does not move until the proposal is decided.
pub struct UpdateProposal {
    pub document_id: Uuid,
    pub catalog: Arc<PostgresCatalog>,
    pub id: Uuid,
    pub expected_version: i64,
    pub tip: Frontiers,
    pub branch: Vec<u8>,
}

impl Command for UpdateProposal {
    type Output = StoredProposal;

    fn name(&self) -> &'static str {
        "proposal-update"
    }

    /// Nothing here checks the branch against the head document: its
    /// operations live on a peer the room's own graph does not have yet
    /// (they arrive only when the proposal resolves), so there is nothing
    /// about it `head` can confirm. The version column is the whole of the
    /// precondition, and it is checked transactionally in `transact`.
    fn evaluate(
        &mut self,
        _head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            self.catalog
                .update_proposal_branch(
                    tx,
                    self.document_id,
                    self.id,
                    self.expected_version,
                    self.tip.encode(),
                    self.branch.clone(),
                )
                .await
                .map_err(CommandError::from)
        })
    }
}

/// What deciding one hunk produced.
#[derive(Debug)]
pub struct ProposalDecided {
    /// Whether this was the decision that completed the review. Until every
    /// hunk has an answer the document does not move (§5.1a): a partial
    /// answer only records the row.
    pub resolved: bool,
}

/// Records one reviewer's answer about one hunk, and resolves the proposal
/// once every hunk has one.
///
/// `evaluate` has no database access (§7 step 2 is synchronous), so the
/// proposal row and the hunks already decided are read in `load`, which runs
/// under the sequencer lock, and left on `stored` and `decided` for
/// `evaluate` to use. The caller may hand in whatever it read before the
/// command started -- the socket reads the row anyway, to refuse an unknown
/// proposal with something better than a conflict -- but nothing prepared
/// here depends on that copy.
///
/// It has to be read under the lock because the merge this command prepares
/// depends on it: which hunks were declined, and whether this answer is the
/// last one. A snapshot taken before the lock can be one decision short of
/// what `transact` will count, and then the decision that completes the
/// review prepares no source at all while still resolving the proposal.
///
/// §7.1's precondition -- that the proposal's tip still matches what the
/// reviewer saw -- is checked against that reread here; the authoritative
/// check, against a fresh row locked `FOR UPDATE`, happens inside
/// `catalog.decide_proposal_hunk` in `transact`, which is what adjudicates a
/// race against an `UpdateProposal` from a different writer.
pub struct DecideProposalHunk {
    pub document_id: Uuid,
    pub catalog: Arc<PostgresCatalog>,
    pub proposal_id: Uuid,
    /// The proposal row. Whatever the caller read is replaced by `load`.
    pub stored: StoredProposal,
    /// Every hunk already decided, for the same proposal. Read by `load`.
    pub decided: Vec<StoredDecision>,
    pub hunk_index: i32,
    pub accepted: bool,
    pub decided_by: String,
    pub note: Option<String>,
    /// The tip the reviewer was looking at when they answered, encoded the
    /// same way the browser encoded it (`Proposal::tip.encode()`).
    pub against: Vec<u8>,
    /// The peer the revert is written as, so that declining attributes the
    /// removal to whoever declined it.
    pub reviewer: PeerID,
    pub request_id: Uuid,
    /// Set by `evaluate`, so `transact` does not have to recompute it under a
    /// different lock than the one it was measured under. The caller
    /// constructs this at zero; `evaluate` always runs before `transact` and
    /// overwrites it before it is read.
    pub total_hunks: i64,
}

impl Command for DecideProposalHunk {
    type Output = ProposalDecided;

    fn name(&self) -> &'static str {
        "proposal-decide"
    }

    // §7.2: a decision only produces source, and so only writes a
    // `document_labels` row, when it is the one that completes the review
    // (§5.1a). A retry with the same `request_id` of exactly that decision
    // finds the row and returns the completion it already recorded, rather
    // than re-running `resolve` against a proposal `decide_proposal_hunk`
    // would now refuse as no longer pending. A retry of a decision that did
    // not complete the review wrote no label and is unaffected: the answer
    // itself is idempotent by the hunk's primary key, which is checked
    // inside `transact`.
    fn replay(&mut self) -> BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>> {
        Box::pin(async move {
            let found = self
                .catalog
                .label_by_request(self.document_id, self.request_id)
                .await
                .map_err(CommandError::from)?;
            Ok(found.map(|_| ProposalDecided { resolved: true }))
        })
    }

    // The decision set this command merges from, read with the sequencer
    // lock held. Only another semantic command can add to it, and every one
    // of those holds the same lock, so what this reads is what `transact`
    // will count.
    fn load(&mut self) -> BoxFuture<'_, std::result::Result<(), CommandError>> {
        Box::pin(async move {
            let stored = self
                .catalog
                .proposal(self.proposal_id)
                .await
                .map_err(CommandError::from)?
                .ok_or_else(|| CommandError::Conflict("that proposal is not open".into()))?;
            // That the row belongs to this document is checked in `transact`
            // under the row lock, where it also has to be checked for the
            // benefit of anything that reached `transact` another way.
            self.stored = stored;
            self.decided = self
                .catalog
                .decisions(self.proposal_id)
                .await
                .map_err(CommandError::from)?;
            Ok(())
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        if self.stored.tip_frontiers != self.against {
            return Err(CommandError::Conflict(ProposalError::Stale.to_string()));
        }
        let proposal = Proposal {
            base: Frontiers::decode(&self.stored.base_frontiers)
                .map_err(|error| CommandError::Conflict(error.to_string()))?,
            tip: Frontiers::decode(&self.stored.tip_frontiers)
                .map_err(|error| CommandError::Conflict(error.to_string()))?,
        };
        let found = hunks(head.doc(), &proposal, &self.stored.branch_bytes)
            .map_err(|error| CommandError::Conflict(error.to_string()))?;
        self.total_hunks = found.len() as i64;
        if self.hunk_index < 0 || i64::from(self.hunk_index) >= self.total_hunks {
            return Err(CommandError::Conflict(
                "that hunk is not part of this proposal".into(),
            ));
        }

        let mut decided = self.decided.clone();
        if let Some(existing) = decided
            .iter_mut()
            .find(|item| item.hunk_index == self.hunk_index)
        {
            existing.accepted = self.accepted;
            existing.decided_by = self.decided_by.clone();
            existing.note = self.note.clone();
        } else {
            decided.push(StoredDecision {
                hunk_index: self.hunk_index,
                accepted: self.accepted,
                decided_by: self.decided_by.clone(),
                note: self.note.clone(),
            });
        }
        if decided.len() as i64 != self.total_hunks {
            // Not the last answer yet: the row this command writes in
            // `transact` is all there is to do.
            return Ok(None);
        }

        let declined: HashSet<usize> = decided
            .iter()
            .filter(|item| !item.accepted)
            .map(|item| item.hunk_index as usize)
            .collect();
        let branch_bytes = self.stored.branch_bytes.clone();
        let reviewer = self.reviewer;
        let client_seq = client_seq_of(self.request_id);
        head.prepare(client_seq, |draft| {
            resolve(
                draft,
                &proposal,
                &branch_bytes,
                &declined,
                &proposal.tip,
                reviewer,
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
        })
        .map_err(CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let complete = self
                .catalog
                .decide_proposal_hunk(
                    &mut *tx,
                    self.document_id,
                    self.proposal_id,
                    self.hunk_index,
                    self.accepted,
                    &self.decided_by,
                    &self.against,
                    self.note.as_deref(),
                    self.total_hunks,
                )
                .await?;
            if !complete {
                return Ok(ProposalDecided { resolved: false });
            }

            self.catalog
                .resolve_proposal(&mut *tx, self.proposal_id, &self.decided_by)
                .await?;

            // The suggestion this proposal answers, if it is one, stops
            // being open discussion the moment its branch is decided. A
            // proposal typed directly rather than suggested has no matching
            // row, so this touches nothing for it.
            sqlx::query(
                "UPDATE annotations SET resolved_at=COALESCE(resolved_at,now()),updated_at=now() \
                 WHERE proposal_id=$1",
            )
            .bind(self.proposal_id)
            .execute(&mut **tx)
            .await?;

            // The idempotency record for the merge this decision produced,
            // if it produced one: a retry with the same request id finds
            // this row rather than resolving the proposal a second time
            // (§7.2). A retry that lands after `head` already reflects the
            // resolution reruns `resolve` against already-seen operations,
            // which `Head::prepare` exports as an empty batch -- so
            // `evidence.after_frontier` is `None` and the label is written
            // against the head it found instead, which is the same state.
            let frontier = evidence
                .after_frontier
                .clone()
                .unwrap_or_else(|| evidence.before_frontier.clone());
            self.catalog
                .insert_label(
                    &mut *tx,
                    &NewLabel {
                        id: postgres::new_id(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier,
                        tree_digest: None,
                        label: None,
                        reason: "accept".into(),
                        request_id: Some(self.request_id),
                        author_account_id: None,
                        author_label: self.decided_by.clone(),
                    },
                )
                .await?;

            Ok(ProposalDecided { resolved: true })
        })
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
    /// `OpenProposal` records the base the client sent, never the room's own
    /// frontier at the moment the command happens to run: the two agree in a
    /// quiet room, so a regression here is invisible until somebody else is
    /// typing -- which is the only time proposals matter.
    ///
    /// This pins the property that makes the base worth getting right: given
    /// the author's own fork point, the hunks of a proposal describe only what
    /// its author did, and resolving it leaves a concurrent writer's sentence
    /// alone.
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
