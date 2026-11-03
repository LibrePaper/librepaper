//! Who wrote each Loro peer of a document.
//!
//! A browser session gets a random PeerID, so the peer alone says nothing
//! about who typed a character. Each write records which account, or which
//! agent token, owns the peers it carried. The first writer wins: once a peer
//! is attributed it is never reassigned, even if a later write claims it.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{PostgresCatalog, Result};

/// One peer of a document and the name it should display as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerAuthor {
    pub peer: u64,
    pub account_id: Option<Uuid>,
    /// The label of the agent token or share link that wrote the peer, a
    /// registered account's current display name, the label stored when the
    /// peer was first seen, or empty, in that order. An anonymous visitor's
    /// stored label is their pseudonym in this document, which is what the
    /// rest of the document calls them.
    pub name: String,
    /// True when the peer was written by an agent: an agent token, or a share
    /// link an automation client used.
    pub agent: bool,
    pub first_seen: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct PeerRow {
    peer: i64,
    author_account_id: Option<Uuid>,
    first_seen: OffsetDateTime,
    name: String,
    agent: bool,
}

/// Inserts every peer in one statement. A conflict keeps the row already
/// there, which is what makes the first writer win.
async fn insert_document_peers(
    executor: impl sqlx::PgExecutor<'_>,
    document_id: Uuid,
    peers: &[u64],
    account_id: Option<Uuid>,
    link_hash: Option<&[u8]>,
    label: &str,
) -> Result<()> {
    if peers.is_empty() {
        return Ok(());
    }
    // Loro's PeerID is a u64, and bigint is signed. The cast keeps the bits,
    // and the read casts them back.
    let peers: Vec<i64> = peers.iter().map(|&peer| peer as i64).collect();
    sqlx::query(
        "INSERT INTO document_peers(document_id,peer,author_account_id,link_hash,author_label)
         SELECT $1::uuid,p,$3::uuid,$4::bytea,$5::text FROM UNNEST($2::bigint[]) AS p
         ON CONFLICT(document_id,peer) DO NOTHING",
    )
    .bind(document_id)
    .bind(peers)
    .bind(account_id)
    .bind(link_hash)
    .bind(label)
    .execute(executor)
    .await?;
    Ok(())
}

impl PostgresCatalog {
    /// Records who a session's peers belong to. First writer wins (ON CONFLICT (document_id, peer) DO NOTHING).
    pub async fn record_document_peers(
        &self,
        document_id: Uuid,
        peers: &[u64],
        account_id: Option<Uuid>,
        label: &str,
    ) -> Result<()> {
        insert_document_peers(&self.pool, document_id, peers, account_id, None, label).await
    }

    /// Same, inside a caller's transaction (used by the command path). The
    /// command path writes the peers in the same transaction as the source
    /// row, so an attribution exists exactly when the source it describes does.
    pub(crate) async fn record_document_peers_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        peers: &[u64],
        account_id: Option<Uuid>,
        link_hash: Option<&[u8]>,
        label: &str,
    ) -> Result<()> {
        insert_document_peers(&mut **tx, document_id, peers, account_id, link_hash, label).await
    }

    /// Every recorded peer of a document with a display name resolved now.
    /// Names are resolved at read time, so a renamed account or a relabelled
    /// agent token shows its current name on every peer it wrote.
    pub async fn document_peer_authors(&self, document_id: Uuid) -> Result<Vec<PeerAuthor>> {
        let rows: Vec<PeerRow> = sqlx::query_as(
            "SELECT p.peer, p.author_account_id, p.first_seen,
                    COALESCE(NULLIF(sl.label,''),
                             CASE WHEN a.kind = 'registered' THEN a.display_name END,
                             NULLIF(p.author_label,''), a.display_name, '') AS name,
                    (p.link_hash IS NOT NULL) AS agent
             FROM document_peers p
             LEFT JOIN accounts a ON a.id = p.author_account_id
             LEFT JOIN share_links sl ON sl.token_hash = p.link_hash
             WHERE p.document_id = $1 ORDER BY p.first_seen",
        )
        .bind(document_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| PeerAuthor {
                peer: row.peer as u64,
                account_id: row.author_account_id,
                name: row.name,
                agent: row.agent,
                first_seen: row.first_seen,
            })
            .collect())
    }
}

#[cfg(test)]
#[path = "peers_regression_tests.rs"]
mod peers_regression_tests;
