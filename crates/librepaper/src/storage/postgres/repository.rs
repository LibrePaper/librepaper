use serde_json::Value;
use sqlx::Row;
use time::OffsetDateTime;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

#[derive(Clone, Debug)]
pub struct NewAccount {
    pub kind: String,
    pub provider: Option<String>,
    pub provider_subject: Option<String>,
    pub handle: String,
    pub display_name: String,
    pub email: Option<String>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AccountRecord {
    pub id: Uuid,
    pub kind: String,
    pub provider: Option<String>,
    pub provider_subject: Option<String>,
    pub handle: String,
    pub display_name: String,
    pub email: Option<String>,
    pub status: String,
    pub session_generation: i64,
    pub preferences: Value,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct NewDocument {
    pub slug: String,
    pub owner_id: Uuid,
    pub ownership_mode: String,
    pub title: String,
    pub source_format: String,
    pub main_path: String,
    pub settings: Value,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct DocumentRecord {
    pub id: Uuid,
    pub slug: String,
    pub owner_id: Uuid,
    pub ownership_mode: String,
    pub title: String,
    pub title_key: String,
    pub status: String,
    pub source_format: String,
    pub main_path: String,
    pub update_sequence: i64,
    pub project_generation: i64,
    pub current_version_id: Option<Uuid>,
    pub current_publication_id: Option<Uuid>,
    pub settings: Value,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug)]
pub struct NewAsset {
    pub document_id: Uuid,
    pub storage_key: String,
    pub digest: [u8; 32],
    pub byte_length: i64,
    pub media_type: String,
    pub original_name: Option<String>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AssetRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub storage_key: String,
    pub digest: Vec<u8>,
    pub byte_length: i64,
    pub media_type: String,
    pub original_name: Option<String>,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct NewVersion {
    pub document_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub through_update_sequence: i64,
    pub project_generation: i64,
    pub archive_key: String,
    pub archive_encoding_version: i16,
    pub archive_digest: [u8; 32],
    pub archive_bytes: i64,
    pub logical_bytes: i64,
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub make_current: bool,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct VersionRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub sequence: i64,
    pub parent_id: Option<Uuid>,
    pub through_update_sequence: i64,
    pub project_generation: i64,
    pub archive_key: String,
    pub archive_encoding_version: i16,
    pub archive_digest: Vec<u8>,
    pub archive_bytes: i64,
    pub logical_bytes: i64,
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub created_at: OffsetDateTime,
}

impl PostgresCatalog {
    pub async fn runtime_state(&self, name: &str) -> Result<Option<serde_json::Value>> {
        sqlx::query_scalar("SELECT value FROM server_runtime_state WHERE name=$1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    pub async fn set_runtime_state(&self, name: &str, value: serde_json::Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO server_runtime_state(name,value) VALUES($1,$2)
             ON CONFLICT(name) DO UPDATE SET value=excluded.value,updated_at=now()",
        )
        .bind(name)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn begin_account_erasure(
        &self,
        account_id: Uuid,
        recovery_grace: time::Duration,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM accounts WHERE id=$1 FOR UPDATE")
                .bind(account_id)
                .fetch_optional(&mut *tx)
                .await?;
        match status.as_deref() {
            None => return Err(Error::NotFound),
            Some("active") => {}
            Some("erasing") => {
                tx.commit().await?;
                return Ok(false);
            }
            Some(_) => return Err(Error::Conflict("account is blocked".into())),
        }
        sqlx::query(
            "UPDATE accounts SET status='erasing',session_generation=session_generation+1 WHERE id=$1",
        )
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
        let document_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM documents WHERE owner_id=$1 ORDER BY id FOR UPDATE")
                .bind(account_id)
                .fetch_all(&mut *tx)
                .await?;
        sqlx::query(
            "UPDATE documents SET status='deleting',deleted_at=COALESCE(deleted_at,now()),updated_at=now()
             WHERE owner_id=$1 AND status='active'",
        )
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
        let delete_after = OffsetDateTime::now_utc() + recovery_grace;
        for document_id in document_ids {
            sqlx::query(
                "INSERT INTO jobs(id,kind,document_id,account_id,scope_key,dedupe_key,payload,status,priority,max_attempts,run_after)
                 VALUES($1,'document_deletion',$2,$3,$4,'delete',$5,'queued',0,100,$6)
                 ON CONFLICT (kind,scope_key,dedupe_key)
                 WHERE dedupe_key IS NOT NULL AND status IN ('queued','running') DO NOTHING",
            )
            .bind(new_id())
            .bind(document_id)
            .bind(account_id)
            .bind(format!("document:{document_id}"))
            .bind(serde_json::json!({"document_id":document_id}))
            .bind(delete_after)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO jobs(id,kind,account_id,scope_key,dedupe_key,payload,status,priority,max_attempts,run_after)
             VALUES($1,'account_deletion',$2,$3,'delete',$4,'queued',0,100,$5)
             ON CONFLICT (kind,scope_key,dedupe_key)
             WHERE dedupe_key IS NOT NULL AND status IN ('queued','running') DO NOTHING",
        )
        .bind(new_id())
        .bind(account_id)
        .bind(format!("account:{account_id}"))
        .bind(serde_json::json!({"account_id":account_id}))
        .bind(delete_after + time::Duration::minutes(5))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn finish_account_erasure(&self, account_id: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE owner_id=$1")
            .bind(account_id)
            .fetch_one(&mut *tx)
            .await?;
        if remaining != 0 {
            return Err(Error::Conflict(
                "account still has documents awaiting deletion".into(),
            ));
        }
        sqlx::query("DELETE FROM grants WHERE account_id=$1")
            .bind(account_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE annotations SET author_account_id=NULL,author_key='erased',author_label='Deleted user' WHERE author_account_id=$1")
            .bind(account_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE replies SET author_account_id=NULL,author_key='erased',author_label='Deleted user' WHERE author_account_id=$1")
            .bind(account_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE document_versions SET author_account_id=NULL,author_label='Deleted user' WHERE author_account_id=$1")
            .bind(account_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE publications SET publisher_account_id=NULL,publisher_label='Deleted user' WHERE publisher_account_id=$1")
            .bind(account_id).execute(&mut *tx).await?;
        let deleted = sqlx::query("DELETE FROM accounts WHERE id=$1 AND status='erasing'")
            .bind(account_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(deleted)
    }

    pub async fn check_storage_admission(
        &self,
        document_id: Uuid,
        incoming_bytes: i64,
    ) -> Result<()> {
        if incoming_bytes < 0 {
            return Err(Error::Invalid("invalid incoming byte count".into()));
        }
        let owner_id: Uuid =
            sqlx::query_scalar("SELECT owner_id FROM documents WHERE id=$1 AND status='active'")
                .bind(document_id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(Error::NotFound)?;
        let owner_usage = self.usage_bytes(Some(owner_id)).await?;
        let deployment_usage = self.usage_bytes(None).await?;
        if owner_usage.saturating_add(incoming_bytes) > self.policy.owner_bytes {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        if deployment_usage.saturating_add(incoming_bytes) > self.policy.deployment_bytes {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
        }
        Ok(())
    }

    pub async fn update_document_identity(
        &self,
        id: Uuid,
        title: &str,
        owner_id: Uuid,
        ownership_mode: &str,
    ) -> Result<bool> {
        if title.is_empty() || !matches!(ownership_mode, "owned" | "open" | "example") {
            return Err(Error::Invalid("invalid document metadata".into()));
        }
        Ok(sqlx::query("UPDATE documents SET title=$2,title_key=$3,owner_id=$4,ownership_mode=$5,updated_at=now() WHERE id=$1 AND status='active'")
            .bind(id).bind(title).bind(title_key(title)).bind(owner_id).bind(ownership_mode)
            .execute(&self.pool).await?.rows_affected()==1)
    }

    pub async fn create_account(&self, input: NewAccount) -> Result<AccountRecord> {
        validate_account(&input)?;
        let id = new_id();
        sqlx::query_as::<_, AccountRecord>(
            "INSERT INTO accounts
             (id,kind,provider,provider_subject,handle,display_name,email,status)
             VALUES($1,$2,$3,$4,$5,$6,$7,'active') RETURNING *",
        )
        .bind(id)
        .bind(input.kind)
        .bind(input.provider)
        .bind(input.provider_subject)
        .bind(input.handle)
        .bind(input.display_name)
        .bind(input.email)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn upsert_registered_account(&self, input: NewAccount) -> Result<AccountRecord> {
        validate_account(&input)?;
        if input.kind != "registered" {
            return Err(Error::Invalid(
                "only registered accounts can be upserted".into(),
            ));
        }
        sqlx::query_as::<_,AccountRecord>(
            "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status)
             VALUES($1,'registered',$2,$3,$4,$5,$6,'active')
             ON CONFLICT(provider,provider_subject) WHERE kind='registered' DO UPDATE SET
               handle=excluded.handle,display_name=excluded.display_name,email=excluded.email,last_seen_at=now()
             WHERE accounts.status='active' RETURNING accounts.*",
        ).bind(new_id()).bind(input.provider).bind(input.provider_subject).bind(input.handle)
         .bind(input.display_name).bind(input.email).fetch_optional(&self.pool).await?
         .ok_or_else(||Error::Conflict("account is not active".into()))
    }

    pub async fn account(&self, id: Uuid) -> Result<Option<AccountRecord>> {
        sqlx::query_as::<_, AccountRecord>("SELECT * FROM accounts WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    pub async fn create_document(&self, input: NewDocument) -> Result<DocumentRecord> {
        validate_document(&input)?;
        let id = new_id();
        let title_key = title_key(&input.title);
        sqlx::query_as::<_, DocumentRecord>(
            "INSERT INTO documents
             (id,slug,owner_id,ownership_mode,title,title_key,status,source_format,main_path,settings)
             VALUES($1,$2,$3,$4,$5,$6,'active',$7,$8,$9) RETURNING *",
        )
        .bind(id)
        .bind(input.slug)
        .bind(input.owner_id)
        .bind(input.ownership_mode)
        .bind(input.title)
        .bind(title_key)
        .bind(input.source_format)
        .bind(input.main_path)
        .bind(input.settings)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn document_by_slug(&self, slug: &str) -> Result<Option<DocumentRecord>> {
        sqlx::query_as::<_, DocumentRecord>("SELECT * FROM documents WHERE slug=$1")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    pub async fn document(&self, id: Uuid) -> Result<Option<DocumentRecord>> {
        sqlx::query_as::<_, DocumentRecord>("SELECT * FROM documents WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    pub async fn list_documents(
        &self,
        before: Option<(OffsetDateTime, Uuid)>,
        limit: i64,
    ) -> Result<Vec<DocumentRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("document page limit must be 1..=200".into()));
        }
        sqlx::query_as::<_, DocumentRecord>(
            "SELECT * FROM documents
             WHERE status='active'
               AND ($1::timestamptz IS NULL OR (updated_at,id) < ($1,$2))
             ORDER BY updated_at DESC,id DESC LIMIT $3",
        )
        .bind(before.map(|value| value.0))
        .bind(before.map(|value| value.1))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn documents_by_owner(
        &self,
        owner_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DocumentRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("document page limit must be 1..=200".into()));
        }
        sqlx::query_as::<_,DocumentRecord>("SELECT * FROM documents WHERE owner_id=$1 AND status='active' ORDER BY updated_at DESC,id DESC LIMIT $2")
            .bind(owner_id).bind(limit).fetch_all(&self.pool).await.map_err(Error::from)
    }

    pub async fn documents_by_owner_page(
        &self,
        owner_id: Uuid,
        before: Option<(OffsetDateTime, Uuid)>,
        limit: i64,
    ) -> Result<Vec<DocumentRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("document page limit must be 1..=200".into()));
        }
        sqlx::query_as::<_, DocumentRecord>(
            "SELECT * FROM documents WHERE owner_id=$1 AND status='active'
             AND ($2::timestamptz IS NULL OR (updated_at,id)<($2,$3))
             ORDER BY updated_at DESC,id DESC LIMIT $4",
        )
        .bind(owner_id)
        .bind(before.map(|value| value.0))
        .bind(before.map(|value| value.1))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn visible_documents(
        &self,
        account_id: Option<Uuid>,
        before: Option<(OffsetDateTime, Uuid)>,
        limit: i64,
        include_examples: bool,
    ) -> Result<Vec<DocumentRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("document page limit must be 1..=200".into()));
        }
        sqlx::query_as::<_, DocumentRecord>(
            "SELECT d.* FROM documents d
             WHERE d.status='active'
               AND (d.owner_id=$1 OR EXISTS (
                    SELECT 1 FROM grants g WHERE g.document_id=d.id AND g.account_id=$1
                      AND (g.source_link_hash IS NULL OR EXISTS (
                        SELECT 1 FROM share_links l WHERE l.document_id=d.id
                          AND l.token_hash=g.source_link_hash AND l.revoked_at IS NULL
                          AND (l.expires_at IS NULL OR l.expires_at>now())
                      ))
               ) OR ($4 AND d.ownership_mode='example'))
               AND ($2::timestamptz IS NULL OR (d.updated_at,d.id) < ($2,$3))
             ORDER BY d.updated_at DESC,d.id DESC LIMIT $5",
        )
        .bind(account_id)
        .bind(before.map(|value| value.0))
        .bind(before.map(|value| value.1))
        .bind(include_examples)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn document_counts_by_owner(&self, owner_id: Uuid) -> Result<(i64, i64)> {
        sqlx::query_as(
            "SELECT count(DISTINCT d.id)::bigint,
                    count(v.id)::bigint
             FROM documents d LEFT JOIN document_versions v ON v.document_id=d.id
             WHERE d.owner_id=$1 AND d.status='active'",
        )
        .bind(owner_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn complete_asset(&self, input: NewAsset) -> Result<(AssetRecord, bool)> {
        self.complete_asset_with_limit(input, i64::MAX).await
    }

    pub async fn complete_asset_with_limit(
        &self,
        input: NewAsset,
        retained_byte_limit: i64,
    ) -> Result<(AssetRecord, bool)> {
        self.complete_assets_with_limit(vec![input], retained_byte_limit)
            .await?
            .pop()
            .ok_or_else(|| Error::Invalid("asset batch was empty".into()))
    }

    pub async fn complete_assets_with_limit(
        &self,
        inputs: Vec<NewAsset>,
        retained_byte_limit: i64,
    ) -> Result<Vec<(AssetRecord, bool)>> {
        if inputs.is_empty() || inputs.len() > 4096 {
            return Err(Error::Invalid("invalid completed asset batch".into()));
        }
        let document_id = inputs[0].document_id;
        if inputs.iter().any(|input| {
            input.document_id != document_id
                || input.byte_length < 0
                || input.media_type.is_empty()
                || input.storage_key.is_empty()
        }) {
            return Err(Error::Invalid("invalid completed asset".into()));
        }
        let unique: std::collections::HashSet<_> = inputs
            .iter()
            .map(|input| (input.digest, input.byte_length))
            .collect();
        if unique.len() != inputs.len() {
            return Err(Error::Invalid("duplicate asset in completion batch".into()));
        }
        if retained_byte_limit < 0 {
            return Err(Error::Invalid("invalid retained asset limit".into()));
        }
        let mut tx = self.pool.begin().await?;
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?;
        match status.as_deref() {
            None => return Err(Error::NotFound),
            Some("active") => {}
            Some(_) => return Err(Error::Conflict("document is being deleted".into())),
        }
        let digests: Vec<Vec<u8>> = inputs.iter().map(|input| input.digest.to_vec()).collect();
        let existing = sqlx::query_as::<_, AssetRecord>(
            "SELECT * FROM document_assets WHERE document_id=$1 AND digest=ANY($2)",
        )
        .bind(document_id)
        .bind(&digests)
        .fetch_all(&mut *tx)
        .await?;
        let existing_keys: std::collections::HashSet<_> = existing
            .iter()
            .map(|asset| (asset.digest.clone(), asset.byte_length))
            .collect();
        let new_inputs: Vec<&NewAsset> = inputs
            .iter()
            .filter(|input| !existing_keys.contains(&(input.digest.to_vec(), input.byte_length)))
            .collect();
        if new_inputs.is_empty() {
            tx.commit().await?;
            return Ok(inputs
                .into_iter()
                .map(|input| {
                    let row = existing
                        .iter()
                        .find(|asset| {
                            asset.digest == input.digest && asset.byte_length == input.byte_length
                        })
                        .expect("existing asset was selected")
                        .clone();
                    (row, false)
                })
                .collect());
        }
        let uploads:i64=sqlx::query_scalar("SELECT count(*) FROM document_assets a JOIN documents d ON d.id=a.document_id WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1) AND a.created_at>=now()-interval '1 hour'")
            .bind(document_id).fetch_one(&mut *tx).await?;
        if uploads.saturating_add(new_inputs.len() as i64) > self.policy.asset_uploads_per_hour {
            return Err(Error::Conflict("account asset upload rate exceeded".into()));
        }
        let retained: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(byte_length),0)::bigint FROM document_assets WHERE document_id=$1",
        )
        .bind(document_id)
        .fetch_one(&mut *tx)
        .await?;
        let incoming_bytes = new_inputs.iter().fold(0_i64, |total, input| {
            total.saturating_add(input.byte_length)
        });
        if retained.saturating_add(incoming_bytes) > retained_byte_limit {
            return Err(Error::Conflict(
                "retained document asset quota exceeded".into(),
            ));
        }
        let owner_usage:i64=sqlx::query_scalar("SELECT COALESCE(sum(bytes),0)::bigint FROM (SELECT a.byte_length bytes FROM document_assets a JOIN documents d ON d.id=a.document_id WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1) UNION ALL SELECT v.archive_bytes FROM document_versions v JOIN documents d ON d.id=v.document_id WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1) UNION ALL SELECT f.byte_length FROM publication_files f JOIN publications p ON p.id=f.publication_id JOIN documents d ON d.id=p.document_id WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1)) q")
            .bind(document_id).fetch_one(&mut *tx).await?;
        let deployment_usage: i64 =
            sqlx::query_scalar("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        if owner_usage.saturating_add(incoming_bytes) > self.policy.owner_bytes {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        if deployment_usage.saturating_add(incoming_bytes) > self.policy.deployment_bytes {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
        }
        let ids: Vec<Uuid> = new_inputs.iter().map(|_| new_id()).collect();
        let keys: Vec<String> = new_inputs
            .iter()
            .map(|input| input.storage_key.clone())
            .collect();
        let new_digests: Vec<Vec<u8>> = new_inputs
            .iter()
            .map(|input| input.digest.to_vec())
            .collect();
        let lengths: Vec<i64> = new_inputs.iter().map(|input| input.byte_length).collect();
        let media_types: Vec<String> = new_inputs
            .iter()
            .map(|input| input.media_type.clone())
            .collect();
        let names: Vec<Option<String>> = new_inputs
            .iter()
            .map(|input| input.original_name.clone())
            .collect();
        let inserted = sqlx::query_as::<_, AssetRecord>(
            "INSERT INTO document_assets
             (id,document_id,storage_key,digest,byte_length,media_type,original_name)
             SELECT input.id,$1,input.storage_key,input.digest,input.byte_length,input.media_type,input.original_name
             FROM unnest($2::uuid[],$3::text[],$4::bytea[],$5::bigint[],$6::text[],$7::text[])
               AS input(id,storage_key,digest,byte_length,media_type,original_name)
             ON CONFLICT (document_id,digest,byte_length) DO NOTHING
             RETURNING *",
        )
        .bind(document_id).bind(ids).bind(keys).bind(new_digests).bind(lengths).bind(media_types).bind(names)
        .fetch_all(&mut *tx)
        .await?;
        let inserted_keys: std::collections::HashSet<_> = inserted
            .iter()
            .map(|asset| (asset.digest.clone(), asset.byte_length))
            .collect();
        let all = sqlx::query_as::<_, AssetRecord>(
            "SELECT * FROM document_assets WHERE document_id=$1 AND digest=ANY($2)",
        )
        .bind(document_id)
        .bind(&digests)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(inputs
            .into_iter()
            .map(|input| {
                let row = all
                    .iter()
                    .find(|asset| {
                        asset.digest == input.digest && asset.byte_length == input.byte_length
                    })
                    .expect("completed asset was selected")
                    .clone();
                let created = inserted_keys.contains(&(row.digest.clone(), row.byte_length));
                (row, created)
            })
            .collect())
    }

    pub async fn retained_asset_bytes(&self, document_id: Uuid) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(sum(byte_length),0)::bigint FROM document_assets WHERE document_id=$1",
        )
        .bind(document_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn assets(&self, document_id: Uuid, ids: &[Uuid]) -> Result<Vec<AssetRecord>> {
        if ids.len() > 4096 {
            return Err(Error::Invalid("too many assets requested".into()));
        }
        sqlx::query_as::<_, AssetRecord>(
            "SELECT * FROM document_assets WHERE document_id=$1 AND id=ANY($2)",
        )
        .bind(document_id)
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn asset_sizes_by_digests(
        &self,
        document_id: Uuid,
        digests: &[String],
    ) -> Result<std::collections::HashMap<String, i64>> {
        if digests.len() > 4096 {
            return Err(Error::Invalid("too many asset digests requested".into()));
        }
        let decoded: Vec<Vec<u8>> = digests
            .iter()
            .map(|digest| {
                hex::decode(digest).map_err(|_| Error::Invalid("invalid asset digest".into()))
            })
            .collect::<Result<_>>()?;
        let rows: Vec<(Vec<u8>,i64)> = sqlx::query_as("SELECT digest,byte_length FROM document_assets WHERE document_id=$1 AND digest=ANY($2)")
            .bind(document_id).bind(&decoded).fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|(digest, size)| (hex::encode(digest), size))
            .collect())
    }

    pub async fn assets_by_digests(
        &self,
        document_id: Uuid,
        digests: &[String],
    ) -> Result<Vec<AssetRecord>> {
        if digests.len() > 4096 {
            return Err(Error::Invalid("too many asset digests requested".into()));
        }
        let decoded: Vec<Vec<u8>> = digests
            .iter()
            .map(|digest| {
                hex::decode(digest).map_err(|_| Error::Invalid("invalid asset digest".into()))
            })
            .collect::<Result<_>>()?;
        sqlx::query_as::<_, AssetRecord>(
            "SELECT * FROM document_assets WHERE document_id=$1 AND digest=ANY($2)",
        )
        .bind(document_id)
        .bind(&decoded)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn current_version(&self, document_id: Uuid) -> Result<Option<VersionRecord>> {
        sqlx::query_as::<_, VersionRecord>(
            "SELECT v.* FROM documents d
             JOIN document_versions v ON v.id=d.current_version_id
             WHERE d.id=$1 AND d.status='active'",
        )
        .bind(document_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn create_version(&self, input: NewVersion) -> Result<VersionRecord> {
        if input.archive_bytes < 0 || input.logical_bytes < 0 || input.archive_key.is_empty() {
            return Err(Error::Invalid("invalid source version".into()));
        }
        let mut tx = self.pool.begin().await?;
        let document = sqlx::query(
            "SELECT update_sequence,project_generation,status FROM documents WHERE id=$1 FOR UPDATE",
        )
        .bind(input.document_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if document.get::<String, _>("status") != "active" {
            return Err(Error::Conflict("document is being deleted".into()));
        }
        if input.through_update_sequence > document.get::<i64, _>("update_sequence") {
            return Err(Error::Conflict("version is ahead of durable source".into()));
        }
        if input.project_generation > document.get::<i64, _>("project_generation") {
            return Err(Error::Conflict(
                "version is ahead of project structure".into(),
            ));
        }
        if let Some(parent_id) = input.parent_id {
            let belongs: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM document_versions WHERE id=$1 AND document_id=$2)",
            )
            .bind(parent_id)
            .bind(input.document_id)
            .fetch_one(&mut *tx)
            .await?;
            if !belongs {
                return Err(Error::Invalid(
                    "version parent belongs to another document".into(),
                ));
            }
        }
        let mut version_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_versions WHERE document_id=$1")
                .bind(input.document_id)
                .fetch_one(&mut *tx)
                .await?;
        while version_count >= 1000 {
            // Make room only from history already eligible under the default
            // retention predicate. Current and labeled versions are never
            // selected. The immutable archive may remain as an orphan until
            // provider lifecycle cleanup, which is an accepted coarse loss.
            let removed: Option<Uuid> = sqlx::query_scalar(
                "WITH eligible AS (
                   SELECT v.id FROM document_versions v
                   JOIN documents d ON d.id=v.document_id
                   WHERE v.document_id=$1 AND v.label IS NULL
                     AND v.id IS DISTINCT FROM d.current_version_id
                     AND v.created_at < now()-interval '30 days'
                     AND v.id NOT IN (
                       SELECT id FROM document_versions
                       WHERE document_id=$1 AND label IS NULL
                       ORDER BY sequence DESC,id DESC LIMIT 50
                     )
                   ORDER BY v.sequence,v.id LIMIT 1
                 )
                 DELETE FROM document_versions v USING eligible e
                 WHERE v.id=e.id RETURNING v.id",
            )
            .bind(input.document_id)
            .fetch_optional(&mut *tx)
            .await?;
            if removed.is_none() {
                return Err(Error::Conflict("retained version ceiling reached".into()));
            }
            version_count -= 1;
        }
        let recent:i64=sqlx::query_scalar("SELECT count(*) FROM document_versions v JOIN documents d ON d.id=v.document_id WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1) AND v.created_at>=now()-interval '1 hour'").bind(input.document_id).fetch_one(&mut *tx).await?;
        if recent >= self.policy.versions_per_hour {
            return Err(Error::Conflict(
                "account version creation rate exceeded".into(),
            ));
        }
        let owner_usage: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(bytes),0)::bigint FROM (
               SELECT a.byte_length bytes FROM document_assets a JOIN documents d ON d.id=a.document_id
                 WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1)
               UNION ALL SELECT v.archive_bytes FROM document_versions v JOIN documents d ON d.id=v.document_id
                 WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1)
               UNION ALL SELECT f.byte_length FROM publication_files f JOIN publications p ON p.id=f.publication_id
                 JOIN documents d ON d.id=p.document_id WHERE d.owner_id=(SELECT owner_id FROM documents WHERE id=$1)
             ) usage",
        )
        .bind(input.document_id)
        .fetch_one(&mut *tx)
        .await?;
        let deployment_usage: i64 =
            sqlx::query_scalar("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        if owner_usage.saturating_add(input.archive_bytes) > self.policy.owner_bytes {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        if deployment_usage.saturating_add(input.archive_bytes) > self.policy.deployment_bytes {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
        }
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(max(sequence),0)+1 FROM document_versions WHERE document_id=$1",
        )
        .bind(input.document_id)
        .fetch_one(&mut *tx)
        .await?;
        let id = new_id();
        let version = sqlx::query_as::<_, VersionRecord>(
            "INSERT INTO document_versions
             (id,document_id,sequence,parent_id,through_update_sequence,project_generation,
              archive_key,archive_encoding_version,archive_digest,archive_bytes,logical_bytes,
              reason,label,author_account_id,author_label)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) RETURNING *",
        )
        .bind(id)
        .bind(input.document_id)
        .bind(sequence)
        .bind(input.parent_id)
        .bind(input.through_update_sequence)
        .bind(input.project_generation)
        .bind(input.archive_key)
        .bind(input.archive_encoding_version)
        .bind(input.archive_digest.as_slice())
        .bind(input.archive_bytes)
        .bind(input.logical_bytes)
        .bind(input.reason)
        .bind(input.label)
        .bind(input.author_account_id)
        .bind(input.author_label)
        .fetch_one(&mut *tx)
        .await?;
        if input.make_current {
            sqlx::query("UPDATE documents SET current_version_id=$2,updated_at=now() WHERE id=$1")
                .bind(input.document_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(version)
    }

    pub async fn versions(&self, document_id: Uuid, limit: i64) -> Result<Vec<VersionRecord>> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::Invalid("version page limit must be 1..=1000".into()));
        }
        sqlx::query_as::<_, VersionRecord>(
            "SELECT * FROM document_versions WHERE document_id=$1
             ORDER BY sequence DESC,id DESC LIMIT $2",
        )
        .bind(document_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn version(&self, document_id: Uuid, id: Uuid) -> Result<Option<VersionRecord>> {
        sqlx::query_as::<_, VersionRecord>(
            "SELECT * FROM document_versions WHERE document_id=$1 AND id=$2",
        )
        .bind(document_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn version_page(
        &self,
        document_id: Uuid,
        after_sequence: Option<i64>,
        limit: i64,
    ) -> Result<Vec<VersionRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("version page limit must be 1..=200".into()));
        }
        sqlx::query_as::<_,VersionRecord>("SELECT * FROM document_versions WHERE document_id=$1 AND ($2::bigint IS NULL OR sequence<$2) ORDER BY sequence DESC,id DESC LIMIT $3")
            .bind(document_id).bind(after_sequence).bind(limit).fetch_all(&self.pool).await.map_err(Error::from)
    }

    pub async fn label_version(
        &self,
        document_id: Uuid,
        id: Uuid,
        label: Option<&str>,
    ) -> Result<bool> {
        Ok(
            sqlx::query("UPDATE document_versions SET label=$3 WHERE document_id=$1 AND id=$2")
                .bind(document_id)
                .bind(id)
                .bind(label.filter(|v| !v.is_empty()))
                .execute(&self.pool)
                .await?
                .rows_affected()
                == 1,
        )
    }

    pub async fn mark_document_deleting(&self, document_id: Uuid) -> Result<bool> {
        Ok(sqlx::query(
            "UPDATE documents SET status='deleting',deleted_at=COALESCE(deleted_at,now()),updated_at=now()
             WHERE id=$1 AND status='active'",
        ).bind(document_id).execute(&self.pool).await?.rows_affected()==1)
    }

    pub async fn finish_document_deletion(&self, document_id: Uuid) -> Result<bool> {
        Ok(
            sqlx::query("DELETE FROM documents WHERE id=$1 AND status='deleting'")
                .bind(document_id)
                .execute(&self.pool)
                .await?
                .rows_affected()
                == 1,
        )
    }

    pub async fn usage_bytes(&self, account_id: Option<Uuid>) -> Result<i64> {
        if account_id.is_none() {
            return sqlx::query_scalar("SELECT bytes FROM storage_usage WHERE singleton")
                .fetch_one(&self.pool)
                .await
                .map_err(Error::from);
        }
        sqlx::query_scalar(
            "SELECT COALESCE(sum(bytes),0)::bigint FROM (
               SELECT v.archive_bytes AS bytes FROM document_versions v JOIN documents d ON d.id=v.document_id WHERE d.owner_id=$1
               UNION ALL SELECT a.byte_length FROM document_assets a JOIN documents d ON d.id=a.document_id WHERE d.owner_id=$1
               UNION ALL SELECT f.byte_length FROM publication_files f JOIN publications p ON p.id=f.publication_id JOIN documents d ON d.id=p.document_id WHERE d.owner_id=$1
             ) usage",
        ).bind(account_id).fetch_one(&self.pool).await.map_err(Error::from)
    }
}

fn title_key(value: &str) -> String {
    value.nfc().collect::<String>().trim().to_lowercase()
}

fn validate_account(input: &NewAccount) -> Result<()> {
    if input.handle.is_empty() || input.display_name.is_empty() {
        return Err(Error::Invalid("account labels may not be empty".into()));
    }
    let registered = input.kind == "registered";
    if !matches!(input.kind.as_str(), "registered" | "anonymous" | "system")
        || registered != (input.provider.is_some() && input.provider_subject.is_some())
    {
        return Err(Error::Invalid("invalid account identity".into()));
    }
    Ok(())
}

fn validate_document(input: &NewDocument) -> Result<()> {
    if input.slug.is_empty() || input.title.is_empty() || input.main_path.is_empty() {
        return Err(Error::Invalid(
            "document labels and main path may not be empty".into(),
        ));
    }
    if !matches!(input.ownership_mode.as_str(), "owned" | "open" | "example")
        || !matches!(
            input.source_format.as_str(),
            "markdown" | "html" | "typst" | "latex" | "quarto"
        )
    {
        return Err(Error::Invalid("invalid document kind".into()));
    }
    Ok(())
}
