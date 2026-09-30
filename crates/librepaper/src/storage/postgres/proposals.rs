//! Proposals: a branch, and the server-owned record of what was decided
//! about it.
//!
//! Every write here takes a transaction the caller owns rather than opening
//! one. A proposal is a semantic command (SPEC-server-is-a-log §7) -- what it
//! means depends on the source it forked from and on the source accepting it
//! produces -- so it is assembled by the sequencer, and its rows go in the
//! same transaction as the log row that made the state it was decided
//! against durable. A method here that opened its own transaction could not
//! be part of that.
//!
//! Retries are handled by the two mechanisms of §7.2, not by a receipt
//! table: a create carries a client-generated UUID as primary key and is
//! inserted with `ON CONFLICT DO NOTHING`, and a transition is conditional on
//! the row's `version` and returns the current row on a mismatch.

use sqlx::{FromRow, Row};
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

/// A proposal as it is opened. These arrive together and describe one thing,
/// so they travel as one rather than as a row of positional arguments where
/// two byte vectors sit next to each other and could be swapped unnoticed.
pub struct NewProposal {
    pub document_id: Uuid,
    pub id: Uuid,
    pub author: String,
    pub author_peer: i64,
    pub base_frontiers: Vec<u8>,
    pub tip_frontiers: Vec<u8>,
    pub branch_bytes: Vec<u8>,
}

/// A proposal as it is persisted: a branch and its review state.
#[derive(Clone, Debug, FromRow)]
pub struct StoredProposal {
    pub id: Uuid,
    pub author: String,
    /// The peer that wrote the branch. Loro's PeerID is u64; Postgres has no
    /// unsigned so the bit pattern is stored as bigint and interpreted where
    /// needed without conversion. This is what lets the sequencer tell a
    /// proposal's operations from everyone else's without decoding the blob.
    pub author_peer: i64,
    pub base_frontiers: Vec<u8>,
    pub tip_frontiers: Vec<u8>,
    pub branch_bytes: Vec<u8>,
    /// Bumped by every transition. A decision made against a version
    /// somebody else has already moved is refused rather than applied.
    pub version: i64,
}

/// A hunk decision: one row per (proposal_id, hunk_index).
#[derive(Clone, Debug, FromRow)]
pub struct StoredDecision {
    pub hunk_index: i32,
    pub accepted: bool,
    pub decided_by: String,
    pub note: Option<String>,
}

const SELECT: &str = "SELECT id,author,author_peer,base_frontiers,tip_frontiers,branch_bytes,\
     version FROM document_proposals";

