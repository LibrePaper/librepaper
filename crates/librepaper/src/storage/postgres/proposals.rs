use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, Row};
use uuid::Uuid;

use super::{Authority, Error, PostgresCatalog, Result, SemanticReceipt};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProposalMutationResult {
    pub proposal_id: Uuid,
    #[serde(default, skip_serializing)]
    pub replayed: bool,
    pub commit_sequence: String,
    pub source_revision: String,
}

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
    pub async fn open_proposal(
        &self,
        new: NewProposal,
        authority: &Authority,
        receipt: &SemanticReceipt,
    ) -> Result<ProposalMutationResult> {
        let NewProposal {
            document_id,
            id,
            author,
            author_peer,
            base_frontiers,
            tip_frontiers,
            branch_bytes,
        } = new;
        let mut tx = self.begin_document_commit(document_id, authority).await?;
        let digest = receipt.digest()?;
        if let Some(row) = sqlx::query(
            "SELECT command_digest,result FROM document_command_receipts \
             WHERE document_id=$1 AND principal_key=$2 AND request_id=$3",
        )
        .bind(document_id)
        .bind(&authority.principal_key)
        .bind(receipt.request_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let stored: Vec<u8> = row.get(0);
            if stored != digest {
                return Err(Error::Conflict(
                    "request id was already used for different content".into(),
                ));
            }
            let value: Value = row.get(1);
            let mut result: ProposalMutationResult =
                serde_json::from_value(value).map_err(|error| Error::Invalid(error.to_string()))?;
            result.replayed = true;
            return Ok(result);
        }
        let inserted = sqlx::query(
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
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(Error::Conflict("proposal already exists".into()));
        }
        let row = sqlx::query(
            "UPDATE documents SET commit_sequence=commit_sequence+1,updated_at=now() WHERE id=$1 \
             RETURNING commit_sequence,source_revision",
        )
        .bind(document_id)
        .fetch_one(&mut *tx)
        .await?;
        let result = ProposalMutationResult {
            proposal_id: id,
            replayed: false,
            commit_sequence: row.get::<i64, _>(0).to_string(),
            source_revision: row.get::<i64, _>(1).to_string(),
        };
        sqlx::query(
            "INSERT INTO document_command_receipts(document_id,principal_key,request_id,command_digest,status, \
             commit_sequence,source_revision,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(document_id)
        .bind(&authority.principal_key)
        .bind(receipt.request_id)
        .bind(digest.as_slice())
        .bind(&receipt.status)
        .bind(result.commit_sequence.parse::<i64>().unwrap_or_default())
        .bind(result.source_revision.parse::<i64>().unwrap_or_default())
        .bind(serde_json::to_value(&result).map_err(|error| Error::Invalid(error.to_string()))?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Updates a proposal's branch to a new tip.
    ///
    /// Called when the author types again and the proposal's frontier moves.
    /// This bumps updated_at, which signals to reviewing clients that the
    /// proposal has changed.
    pub async fn update_proposal_branch(
        &self,
        document_id: Uuid,
        id: Uuid,
        tip_frontiers: Vec<u8>,
        branch_bytes: Vec<u8>,
        authority: &Authority,
        receipt: &SemanticReceipt,
    ) -> Result<ProposalMutationResult> {
        let mut tx = self.begin_document_commit(document_id, authority).await?;
        let digest = receipt.digest()?;
        if let Some(row) = sqlx::query(
            "SELECT command_digest,result FROM document_command_receipts \
             WHERE document_id=$1 AND principal_key=$2 AND request_id=$3",
        )
        .bind(document_id)
        .bind(&authority.principal_key)
        .bind(receipt.request_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let stored: Vec<u8> = row.get(0);
            if stored != digest {
                return Err(Error::Conflict(
                    "request id was already used for different content".into(),
                ));
            }
            let mut result: ProposalMutationResult = serde_json::from_value(row.get(1))
                .map_err(|error| Error::Invalid(error.to_string()))?;
            result.replayed = true;
            return Ok(result);
        }
        let updated = sqlx::query(
            "UPDATE document_proposals SET tip_frontiers=$2,branch_bytes=$3,updated_at=now()
             WHERE id=$1 AND document_id=$4 AND status='pending'",
        )
        .bind(id)
        .bind(tip_frontiers)
        .bind(branch_bytes)
        .bind(document_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(Error::Conflict(
                "proposal is not pending on this document".into(),
            ));
        }
        let row = sqlx::query(
            "UPDATE documents SET commit_sequence=commit_sequence+1,updated_at=now() WHERE id=$1 \
             RETURNING commit_sequence,source_revision",
        )
        .bind(document_id)
        .fetch_one(&mut *tx)
        .await?;
        let result = ProposalMutationResult {
            proposal_id: id,
            replayed: false,
            commit_sequence: row.get::<i64, _>(0).to_string(),
            source_revision: row.get::<i64, _>(1).to_string(),
        };
        sqlx::query(
            "INSERT INTO document_command_receipts(document_id,principal_key,request_id,command_digest,status, \
             commit_sequence,source_revision,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(document_id)
        .bind(&authority.principal_key)
        .bind(receipt.request_id)
        .bind(digest.as_slice())
        .bind(&receipt.status)
        .bind(result.commit_sequence.parse::<i64>().unwrap_or_default())
        .bind(result.source_revision.parse::<i64>().unwrap_or_default())
        .bind(serde_json::to_value(&result).map_err(|error| Error::Invalid(error.to_string()))?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result)
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
}
