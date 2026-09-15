use sqlx::FromRow;
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
    pub role: String,
    pub source_link_hash: Option<Vec<u8>>,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, FromRow)]
pub struct ShareLinkRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub role: String,
    pub token_hash: Vec<u8>,
    pub label: String,
    pub comment_budget: Option<i64>,
    pub generation: i64,
    pub created_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

impl PostgresCatalog {
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
        found.map(|_| ()).ok_or(Error::NotFound)
    }

    #[allow(clippy::type_complexity)]
    pub async fn replace_share_links(
        &self,
        document_id: Uuid,
        links: &[(
            String,
            [u8; 32],
            String,
            Option<OffsetDateTime>,
            Option<i64>,
        )],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::lock_active_document(&mut tx, document_id).await?;
        sqlx::query!(
            "UPDATE share_links SET revoked_at=now(),generation=generation+1
             WHERE document_id=$1 AND revoked_at IS NULL",
            document_id,
        )
        .execute(&mut *tx)
        .await?;
        for (role, hash, label, expires, budget) in links {
            sqlx::query!(
                "INSERT INTO share_links(id,document_id,role,token_hash,label,expires_at,comment_budget)
                 VALUES($1,$2,$3,$4,$5,$6,$7)",
                new_id(),
                document_id,
                role,
                hash.as_slice(),
                label,
                *expires,
                *budget,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn prune_link_grants(
        &self,
        document_id: Uuid,
        live_hashes: &[Vec<u8>],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::lock_active_document(&mut tx, document_id).await?;
        sqlx::query!(
            "DELETE FROM grants WHERE document_id=$1 AND source_link_hash IS NOT NULL
             AND NOT (source_link_hash=ANY($2))",
            document_id,
            live_hashes,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn set_grant(
        &self,
        document_id: Uuid,
        account_id: Uuid,
        role: AccessRole,
    ) -> Result<GrantRecord> {
        let role = role.persisted()?;
        let mut tx = self.pool.begin().await?;
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
        role: AccessRole,
        source_link_hash: [u8; 32],
    ) -> Result<()> {
        let role = role.persisted()?;
        let mut tx = self.pool.begin().await?;
        Self::lock_active_document(&mut tx, document_id).await?;
        sqlx::query!(
            "INSERT INTO grants(document_id,account_id,role,source_link_hash) VALUES($1,$2,$3,$4)
             ON CONFLICT(document_id,account_id) DO UPDATE SET
               role=excluded.role,source_link_hash=excluded.source_link_hash",
            document_id,
            account_id,
            role,
            source_link_hash.as_slice(),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_grant(&self, document_id: Uuid, account_id: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        Self::lock_active_document(&mut tx, document_id).await?;
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
        let mut tx = self.pool.begin().await?;
        Self::lock_active_document(&mut tx, document_id).await?;
        let row = sqlx::query_as!(
            ShareLinkRecord,
            "INSERT INTO share_links(id,document_id,role,token_hash,label,expires_at,comment_budget)
             VALUES($1,$2,$3,$4,$5,$6,$7)
             RETURNING id,document_id,role,token_hash,label,comment_budget,generation,
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

    pub async fn share_links(&self, document_id: Uuid) -> Result<Vec<ShareLinkRecord>> {
        sqlx::query_as!(
            ShareLinkRecord,
            "SELECT id,document_id,role,token_hash,label,comment_budget,generation,
                    created_at,expires_at,revoked_at
             FROM share_links WHERE document_id=$1 AND revoked_at IS NULL ORDER BY created_at,id",
            document_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn revoke_share_link(&self, document_id: Uuid, id: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        Self::lock_active_document(&mut tx, document_id).await?;
        let changed = sqlx::query!(
            "UPDATE share_links SET revoked_at=now(),generation=generation+1
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
        let row = sqlx::query!(
            r#"SELECT d.owner_id,
                (SELECT role FROM grants g WHERE g.document_id=d.id AND g.account_id=$2
                   AND (g.source_link_hash IS NULL OR EXISTS (
                     SELECT 1 FROM share_links gl WHERE gl.document_id=d.id
                       AND gl.token_hash=g.source_link_hash AND gl.revoked_at IS NULL
                       AND (gl.expires_at IS NULL OR gl.expires_at>$4)))) AS "grant_role?",
                (SELECT role FROM share_links l WHERE l.document_id=d.id AND l.token_hash=$3
                    AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>$4)) AS "link_role?"
             FROM documents d WHERE d.id=$1 AND d.status='active'"#,
            document_id,
            account_id,
            token_hash.map(|v| v.to_vec()),
            now,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if account_id == Some(row.owner_id) {
            return Ok(Some(AccessRole::Owner));
        }
        Ok(highest(row.grant_role.as_deref(), row.link_role.as_deref()))
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
