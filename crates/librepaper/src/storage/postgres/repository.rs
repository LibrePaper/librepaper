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
        let mut tx = self.begin_writer_transaction().await?;
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
        // No queue rows. `documents.status = 'deleting'` is the durable
        // state the background worker rediscovers at startup and acts on
        // (§8.6), and the account row's own `erasing` status is what says
        // the account is still owed its deletion. A queue row would be a
        // third copy of both, able to disagree with either.
        let _ = (&document_ids, recovery_grace);
        tx.commit().await?;
        Ok(true)
    }

    /// One keyset-paginated page of the documents an erasing account still
    /// owns, whatever state they are in.
    ///
    /// Not `documents_by_owner`: that one is about a person's listing and so
    /// asks only for `status='active'`, and the documents an erasure is
    /// waiting on are precisely the ones already marked `deleting` or
    /// `purging`. Ordered by id, which is what makes the cursor total: a
    /// row's `updated_at` moves under a walk, an id does not.
    pub async fn documents_awaiting_account_erasure(
        &self,
        owner_id: Uuid,
        after: Uuid,
        limit: i64,
    ) -> Result<Vec<Uuid>> {
        if !(1..=500).contains(&limit) {
            return Err(Error::Invalid(
                "erasure document page limit must be 1..=500".into(),
            ));
        }
        sqlx::query_scalar!(
            "SELECT id FROM documents WHERE owner_id=$1 AND id > $2 ORDER BY id LIMIT $3",
            owner_id,
            after,
            limit,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn finish_account_erasure(&self, account_id: Uuid) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
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
            "UPDATE document_labels SET author_account_id=NULL,author_label='Deleted user'
             WHERE author_account_id=$1",
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
        let mut tx = self.begin_writer_transaction().await?;
        let changed = sqlx::query!(
            "UPDATE documents SET title=$2,owner_id=$3,ownership_mode=$4,
             updated_at=now() WHERE id=$1 AND status='active'",
            id,
            title,
            owner_id,
            ownership_mode,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
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
                       main_path,update_sequence,
                       settings,created_at,updated_at,deleted_at",
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
                    main_path,update_sequence,
                       settings,created_at,updated_at,deleted_at
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
                    main_path,update_sequence,
                       settings,created_at,updated_at,deleted_at
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
                    main_path,update_sequence,
                       settings,created_at,updated_at,deleted_at
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
        // Build a bounded candidate set from each access path before joining
        // back to documents.  The old OR predicate encouraged PostgreSQL to
        // walk every active document when a caller owned only a small subset;
        // each branch gets the cursor and page limit, while UNION removes
        // duplicates when a document is both owned and granted (or an example).
        sqlx::query_as!(
            DocumentRecord,
            "WITH candidates AS (
                 (SELECT d.id,d.updated_at
                 FROM documents d
                 WHERE d.owner_id=$1 AND d.status='active'
                   AND (d.updated_at,d.id) <
                       (COALESCE($2::timestamptz,'infinity'),
                        COALESCE($3::uuid,'ffffffff-ffff-ffff-ffff-ffffffffffff'))
                 ORDER BY d.updated_at DESC,d.id DESC LIMIT $5)
                 UNION
                 (SELECT g.document_id,d.updated_at
                 FROM grants g
                 JOIN documents d ON d.id=g.document_id AND d.status='active'
                 WHERE g.account_id=$1
                   AND (g.source_link_hash IS NULL OR EXISTS (
                     SELECT 1 FROM share_links l
                     WHERE l.document_id=g.document_id
                       AND l.token_hash=g.source_link_hash
                       AND l.revoked_at IS NULL
                       AND (l.expires_at IS NULL OR l.expires_at>now())
                   ))
                   AND (d.updated_at,d.id) <
                       (COALESCE($2::timestamptz,'infinity'),
                        COALESCE($3::uuid,'ffffffff-ffff-ffff-ffff-ffffffffffff'))
                 ORDER BY d.updated_at DESC,d.id DESC LIMIT $5)
                 UNION
                 (SELECT d.id,d.updated_at
                 FROM documents d
                 WHERE $4 AND d.ownership_mode='example' AND d.status='active'
                   AND (d.updated_at,d.id) <
                       (COALESCE($2::timestamptz,'infinity'),
                        COALESCE($3::uuid,'ffffffff-ffff-ffff-ffff-ffffffffffff'))
                 ORDER BY d.updated_at DESC,d.id DESC LIMIT $5)
             )
             SELECT d.id,d.slug,d.owner_id,d.ownership_mode,d.title,d.status,
                    d.source_format,d.main_path,d.update_sequence,
                    d.settings,d.created_at,
                    d.updated_at,d.deleted_at
             FROM documents d
             JOIN candidates c ON c.id=d.id
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
            // Counted separately rather than over a join: joining documents
            // to their labels multiplies one side by the other only to
            // collapse it again with DISTINCT, and materializes a row per
            // label to count the documents.
            r#"SELECT (SELECT count(*) FROM documents d
                       WHERE d.owner_id=$1 AND d.status='active')::bigint AS "documents!",
                      (SELECT count(*) FROM document_labels l
                       JOIN documents d ON d.id=l.document_id
                       WHERE d.owner_id=$1 AND d.status='active')::bigint AS "versions!""#,
            owner_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok((row.documents, row.versions))
    }

    pub async fn complete_asset(&self, input: NewAsset) -> Result<(AssetRecord, bool)> {
        self.complete_assets_inner(vec![input], None)
            .await?
            .pop()
            .ok_or_else(|| Error::Invalid("asset batch was empty".into()))
    }

    pub async fn complete_assets(
        &self,
        inputs: Vec<NewAsset>,
    ) -> Result<Vec<(AssetRecord, bool)>> {
        self.complete_assets_inner(inputs, None)
            .await
    }

    pub async fn complete_asset_authorized(
        &self,
        input: NewAsset,
        actor: &super::MutationAuthorization,
    ) -> Result<(AssetRecord, bool)> {
        self.complete_assets_inner(vec![input], Some(actor))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Invalid("asset batch was empty".into()))
    }

    async fn complete_assets_inner(
        &self,
        inputs: Vec<NewAsset>,
        actor: Option<&super::MutationAuthorization>,
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
        let mut tx = self.begin_writer_transaction().await?;
        if let Some(actor) = actor {
            PostgresCatalog::authorize_annotation_mutation(&mut tx, document_id, actor, true)
                .await?;
        }
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
        let incoming_bytes = new_inputs.iter().fold(0_i64, |total, input| {
            total.saturating_add(input.byte_length)
        });
        let owner_usage = owner_usage_bytes(&mut *tx, document.owner_id).await?;
        // Lock taken by the first statement; the deployment total is blob bytes
        // plus every current base plus every document's uncompacted rows.
        let _lock = sqlx::query_scalar!("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE",)
            .fetch_one(&mut *tx)
            .await?;
        let log_usage = sqlx::query_scalar!(
            r#"SELECT (COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0) + COALESCE((SELECT sum(uncompacted_update_bytes)::bigint FROM documents),0))::bigint AS "total!""#
        )
        .fetch_one(&mut *tx)
        .await?;
        let deployment_usage = _lock.saturating_add(log_usage);
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

    pub async fn mark_document_deleting(&self, document_id: Uuid) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
        let changed = sqlx::query!(
            "UPDATE documents SET status='deleting',deleted_at=COALESCE(deleted_at,now()),
             updated_at=now() WHERE id=$1 AND status='active'",
            document_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
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
                    d.source_format,d.main_path,d.update_sequence,
                    d.settings,d.created_at,
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

    /// When the purge of a deleted document is due, so a listing can say how
    /// long is left rather than making the reader count days from the
    /// deletion themselves. `None` for a document that is not in the trash.
    ///
    /// Derived from `deleted_at` rather than read off a queue row. There is
    /// no queue: the status and the timestamp on the document are the
    /// durable state, and the background worker rediscovers them (§8.6).
    pub async fn deletion_due(&self, document_id: Uuid) -> Result<Option<OffsetDateTime>> {
        Ok(sqlx::query_scalar!(
            "SELECT deleted_at FROM documents WHERE id=$1 AND status='deleting'",
            document_id,
        )
        .fetch_optional(&self.pool)
        .await?
        .flatten()
        .map(|at| at + crate::storage::maintenance::DELETION_GRACE))
    }

    /// Take a document back out of the trash.
    ///
    /// The status is the whole of it. The worker reads `status='deleting'`
    /// under the document lock before it touches a blob and again after, so
    /// a document restored while a purge is in flight is left alone rather
    /// than half deleted.
    pub async fn restore_document(&self, document_id: Uuid) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
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

    /// Atomically closes the recovery window before the worker touches any
    /// blob. Once this succeeds, restoration refuses the document even if
    /// blob deletion or a process restart delays completion of the purge.
    pub async fn claim_document_purge(&self, document_id: Uuid) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
        let claimed = sqlx::query(
            "UPDATE documents SET status='purging',updated_at=now()
             WHERE id=$1 AND status='deleting'
               AND deleted_at + $2::interval <= now()",
        )
        .bind(document_id)
        .bind(
            sqlx::postgres::types::PgInterval::try_from(
                crate::storage::maintenance::DELETION_GRACE,
            )
            .expect("seven days fits in an interval"),
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(claimed)
    }

    /// Purge a deleted document now instead of when its seven days are up.
    /// "Delete forever" in the trash: backdating `deleted_at` past the grace
    /// period is what makes the next sweep pick it up.
    pub async fn hasten_deletion(&self, document_id: Uuid) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
        let changed = sqlx::query!(
            "UPDATE documents SET deleted_at=now() - $2::interval,updated_at=now()
             WHERE id=$1 AND status='deleting'",
            document_id,
            // `query!` resolves an `interval` parameter to `PgInterval`, not
            // to `time::Duration` (recent `time` releases alias `Duration`
            // to `SignedDuration`, which sqlx does not pick as the default
            // encoding for this oid), so the conversion has to be explicit.
            // `DELETION_GRACE * 2` is a fixed fourteen days, nowhere near
            // the range `PgInterval` can reject, so the conversion is
            // infallible in practice.
            sqlx::postgres::types::PgInterval::try_from(
                crate::storage::maintenance::DELETION_GRACE * 2,
            )
            .expect("fourteen days fits in an interval"),
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn finish_document_deletion(&self, document_id: Uuid) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
        let changed = sqlx::query("DELETE FROM documents WHERE id=$1 AND status='purging'")
            .bind(document_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn usage_bytes(&self, account_id: Option<Uuid>) -> Result<i64> {
        let Some(account_id) = account_id else {
            return sqlx::query_scalar!(
                r#"SELECT (
                     (SELECT bytes FROM storage_usage WHERE singleton) +
                     COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL), 0) +
                     COALESCE((SELECT sum(uncompacted_update_bytes)::bigint FROM documents), 0)
                   )::bigint AS "total!""#
            )
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

/// Every byte an account is charged for: label archives, document assets,
/// and each document's editing log (base snapshot plus uncompacted rows).
/// This is the single definition of what counts toward the owner quota -- it
/// was copied into three call sites before, keyed two different ways, so a
/// new kind of billable byte had to be remembered in three places to be
/// counted in any of them.
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
             SELECT l.archive_bytes AS bytes FROM (
               SELECT DISTINCT ON (l.archive_key) l.archive_key, l.archive_bytes
               FROM document_labels l
               JOIN documents d ON d.id=l.document_id
               WHERE d.owner_id=$1 AND l.archive_key IS NOT NULL
               ORDER BY l.archive_key, l.id
             ) l
             UNION ALL SELECT a.byte_length FROM document_assets a
               JOIN documents d ON d.id=a.document_id WHERE d.owner_id=$1
             UNION ALL SELECT s.snapshot_bytes FROM document_snapshots s
               JOIN documents d ON d.id=s.document_id
               WHERE d.owner_id=$1 AND s.delete_after IS NULL
             UNION ALL SELECT d.uncompacted_update_bytes FROM documents d
               WHERE d.owner_id=$1
           ) usage"#,
        account_id,
    )
    .fetch_one(executor)
    .await
    .map_err(Error::from)
}

