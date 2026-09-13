//! Account identity, preferences, bounded usage queries, and erasure.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ERASURE_BATCH: u32 = 250;

fn update_erasure_progress(
    tx: &Transaction<'_>,
    account_id: &str,
    stage: &str,
    cursor: Option<&str>,
    updated_at: i64,
) -> CatalogResult<()> {
    if !matches!(
        stage,
        "owned_documents"
            | "operations"
            | "operations_owned_documents"
            | "grants"
            | "bookmarks"
            | "annotation_replies"
            | "annotations"
            | "replies"
            | "checkpoints"
    ) {
        return Err(CatalogError::Invalid("unknown erasure stage".into()));
    }
    let (raw, operation_generation, current_generation): (String, String, String) = tx
        .query_row(
            "SELECT o.plan_json, o.writer_generation, s.writer_generation
             FROM operations o CROSS JOIN server_state s
             WHERE o.account_id=?1 AND o.kind='erase_account' AND o.state='prepared' AND s.id=1",
            [account_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(CatalogError::from)?;
    if operation_generation != current_generation {
        return Err(CatalogError::Conflict(
            "account erasure writer generation changed".into(),
        ));
    }
    let mut plan: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| CatalogError::Invalid(format!("invalid erasure plan: {error}")))?;
    if !plan.is_object() || plan.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(CatalogError::Invalid(
            "unsupported erasure plan version".into(),
        ));
    }
    plan["stage"] = serde_json::Value::String(stage.to_owned());
    plan["cursor"] = match cursor {
        Some(value) => serde_json::from_str(value)
            .map_err(|_| CatalogError::Invalid("invalid erasure cursor".into()))?,
        None => serde_json::Value::Null,
    };
    let encoded = serde_json::to_string(&plan)
        .map_err(|error| CatalogError::Invalid(format!("cannot encode erasure plan: {error}")))?;
    if encoded.len() > 65_536 {
        return Err(CatalogError::Invalid(
            "erasure plan exceeds its size limit".into(),
        ));
    }
    let changed = tx
        .execute(
            "UPDATE operations SET plan_json=?2,updated_at=max(updated_at,?3)
             WHERE account_id=?1 AND kind='erase_account' AND state='prepared'
               AND writer_generation=?4",
            params![account_id, encoded, updated_at, current_generation],
        )
        .map_err(CatalogError::from)?;
    if changed != 1 {
        return Err(CatalogError::Conflict(
            "account erasure operation is missing".into(),
        ));
    }
    Ok(())
}