impl PostgresCatalog {
    /// Opens a proposal. The id is the client's, so a retry after a lost
    /// response finds the row it already made rather than opening a second
    /// one (§7.2).
    pub async fn open_proposal(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        new: NewProposal,
    ) -> Result<StoredProposal> {
        let NewProposal {
            document_id,
            id,
            author,
            author_peer,
            base_frontiers,
            tip_frontiers,
            branch_bytes,
        } = new;
        sqlx::query(
            "INSERT INTO document_proposals(id,document_id,author,author_peer,base_frontiers,
                                            tip_frontiers,branch_bytes)
             VALUES($1,$2,$3,$4,$5,$6,$7)
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(id)
        .bind(document_id)
        .bind(author)
        .bind(author_peer)
        .bind(base_frontiers)
        .bind(tip_frontiers)
        .bind(branch_bytes)
        .execute(&mut **tx)
        .await?;
        sqlx::query_as::<_, StoredProposal>(&format!("{SELECT} WHERE id=$1 AND document_id=$2"))
            .bind(id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(Error::NotFound)
    }

    /// Moves a proposal's branch to a new tip, conditional on the version the
    /// caller read. The author typing again is what calls this.
    pub async fn update_proposal_branch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        id: Uuid,
        expected_version: i64,
        tip_frontiers: Vec<u8>,
        base_frontiers: Option<Vec<u8>>,
        branch_bytes: Vec<u8>,
    ) -> Result<StoredProposal> {
        sqlx::query(
            "UPDATE document_proposals
             SET tip_frontiers=$3,branch_bytes=$4,version=version+1,updated_at=now(),
                 base_frontiers=COALESCE($6, base_frontiers)
             WHERE id=$1 AND document_id=$2 AND version=$5",
        )
        .bind(id)
        .bind(document_id)
        .bind(tip_frontiers)
        .bind(branch_bytes)
        .bind(expected_version)
        .bind(base_frontiers)
        .execute(&mut **tx)
        .await?;
        // Whether or not the update applied, the caller is handed the row as
        // it now stands and compares it with what it asked for (§7.2).
        sqlx::query_as::<_, StoredProposal>(&format!("{SELECT} WHERE id=$1 AND document_id=$2"))
            .bind(id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(Error::NotFound)
    }

    /// Records one hunk decision and says whether it was the last.
    ///
    /// The tip it was decided against is recorded beside it, because a hunk
    /// index means nothing except against one diff. A decision whose tip is
    /// no longer the proposal's tip is refused here rather than applied to
    /// text that has since changed underneath the reviewer.
    #[allow(clippy::too_many_arguments)]
    pub async fn decide_proposal_hunk(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        proposal_id: Uuid,
        hunk_index: i32,
        accepted: bool,
        decided_by: &str,
        decided_against: &[u8],
        note: Option<&str>,
        total_hunks: i64,
    ) -> Result<bool> {
        if total_hunks <= 0 || hunk_index < 0 || i64::from(hunk_index) >= total_hunks {
            return Err(Error::Invalid(
                "proposal hunk is outside the reviewed diff".into(),
            ));
        }
        let proposal = sqlx::query(
            "SELECT document_id,tip_frontiers FROM document_proposals WHERE id=$1 FOR UPDATE",
        )
        .bind(proposal_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(Error::NotFound)?;
        let proposal_document: Uuid = proposal.get(0);
        let tip: Vec<u8> = proposal.get(1);
        if proposal_document != document_id {
            return Err(Error::Conflict("proposal is no longer open".into()));
        }
        if tip != decided_against {
            return Err(Error::Conflict("proposal tip changed".into()));
        }
        sqlx::query(
            "INSERT INTO document_proposal_hunks(proposal_id,hunk_index,accepted,decided_by,
                                                 decided_against,note)
             VALUES($1,$2,$3,$4,$5,$6)
             ON CONFLICT(proposal_id,hunk_index) DO UPDATE SET accepted=excluded.accepted,
               decided_by=excluded.decided_by,decided_against=excluded.decided_against,
               note=excluded.note,decided_at=now()",
        )
        .bind(proposal_id)
        .bind(hunk_index)
        .bind(accepted)
        .bind(decided_by)
        .bind(decided_against)
        .bind(note)
        .execute(&mut **tx)
        .await?;
        let decided: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_proposal_hunks WHERE proposal_id=$1")
                .bind(proposal_id)
                .fetch_one(&mut **tx)
                .await?;
        if decided > total_hunks {
            return Err(Error::Conflict(
                "proposal decision set is inconsistent".into(),
            ));
        }
        Ok(decided == total_hunks)
    }

    /// Deletes a resolved proposal, in the same transaction as the source its
    /// accepted hunks produced. The hunks and the suggestion comment that
    /// carries the discussion go with it (`ON DELETE CASCADE`); the ids of
    /// those comments come back so the room can tell its clients they are gone.
    pub async fn delete_proposal(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        proposal_id: Uuid,
    ) -> Result<Vec<Uuid>> {
        let comments: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM annotations WHERE proposal_id=$1")
                .bind(proposal_id)
                .fetch_all(&mut **tx)
                .await?;
        sqlx::query("DELETE FROM document_proposals WHERE id=$1")
            .bind(proposal_id)
            .execute(&mut **tx)
            .await?;
        Ok(comments)
    }

    /// The open proposals for a document, which is what a joining client asks
    /// for and what the sequencer broadcasts after every decision.
    pub async fn open_proposals(&self, document_id: Uuid) -> Result<Vec<StoredProposal>> {
        sqlx::query_as::<_, StoredProposal>(&format!(
            "{SELECT} WHERE document_id=$1 ORDER BY created_at"
        ))
        .bind(document_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn proposal(&self, id: Uuid) -> Result<Option<StoredProposal>> {
        sqlx::query_as::<_, StoredProposal>(&format!("{SELECT} WHERE id=$1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    /// What has been decided so far, ordered by hunk index. Decisions stream
    /// as the reviewer works through them.
    pub async fn decisions(&self, proposal_id: Uuid) -> Result<Vec<StoredDecision>> {
        sqlx::query_as::<_, StoredDecision>(
            "SELECT hunk_index,accepted,decided_by,note
             FROM document_proposal_hunks WHERE proposal_id=$1
             ORDER BY hunk_index",
        )
        .bind(proposal_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Every decision on every open proposal of a document, as (proposal id,
    /// hunk index, accepted), so a listing costs one query rather than one per
    /// proposal.
    pub async fn decisions_for_document(
        &self,
        document_id: Uuid,
    ) -> Result<Vec<(Uuid, i32, bool)>> {
        sqlx::query_as::<_, (Uuid, i32, bool)>(
            "SELECT h.proposal_id,h.hunk_index,h.accepted
             FROM document_proposal_hunks h
             JOIN document_proposals p ON p.id=h.proposal_id
             WHERE p.document_id=$1
             ORDER BY h.proposal_id,h.hunk_index",
        )
        .bind(document_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Replaces a proposal's decisions with the ones that survived a tip
    /// change, renumbered against the new diff and stamped with the new tip.
    /// The old rows are numbered against a diff that no longer exists, so none
    /// of them may stay.
    pub async fn replace_decisions(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        proposal_id: Uuid,
        decided_against: &[u8],
        rows: &[(i32, bool, String, Option<String>)],
    ) -> Result<()> {
        sqlx::query("DELETE FROM document_proposal_hunks WHERE proposal_id=$1")
            .bind(proposal_id)
            .execute(&mut **tx)
            .await?;
        for (hunk_index, accepted, decided_by, note) in rows {
            sqlx::query(
                "INSERT INTO document_proposal_hunks(proposal_id,hunk_index,accepted,decided_by,
                                                     decided_against,note)
                 VALUES($1,$2,$3,$4,$5,$6)",
            )
            .bind(proposal_id)
            .bind(hunk_index)
            .bind(accepted)
            .bind(decided_by)
            .bind(decided_against)
            .bind(note)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }
}
