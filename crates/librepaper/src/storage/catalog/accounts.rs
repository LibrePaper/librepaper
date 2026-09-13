//! Account identity, preferences, bounded usage queries, and erasure.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl Catalog {
    pub fn account_open_annotation_references(
        &self,
        account_id: &str,
    ) -> CatalogResult<BTreeMap<String, BTreeSet<String>>> {
        self.with_connection(|connection| {
            let mut statement=connection.prepare("SELECT d.slug,a.protected_checkpoint_id FROM annotations a JOIN documents d ON d.id=a.document_id WHERE d.owner_id=?1 AND d.status='active' AND a.resolved_at IS NULL AND a.protected_checkpoint_id IS NOT NULL ORDER BY d.slug,a.protected_checkpoint_id")?;
            let mut references=BTreeMap::<String,BTreeSet<String>>::new();
            let rows=statement.query_map([account_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?;
            for row in rows { let (slug,event)=row?; references.entry(slug).or_default().insert(event); }
            Ok(references)
        })
    }

    /// An advisory account settings view. Mutation admission never enumerates
    /// these rows; it uses the maintained account and deployment counters.
    pub fn account_checkpoints(&self, account_id: &str) -> CatalogResult<Vec<Checkpoint>> {
        self.with_connection(|connection| {
            let mut statement=connection.prepare("SELECT d.slug,c.id,c.seq,c.journal_sequence,c.tree_digest,COALESCE(c.parent_id,''),c.created_at,c.author_label,c.author_account_id,c.reason,c.source_format,c.logical_bytes,COALESCE(c.label,''),c.metadata_json FROM checkpoints c JOIN documents d ON d.id=c.document_id WHERE d.owner_id=?1 AND d.status='active' ORDER BY d.slug,c.seq")?;
            let rows=statement.query_map([account_id],|row| {
                let raw:String=row.get(13)?;
                let metadata:serde_json::Value=serde_json::from_str(&raw).map_err(|error|rusqlite::Error::FromSqlConversionFailure(13,rusqlite::types::Type::Text,Box::new(error)))?;
                Ok(Checkpoint {slug:row.get(0)?,sha:row.get(1)?,seq:row.get(2)?,durable_seq:row.get(3)?,tree_sha:row.get(4)?,parent:row.get(5)?,at:crate::util::format_unix(row.get::<_,i64>(6)?/1000),by:row.get(7)?,by_account:row.get(8)?,why:row.get(9)?,source_format:row.get(10)?,size:row.get(11)?,label:row.get(12)?,git_commit:metadata["commit"].as_str().unwrap_or_default().to_owned(),dirty:metadata["dirty"].as_bool().unwrap_or(false),changed:metadata.get("changed").cloned().unwrap_or_else(||serde_json::json!([])).to_string()})
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
        connection.query_row("SELECT stored_bytes+reserved_bytes,document_count,(SELECT count(*) FROM checkpoints c JOIN documents d ON d.id=c.document_id WHERE d.owner_id=?1) FROM accounts WHERE id=?1",[account_id],|row|Ok(AccountStorageUsage{charged_bytes:row.get(0)?,document_count:row.get(1)?,checkpoint_count:row.get(2)?,physical_accounting:true,live_bytes:0,history_bytes:0,asset_bytes:0,publication_bytes:0,metadata_bytes:0})).optional()?.ok_or(CatalogError::NotFound)
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
                "SELECT stored_bytes+reserved_bytes FROM accounts WHERE id=?1",
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
                "SELECT stored_bytes+reserved_bytes FROM server_state WHERE id=1",
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
            let values:Option<(i64,i64)>=connection.query_row("SELECT a.stored_bytes+a.reserved_bytes,s.stored_bytes+s.reserved_bytes FROM documents d JOIN accounts a ON a.id=d.owner_id CROSS JOIN server_state s WHERE d.slug=?1 AND d.status='active'",[slug],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            Ok(values.map(|(owner,total)| {
                let owner_limit=if owner_limit<0 {i64::MAX}else{owner_limit};
                let global=if total_limit<0{i64::MAX}else{total_limit.saturating_sub(total)};
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
        let (owner_bytes,global_bytes):(i64,i64)=connection.query_row("SELECT a.stored_bytes+a.reserved_bytes,s.stored_bytes+s.reserved_bytes FROM documents d JOIN accounts a ON a.id=d.owner_id CROSS JOIN server_state s WHERE d.slug=?1",[slug],|row|Ok((row.get(0)?,row.get(1)?)))?;
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
        let prefix="WITH wanted AS (SELECT d.id AS document_id,json_extract(w.value,'$[1]') AS checkpoint_id FROM json_each(?1) w JOIN documents d ON d.slug=json_extract(w.value,'$[0]'))";
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
                     FROM accounts WHERE id=?1",
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
                    "SELECT preferences_revision FROM accounts WHERE id=?1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            match (current, expected) {
                (Some(revision), expected) if revision != expected => {
                    return Err(CatalogError::Conflict("quota preference revision is stale".into()))
                }
                (None, expected) if expected != 0 => {
                    return Err(CatalogError::Conflict("quota preference revision is stale".into()))
                }
                _ => {}
            }
            let revision = current.unwrap_or(0).checked_add(1).ok_or_else(||CatalogError::Invalid("preference revision overflow".into()))?;
            let account_active: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active')",
                    [account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !account_active {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE accounts SET preferences_revision=?2, preferences_json=?3,
                    last_seen_at=max(last_seen_at,?4) WHERE id=?1",
                params![account_id, revision, payload, updated_at],
            )
            .map_err(CatalogError::from)?;
            // Mark owned documents due for bounded policy evaluation. Their
            // previous evaluation revision no longer authorizes deletion.
            tx.execute("UPDATE documents SET retention_due_at=0 WHERE owner_id=?1 AND status='active'",[account_id])?;
            tx.execute("UPDATE server_state SET catalog_revision=catalog_revision+1,updated_at=max(updated_at,?1) WHERE id=1",[updated_at])?;
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
            let existing:Option<(String,Option<String>,Option<String>)>=tx.query_row("SELECT status,provider,provider_subject FROM accounts WHERE id=?1",[&profile.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let now=unix_millis();
            if let Some((status,old_provider,old_subject))=existing {
                if status!="active" || old_provider.as_deref()!=provider || old_subject.as_deref()!=subject {return Err(CatalogError::Conflict("account lifecycle or provider identity changed".into()));}
                tx.execute("UPDATE accounts SET handle=?2,display_name=?3,email=?4,last_seen_at=max(last_seen_at,?5) WHERE id=?1",params![profile.id,profile.handle,profile.name,(!profile.email.is_empty()).then_some(&profile.email),now])?;
            } else {
                let items:Vec<_>=(0..crate::seed::ACCOUNT_EXAMPLE_COUNT).map(|position|serde_json::json!({"position":position,"slug":format!("starter-{}",crate::util::new_id()),"completed":false})).collect();
                let onboarding=serde_json::json!({"version":1,"items":items}).to_string();
                let preferences=serde_json::to_string(&crate::document::quota::QuotaPreferences::default()).map_err(|error|CatalogError::Invalid(error.to_string()))?;
                tx.execute("INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,plan,status,session_generation,created_at,last_seen_at,preferences_json,onboarding_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'active',?9,?10,?10,?11,?12)",params![profile.id,if provider.is_some(){"registered"}else{"anonymous"},provider,subject,profile.handle,profile.name,(!profile.email.is_empty()).then_some(&profile.email),profile.plan,profile.session_generation,now,preferences,onboarding])?;
            }
            tx.execute("UPDATE server_state SET catalog_revision=catalog_revision+1,updated_at=max(updated_at,?1) WHERE id=1",[now])?;
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
                "SELECT onboarding_json FROM accounts WHERE id=?1 AND status='active'",
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
                "SELECT onboarding_json FROM accounts WHERE id=?1 AND status='active'",
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
                "UPDATE accounts SET onboarding_json=?2 WHERE id=?1",
                params![id, payload],
            )?;
            tx.execute(
                "UPDATE server_state SET catalog_revision=catalog_revision+1 WHERE id=1",
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
             FROM accounts WHERE id = ?1",
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
                 FROM accounts WHERE id = ?1",
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
                    "UPDATE accounts SET session_generation = ?2 WHERE id = ?1 AND status = 'active'",
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
            let changed = tx
                .execute(
                    "UPDATE accounts SET status = 'erasing', session_generation = ?2,
                     erasure_cursor = NULL WHERE id = ?1 AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            // Withdrawal is part of the lifecycle transition.  The erasure
            // worker still owns physical reclamation and must retain these
            // rows (and their reservations) until object cleanup succeeds.
            // Withdraw every owned lifecycle row, including a creation whose
            // publication receipt is still prepared.  A prepared receipt is
            // an externally visible reservation even though its document is
            // not listed; leaving it behind would let a restart resurrect
            // content for an account that has already been erased.
            tx.execute(
                "UPDATE documents SET status='deleting', publication_id=NULL,
                 publication_object_id=NULL, published_at=NULL
                 WHERE owner_id=?1 AND status IN ('active','creating')",
                [id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE operations SET state='aborted', result_json=?2, completed_at=?3, receipt_expires_at=?3, updated_at=max(updated_at,?3)
                 WHERE state='prepared' AND document_id IN
                   (SELECT id FROM documents WHERE owner_id=?1 AND status='deleting')",
                params![id, r#"{"version":1,"reason":"account_erasure"}"#, super::unix_millis()],
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
            let changed=tx.execute("UPDATE accounts SET last_active_at=max(COALESCE(last_active_at,0),?2) WHERE id=?1 AND status='active'",params![id,at])?;
            if changed!=1{return Err(CatalogError::NotFound);}
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
        if stage.is_empty() || stage.len() > 64 || limit == 0 || limit > 1000 {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: String = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if status != "erasing" {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            tx.execute(
                "INSERT INTO erasure_batches(account_id, stage, cursor, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,
                   cursor=excluded.cursor, updated_at=excluded.updated_at",
                params![id, stage, cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor = ?2 WHERE id = ?1",
                params![id, cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(limit)
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
        if limit == 0 || limit > 1000 || stage.is_empty() {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id=?1", [id], |r| r.get(0))
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
                    serde_json::from_str::<Vec<String>>(value).map_err(|_| {
                        CatalogError::Invalid("invalid erasure cursor".into())
                    })
                })
                .transpose()?;
            let n_and_cursor = match stage {
                "grants" => {
                    let (after_slug, after_role) = cursor_parts
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
                            "SELECT slug, role FROM grants
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND role>?3))
                             ORDER BY slug, role LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_role, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, role) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                            params![slug, role, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, role)| serde_json::json!([slug, role]).to_string()), has_rows)
                }
                "guests" => {
                    let (after_slug, after_link) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid guests cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, link_hash FROM guests
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND link_hash>?3))
                             ORDER BY slug, link_hash LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_link, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, link_hash) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                            params![slug, id, link_hash],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, link)| serde_json::json!([slug, link]).to_string()), has_rows)
                }
                "comments" => {
                    let (after_slug, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid comments cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, id FROM comments
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND id>?3))
                             ORDER BY slug, id LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_id, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, comment_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM comments WHERE slug=?1 AND id=?2 AND author=?3",
                            params![slug, comment_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment_id)| serde_json::json!([slug, comment_id]).to_string()), has_rows)
                }
                "replies" => {
                    let (after_slug, after_comment, after_id) = cursor_parts
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
                            "SELECT slug, comment_id, id FROM replies
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND comment_id>?3)
                                    OR (slug=?2 AND comment_id=?3 AND id>?4))
                             ORDER BY slug, comment_id, id LIMIT ?5",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![id, after_slug, after_comment, after_id, i64::from(limit)],
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
                    let has_rows = last.is_some();
                    for (slug, comment_id, reply_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM replies
                             WHERE slug=?1 AND comment_id=?2 AND id=?3 AND author=?4",
                            params![slug, comment_id, reply_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment, reply)| serde_json::json!([slug, comment, reply]).to_string()), has_rows)
                }
                // Checkpoints are the account's contributions to documents it
                // may not own.  The row itself is retained -- its content,
                // sha, tree_sha, parent, timestamps and label are the
                // document's history, and the event identity other rows point
                // at -- and only the identifying attribution is cleared.
                //
                // Two stages, because there are two ways a row can name this
                // account.  The indexed one is the stable id; the legacy one
                // is a pre-migration row whose `by` literally holds an account
                // id, which is what the erasure query matched before this
                // column existed.  That second match is on the account id, not
                // on a handle: no row is selected because its display name
                // resembles the account's.
                "checkpoints" => {
                    let (after_slug, after_sha) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid checkpoints cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, sha FROM checkpoints
                             WHERE by_account=?1
                               AND (slug>?2 OR (slug=?2 AND sha>?3))
                             ORDER BY slug, sha LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_sha, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, sha) in rows.drain(..) {
                        tx.execute(
                            "UPDATE checkpoints SET by=?4, by_account=NULL
                             WHERE slug=?1 AND sha=?2 AND by_account=?3",
                            params![slug, sha, id, ERASED_ATTRIBUTION],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, sha)| serde_json::json!([slug, sha]).to_string()), has_rows)
                }
                "checkpoints_legacy" => {
                    let (after_slug, after_sha) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid(
                                    "invalid legacy checkpoints cursor".into(),
                                ));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, sha FROM checkpoints
                             WHERE by_account IS NULL AND by=?1
                               AND (slug>?2 OR (slug=?2 AND sha>?3))
                             ORDER BY slug, sha LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_sha, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, sha) in rows.drain(..) {
                        tx.execute(
                            "UPDATE checkpoints SET by=?4
                             WHERE slug=?1 AND sha=?2 AND by_account IS NULL AND by=?3",
                            params![slug, sha, id, ERASED_ATTRIBUTION],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, sha)| serde_json::json!([slug, sha]).to_string()), has_rows)
                }
                _ => return Err(CatalogError::Invalid("unknown erasure stage".into())),
            };
            let n = u32::from(n_and_cursor.1);
            let next_cursor = n_and_cursor.0;
            tx.execute(
                "INSERT INTO erasure_batches(account_id,stage,cursor,updated_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,cursor=excluded.cursor,updated_at=excluded.updated_at",
                params![id, stage, next_cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor=?2 WHERE id=?1",
                params![id, next_cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(n)
        })
    }

    pub fn finish_erasure(&self, id: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::NotFound);
            }
            let owned: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE owner_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            if owned != 0 {
                return Err(CatalogError::Conflict("owned documents remain".into()));
            }
            let references: i64 = tx
                .query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM grants WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM guests WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM comments WHERE author=?1) +
                       (SELECT COUNT(*) FROM replies WHERE author=?1) +
                       (SELECT COUNT(*) FROM checkpoints WHERE by_account=?1) +
                       (SELECT COUNT(*) FROM checkpoints
                        WHERE by_account IS NULL AND by=?1)",
                    [id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if references != 0 {
                return Err(CatalogError::Conflict("account attribution remains".into()));
            }
            tx.execute("DELETE FROM accounts WHERE id = ?1", [id])
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
        let limit = i64::from(limit.clamp(1, 100));
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM accounts WHERE status='erasing' AND (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2"
            ).map_err(CatalogError::from)?;
            let rows = statement.query_map(params![after_id,limit], |row| row.get(0)).map_err(CatalogError::from)?;
            rows.collect::<Result<_,_>>().map_err(CatalogError::from)
        })
    }

    pub fn erasure_stage(&self, id: &str) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn erasure_progress(&self, id: &str) -> CatalogResult<Option<(String, Option<String>)>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage, cursor FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)
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