impl PostgresCatalog {
    /// Every blob key one document's assets name, for the purge. Assets are
    /// content-addressed and unique per document, so each key here is this
    /// document's alone.
    pub async fn document_asset_keys(&self, document_id: Uuid) -> Result<Vec<String>> {
        sqlx::query_scalar!(
            "SELECT storage_key FROM document_assets WHERE document_id=$1",
            document_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }
}

#[derive(Clone, Debug)]
pub struct DocumentStorage {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
    pub figure_bytes: i64,
    pub archive_bytes: i64,
    pub history_bytes: i64,
}

impl PostgresCatalog {
    pub async fn document_storage_by_owner(&self, owner_id: Uuid) -> Result<Vec<DocumentStorage>> {
        sqlx::query_as!(
            DocumentStorage,
            r#"SELECT
                 d.id,
                 d.slug,
                 d.title,
                 COALESCE(SUM(a.byte_length), 0)::bigint AS "figure_bytes!",
                 COALESCE((
                   SELECT COALESCE(SUM(archive_bytes), 0)::bigint
                   FROM (
                     SELECT DISTINCT ON (archive_key) archive_bytes
                     FROM document_labels
                     WHERE document_id = d.id AND archive_key IS NOT NULL
                   ) DISTINCT_labels
                 ), 0)::bigint AS "archive_bytes!",
                 (COALESCE(
                   (SELECT snapshot_bytes FROM document_snapshots WHERE document_id = d.id AND delete_after IS NULL),
                   0
                 ) + d.uncompacted_update_bytes)::bigint AS "history_bytes!"
               FROM documents d
               LEFT JOIN document_assets a ON a.document_id = d.id
               WHERE d.owner_id = $1 AND d.status = 'active'
               GROUP BY d.id, d.slug, d.title, d.uncompacted_update_bytes
               ORDER BY (
                 COALESCE(SUM(a.byte_length), 0) +
                 COALESCE((
                   SELECT COALESCE(SUM(archive_bytes), 0)
                   FROM (
                     SELECT DISTINCT ON (archive_key) archive_bytes
                     FROM document_labels
                     WHERE document_id = d.id AND archive_key IS NOT NULL
                   ) DISTINCT_labels
                 ), 0) +
                 COALESCE((SELECT snapshot_bytes FROM document_snapshots WHERE document_id = d.id AND delete_after IS NULL), 0) +
                 d.uncompacted_update_bytes
               ) DESC, d.title ASC"#,
            owner_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }
}
