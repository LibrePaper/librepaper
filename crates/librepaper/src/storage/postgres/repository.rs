use serde_json::Value;
use time::OffsetDateTime;
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
    pub status: String,
    pub source_format: String,
    pub main_path: String,
    pub update_sequence: i64,
    pub project_generation: i64,
    pub current_version_id: Option<Uuid>,
    pub current_bundle_id: Option<Uuid>,
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
    /// The digest of the canonical file tree this version holds, when the
    /// writer knew it. It is what says two versions are the same document.
    pub tree_digest: Option<[u8; 32]>,
    /// The paths whose contents differ from the parent version's. `None` when
    /// the writer could not answer, which is not the same as "nothing moved".
    pub changed_paths: Option<Vec<String>>,
    /// How many files this version holds. `None` when the writer did not say,
    /// which is what every version written before the listing needed the
    /// number reads back as. The listing shows it as unknown rather than as
    /// zero: a project has at least a main file, so zero would be a lie.
    pub file_count: Option<i32>,
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
    pub tree_digest: Option<Vec<u8>>,
    pub changed_paths: Option<Vec<String>>,
    pub file_count: Option<i32>,
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub created_at: OffsetDateTime,
}

impl PostgresCatalog {
    pub async fn runtime_state(&self, name: &str) -> Result<Option<serde_json::Value>> {
        sqlx::query_scalar!("SELECT value FROM server_runtime_state WHERE name=$1", name,)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    pub async fn set_runtime_state(&self, name: &str, value: serde_json::Value) -> Result<()> {
        sqlx::query!(
            "INSERT INTO server_runtime_state(name,value) VALUES($1,$2)
             ON CONFLICT(name) DO UPDATE SET value=excluded.value,updated_at=now()",
            name,
            value,
        )
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
        let status = sqlx::query_scalar!(
            "SELECT status FROM accounts WHERE id=$1 FOR UPDATE",
            account_id,
        )
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
        sqlx::query!(
            "UPDATE accounts SET status='erasing',session_generation=session_generation+1
             WHERE id=$1",
            account_id,
        )
        .execute(&mut *tx)
        .await?;
        let document_ids = sqlx::query_scalar!(
            "SELECT id FROM documents WHERE owner_id=$1 ORDER BY id FOR UPDATE",
            account_id,
        )
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE documents SET status='deleting',deleted_at=COALESCE(deleted_at,now()),updated_at=now()
             WHERE owner_id=$1 AND status='active'",
            account_id,
        )
        .execute(&mut *tx)
        .await?;
        let delete_after = OffsetDateTime::now_utc() + recovery_grace;
        for document_id in document_ids {
            sqlx::query!(
                "INSERT INTO jobs(id,kind,document_id,account_id,scope_key,dedupe_key,payload,status,priority,max_attempts,run_after)
                 VALUES($1,'document_deletion',$2,$3,$4,'delete',$5,'queued',0,100,$6)
                 ON CONFLICT (kind,scope_key,dedupe_key)
                 WHERE dedupe_key IS NOT NULL AND status IN ('queued','running') DO NOTHING",
                new_id(),
                document_id,
                account_id,
                format!("document:{document_id}"),
                serde_json::json!({"document_id":document_id}),
                delete_after,
            )
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query!(
            "INSERT INTO jobs(id,kind,account_id,scope_key,dedupe_key,payload,status,priority,max_attempts,run_after)
             VALUES($1,'account_deletion',$2,$3,'delete',$4,'queued',0,100,$5)
             ON CONFLICT (kind,scope_key,dedupe_key)
             WHERE dedupe_key IS NOT NULL AND status IN ('queued','running') DO NOTHING",
            new_id(),
            account_id,
            format!("account:{account_id}"),
            serde_json::json!({"account_id":account_id}),
            delete_after + time::Duration::minutes(5),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn finish_account_erasure(&self, account_id: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let remaining = sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!" FROM documents WHERE owner_id=$1"#,
            account_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if remaining != 0 {
            return Err(Error::Conflict(
                "account still has documents awaiting deletion".into(),
            ));
        }
        sqlx::query!("DELETE FROM grants WHERE account_id=$1", account_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query!(
            "UPDATE annotations SET author_account_id=NULL,author_key='erased',
             author_label='Deleted user' WHERE author_account_id=$1",
            account_id,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE replies SET author_account_id=NULL,author_key='erased',
             author_label='Deleted user' WHERE author_account_id=$1",
            account_id,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE document_versions SET author_account_id=NULL,author_label='Deleted user'
             WHERE author_account_id=$1",
            account_id,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE bundles SET rendered_by_account_id=NULL,rendered_by_label='Deleted user'
             WHERE rendered_by_account_id=$1",
            account_id,
        )
        .execute(&mut *tx)
        .await?;
        let deleted = sqlx::query!(
            "DELETE FROM accounts WHERE id=$1 AND status='erasing'",
            account_id,
        )
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
        let owner_id = sqlx::query_scalar!(
            "SELECT owner_id FROM documents WHERE id=$1 AND status='active'",
            document_id,
        )
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
        Ok(sqlx::query!(
            "UPDATE documents SET title=$2,owner_id=$3,ownership_mode=$4,
             updated_at=now() WHERE id=$1 AND status='active'",
            id,
            title,
            owner_id,
            ownership_mode,
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub async fn create_account(&self, input: NewAccount) -> Result<AccountRecord> {
        validate_account(&input)?;
        let id = new_id();
        sqlx::query_as!(
            AccountRecord,
            "INSERT INTO accounts
             (id,kind,provider,provider_subject,handle,display_name,email,status)
             VALUES($1,$2,$3,$4,$5,$6,$7,'active')
             RETURNING id,kind,provider,provider_subject,handle,display_name,email,status,
                       session_generation,preferences,created_at,last_seen_at",
            id,
            input.kind,
            input.provider,
            input.provider_subject,
            input.handle,
            input.display_name,
            input.email,
        )
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
        sqlx::query_as!(
            AccountRecord,
            "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status)
             VALUES($1,'registered',$2,$3,$4,$5,$6,'active')
             ON CONFLICT(provider,provider_subject) WHERE kind='registered' DO UPDATE SET
               handle=excluded.handle,display_name=excluded.display_name,email=excluded.email,last_seen_at=now()
             WHERE accounts.status='active'
             RETURNING accounts.id,accounts.kind,accounts.provider,accounts.provider_subject,
                       accounts.handle,accounts.display_name,accounts.email,accounts.status,
                       accounts.session_generation,accounts.preferences,accounts.created_at,
                       accounts.last_seen_at",
            new_id(),
            input.provider,
            input.provider_subject,
            input.handle,
            input.display_name,
            input.email,
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::Conflict("account is not active".into()))
    }

    /// The accounts named by `ids`, in one round trip, skipping any that no
    /// longer exist. Listing a page of documents needs an owner and a display
    /// name for each guest on it; asking row by row made the cost of a page
    /// proportional to the people on it rather than to the page.
    pub async fn accounts_by_ids(&self, ids: &[Uuid]) -> Result<Vec<AccountRecord>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as!(
            AccountRecord,
            "SELECT id,kind,provider,provider_subject,handle,display_name,email,status,
                    session_generation,preferences,created_at,last_seen_at
             FROM accounts WHERE id=ANY($1)",
            ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn account(&self, id: Uuid) -> Result<Option<AccountRecord>> {
        sqlx::query_as!(
            AccountRecord,
            "SELECT id,kind,provider,provider_subject,handle,display_name,email,status,
                    session_generation,preferences,created_at,last_seen_at
             FROM accounts WHERE id=$1",
            id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// The name a version written by this account is signed with.
    ///
    /// A signed-in caller's `Identity::id` is this row's id -- the catalogue
    /// uuid, minted here and stable across a renamed handle -- so anything
    /// that signs a version with the id it authenticated as signs it with a
    /// uuid, and the timeline has nobody's name to show. The account row is
    /// the only place the two are side by side.
    ///
    /// Empty when the account cannot be read, rather than a fabricated name
    /// or the id again: a caller that has nothing to show must be free to
    /// fall back to whatever it knew before asking.
    pub async fn account_display_name(&self, id: Uuid) -> String {
        let Ok(Some(account)) = self.account(id).await else {
            return String::new();
        };
        if !account.display_name.is_empty() {
            account.display_name
        } else {
            account.handle
        }
    }

    pub async fn create_document(&self, input: NewDocument) -> Result<DocumentRecord> {
        validate_document(&input)?;
        let id = new_id();
        sqlx::query_as!(
            DocumentRecord,
            "INSERT INTO documents
             (id,slug,owner_id,ownership_mode,title,status,source_format,main_path,settings)
             VALUES($1,$2,$3,$4,$5,'active',$6,$7,$8)
             RETURNING id,slug,owner_id,ownership_mode,title,status,source_format,
                       main_path,update_sequence,project_generation,current_version_id,
                       current_bundle_id,settings,created_at,updated_at,deleted_at",
            id,
            input.slug,
            input.owner_id,
            input.ownership_mode,
            input.title,
            input.source_format,
            input.main_path,
            input.settings,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn document_by_slug(&self, slug: &str) -> Result<Option<DocumentRecord>> {
        sqlx::query_as!(
            DocumentRecord,
            "SELECT id,slug,owner_id,ownership_mode,title,status,source_format,
                    main_path,update_sequence,project_generation,current_version_id,
                    current_bundle_id,settings,created_at,updated_at,deleted_at
             FROM documents WHERE slug=$1",
            slug,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn document(&self, id: Uuid) -> Result<Option<DocumentRecord>> {
        sqlx::query_as!(
            DocumentRecord,
            "SELECT id,slug,owner_id,ownership_mode,title,status,source_format,
                    main_path,update_sequence,project_generation,current_version_id,
                    current_bundle_id,settings,created_at,updated_at,deleted_at
             FROM documents WHERE id=$1",
            id,
        )
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
        sqlx::query_as!(
            DocumentRecord,
            "SELECT id,slug,owner_id,ownership_mode,title,status,source_format,
                    main_path,update_sequence,project_generation,current_version_id,
                    current_bundle_id,settings,created_at,updated_at,deleted_at
             FROM documents
             WHERE status='active'
               AND (updated_at,id) < (COALESCE($1::timestamptz,'infinity'),
                                      COALESCE($2::uuid,'ffffffff-ffff-ffff-ffff-ffffffffffff'))
             ORDER BY updated_at DESC,id DESC LIMIT $3",
            before.map(|value| value.0),
            before.map(|value| value.1),
            limit,
        )
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
        sqlx::query_as!(
            DocumentRecord,
            "SELECT id,slug,owner_id,ownership_mode,title,status,source_format,
                    main_path,update_sequence,project_generation,current_version_id,
                    current_bundle_id,settings,created_at,updated_at,deleted_at
             FROM documents WHERE owner_id=$1 AND status='active'
             ORDER BY updated_at DESC,id DESC LIMIT $2",
            owner_id,
            limit,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
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
        sqlx::query_as!(
            DocumentRecord,
            "SELECT id,slug,owner_id,ownership_mode,title,status,source_format,
                    main_path,update_sequence,project_generation,current_version_id,
                    current_bundle_id,settings,created_at,updated_at,deleted_at
             FROM documents WHERE owner_id=$1 AND status='active'
             AND (updated_at,id) < (COALESCE($2::timestamptz,'infinity'),
                                    COALESCE($3::uuid,'ffffffff-ffff-ffff-ffff-ffffffffffff'))
             ORDER BY updated_at DESC,id DESC LIMIT $4",
            owner_id,
            before.map(|value| value.0),
            before.map(|value| value.1),
            limit,
        )
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
        sqlx::query_as!(
            DocumentRecord,
            "SELECT d.id,d.slug,d.owner_id,d.ownership_mode,d.title,d.status,
                    d.source_format,d.main_path,d.update_sequence,d.project_generation,
                    d.current_version_id,d.current_bundle_id,d.settings,d.created_at,
                    d.updated_at,d.deleted_at
             FROM documents d
             WHERE d.status='active'
               AND (d.owner_id=$1 OR EXISTS (
                    SELECT 1 FROM grants g WHERE g.document_id=d.id AND g.account_id=$1
                      AND (g.source_link_hash IS NULL OR EXISTS (
                        SELECT 1 FROM share_links l WHERE l.document_id=d.id
                          AND l.token_hash=g.source_link_hash AND l.revoked_at IS NULL
                          AND (l.expires_at IS NULL OR l.expires_at>now())
                      ))
               ) OR ($4 AND d.ownership_mode='example'))
               AND (d.updated_at,d.id) < (COALESCE($2::timestamptz,'infinity'),
                                            COALESCE($3::uuid,'ffffffff-ffff-ffff-ffff-ffffffffffff'))
             ORDER BY d.updated_at DESC,d.id DESC LIMIT $5",
            account_id,
            before.map(|value| value.0),
            before.map(|value| value.1),
            include_examples,
            limit,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn document_counts_by_owner(&self, owner_id: Uuid) -> Result<(i64, i64)> {
        let row = sqlx::query!(
            // Counted separately rather than over a join: joining documents to
            // their versions multiplies one side by the other only to collapse
            // it again with DISTINCT, and materializes a row per version to
            // count the documents.
            r#"SELECT (SELECT count(*) FROM documents d
                       WHERE d.owner_id=$1 AND d.status='active')::bigint AS "documents!",
                      (SELECT count(*) FROM document_versions v
                       JOIN documents d ON d.id=v.document_id
                       WHERE d.owner_id=$1 AND d.status='active')::bigint AS "versions!""#,
            owner_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok((row.documents, row.versions))
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
        // The owner is read under the same lock that guards the document's
        // status, so the quota below is measured against the owner this
        // document still has when the assets are written.
        let document = sqlx::query!(
            "SELECT status,owner_id FROM documents WHERE id=$1 FOR UPDATE",
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let document = document.ok_or(Error::NotFound)?;
        match document.status.as_str() {
            "active" => {}
            _ => return Err(Error::Conflict("document is being deleted".into())),
        }
        let digests: Vec<Vec<u8>> = inputs.iter().map(|input| input.digest.to_vec()).collect();
        let existing = sqlx::query_as!(
            AssetRecord,
            "SELECT id,document_id,storage_key,digest,byte_length,media_type,original_name,
                    created_at
             FROM document_assets WHERE document_id=$1 AND digest=ANY($2)",
            document_id,
            &digests,
        )
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
        let uploads = sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!" FROM document_assets a
               JOIN documents d ON d.id=a.document_id
               WHERE d.owner_id=$1 AND a.created_at>=now()-interval '1 hour'"#,
            document.owner_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if uploads.saturating_add(new_inputs.len() as i64) > self.policy.asset_uploads_per_hour {
            return Err(Error::Conflict("account asset upload rate exceeded".into()));
        }
        let retained = sqlx::query_scalar!(
            r#"SELECT COALESCE(sum(byte_length),0)::bigint AS "total!"
               FROM document_assets WHERE document_id=$1"#,
            document_id,
        )
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
        let owner_usage = owner_usage_bytes(&mut *tx, document.owner_id).await?;
        let deployment_usage =
            sqlx::query_scalar!("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE",)
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
        let inserted = sqlx::query_as!(
            AssetRecord,
            "INSERT INTO document_assets
             (id,document_id,storage_key,digest,byte_length,media_type,original_name)
             SELECT input.id,$1,input.storage_key,input.digest,input.byte_length,input.media_type,input.original_name
             FROM unnest($2::uuid[],$3::text[],$4::bytea[],$5::bigint[],$6::text[],$7::text[])
               AS input(id,storage_key,digest,byte_length,media_type,original_name)
             ON CONFLICT (document_id,digest,byte_length) DO NOTHING
             RETURNING id,document_id,storage_key,digest,byte_length,media_type,original_name,
                       created_at",
            document_id,
            &ids,
            &keys,
            &new_digests,
            &lengths,
            &media_types,
            &names as &[Option<String>],
        )
        .fetch_all(&mut *tx)
        .await?;
        let inserted_keys: std::collections::HashSet<_> = inserted
            .iter()
            .map(|asset| (asset.digest.clone(), asset.byte_length))
            .collect();
        let all = sqlx::query_as!(
            AssetRecord,
            "SELECT id,document_id,storage_key,digest,byte_length,media_type,original_name,
                    created_at
             FROM document_assets WHERE document_id=$1 AND digest=ANY($2)",
            document_id,
            &digests,
        )
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
        sqlx::query_scalar!(
            r#"SELECT COALESCE(sum(byte_length),0)::bigint AS "total!"
               FROM document_assets WHERE document_id=$1"#,
            document_id,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn assets(&self, document_id: Uuid, ids: &[Uuid]) -> Result<Vec<AssetRecord>> {
        if ids.len() > 4096 {
            return Err(Error::Invalid("too many assets requested".into()));
        }
        sqlx::query_as!(
            AssetRecord,
            "SELECT id,document_id,storage_key,digest,byte_length,media_type,original_name,
                    created_at
             FROM document_assets WHERE document_id=$1 AND id=ANY($2)",
            document_id,
            ids,
        )
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
        let rows = sqlx::query!(
            "SELECT digest,byte_length FROM document_assets
             WHERE document_id=$1 AND digest=ANY($2)",
            document_id,
            &decoded,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| (hex::encode(row.digest), row.byte_length))
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
        sqlx::query_as!(
            AssetRecord,
            "SELECT id,document_id,storage_key,digest,byte_length,media_type,original_name,
                    created_at
             FROM document_assets WHERE document_id=$1 AND digest=ANY($2)",
            document_id,
            &decoded,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn current_version(&self, document_id: Uuid) -> Result<Option<VersionRecord>> {
        sqlx::query_as!(
            VersionRecord,
            "SELECT v.id,v.document_id,v.sequence,v.parent_id,v.through_update_sequence,
                    v.project_generation,v.archive_key,v.archive_encoding_version,v.archive_digest,
                    v.archive_bytes,v.logical_bytes,v.tree_digest,v.changed_paths,v.file_count,v.reason,
                    v.label,v.author_account_id,v.author_label,v.created_at
             FROM documents d
             JOIN document_versions v ON v.id=d.current_version_id
             WHERE d.id=$1 AND d.status='active'",
            document_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// The immutable source checkpoint a published bundle names by tree
    /// digest. The newest match is enough: identical trees have identical
    /// source bytes and stable file identities.
    pub async fn version_by_tree_digest(
        &self,
        document_id: Uuid,
        digest: &[u8],
    ) -> Result<Option<VersionRecord>> {
        sqlx::query_as::<_, VersionRecord>(
            // Every column `VersionRecord` has, because this is the one
            // version query that is not the checked macro: `FromRow` looks
            // the fields up by name at run time, so a column left out here is
            // a request that fails rather than a build that does.
            "SELECT id,document_id,sequence,parent_id,through_update_sequence,
                    project_generation,archive_key,archive_encoding_version,archive_digest,
                    archive_bytes,logical_bytes,tree_digest,changed_paths,file_count,reason,label,
                    author_account_id,author_label,created_at
             FROM document_versions WHERE document_id=$1 AND tree_digest=$2
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(document_id)
        .bind(digest)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn create_version(&self, input: NewVersion) -> Result<VersionRecord> {
        if input.archive_bytes < 0 || input.logical_bytes < 0 || input.archive_key.is_empty() {
            return Err(Error::Invalid("invalid source version".into()));
        }
        let mut tx = self.pool.begin().await?;
        let document = sqlx::query!(
            "SELECT update_sequence,project_generation,status,owner_id FROM documents
             WHERE id=$1 FOR UPDATE",
            input.document_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if document.status != "active" {
            return Err(Error::Conflict("document is being deleted".into()));
        }
        if input.through_update_sequence > document.update_sequence {
            return Err(Error::Conflict("version is ahead of durable source".into()));
        }
        if input.project_generation > document.project_generation {
            return Err(Error::Conflict(
                "version is ahead of project structure".into(),
            ));
        }
        if let Some(parent_id) = input.parent_id {
            let belongs = sqlx::query_scalar!(
                r#"SELECT EXISTS(SELECT 1 FROM document_versions WHERE id=$1 AND document_id=$2)
                   AS "exists!""#,
                parent_id,
                input.document_id,
            )
            .fetch_one(&mut *tx)
            .await?;
            if !belongs {
                return Err(Error::Invalid(
                    "version parent belongs to another document".into(),
                ));
            }
        }
        // Nothing is pruned here. A version exists because somebody asked for
        // one -- a label, a bundle, a restore, a comment's anchor -- and
        // there is no routine churn left to shed. What a document did between
        // two of them is in the operation history, which is kept whole.
        let recent = sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!" FROM document_versions v
               JOIN documents d ON d.id=v.document_id
               WHERE d.owner_id=$1 AND v.created_at>=now()-interval '1 hour'"#,
            document.owner_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if recent >= self.policy.versions_per_hour {
            return Err(Error::Conflict(
                "account version creation rate exceeded".into(),
            ));
        }
        // What this version adds to the store, which is not the same as what
        // it holds. An archive that another version already names is already
        // there; naming it again writes no bytes, and charging an account for
        // them would bill it twice for one object.
        let held = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM document_versions WHERE archive_key=$1)
               AS "held!""#,
            input.archive_key,
        )
        .fetch_one(&mut *tx)
        .await?;
        let charge = if held { 0 } else { input.archive_bytes };
        let owner_usage = owner_usage_bytes(&mut *tx, document.owner_id).await?;
        let deployment_usage =
            sqlx::query_scalar!("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE",)
                .fetch_one(&mut *tx)
                .await?;
        if owner_usage.saturating_add(charge) > self.policy.owner_bytes {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        if deployment_usage.saturating_add(charge) > self.policy.deployment_bytes {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
        }
        let sequence = sqlx::query_scalar!(
            r#"SELECT COALESCE(max(sequence),0)+1 AS "sequence!"
               FROM document_versions WHERE document_id=$1"#,
            input.document_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        let id = new_id();
        let tree_digest = input.tree_digest.map(|digest| digest.to_vec());
        let version = sqlx::query_as!(
            VersionRecord,
            "INSERT INTO document_versions
             (id,document_id,sequence,parent_id,through_update_sequence,project_generation,
              archive_key,archive_encoding_version,archive_digest,archive_bytes,logical_bytes,
              tree_digest,changed_paths,file_count,reason,label,author_account_id,author_label)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
             RETURNING id,document_id,sequence,parent_id,through_update_sequence,project_generation,
                       archive_key,archive_encoding_version,archive_digest,archive_bytes,
                       logical_bytes,tree_digest,changed_paths,file_count,reason,label,
                       author_account_id,author_label,created_at",
            id,
            input.document_id,
            sequence,
            input.parent_id,
            input.through_update_sequence,
            input.project_generation,
            input.archive_key,
            input.archive_encoding_version,
            input.archive_digest.as_slice(),
            input.archive_bytes,
            input.logical_bytes,
            tree_digest,
            input.changed_paths.as_deref(),
            input.file_count,
            input.reason,
            input.label,
            input.author_account_id,
            input.author_label,
        )
        .fetch_one(&mut *tx)
        .await?;
        if input.make_current {
            sqlx::query!(
                "UPDATE documents SET current_version_id=$2,updated_at=now() WHERE id=$1",
                input.document_id,
                id,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(version)
    }

    /// Move a version back in time, for a writer that has the moment in hand
    /// rather than the clock -- today, the seeded examples, which are written
    /// as a history rather than accumulated as one. See `seed::activity`: the
    /// archive and everything named in it are real, and this column is the
    /// one thing about them that is invented.
    ///
    /// Nothing else writes `created_at`; a version records when it was made,
    /// and that is not a fact a caller gets to supply.
    pub async fn backdate_version(&self, id: Uuid, at: OffsetDateTime) -> Result<()> {
        sqlx::query!(
            "UPDATE document_versions SET created_at=$2 WHERE id=$1",
            id,
            at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn versions(&self, document_id: Uuid, limit: i64) -> Result<Vec<VersionRecord>> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::Invalid("version page limit must be 1..=1000".into()));
        }
        sqlx::query_as!(
            VersionRecord,
            "SELECT id,document_id,sequence,parent_id,through_update_sequence,project_generation,
                    archive_key,archive_encoding_version,archive_digest,archive_bytes,
                    logical_bytes,tree_digest,changed_paths,file_count,reason,label,author_account_id,
                    author_label,created_at
             FROM document_versions WHERE document_id=$1
             ORDER BY sequence DESC LIMIT $2",
            document_id,
            limit,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// The object an existing version of this document already holds these
    /// exact archive bytes under, if one does.
    ///
    /// Returns the stored key rather than a bare yes, because it is the key a
    /// new version must name to share the object -- and it is not always the
    /// content-addressed name a writer would mint today. A version written
    /// before archives were shared holds its bytes under its own id, and the
    /// cheapest thing a duplicate of it can do is point at that.
    pub async fn archive_for_digest(
        &self,
        document_id: Uuid,
        digest: &[u8],
    ) -> Result<Option<String>> {
        sqlx::query_scalar!(
            "SELECT archive_key FROM document_versions
             WHERE document_id=$1 AND archive_digest=$2
             ORDER BY sequence LIMIT 1",
            document_id,
            digest,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn version(&self, document_id: Uuid, id: Uuid) -> Result<Option<VersionRecord>> {
        sqlx::query_as!(
            VersionRecord,
            "SELECT id,document_id,sequence,parent_id,through_update_sequence,project_generation,
                    archive_key,archive_encoding_version,archive_digest,archive_bytes,
                    logical_bytes,tree_digest,changed_paths,file_count,reason,label,author_account_id,
                    author_label,created_at
             FROM document_versions WHERE document_id=$1 AND id=$2",
            document_id,
            id,
        )
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
        sqlx::query_as!(
            VersionRecord,
            "SELECT id,document_id,sequence,parent_id,through_update_sequence,project_generation,
                    archive_key,archive_encoding_version,archive_digest,archive_bytes,
                    logical_bytes,tree_digest,changed_paths,file_count,reason,label,author_account_id,
                    author_label,created_at
             FROM document_versions WHERE document_id=$1
             AND sequence < COALESCE($2::bigint, 9223372036854775807) ORDER BY sequence DESC LIMIT $3",
            document_id,
            after_sequence,
            limit,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn label_version(
        &self,
        document_id: Uuid,
        id: Uuid,
        label: Option<&str>,
    ) -> Result<bool> {
        Ok(sqlx::query!(
            "UPDATE document_versions SET label=$3 WHERE document_id=$1 AND id=$2",
            document_id,
            id,
            label.filter(|v| !v.is_empty()),
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub async fn mark_document_deleting(&self, document_id: Uuid) -> Result<bool> {
        Ok(sqlx::query!(
            "UPDATE documents SET status='deleting',deleted_at=COALESCE(deleted_at,now()),
             updated_at=now() WHERE id=$1 AND status='active'",
            document_id,
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    /// The documents this account has deleted and can still get back.
    ///
    /// Deleting a project has never destroyed it immediately: it is marked
    /// 'deleting' and a `document_deletion` job is queued to run seven days
    /// later, and until that job runs every row, blob and comment is still
    /// here. That window existed for years with no way to reach it, which is
    /// what this query and `restore_document` are for.
    ///
    /// Ordered by when they went rather than by `updated_at`: the trash is
    /// read as "what did I just do", and the deletion is the event.
    pub async fn trashed_documents(
        &self,
        owner_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DocumentRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("document page limit must be 1..=200".into()));
        }
        sqlx::query_as!(
            DocumentRecord,
            "SELECT d.id,d.slug,d.owner_id,d.ownership_mode,d.title,d.status,
                    d.source_format,d.main_path,d.update_sequence,d.project_generation,
                    d.current_version_id,d.current_bundle_id,d.settings,d.created_at,
                    d.updated_at,d.deleted_at
             FROM documents d
             WHERE d.owner_id=$1 AND d.status='deleting'
             ORDER BY d.deleted_at DESC,d.id DESC LIMIT $2",
            owner_id,
            limit,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// When the queued purge of a deleted document is due, so a listing can
    /// say how long is left rather than making the reader count days from the
    /// deletion themselves. `None` for a document with no purge queued, which
    /// is a document that is not in the trash.
    pub async fn deletion_due(&self, document_id: Uuid) -> Result<Option<OffsetDateTime>> {
        Ok(sqlx::query_scalar!(
            "SELECT run_after FROM jobs
             WHERE kind='document_deletion' AND document_id=$1 AND status='queued'
             ORDER BY run_after LIMIT 1",
            document_id,
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Take a document back out of the trash.
    ///
    /// Cancelling the queued purge is the operation; flipping the status back
    /// is bookkeeping that follows from it. They happen in one transaction and
    /// in that order because the job is the thing that can be lost: if the
    /// purge has already been claimed and is running, its blobs are already
    /// going, and a document restored on top of that would be an entry in the
    /// listing pointing at files that are half gone. So a purge that is no
    /// longer 'queued' refuses the restore rather than racing it.
    pub async fn restore_document(&self, document_id: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let cancelled = sqlx::query!(
            "UPDATE jobs SET status='cancelled',updated_at=now()
             WHERE kind='document_deletion' AND document_id=$1 AND status='queued'",
            document_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if cancelled == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        let restored = sqlx::query!(
            "UPDATE documents SET status='active',deleted_at=NULL,updated_at=now()
             WHERE id=$1 AND status='deleting'",
            document_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if restored == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Purge a deleted document now instead of when its seven days are up.
    /// "Delete forever" in the trash: the job is already queued and already
    /// knows how to do it, so this only brings it forward.
    pub async fn hasten_deletion(&self, document_id: Uuid) -> Result<bool> {
        Ok(sqlx::query!(
            "UPDATE jobs SET run_after=now(),updated_at=now()
             WHERE kind='document_deletion' AND document_id=$1 AND status='queued'",
            document_id,
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn finish_document_deletion(&self, document_id: Uuid) -> Result<bool> {
        Ok(sqlx::query!(
            "DELETE FROM documents WHERE id=$1 AND status='deleting'",
            document_id,
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub async fn usage_bytes(&self, account_id: Option<Uuid>) -> Result<i64> {
        let Some(account_id) = account_id else {
            return sqlx::query_scalar!("SELECT bytes FROM storage_usage WHERE singleton")
                .fetch_one(&self.pool)
                .await
                .map_err(Error::from);
        };
        owner_usage_bytes(&self.pool, account_id).await
    }
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

/// Every byte an account is charged for: version archives, document assets,
/// and published files. This is the single definition of what counts toward
/// the owner quota -- it was copied into three call sites before, keyed two
/// different ways, so a new kind of billable byte had to be remembered in
/// three places to be counted in any of them.
///
/// Callers holding a document row pass its `owner_id` from whatever lock they
/// already took, so the sum is measured against the owner the document has
/// under that lock.
pub(super) async fn owner_usage_bytes<'e, E>(executor: E, account_id: Uuid) -> Result<i64>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query_scalar!(
        r#"SELECT COALESCE(sum(bytes),0)::bigint AS "total!" FROM (
             SELECT v.archive_bytes AS bytes FROM (
               SELECT DISTINCT ON (v.archive_key) v.archive_key, v.archive_bytes
               FROM document_versions v
               JOIN documents d ON d.id=v.document_id WHERE d.owner_id=$1
               ORDER BY v.archive_key, v.id
             ) v
             UNION ALL SELECT a.byte_length FROM document_assets a
               JOIN documents d ON d.id=a.document_id WHERE d.owner_id=$1
             UNION ALL SELECT f.byte_length FROM bundle_files f
               JOIN bundles p ON p.id=f.bundle_id
               JOIN documents d ON d.id=p.document_id WHERE d.owner_id=$1
           ) usage"#,
        account_id,
    )
    .fetch_one(executor)
    .await
    .map_err(Error::from)
}
