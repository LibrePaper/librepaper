use sqlx::{FromRow, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessRole {
    Reader,
    Commenter,
    Editor,
    Owner,
}

impl AccessRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Commenter => "commenter",
            Self::Editor => "editor",
            Self::Owner => "owner",
        }
    }
    fn persisted(self) -> Result<&'static str> {
        match self {
            Self::Owner => Err(Error::Invalid("ownership is not a grant".into())),
            other => Ok(other.as_str()),
        }
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct GrantRecord {
    pub document_id: Uuid,
    pub account_id: Uuid,
    pub role: Option<String>,
    pub source_link_hash: Option<Vec<u8>>,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, FromRow)]
pub struct ShareLinkRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub role: String,
    pub token_hash: Vec<u8>,
    /// The link's own key, sealed by the server that minted it. Empty for a
    /// link from before the column existed, and meaningless to anybody who
    /// does not hold the deployment's session key.
    pub sealed_token: Vec<u8>,
    pub label: String,
    pub comment_budget: Option<i64>,
    pub created_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

impl PostgresCatalog {
    async fn begin_document_admin(
        &self,
        document_id: Uuid,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>> {
        let mut tx = self.begin_writer_transaction().await?;
        Self::lock_active_document(&mut tx, document_id).await?;
        Ok(tx)
    }

    async fn lock_active_document(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
    ) -> Result<()> {
        let found = sqlx::query_scalar!(
            "SELECT id FROM documents WHERE id=$1 AND status='active' FOR UPDATE",
            document_id,
        )
        .fetch_optional(&mut **tx)
        .await?;
        if found.is_none() {
            return Err(Error::NotFound);
        }
        let hidden = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM moderated_projects WHERE document_id=$1)",
        )
        .bind(document_id)
        .fetch_one(&mut **tx)
        .await?;
        if hidden {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    /// Makes this document's live links exactly `links`.
    ///
    /// A link *is* its token, so a link whose token is in the wanted set is
    /// the same link and keeps its row: its id and the day it was created.
    /// Only what the owner actually changed is written.
    ///
    /// It used to revoke every live row and write the whole set back, which
    /// was wrong twice over. A revoked row is a tombstone that stays on
    /// record, and `token_hash` is unique across the deployment, so writing an
    /// unchanged link back collided with the row that had just been revoked
    /// for it -- every save of a document that already had one link failed,
    /// which is what sharing with a second role does. And a link that did
    /// survive came back looking newly made.
    #[allow(clippy::type_complexity)]
    pub async fn replace_share_links(
        &self,
        document_id: Uuid,
        links: &[(
            String,
            [u8; 32],
            Vec<u8>,
            String,
            Option<OffsetDateTime>,
            Option<i64>,
        )],
    ) -> Result<()> {
        let mut tx = self.begin_document_admin(document_id).await?;
        Self::replace_share_links_tx(&mut tx, document_id, links).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replace links inside a caller-owned transaction. This takes and
    /// verifies the active document row lock in that transaction, so link
    /// edits stay atomic with the caller's other document mutations.
    #[allow(clippy::type_complexity)]
    pub async fn replace_share_links_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        links: &[(
            String,
            [u8; 32],
            Vec<u8>,
            String,
            Option<OffsetDateTime>,
            Option<i64>,
        )],
    ) -> Result<()> {
        let found = sqlx::query_scalar!(
            "SELECT id FROM documents WHERE id=$1 AND status='active' FOR UPDATE",
            document_id,
        )
        .fetch_optional(&mut **tx)
        .await?;
        if found.is_none() {
            return Err(Error::NotFound);
        }
        let wanted: Vec<Vec<u8>> = links.iter().map(|(_, hash, ..)| hash.to_vec()).collect();
        // Whatever this save is not keeping. A revoked row is kept rather than
        // deleted so a guest admitted through it is still recognisable as
        // having come in that way.
        sqlx::query!(
            "UPDATE share_links SET revoked_at=now()
             WHERE document_id=$1 AND revoked_at IS NULL AND NOT (token_hash=ANY($2))",
            document_id,
            &wanted,
        )
        .execute(&mut **tx)
        .await?;
        for (role, hash, sealed, label, expires, budget) in links {
            // Scoped to this document, so a token that somehow belongs to
            // another one is not quietly moved: nothing is updated, and the
            // insert below refuses it by the unique constraint.
            //
            // The sealed key is the one field a save can only ever restate:
            // the same token is the same key, and the entry being written back
            // may have come from a server that could not read it. An empty one
            // is therefore "unchanged", never "forget it" -- losing it would
            // cost the owner the URL of a link that is working.
            let kept = sqlx::query!(
                "UPDATE share_links SET role=$3,label=$5,expires_at=$6,comment_budget=$7,
                        sealed_token=COALESCE(NULLIF($4,''::bytea),sealed_token),
                        revoked_at=NULL
                 WHERE document_id=$1 AND token_hash=$2",
                document_id,
                hash.as_slice(),
                role,
                sealed.as_slice(),
                label,
                *expires,
                *budget,
            )
            .execute(&mut **tx)
            .await?
            .rows_affected();
            if kept == 0 {
                sqlx::query!(
                    "INSERT INTO share_links(id,document_id,role,token_hash,sealed_token,label,expires_at,comment_budget)
                     VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
                    new_id(),
                    document_id,
                    role,
                    hash.as_slice(),
                    sealed.as_slice(),
                    label,
                    *expires,
                    *budget,
                )
                .execute(&mut **tx)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn prune_link_grants(
        &self,
        document_id: Uuid,
        live_hashes: &[Vec<u8>],
    ) -> Result<()> {
        let mut tx = self.begin_document_admin(document_id).await?;
        Self::prune_link_grants_tx(&mut tx, document_id, live_hashes).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn prune_link_grants_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        live_hashes: &[Vec<u8>],
    ) -> Result<()> {
        sqlx::query!(
            "DELETE FROM grants WHERE document_id=$1 AND source_link_hash IS NOT NULL
             AND NOT (source_link_hash=ANY($2))",
            document_id,
            live_hashes,
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
    pub async fn set_grant(
        &self,
        document_id: Uuid,
        account_id: Uuid,
        role: AccessRole,
    ) -> Result<GrantRecord> {
        let role = role.persisted()?;
        let mut tx = self.begin_writer_transaction().await?;
        let target = sqlx::query_scalar!(
            "SELECT id FROM accounts WHERE id=$1 AND status='active' FOR SHARE",
            account_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if target.is_none() {
            return Err(Error::NotFound);
        }
        Self::lock_active_document(&mut tx, document_id).await?;
        let row = sqlx::query_as!(
            GrantRecord,
            "INSERT INTO grants(document_id,account_id,role,source_link_hash) VALUES($1,$2,$3,NULL)
             ON CONFLICT(document_id,account_id) DO UPDATE SET role=excluded.role,source_link_hash=NULL
             RETURNING document_id,account_id,role,source_link_hash,created_at",
            document_id,
            account_id,
            role,
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn pin_link_guest(
        &self,
        document_id: Uuid,
        account_id: Uuid,
        _role: AccessRole,
        source_link_hash: [u8; 32],
    ) -> Result<()> {
        let mut tx = self.begin_writer_transaction().await?;
        let target = sqlx::query_scalar!(
            "SELECT id FROM accounts WHERE id=$1 AND status='active' FOR SHARE",
            account_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if target.is_none() {
            return Err(Error::NotFound);
        }
        Self::lock_active_document(&mut tx, document_id).await?;
        sqlx::query!(
            "INSERT INTO grants(document_id,account_id,role,source_link_hash)
             SELECT $1,$2,NULL,$3 FROM share_links l
             WHERE l.document_id=$1 AND l.token_hash=$3 AND l.revoked_at IS NULL
               AND (l.expires_at IS NULL OR l.expires_at>now())
             ON CONFLICT(document_id,account_id) DO UPDATE SET
               role=NULL,source_link_hash=excluded.source_link_hash
             WHERE grants.source_link_hash IS NOT NULL",
            document_id,
            account_id,
            source_link_hash.as_slice(),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_grant(&self, document_id: Uuid, account_id: Uuid) -> Result<bool> {
        let mut tx = self.begin_document_admin(document_id).await?;
        let changed = sqlx::query!(
            "DELETE FROM grants WHERE document_id=$1 AND account_id=$2",
            document_id,
            account_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }

    /// Every grant on each of `document_ids`, in one round trip. The rows carry
    /// their own `document_id`, so the caller groups them without a query per
    /// document. Ordered so that grouping preserves the per-document order
    /// [`grants`] returns.
    pub async fn grants_for_documents(&self, document_ids: &[Uuid]) -> Result<Vec<GrantRecord>> {
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as!(
            GrantRecord,
            "SELECT document_id,account_id,role,source_link_hash,created_at
             FROM grants WHERE document_id=ANY($1) ORDER BY document_id,created_at,account_id",
            document_ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn grants(&self, document_id: Uuid) -> Result<Vec<GrantRecord>> {
        sqlx::query_as!(
            GrantRecord,
            "SELECT document_id,account_id,role,source_link_hash,created_at
             FROM grants WHERE document_id=$1 ORDER BY created_at,account_id",
            document_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn create_share_link(
        &self,
        document_id: Uuid,
        role: AccessRole,
        token_hash: [u8; 32],
        label: String,
        expires_at: Option<OffsetDateTime>,
        comment_budget: Option<i64>,
    ) -> Result<ShareLinkRecord> {
        let role = role.persisted()?;
        if label.len() > 80 {
            return Err(Error::Invalid("share link label is too long".into()));
        }
        let mut tx = self.begin_document_admin(document_id).await?;
        let row = sqlx::query_as!(
            ShareLinkRecord,
            "INSERT INTO share_links(id,document_id,role,token_hash,label,expires_at,comment_budget)
             VALUES($1,$2,$3,$4,$5,$6,$7)
             RETURNING id,document_id,role,token_hash,sealed_token,label,comment_budget,
                       created_at,expires_at,revoked_at",
            new_id(),
            document_id,
            role,
            token_hash.as_slice(),
            label,
            expires_at,
            comment_budget,
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// The live share links on each of `document_ids`, in one round trip.
    /// Grouped by the caller the same way [`grants_for_documents`] is.
    pub async fn share_links_for_documents(
        &self,
        document_ids: &[Uuid],
    ) -> Result<Vec<ShareLinkRecord>> {
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as!(
            ShareLinkRecord,
            "SELECT id,document_id,role,token_hash,sealed_token,label,comment_budget,
                    created_at,expires_at,revoked_at
             FROM share_links WHERE document_id=ANY($1) AND revoked_at IS NULL
             ORDER BY document_id,created_at,id",
            document_ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn share_links(&self, document_id: Uuid) -> Result<Vec<ShareLinkRecord>> {
        sqlx::query_as!(
            ShareLinkRecord,
            "SELECT id,document_id,role,token_hash,sealed_token,label,comment_budget,
                    created_at,expires_at,revoked_at
             FROM share_links WHERE document_id=$1 AND revoked_at IS NULL ORDER BY created_at,id",
            document_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn revoke_share_link(&self, document_id: Uuid, id: Uuid) -> Result<bool> {
        let mut tx = self.begin_document_admin(document_id).await?;
        let changed = sqlx::query!(
            "UPDATE share_links SET revoked_at=now()
             WHERE document_id=$1 AND id=$2 AND revoked_at IS NULL",
            document_id,
            id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn access_role(
        &self,
        document_id: Uuid,
        account_id: Option<Uuid>,
        token_hash: Option<[u8; 32]>,
        now: OffsetDateTime,
    ) -> Result<Option<AccessRole>> {
        let row = sqlx::query(
            r#"SELECT d.owner_id,
                (SELECT CASE WHEN g.source_link_hash IS NULL THEN g.role ELSE gl.role END
                   FROM grants g LEFT JOIN share_links gl
                     ON gl.document_id=g.document_id AND gl.token_hash=g.source_link_hash
                    AND gl.revoked_at IS NULL AND (gl.expires_at IS NULL OR gl.expires_at>$4)
                   WHERE g.document_id=d.id AND g.account_id=$2
                     AND (g.source_link_hash IS NULL OR gl.token_hash IS NOT NULL)) AS grant_role,
                (SELECT role FROM share_links l WHERE l.document_id=d.id AND l.token_hash=$3
                    AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>$4)) AS link_role
             FROM documents d WHERE d.id=$1 AND d.status='active'
               AND NOT EXISTS (SELECT 1 FROM moderated_projects m WHERE m.document_id=d.id)"#,
        )
        .bind(document_id)
        .bind(account_id)
        .bind(token_hash.map(|v| v.to_vec()))
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let owner_id: Uuid = row.try_get("owner_id")?;
        let grant_role: Option<String> = row.try_get("grant_role")?;
        let link_role: Option<String> = row.try_get("link_role")?;
        if account_id == Some(owner_id) {
            return Ok(Some(AccessRole::Owner));
        }
        Ok(highest(grant_role.as_deref(), link_role.as_deref()))
    }
}

fn highest(left: Option<&str>, right: Option<&str>) -> Option<AccessRole> {
    [left, right]
        .into_iter()
        .flatten()
        .filter_map(|role| match role {
            "reader" => Some(AccessRole::Reader),
            "commenter" => Some(AccessRole::Commenter),
            "editor" => Some(AccessRole::Editor),
            _ => None,
        })
        .max_by_key(|role| match role {
            AccessRole::Reader => 1,
            AccessRole::Commenter => 2,
            AccessRole::Editor => 3,
            AccessRole::Owner => 4,
        })
}