impl Catalog {
    pub fn account_open_annotation_references(
        &self,
        account_id: &str,
    ) -> CatalogResult<BTreeMap<String, BTreeSet<String>>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT d.slug,a.protected_checkpoint_id
                    FROM annotations a
                    JOIN documents d ON d.id=a.document_id
                    WHERE d.owner_id=?1
                    AND d.status='active'
                    AND a.resolved_at IS NULL
                    AND a.protected_checkpoint_id IS NOT NULL
                    ORDER BY d.slug,a.protected_checkpoint_id",
            )?;
            let mut references = BTreeMap::<String, BTreeSet<String>>::new();
            let rows = statement.query_map([account_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (slug, event) = row?;
                references.entry(slug).or_default().insert(event);
            }
            Ok(references)
        })
    }

    /// An advisory account settings view. Mutation admission never enumerates
    /// these rows; it uses the maintained account and deployment counters.
    pub fn account_checkpoints(&self, account_id: &str) -> CatalogResult<Vec<Checkpoint>> {
        self.with_connection(|connection| {
            let mut statement=connection.prepare("SELECT d.slug,c.id,c.seq,c.journal_sequence,c.tree_digest,COALESCE(c.parent_id,''),c.created_at,c.author_label,c.author_account_id,c.reason,c.source_format,c.logical_bytes,COALESCE(c.label,''),c.metadata_json
                    FROM checkpoints c
                    JOIN documents d ON d.id=c.document_id
                    WHERE d.owner_id=?1
                    AND d.status='active'
                    ORDER BY d.slug,c.seq")?;
            let rows=statement.query_map([account_id],|row| {
                let raw:String=row.get(13)?;
                let metadata:serde_json::Value=serde_json::from_str(&raw).map_err(|error|rusqlite::Error::FromSqlConversionFailure(13,rusqlite::types::Type::Text,Box::new(error)))?;
                Ok(Checkpoint {slug:row.get(0)?,sha:row.get(1)?,seq:row.get(2)?,durable_seq:row.get(3)?,tree_sha:row.get(4)?,parent:row.get(5)?,at:crate::util::format_unix(row.get::<_,i64>(6)?/1000),by:row.get(7)?,by_account:row.get(8)?,why:row.get(9)?,source_format:row.get(10)?,size:row.get(11)?,label:row.get(12)?,git_commit:metadata["commit"].as_str().unwrap_or_default().to_owned(),dirty:metadata["dirty"].as_bool().unwrap_or(false),changed:metadata.get("changed").map(serde_json::Value::to_string)})
            })?;
            rows.collect::<Result<Vec<_>,_>>().map_err(CatalogError::from)
        })
    }

    pub fn account_storage_usage(&self, account_id: &str) -> CatalogResult<AccountStorageUsage> {
        self.with_connection(|connection| Self::account_storage_usage_on(connection, account_id))
    }
    pub(super) fn account_storage_usage_on(
        connection: &Connection,
        account_id: &str,
    ) -> CatalogResult<AccountStorageUsage> {
        connection
            .query_row(
                "SELECT stored_bytes+reserved_bytes,document_count,(SELECT count(*)
                    FROM checkpoints c
                    JOIN documents d ON d.id=c.document_id
                    WHERE d.owner_id=?1)
                    FROM accounts
                    WHERE id=?1",
                [account_id],
                |row| {
                    Ok(AccountStorageUsage {
                        charged_bytes: row.get(0)?,
                        document_count: row.get(1)?,
                        checkpoint_count: row.get(2)?,
                        physical_accounting: true,
                        live_bytes: 0,
                        history_bytes: 0,
                        asset_bytes: 0,
                        publication_bytes: 0,
                        metadata_bytes: 0,
                    })
                },
            )
            .optional()?
            .ok_or(CatalogError::NotFound)
    }
    pub(super) fn owner_admission_bytes_on(
        connection: &Connection,
        owner_id: Option<&str>,
        _owner_key: &str,
    ) -> CatalogResult<(i64, bool)> {
        let owner = owner_id
            .ok_or_else(|| CatalogError::Invalid("every v2 document requires an account".into()))?;
        Ok((
            connection.query_row(
                "SELECT stored_bytes+reserved_bytes
                    FROM accounts
                    WHERE id=?1",
                [owner],
                |row| row.get(0),
            )?,
            true,
        ))
    }
    pub(super) fn deployment_admission_bytes_on(
        connection: &Connection,
    ) -> CatalogResult<(i64, bool)> {
        Ok((
            connection.query_row(
                "SELECT stored_bytes+reserved_bytes
                    FROM server_state
                    WHERE id=1",
                [],
                |row| row.get(0),
            )?,
            true,
        ))
    }
    /// Advisory remaining allocation headroom. Existing immutable bytes stay
    /// charged throughout a replacement and cannot be subtracted prospectively.
    pub fn physical_room_for(
        &self,
        slug: &str,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<Option<i64>> {
        self.with_connection(|connection| {
            let values: Option<(i64, i64)> = connection
                .query_row(
                    "SELECT a.stored_bytes+a.reserved_bytes,s.stored_bytes+s.reserved_bytes
                    FROM documents d
                    JOIN accounts a ON a.id=d.owner_id
                    CROSS JOIN server_state s
                    WHERE d.slug=?1
                    AND d.status='active'",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            Ok(values.map(|(owner, total)| {
                let owner_limit = if owner_limit < 0 {
                    i64::MAX
                } else {
                    owner_limit
                };
                let global = if total_limit < 0 {
                    i64::MAX
                } else {
                    total_limit.saturating_sub(total)
                };
                global.min(owner_limit.saturating_sub(owner)).max(0)
            }))
        })
    }
    pub(super) fn enforce_physical_quota_on(
        connection: &Connection,
        slug: &str,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<()> {
        let (owner_bytes, global_bytes): (i64, i64) = connection.query_row(
            "SELECT a.stored_bytes+a.reserved_bytes,s.stored_bytes+s.reserved_bytes
                    FROM documents d
                    JOIN accounts a ON a.id=d.owner_id
                    CROSS JOIN server_state s
                    WHERE d.slug=?1",
            [slug],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if owner_limit >= 0 && owner_bytes > owner_limit {
            return Err(CatalogError::refused(
                CatalogRefusal::OwnerBytes,
                "owner storage quota exceeded",
            ));
        }
        if total_limit >= 0 && global_bytes > total_limit {
            return Err(CatalogError::refused(
                CatalogRefusal::DeploymentBytes,
                "deployment storage quota exceeded",
            ));
        }
        Ok(())
    }

    /// Advisory bytes that would lose their final checkpoint reference. This
    /// is never a quota credit: only confirmed physical deletion releases bytes.
    pub fn reclaimable_checkpoint_bytes(
        &self,
        candidates: &[(String, String)],
    ) -> CatalogResult<i64> {
        self.with_connection(|connection| {
            Self::reclaimable_checkpoint_bytes_on(connection, candidates)
        })
    }
    pub(super) fn reclaimable_checkpoint_bytes_on(
        connection: &Connection,
        candidates: &[(String, String)],
    ) -> CatalogResult<i64> {
        if candidates.len() > 128 {
            return Err(CatalogError::Invalid(
                "checkpoint deletion selection exceeds 128".into(),
            ));
        }
        let wanted = serde_json::to_string(candidates)
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        let prefix="WITH wanted AS (SELECT d.id AS document_id,json_extract(w.value,'$[1]') AS checkpoint_id
                    FROM json_each(?1) w
                    JOIN documents d ON d.slug=json_extract(w.value,'$[0]'))";
        let edges:i64=connection.query_row(&format!("{prefix} SELECT count(*) FROM checkpoint_objects co JOIN wanted w USING(document_id,checkpoint_id)"),[&wanted],|row|row.get(0))?;
        if edges > 32768 {
            return Err(CatalogError::Invalid(
                "checkpoint deletion selection exceeds 32768 closure references".into(),
            ));
        }
        connection.query_row(&format!("{prefix} SELECT COALESCE(SUM(o.byte_length),0) FROM objects o WHERE o.state='available' AND o.live_root=0 AND o.publication_root=0 AND EXISTS(SELECT 1 FROM checkpoint_objects co JOIN wanted w USING(document_id,checkpoint_id) WHERE co.document_id=o.document_id AND co.object_id=o.id) AND NOT EXISTS(SELECT 1 FROM checkpoint_objects co WHERE co.document_id=o.document_id AND co.object_id=o.id AND NOT EXISTS(SELECT 1 FROM wanted w WHERE w.document_id=co.document_id AND w.checkpoint_id=co.checkpoint_id)) AND NOT EXISTS(SELECT 1 FROM object_leases l WHERE l.document_id=o.document_id AND l.object_id=o.id) AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.id=o.document_id AND (d.journal_base_object_id=o.id OR d.publication_object_id=o.id))"),[wanted],|row|row.get(0)).map_err(CatalogError::from)
    }
    /// Read the account owner's saved quota intent.  Missing preferences are
    /// deliberately distinct from a malformed payload: callers can expose a
    /// safe read-only state rather than authorizing destructive thinning.
    pub fn quota_preferences(
        &self,
        account_id: &str,
    ) -> CatalogResult<Option<QuotaPreferencesRecord>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, preferences_revision, preferences_json, created_at, last_seen_at

                    FROM accounts
                    WHERE id=?1",
                    [account_id],
                    |row| {
                        Ok(QuotaPreferencesRecord {
                            account_id: row.get(0)?,
                            revision: row.get(1)?,
                            payload: row.get(2)?,
                            policy_generation: String::new(),
                            updated_at: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    /// Save account quota intent with optimistic concurrency.  `expected` is
    /// zero for an initial insert; a non-zero value must equal the persisted
    /// revision.  The generation is an immutable operation identity used by
    /// preview/apply and thinning workers.
    pub fn save_quota_preferences(
        &self,
        account_id: &str,
        expected: i64,
        payload: &str,
        policy_generation: &str,
        updated_at: i64,
    ) -> CatalogResult<QuotaPreferencesRecord> {
        if account_id.is_empty() || payload.is_empty() || payload.len() > 65_536 || updated_at < 0 {
            return Err(CatalogError::Invalid(
                "invalid quota preference record".into(),
            ));
        }
        let preferences: crate::document::quota::QuotaPreferences =
            serde_json::from_str(payload)
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        preferences.validate().map_err(CatalogError::Invalid)?;
        self.immediate(|tx| {
            let current: Option<i64> = tx
                .query_row(
                    "SELECT preferences_revision
                    FROM accounts
                    WHERE id=?1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            match (current, expected) {
                (Some(revision), expected) if revision != expected => {
                    return Err(CatalogError::Conflict(
                        "quota preference revision is stale".into(),
                    ))
                }
                (None, expected) if expected != 0 => {
                    return Err(CatalogError::Conflict(
                        "quota preference revision is stale".into(),
                    ))
                }
                _ => {}
            }
            let revision = current
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| CatalogError::Invalid("preference revision overflow".into()))?;
            let account_active: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1
                    FROM accounts
                    WHERE id=?1
                    AND status='active')",
                    [account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !account_active {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE accounts
                    SET preferences_revision=?2, preferences_json=?3,
                    last_seen_at=max(last_seen_at,?4)
                    WHERE id=?1",
                params![account_id, revision, payload, updated_at],
            )
            .map_err(CatalogError::from)?;
            // Mark owned documents due for bounded policy evaluation. Their
            // previous evaluation revision no longer authorizes deletion.
            tx.execute(
                "UPDATE documents
                    SET retention_due_at=0
                    WHERE owner_id=?1
                    AND status='active'",
                [account_id],
            )?;
            tx.execute(
                "UPDATE server_state
                    SET catalog_revision=catalog_revision+1,updated_at=max(updated_at,?1)
                    WHERE id=1",
                [updated_at],
            )?;
            Ok(QuotaPreferencesRecord {
                account_id: account_id.to_string(),
                revision,
                payload: payload.to_string(),
                policy_generation: policy_generation.to_string(),
                updated_at,
            })
        })
    }
    /// Refresh display metadata while preserving session revocation and policy.
    pub fn upsert_account(&self, profile: &Account) -> CatalogResult<Account> {
        if profile.id.is_empty() || profile.session_generation.is_empty() {
            return Err(CatalogError::Invalid(
                "account identity and session generation are required".into(),
            ));
        }
        let provider = (!profile.provider.is_empty()).then_some(profile.provider.as_str());
        let prefix = format!("{}:", profile.provider);
        let subject = provider.map(|_| profile.id.strip_prefix(&prefix).unwrap_or(&profile.id));
        self.immediate(|tx| {
            let existing:Option<(String,Option<String>,Option<String>)>=tx.query_row("SELECT status,provider,provider_subject
                    FROM accounts
                    WHERE id=?1",[&profile.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let now=unix_millis();
            if let Some((status,old_provider,old_subject))=existing {
                if status!="active" || old_provider.as_deref()!=provider || old_subject.as_deref()!=subject {return Err(CatalogError::Conflict("account lifecycle or provider identity changed".into()));}
                tx.execute("UPDATE accounts
                    SET handle=?2,display_name=?3,email=?4,last_seen_at=max(last_seen_at,?5)
                    WHERE id=?1",params![profile.id,profile.handle,profile.name,(!profile.email.is_empty()).then_some(&profile.email),now])?;
            } else {
                let items:Vec<_>=(0..crate::seed::ACCOUNT_EXAMPLE_COUNT).map(|position|serde_json::json!({"position":position,"slug":format!("starter-{}",crate::util::new_id()),"completed":false})).collect();
                let onboarding=serde_json::json!({"version":1,"items":items}).to_string();
                let preferences=serde_json::to_string(&crate::document::quota::QuotaPreferences::default()).map_err(|error|CatalogError::Invalid(error.to_string()))?;
                tx.execute("INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,plan,status,session_generation,created_at,last_seen_at,preferences_json,onboarding_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'active',?9,?10,?10,?11,?12)",params![profile.id,if provider.is_some(){"registered"}else{"anonymous"},provider,subject,profile.handle,profile.name,(!profile.email.is_empty()).then_some(&profile.email),profile.plan,profile.session_generation,now,preferences,onboarding])?;
            }
            tx.execute("UPDATE server_state
                    SET catalog_revision=catalog_revision+1,updated_at=max(updated_at,?1)
                    WHERE id=1",[now])?;
            self.account_in_tx(tx,&profile.id,None)
        })
    }

    pub fn account(&self, id: &str) -> CatalogResult<Option<Account>> {
        self.with_connection(|connection| {
            Self::account_on(connection, id).map_err(CatalogError::from)
        })
    }
    pub fn pending_account_examples(&self, id: &str) -> CatalogResult<Vec<(usize, String)>> {
        self.with_connection(|connection| {
            let payload: String = connection.query_row(
                "SELECT onboarding_json
                    FROM accounts
                    WHERE id=?1
                    AND status='active'",
                [id],
                |row| row.get(0),
            )?;
            let onboarding = decode_onboarding(&payload)?;
            Ok(onboarding
                .items
                .into_iter()
                .filter(|item| !item.completed)
                .map(|item| (item.position, item.slug))
                .collect())
        })
    }
    pub fn complete_account_example(&self, id: &str, position: usize) -> CatalogResult<()> {
        self.immediate(|tx| {
            let payload: String = tx.query_row(
                "SELECT onboarding_json
                    FROM accounts
                    WHERE id=?1
                    AND status='active'",
                [id],
                |row| row.get(0),
            )?;
            let mut onboarding = decode_onboarding(&payload)?;
            let item = onboarding
                .items
                .iter_mut()
                .find(|item| item.position == position)
                .ok_or(CatalogError::NotFound)?;
            item.completed = true;
            let payload = serde_json::to_string(&onboarding)
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
            tx.execute(
                "UPDATE accounts
                    SET onboarding_json=?2
                    WHERE id=?1",
                params![id, payload],
            )?;
            tx.execute(
                "UPDATE server_state
                    SET catalog_revision=catalog_revision+1
                    WHERE id=1",
                [],
            )?;
            Ok(())
        })
    }

    pub(super) fn account_in_tx(
        &self,
        tx: &Transaction<'_>,
        id: &str,
        _existing_generation: Option<String>,
    ) -> CatalogResult<Account> {
        tx.query_row(
            "SELECT id, COALESCE(provider,''), handle, display_name, COALESCE(email,''),
                    created_at, last_seen_at, plan, status, session_generation, NULL

                    FROM accounts
                    WHERE id = ?1",
            [id],
            Self::read_account,
        )
        .map_err(CatalogError::from)
    }

    pub(super) fn account_on(
        connection: &Connection,
        id: &str,
    ) -> rusqlite::Result<Option<Account>> {
        connection
            .query_row(
                "SELECT id, COALESCE(provider,''), handle, display_name, COALESCE(email,''),
                        created_at, last_seen_at, plan, status, session_generation, NULL

                    FROM accounts
                    WHERE id = ?1",
                [id],
                Self::read_account,
            )
            .optional()
    }

    pub(super) fn read_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
        Ok(Account {
            id: row.get(0)?,
            provider: row.get(1)?,
            handle: row.get(2)?,
            name: row.get(3)?,
            email: row.get(4)?,
            first_seen: crate::util::format_unix(row.get::<_, i64>(5)? / 1000),
            last_seen: crate::util::format_unix(row.get::<_, i64>(6)? / 1000),
            plan: row.get(7)?,
            status: row.get(8)?,
            session_generation: row.get(9)?,
            erasure_cursor: row.get(10)?,
        })
    }

    /// Change a session generation in the same authoritative transaction that
    /// marks revocation.  Returns the new generation for cookie invalidation.
    pub fn revoke_sessions(&self, id: &str, new_generation: &str) -> CatalogResult<String> {
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE accounts
                    SET session_generation = ?2
                    WHERE id = ?1
                    AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            Ok(new_generation.to_owned())
        })
    }

    /// Mark an account erasing and revoke all sessions atomically.
    pub fn begin_erasure(&self, id: &str, new_generation: &str) -> CatalogResult<()> {
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id=?1", [id], |row| row.get(0))
                .optional()
                .map_err(CatalogError::from)?;
            match status.as_deref() {
                Some("erasing") => {
                    let prepared: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM operations WHERE account_id=?1 AND kind='erase_account' AND state='prepared'",
                        [id], |row| row.get(0))?;
                    if prepared == 1 {
                        return Ok(());
                    }
                    return Err(CatalogError::Conflict("account erasure has no recoverable operation".into()));
                }
                Some("active") => {}
                Some(_) => return Err(CatalogError::Conflict("account cannot be erased in its current state".into())),
                None => return Err(CatalogError::NotFound),
            }
            tx.execute(
                "UPDATE accounts SET status='erasing', session_generation=?2 WHERE id=?1 AND status='active'",
                params![id, new_generation],
            ).map_err(CatalogError::from)?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let request_digest = hex::encode(Sha256::digest(id.as_bytes()));
            let request_key = format!("erase-account:{}", &request_digest[..32]);
            let plan = r#"{"version":1,"stage":"owned_documents","cursor":null}"#;
            let writer_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0))?;
            let now = super::unix_millis();
            tx.execute(
                "INSERT INTO operations
                    (id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,plan_json,created_at,updated_at)
                 VALUES(?1,?2,'system:erasure',?3,'erase_account',?4,'prepared',?5,?6,?7,?7)",
                params![operation_id, id, request_key, request_digest, writer_generation, plan, now],
            ).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Record qualifying authenticated activity.  Activity is intentionally
    /// monotonic: an imported older timestamp cannot make an account look
    /// recently active, and repeated requests on one UTC day are a no-op.
    pub fn record_activity(&self, id: &str, at: &str) -> CatalogResult<()> {
        let seconds = crate::util::parse_timestamp(at)
            .ok_or_else(|| CatalogError::Invalid("invalid activity timestamp".into()))?;
        let at = seconds
            .checked_mul(1000)
            .filter(|value| *value >= 0)
            .ok_or_else(|| CatalogError::Invalid("activity timestamp overflow".into()))?;
        self.immediate(|tx| {
            let changed = tx.execute(
                "UPDATE accounts
                    SET last_active_at=max(COALESCE(last_active_at,0),?2)
                    WHERE id=?1
                    AND status='active'",
                params![id, at],
            )?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Ok(())
        })
    }

    /// Persist one bounded erasure cursor.  A worker may safely repeat a
    /// batch after a crash because the cursor update is in the same tx as the
    /// deletions performed by the caller through `with_erasure_batch`.
    pub fn erasure_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if stage.is_empty() || stage.len() > 64 || limit == 0 {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: String = tx
                .query_row(
                    "SELECT status
                    FROM accounts
                    WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if status != "erasing" {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            update_erasure_progress(&tx, id, stage, cursor, updated_at)?;
            Ok(limit.min(MAX_ERASURE_BATCH))
        })
    }

    /// Apply one resumable logical-erasure batch.  Physical document cleanup
    /// remains owned by `begin_delete`/`finish_delete`; this method handles
    /// account references in documents that belong to other users.
    pub fn erase_account_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if limit == 0 || stage.is_empty() {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row(
                    "SELECT status
                    FROM accounts
                    WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            // These tables deliberately use WITHOUT ROWID primary keys in
            // the catalogue contract.  Advance by immutable primary-key
            // tuples rather than SQLite's hidden rowid: the cursor remains
            // valid across vacuum/backup/restore and a crash can repeat only
            // an already committed keyset batch.
            let cursor_parts = cursor
                .map(|value| {
                    serde_json::from_str::<Vec<String>>(value)
                        .map_err(|_| CatalogError::Invalid("invalid erasure cursor".into()))
                })
                .transpose()?;
            // The return value is the number of physical catalogue rows
            // changed in this transaction.  It is deliberately separate
            // from cursor progress: a batch may inspect a pinned/terminal
            // operation or an already-withdrawn document without changing a
            // row, while callers still need an exact <=250 work budget.
            let n_and_cursor = match stage {
                "owned_documents" => {
                    let after_document = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 1 {
                                return Err(CatalogError::Invalid(
                                    "invalid owned-document cursor".into(),
                                ));
                            }
                            Ok(parts[0].as_str())
                        })
                        .transpose()?
                        .unwrap_or("");
                    let mut rows = tx
                        .prepare(
                            "SELECT id FROM documents
                             WHERE owner_id=?1 AND status IN ('active','creating') AND id>?2
                             ORDER BY id LIMIT ?3",
                        )?
                        .query_map(
                            params![id, after_document, i64::from(limit.min(MAX_ERASURE_BATCH))],
                            |row| row.get::<_, String>(0),
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let last = rows.last().cloned();
                    let mut changed = 0u32;
                    for document_id in rows.drain(..) {
                        let updated = tx.execute(
                            "UPDATE documents SET status='deleting', publication_id=NULL,
                             publication_object_id=NULL, published_at=NULL
                             WHERE id=?1 AND owner_id=?2 AND status IN ('active','creating')",
                            params![document_id, id],
                        )?;
                        changed = changed.saturating_add(updated as u32);
                        if updated == 0 {
                            continue;
                        }
                        // The v2 deletion worker settles this durable
                        // erase_document receipt after object/journal
                        // reclamation. Keep it distinct from ordinary
                        // account-owned operations so the account stage
                        // cannot abort the physical teardown request.
                        let existing: Option<String> = tx
                            .query_row(
                                "SELECT id FROM operations
                                 WHERE document_id=?1 AND kind='erase_document'
                                 ORDER BY state='prepared' DESC, id LIMIT 1",
                                [&document_id],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(CatalogError::from)?;
                        if existing.is_none() {
                            let digest = Sha256::digest(document_id.as_bytes());
                            let digest_hex = hex::encode(digest);
                            let request_key = format!("erase-document:{}", &digest_hex[..32]);
                            let operation_id = hex::encode(crate::auth::random_bytes(16));
                            let writer_generation: String = tx.query_row(
                                "SELECT writer_generation FROM server_state WHERE id=1",
                                [],
                                |row| row.get(0),
                            )?;
                            let now = super::unix_millis();
                            let plan = serde_json::json!({
                                "version": 1,
                                "reason": "account_erasure",
                                "document_id": document_id,
                            })
                            .to_string();
                            tx.execute(
                                "INSERT INTO operations
                                 (id,document_id,actor_key,request_key,kind,request_digest,
                                  state,writer_generation,plan_json,created_at,updated_at)
                                 VALUES(?1,?2,'system:erasure',?3,'erase_document',?4,
                                        'prepared',?5,?6,?7,?7)",
                                params![
                                    operation_id,
                                    document_id,
                                    request_key,
                                    digest_hex,
                                    writer_generation,
                                    plan,
                                    now
                                ],
                            )?;
                        }
                    }
                    (
                        last.map(|document_id| serde_json::json!([document_id]).to_string()),
                        changed,
                    )
                }
                "grants" => {
                    let (after_document, after_account) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid grants cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT document_id, account_id
                    FROM grants

                    WHERE account_id=?1

                    AND (document_id>?2
                    OR (document_id=?2
                    AND account_id>?3))

                    ORDER BY document_id, account_id LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![
                                id,
                                after_document,
                                after_account,
                                i64::from(limit.min(MAX_ERASURE_BATCH))
                            ],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let mut changed = 0u32;
                    for (document_id, account_id) in rows.drain(..) {
                        changed = changed.saturating_add(
                            tx.execute(
                                "DELETE
                    FROM grants
                    WHERE document_id=?1
                    AND account_id=?2",
                                params![document_id, account_id],
                            )
                            .map_err(CatalogError::from)? as u32,
                        );
                    }
                    (
                        last.map(|(document_id, account_id)| {
                            serde_json::json!([document_id, account_id]).to_string()
                        }),
                        changed,
                    )
                }
                "bookmarks" => (None, 0),
                "annotation_replies" => {
                    let (after_document, after_annotation, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 3 {
                                return Err(CatalogError::Invalid(
                                    "invalid annotation-replies cursor".into(),
                                ));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str(), parts[2].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", "", ""));
                    // Parent annotation deletion is a cascading operation in
                    // v2. Drain every child reply first, by the same stable
                    // primary-key cursor, so one erasure transaction never
                    // removes more than its 250-row budget.
                    let mut rows = tx
                        .prepare(
                            "SELECT r.document_id, r.annotation_id, r.id
                             FROM replies r
                             JOIN annotations a
                               ON a.document_id=r.document_id AND a.id=r.annotation_id
                             WHERE a.author_account_id=?1
                               AND (r.document_id>?2
                                OR (r.document_id=?2 AND r.annotation_id>?3)
                                OR (r.document_id=?2 AND r.annotation_id=?3 AND r.id>?4))
                             ORDER BY r.document_id, r.annotation_id, r.id LIMIT ?5",
                        )?
                        .query_map(
                            params![
                                id,
                                after_document,
                                after_annotation,
                                after_id,
                                i64::from(limit.min(MAX_ERASURE_BATCH))
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let last = rows.last().cloned();
                    let mut changed = 0u32;
                    for (document_id, annotation_id, reply_id) in rows.drain(..) {
                        changed = changed.saturating_add(tx.execute(
                            "DELETE FROM replies
                                 WHERE document_id=?1 AND annotation_id=?2 AND id=?3",
                            params![document_id, annotation_id, reply_id],
                        )? as u32);
                    }
                    (
                        last.map(|(document, annotation, reply)| {
                            serde_json::json!([document, annotation, reply]).to_string()
                        }),
                        changed,
                    )
                }
                "annotations" => {
                    let (after_document, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid(
                                    "invalid annotations cursor".into(),
                                ));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT document_id, id
                    FROM annotations

                    WHERE author_account_id=?1

                    AND (document_id>?2
                    OR (document_id=?2
                    AND id>?3))

                    ORDER BY document_id, id LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![
                                id,
                                after_document,
                                after_id,
                                i64::from(limit.min(MAX_ERASURE_BATCH))
                            ],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let mut changed = 0u32;
                    let mut next_cursor = cursor.map(str::to_owned);
                    for (document_id, annotation_id) in rows.drain(..) {
                        let has_replies: bool = tx
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM replies
                                 WHERE document_id=?1 AND annotation_id=?2)",
                                params![document_id, annotation_id],
                                |row| row.get(0),
                            )
                            .map_err(CatalogError::from)?;
                        if has_replies {
                            if changed == 0 {
                                return Err(CatalogError::Conflict(
                                    "annotation gained a reply during account erasure".into(),
                                ));
                            }
                            break;
                        }
                        changed = changed.saturating_add(
                            tx.execute(
                                "DELETE FROM annotations
                                 WHERE document_id=?1 AND id=?2
                                   AND author_account_id=?3
                                   AND NOT EXISTS (SELECT 1 FROM replies
                                                   WHERE document_id=?1 AND annotation_id=?2)",
                                params![document_id, annotation_id, id],
                            )
                            .map_err(CatalogError::from)? as u32,
                        );
                        if changed != 0 {
                            next_cursor =
                                Some(serde_json::json!([document_id, annotation_id]).to_string());
                        }
                    }
                    (next_cursor, changed)
                }
                "replies" => {
                    let (after_document, after_annotation, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 3 {
                                return Err(CatalogError::Invalid("invalid replies cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str(), parts[2].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", "", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT document_id, annotation_id, id
                    FROM replies

                    WHERE author_account_id=?1

                    AND (document_id>?2
                    OR (document_id=?2
                    AND annotation_id>?3)

                    OR (document_id=?2
                    AND annotation_id=?3
                    AND id>?4))

                    ORDER BY document_id, annotation_id, id LIMIT ?5",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![
                                id,
                                after_document,
                                after_annotation,
                                after_id,
                                i64::from(limit.min(MAX_ERASURE_BATCH))
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let mut changed = 0u32;
                    for (document_id, annotation_id, reply_id) in rows.drain(..) {
                        changed = changed.saturating_add(
                            tx.execute(
                                "DELETE
                    FROM replies

                    WHERE document_id=?1
                    AND annotation_id=?2
                    AND id=?3
                    AND author_account_id=?4",
                                params![document_id, annotation_id, reply_id, id],
                            )
                            .map_err(CatalogError::from)? as u32,
                        );
                    }
                    (
                        last.map(|(document, annotation, reply)| {
                            serde_json::json!([document, annotation, reply]).to_string()
                        }),
                        changed,
                    )
                }
                "checkpoints" => {
                    let (after_document, after_checkpoint) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid(
                                    "invalid checkpoints cursor".into(),
                                ));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT document_id, id
                    FROM checkpoints

                    WHERE author_account_id=?1

                    AND (document_id>?2
                    OR (document_id=?2
                    AND id>?3))

                    ORDER BY document_id, id LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![
                                id,
                                after_document,
                                after_checkpoint,
                                i64::from(limit.min(MAX_ERASURE_BATCH))
                            ],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let mut changed = 0u32;
                    for (document_id, checkpoint_id) in rows.drain(..) {
                        changed = changed.saturating_add(
                            tx.execute(
                                "UPDATE checkpoints
                    SET author_label=?4, author_account_id=NULL

                    WHERE document_id=?1
                    AND id=?2
                    AND author_account_id=?3",
                                params![document_id, checkpoint_id, id, ERASED_ATTRIBUTION],
                            )
                            .map_err(CatalogError::from)? as u32,
                        );
                    }
                    (
                        last.map(|(document_id, checkpoint_id)| {
                            serde_json::json!([document_id, checkpoint_id]).to_string()
                        }),
                        changed,
                    )
                }
                "operations" => {
                    let after_id = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 1 {
                                return Err(CatalogError::Invalid(
                                    "invalid operations cursor".into(),
                                ));
                            }
                            Ok(parts[0].as_str())
                        })
                        .transpose()?
                        .unwrap_or("");
                    // Keep the three owner scopes separately indexable. A
                    // broad OR/EXISTS predicate scans unrelated operations
                    // and makes one account erasure an unbounded read.
                    let mut rows = tx
                        .prepare(
                            "SELECT id, state, kind FROM (
                               SELECT * FROM (
                                 SELECT o.id, o.state, o.kind
                                 FROM operations o
                                 WHERE o.account_id=?1 AND o.id>?2
                                 ORDER BY o.id LIMIT ?3
                               )
                               UNION
                               SELECT * FROM (
                                 SELECT o.id, o.state, o.kind
                                 FROM operations o
                                 WHERE o.actor_key IN (?1, 'account:' || ?1) AND o.id>?2
                                 ORDER BY o.id LIMIT ?3
                               )
                             )
                             WHERE kind NOT IN ('erase_account','erase_document')
                             ORDER BY id LIMIT ?3",
                        )?
                        .query_map(
                            params![id, after_id, i64::from(limit.min(MAX_ERASURE_BATCH))],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let mut progress = None;
                    let mut changed = 0u32;
                    let mut next_cursor = cursor.map(str::to_owned);
                    for (operation_id, state, _kind) in rows.drain(..) {
                        if state == "prepared" {
                            let now = super::unix_millis();
                            let receipt = now.saturating_add(7 * 24 * 60 * 60 * 1000);
                            changed = changed.saturating_add(tx.execute(
                                "UPDATE operations SET state='aborted', result_json=?2,
                                     completed_at=?3,
                                     receipt_expires_at=max(COALESCE(receipt_expires_at,0),?4),
                                     updated_at=max(updated_at,?3)
                                     WHERE id=?1 AND state='prepared'",
                                params![
                                    operation_id,
                                    r#"{"version":1,"reason":"account_erasure"}"#,
                                    now,
                                    receipt
                                ],
                            )? as u32);
                            // Keep the cursor before an aborted row. The
                            // next pass removes this terminal receipt, so a
                            // prepared operation cannot be skipped forever.
                            break;
                        }
                        let pinned: i64 = tx.query_row(
                            "SELECT
                               (SELECT COUNT(*) FROM objects WHERE allocation_operation_id=?1) +
                               (SELECT COUNT(*) FROM object_leases WHERE operation_id=?1)",
                            [&operation_id],
                            |row| row.get(0),
                        )?;
                        if pinned != 0 {
                            if changed == 0 {
                                return Err(CatalogError::Conflict(
                                    "account erasure operation is still physically pinned".into(),
                                ));
                            }
                            // Commit earlier rows and retry the pinned row
                            // from the same cursor on the next pass.
                            break;
                        }
                        changed = changed.saturating_add(tx.execute(
                            "DELETE FROM operations WHERE id=?1 AND state <> 'prepared'",
                            [&operation_id],
                        )? as u32);
                        progress = Some(operation_id);
                    }
                    if let Some(operation_id) = progress {
                        next_cursor = Some(serde_json::json!([operation_id]).to_string());
                    }
                    (next_cursor, changed)
                }
                "operations_owned_documents" => (|| -> CatalogResult<(Option<String>, u32)> {
                    let (after_document, after_operation) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid(
                                    "invalid owned-document operations cursor".into(),
                                ));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let document_id: Option<String> = if cursor_parts.is_none() {
                        tx.query_row(
                            "SELECT id FROM documents
                             WHERE owner_id=?1
                             ORDER BY id LIMIT 1",
                            [id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(CatalogError::from)?
                    } else {
                        Some(after_document.to_owned())
                    };
                    let Some(document_id) = document_id else {
                        return Ok((None, 0));
                    };
                    let mut rows = tx
                        .prepare(
                            "SELECT id, state, kind FROM operations
                             WHERE document_id=?1 AND id>?2
                               AND kind NOT IN ('erase_account','erase_document')
                             ORDER BY id LIMIT ?3",
                        )?
                        .query_map(
                            params![
                                document_id,
                                after_operation,
                                i64::from(limit.min(MAX_ERASURE_BATCH))
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    if rows.is_empty() {
                        let next_document: Option<String> = tx
                            .query_row(
                                "SELECT id FROM documents
                                 WHERE owner_id=?1 AND id>?2
                                 ORDER BY id LIMIT 1",
                                params![id, document_id],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(CatalogError::from)?;
                        return Ok((
                            next_document
                                .map(|document| serde_json::json!([document, ""]).to_string()),
                            0,
                        ));
                    }
                    let mut progress = None;
                    let mut changed = 0u32;
                    for (operation_id, state, _kind) in rows.drain(..) {
                        if state == "prepared" {
                            let now = super::unix_millis();
                            let receipt = now.saturating_add(7 * 24 * 60 * 60 * 1000);
                            changed = changed.saturating_add(tx.execute(
                                "UPDATE operations SET state='aborted', result_json=?2,
                                     completed_at=?3,
                                     receipt_expires_at=max(COALESCE(receipt_expires_at,0),?4),
                                     updated_at=max(updated_at,?3)
                                     WHERE id=?1 AND state='prepared'",
                                params![
                                    operation_id,
                                    r#"{"version":1,"reason":"account_erasure"}"#,
                                    now,
                                    receipt
                                ],
                            )? as u32);
                            break;
                        }
                        let pinned: i64 = tx.query_row(
                            "SELECT
                               (SELECT COUNT(*) FROM objects WHERE allocation_operation_id=?1) +
                               (SELECT COUNT(*) FROM object_leases WHERE operation_id=?1)",
                            [&operation_id],
                            |row| row.get(0),
                        )?;
                        if pinned != 0 {
                            if changed == 0 {
                                return Err(CatalogError::Conflict(
                                    "account erasure operation is still physically pinned".into(),
                                ));
                            }
                            break;
                        }
                        changed = changed.saturating_add(tx.execute(
                            "DELETE FROM operations WHERE id=?1 AND state <> 'prepared'",
                            [&operation_id],
                        )? as u32);
                        progress = Some(operation_id);
                    }
                    let next_cursor = progress
                        .map(|operation| serde_json::json!([document_id, operation]).to_string())
                        .or_else(|| {
                            Some(serde_json::json!([document_id, after_operation]).to_string())
                        });
                    Ok((next_cursor, changed))
                })()?,
                _ => return Err(CatalogError::Invalid("unknown erasure stage".into())),
            };
            let n = n_and_cursor.1;
            let next_cursor = n_and_cursor.0;
            update_erasure_progress(&tx, id, stage, next_cursor.as_deref(), updated_at)?;
            Ok(n)
        })
    }

    pub fn finish_erasure(&self, id: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row(
                    "SELECT status
                    FROM accounts
                    WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::NotFound);
            }
            let owned: i64 = tx
                .query_row(
                    "SELECT COUNT(*)
                    FROM documents
                    WHERE owner_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            if owned != 0 {
                return Err(CatalogError::Conflict("owned documents remain".into()));
            }
            let charges: (i64, i64, i64) = tx.query_row(
                "SELECT stored_bytes, reserved_bytes, document_count FROM accounts WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            if charges != (0, 0, 0) {
                return Err(CatalogError::Conflict("account charges remain".into()));
            }
            let references: i64 = tx
                .query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM grants WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM annotations WHERE author_account_id=?1) +
                       (SELECT COUNT(*) FROM replies WHERE author_account_id=?1) +
                       (SELECT COUNT(*) FROM checkpoints WHERE author_account_id=?1) +
                       (SELECT COUNT(*) FROM operations
                        WHERE account_id=?1 AND kind <> 'erase_account') +
                       (SELECT COUNT(*) FROM operations
                        WHERE actor_key IN (?1, 'account:' || ?1)
                          AND kind <> 'erase_account')",
                    [id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if references != 0 {
                return Err(CatalogError::Conflict("account attribution remains".into()));
            }
            let (operation_id, operation_generation, current_generation): (String, String, String) =
                tx.query_row(
                    "SELECT o.id, o.writer_generation, s.writer_generation
                     FROM operations o CROSS JOIN server_state s
                     WHERE o.account_id=?1 AND o.kind='erase_account'
                       AND o.state='prepared' AND s.id=1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
                .ok_or_else(|| {
                    CatalogError::Conflict("account erasure operation is missing".into())
                })?;
            if operation_generation != current_generation {
                return Err(CatalogError::Conflict(
                    "account erasure writer generation changed".into(),
                ));
            }
            let now = super::unix_millis();
            tx.execute(
                "UPDATE operations SET state='committed', result_json=?2,
                    completed_at=?3, receipt_expires_at=?3, updated_at=max(updated_at,?3)
                 WHERE id=?1 AND account_id=?4 AND kind='erase_account' AND state='prepared'",
                params![operation_id, r#"{"version":1,"status":"erased"}"#, now, id],
            )?;
            // The account row owns this receipt through its foreign key. It
            // must be removed explicitly before deleting the account. All
            // other account/actor receipts were required to be drained by
            // the bounded operations stage above; do not hide leftovers with
            // an unbounded final DELETE.
            tx.execute(
                "DELETE FROM operations WHERE id=?1 AND state='committed'",
                [&operation_id],
            )?;
            tx.execute(
                "DELETE
                    FROM accounts
                    WHERE id = ?1",
                [id],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Accounts awaiting the bounded erasure worker, ordered by immutable id.
    pub fn erasing_accounts(
        &self,
        after_id: Option<&str>,
        limit: u32,
    ) -> CatalogResult<Vec<String>> {
        let limit = i64::from(limit.clamp(1, MAX_ERASURE_BATCH));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id
                    FROM accounts
                    WHERE status='erasing'
                    AND (?1 IS NULL
                    OR id>?1)
                    ORDER BY id LIMIT ?2",
                )
                .map_err(CatalogError::from)?;
            let rows = statement
                .query_map(params![after_id, limit], |row| row.get(0))
                .map_err(CatalogError::from)?;
            rows.collect::<Result<_, _>>().map_err(CatalogError::from)
        })
    }

    pub fn erasure_stage(&self, id: &str) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            let plan: Option<String> = connection
                .query_row(
                    "SELECT plan_json FROM operations
                 WHERE account_id=?1 AND kind='erase_account' AND state='prepared'",
                    [id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            plan.map(|raw| {
                let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
                    CatalogError::Invalid(format!("invalid erasure plan: {error}"))
                })?;
                value
                    .get("stage")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| CatalogError::Invalid("erasure plan has no stage".into()))
            })
            .transpose()
        })
    }

    pub fn erasure_progress(&self, id: &str) -> CatalogResult<Option<(String, Option<String>)>> {
        self.with_connection(|connection| {
            let plan: Option<String> = connection
                .query_row(
                    "SELECT plan_json FROM operations
                 WHERE account_id=?1 AND kind='erase_account' AND state='prepared'",
                    [id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            plan.map(|raw| {
                let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
                    CatalogError::Invalid(format!("invalid erasure plan: {error}"))
                })?;
                let stage = value
                    .get("stage")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| CatalogError::Invalid("erasure plan has no stage".into()))?
                    .to_owned();
                let cursor = match value.get("cursor") {
                    Some(serde_json::Value::Null) | None => None,
                    Some(value) => Some(serde_json::to_string(value).map_err(|error| {
                        CatalogError::Invalid(format!("invalid erasure cursor: {error}"))
                    })?),
                };
                Ok((stage, cursor))
            })
            .transpose()
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Onboarding {
    version: u32,
    items: Vec<OnboardingItem>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OnboardingItem {
    position: usize,
    slug: String,
    completed: bool,
}
fn decode_onboarding(payload: &str) -> CatalogResult<Onboarding> {
    let value: Onboarding = serde_json::from_str(payload)
        .map_err(|error| CatalogError::Invalid(format!("invalid onboarding payload: {error}")))?;
    let mut positions = BTreeSet::new();
    if value.version != 1
        || value.items.len() > 16
        || value.items.iter().any(|item| {
            item.slug.is_empty() || item.slug.len() > 256 || !positions.insert(item.position)
        })
    {
        return Err(CatalogError::Invalid(
            "invalid onboarding payload version or entries".into(),
        ));
    }
    Ok(value)
}
