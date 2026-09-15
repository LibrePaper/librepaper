use sqlx::FromRow;
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
    /// needed without conversion. This is what lets the room tell a proposal's
    /// operations from everyone else's without decoding the blob.
    pub author_peer: i64,
    pub base_frontiers: Vec<u8>,
    pub tip_frontiers: Vec<u8>,
    pub branch_bytes: Vec<u8>,
    pub status: String,
}

/// A hunk decision: one row per (proposal_id, hunk_index).
#[derive(Clone, Debug, FromRow)]
pub struct StoredDecision {
    pub hunk_index: i32,
    pub accepted: bool,
    pub decided_by: String,
    pub note: Option<String>,
}

impl PostgresCatalog {
    /// Opens a proposal on a document.
    ///
    /// The proposal records the author, peer id, and the frontier where the
    /// branch forked (base), and starts with an empty branch. The branch is
    /// added as updates arrive and it is reviewed.
    pub async fn open_proposal(&self, new: NewProposal) -> Result<()> {
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
                                             tip_frontiers,branch_bytes,status)
             VALUES($1,$2,$3,$4,$5,$6,$7,'pending')
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(id)
        .bind(document_id)
        .bind(author)
        .bind(author_peer)
        .bind(base_frontiers)
        .bind(tip_frontiers)
        .bind(branch_bytes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Updates a proposal's branch to a new tip.
    ///
    /// Called when the author types again and the proposal's frontier moves.
    /// This bumps updated_at, which signals to reviewing clients that the
    /// proposal has changed.
    pub async fn update_proposal_branch(
        &self,
        id: Uuid,
        tip_frontiers: Vec<u8>,
        branch_bytes: Vec<u8>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE document_proposals SET tip_frontiers=$2,branch_bytes=$3,updated_at=now()
             WHERE id=$1",
        )
        .bind(id)
        .bind(tip_frontiers)
        .bind(branch_bytes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns the open proposals for a document.
    ///
    /// "Open" means status='pending': the proposal has not been decided yet or
    /// has not become unreachable by a reset.
    pub async fn open_proposals(&self, document_id: Uuid) -> Result<Vec<StoredProposal>> {
        sqlx::query_as::<_, StoredProposal>(
            "SELECT id,author,author_peer,base_frontiers,tip_frontiers,branch_bytes,status
             FROM document_proposals WHERE document_id=$1 AND status='pending'
             ORDER BY created_at",
        )
        .bind(document_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Returns a single proposal by id.
    pub async fn proposal(&self, id: Uuid) -> Result<Option<StoredProposal>> {
        sqlx::query_as::<_, StoredProposal>(
            "SELECT id,author,author_peer,base_frontiers,tip_frontiers,branch_bytes,status
             FROM document_proposals WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Records a hunk decision: accept or decline, and why.
    ///
    /// Upserts on (proposal_id, hunk_index): if the reviewer changes their
    /// mind, the new decision overwrites the old. The decision is stored
    /// against the tip the reviewer was looking at, so a stale decision can be
    /// detected: if the proposal's tip has moved since this decision was made,
    /// the decision describes a diff that no longer exists.
    pub async fn decide_hunk(
        &self,
        proposal_id: Uuid,
        hunk_index: i32,
        accepted: bool,
        decided_by: String,
        decided_against: Vec<u8>,
        note: Option<String>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO document_proposal_hunks(proposal_id,hunk_index,accepted,decided_by,
                                                  decided_against,note)
             VALUES($1,$2,$3,$4,$5,$6)
             ON CONFLICT(proposal_id,hunk_index) DO UPDATE SET
               accepted=excluded.accepted,
               decided_by=excluded.decided_by,
               decided_against=excluded.decided_against,
               note=excluded.note,
               decided_at=now()",
        )
        .bind(proposal_id)
        .bind(hunk_index)
        .bind(accepted)
        .bind(decided_by)
        .bind(decided_against)
        .bind(note)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns all decisions for a proposal.
    ///
    /// Decisions stream as the reviewer works through the hunks. This method
    /// returns what has been decided so far, ordered by hunk index.
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

    /// Marks a proposal resolved after all its hunks have been decided.
    ///
    /// Proposals move from 'pending' to 'resolved' when the reviewer has made
    /// their last decision and the accepted hunks have landed in the document.
    pub async fn resolve_proposal(&self, id: Uuid, resolved_by: String) -> Result<()> {
        sqlx::query(
            "UPDATE document_proposals SET status='resolved',resolved_by=$2,resolved_at=now()
             WHERE id=$1",
        )
        .bind(id)
        .bind(resolved_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically stores the resolved proposal and the update that carried its
    /// accepted hunks into the document, in one transaction.
    ///
    /// This is the critical operation for contract 3.4 in SPEC-loro.md: the
    /// intermediate state where the branch has been merged but the declined
    /// hunks not yet reverted must never be persisted as a version of the
    /// document. merge-then-revert is the application layer; this is the
    /// durability layer that guarantees no peer ever sees half of it.
    ///
    /// Returns the update sequence allocated for the update.
    pub async fn resolve_with_update(
        &self,
        id: Uuid,
        resolved_by: String,
        document_id: Uuid,
        update_bytes: &[u8],
    ) -> Result<i64> {
        if update_bytes.is_empty() || update_bytes.len() > 4 * 1024 * 1024 {
            return Err(Error::Invalid("CRDT update size is outside limits".into()));
        }
        let mut tx = self.pool.begin().await?;
        self.lock_collaboration_capacity(&mut tx, document_id, update_bytes.len())
            .await?;

        // The CTE allocates the update sequence and inserts both the update and
        // the resolved proposal status in one atomic operation. Reusing
        // collaboration.rs's pattern ensures we use the same sequencing logic
        // for all document updates.
        let sequence: i64 = sqlx::query_scalar(
            r#"WITH advanced AS (
               UPDATE documents SET update_sequence=update_sequence+1,
                 uncompacted_update_count=uncompacted_update_count+1,
                 uncompacted_update_bytes=uncompacted_update_bytes+octet_length($2::bytea),
                 updated_at=now()
               WHERE id=$1 AND status='active' RETURNING id,update_sequence
             ), inserted AS (
               INSERT INTO document_updates(document_id,update_sequence,update_bytes)
               SELECT id,update_sequence,$2 FROM advanced RETURNING update_sequence
             ), resolved AS (
               UPDATE document_proposals SET status='resolved',resolved_by=$3,resolved_at=now()
               WHERE id=$4 RETURNING id
             )
             SELECT update_sequence FROM inserted"#,
        )
        .bind(document_id)
        .bind(update_bytes)
        .bind(resolved_by)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        tx.commit().await?;
        Ok(sequence)
    }

    /// Marks all pending proposals for a document as superseded.
    ///
    /// Called when the document is reset or reimported: proposals that were
    /// open at the time are no longer reviewable against the new state. They
    /// stay in the database for the record, but clients treat 'superseded'
    /// proposals the same as 'resolved' in the UI: they are not in the
    /// active review flow.
    ///
    /// Returns the number of proposals superseded.
    pub async fn supersede_proposals(&self, document_id: Uuid) -> Result<u64> {
        sqlx::query("UPDATE document_proposals SET status='superseded' WHERE document_id=$1 AND status='pending'")
            .bind(document_id)
            .execute(&self.pool)
            .await
            .map(|result| result.rows_affected())
            .map_err(Error::from)
    }
}
