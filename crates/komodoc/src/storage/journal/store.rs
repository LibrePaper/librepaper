//! The store: the journal's state as the catalogue records it, and the
//! prepare/commit protocol every segment write goes through.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalState {
    pub deployment_id: String,
    pub writer_generation: String,
    pub revision: i64,
    pub last_operation_id: String,
    pub next_segment_seq: i64,
    pub manifest_key: String,
    pub manifest_digest: String,
    pub manifest_length: i64,
    pub tail_after: i64,
}

impl From<crate::storage::catalog::JournalState> for JournalState {
    fn from(state: crate::storage::catalog::JournalState) -> Self {
        Self {
            deployment_id: state.deployment_id,
            writer_generation: state.writer_generation,
            revision: state.revision,
            last_operation_id: state.last_operation_id,
            next_segment_seq: state.next_segment_seq,
            manifest_key: state.manifest_key,
            manifest_digest: state.manifest_digest,
            manifest_length: state.manifest_length,
            tail_after: state.tail_after,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalPreparation {
    pub operation_id: String,
    pub kind: String,
    pub expected_revision: i64,
    pub expected_generation: String,
    pub created_at: i64,
    pub plan: String,
}

/// SQL half of publication.  It does not write objects and never holds a SQL
/// transaction while a caller awaits object storage.
pub struct JournalStore {
    pub(super) catalog: Arc<Catalog>,
}

pub(super) fn read_state(connection: &rusqlite::Connection) -> rusqlite::Result<JournalState> {
    connection.query_row(
        "SELECT deployment_id, writer_generation, revision, last_operation_id,
                next_segment_seq, manifest_key, manifest_digest, manifest_length, tail_after
         FROM journal_state WHERE id = 1",
        [],
        |row| {
            Ok(JournalState {
                deployment_id: row.get(0)?,
                writer_generation: row.get(1)?,
                revision: row.get(2)?,
                last_operation_id: row.get(3)?,
                next_segment_seq: row.get(4)?,
                manifest_key: row.get(5)?,
                manifest_digest: row.get(6)?,
                manifest_length: row.get(7)?,
                tail_after: row.get(8)?,
            })
        },
    )
}

impl JournalStore {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Self { catalog }
    }

    pub async fn reconcile_object_reservations(&self, blobs: &dyn BlobStore) -> JournalResult<()> {
        let reservations: Vec<(String, String, String, i64)> = self
            .catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT storage_id,operation_id,object_key,new_bytes
                         FROM object_reservations WHERE object_key LIKE 'journal/%'",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)
            })
            .map_err(JournalError::from)?;
        for (storage_id, operation_id, object_key, new_bytes) in reservations {
            match blobs.get(&object_key).await {
                Ok(body) if body.len() as i64 == new_bytes => {
                    let kind = if object_key.contains("/bases/") {
                        "journal_base"
                    } else if object_key.contains("/manifest/") {
                        "journal_manifest"
                    } else {
                        "journal_segment"
                    };
                    self.commit_object(
                        &storage_id,
                        &operation_id,
                        &object_key,
                        kind,
                        &hex::encode(Sha256::digest(&body)),
                    )?;
                }
                Ok(_) | Err(BlobError::NotFound) => {
                    self.abort_object(&storage_id, &operation_id, &object_key)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn reserve_object(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
        kind: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> JournalResult<()> {
        let Some(slug) = self
            .catalog
            .slug_by_storage_id(storage_id)
            .map_err(JournalError::from)?
        else {
            // Isolated journal fixtures may not create a catalogue document;
            // production local rooms always do. Keep the journal contract
            // usable for those fixtures while accounting real owners.
            return Ok(());
        };
        self.catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: &slug,
                operation_id,
                object_key,
                kind,
                new_bytes: bytes,
                owner_limit,
                total_limit,
            })
            .map(|_| ())
            .map_err(JournalError::from)
    }

    pub(super) fn commit_object(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
        kind: &str,
        version: &str,
    ) -> JournalResult<()> {
        self.catalog
            .commit_object_change(storage_id, operation_id, object_key, kind, version)
            .map_err(JournalError::from)
    }

    pub(super) fn abort_object(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
    ) -> JournalResult<()> {
        self.catalog
            .abort_object_change(storage_id, operation_id, object_key)
            .map_err(JournalError::from)
    }

    pub fn initialize(
        &self,
        deployment_id: &str,
        writer_generation: &str,
    ) -> JournalResult<JournalState> {
        if deployment_id.is_empty() || writer_generation.is_empty() {
            return Err(JournalError::Invalid("journal identity is empty".into()));
        }
        self.catalog
            .configure_journal(deployment_id, writer_generation)
            .map(Into::into)
            .map_err(JournalError::from)
    }

    /// Remove one document's journal ownership during deletion. Shared
    /// segments remain while another coverage row names them; unshared
    /// segments and recovery bases become durable retirement jobs for the
    /// journal worker rather than being deleted by document-prefix cleanup.
    pub fn retire_storage(&self, storage_id: &str, retired_at: i64) -> JournalResult<()> {
        if storage_id.is_empty() || retired_at < 0 {
            return Err(JournalError::Invalid("invalid journal retirement".into()));
        }
        self.catalog
            .with_connection(|connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let bases = {
                    let mut statement = tx
                        .prepare(
                            "SELECT object_key, encoded_bytes FROM journal_bases
                             WHERE storage_id = ?1",
                        )
                        .map_err(CatalogError::from)?;
                    let rows = statement
                        .query_map([storage_id], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                        })
                        .map_err(CatalogError::from)?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?
                };
                for (key, bytes) in &bases {
                    tx.execute(
                        "INSERT INTO journal_retirements
                         (object_key, storage_id, kind, encoded_bytes, payload_bytes,
                          maintenance_bytes, retired_revision, modified_at,
                          first_unreferenced_at, delete_after)
                         VALUES (?1, ?2, 'base', ?3, 0, 0,
                                 (SELECT revision FROM journal_state), ?4, ?4, ?4)
                         ON CONFLICT(object_key) DO NOTHING",
                        params![key, storage_id, bytes, retired_at],
                    )
                    .map_err(CatalogError::from)?;
                }
                tx.execute(
                    "DELETE FROM journal_bases WHERE storage_id = ?1",
                    [storage_id],
                )
                .map_err(CatalogError::from)?;

                let segments = {
                    let mut statement = tx
                        .prepare(
                            "SELECT DISTINCT s.segment_id, s.object_key, s.encoded_bytes
                             FROM journal_segments s
                             LEFT JOIN journal_segment_coverage c
                               ON c.segment_id = s.segment_id
                             WHERE c.storage_id = ?1
                                OR (s.storage_id = ?1 AND NOT EXISTS
                                    (SELECT 1 FROM journal_segment_coverage c2
                                     WHERE c2.segment_id = s.segment_id))",
                        )
                        .map_err(CatalogError::from)?;
                    let rows = statement
                        .query_map([storage_id], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        })
                        .map_err(CatalogError::from)?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?
                };
                tx.execute(
                    "DELETE FROM journal_segment_coverage WHERE storage_id = ?1",
                    [storage_id],
                )
                .map_err(CatalogError::from)?;
                for (segment_id, key, bytes) in segments {
                    let still_covered: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM journal_segment_coverage
                             WHERE segment_id = ?1)",
                            [&segment_id],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(CatalogError::from)?
                        != 0;
                    if still_covered {
                        // The segment is shared. Transfer its single quota
                        // owner before the deleting document row can be
                        // removed, otherwise the FK cascade would make the
                        // still-live shared bytes uncharged.
                        let replacement: Option<String> = tx
                            .query_row(
                                "SELECT c.storage_id FROM journal_segment_coverage c
                                 JOIN documents d ON d.storage_id=c.storage_id
                                 WHERE c.segment_id=?1 AND c.storage_id<>?2
                                 ORDER BY (d.status='active') DESC, c.storage_id LIMIT 1",
                                params![segment_id, storage_id],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(CatalogError::from)?;
                        if let Some(replacement) = replacement {
                            let replacement_has_accounting: bool = tx
                                .query_row(
                                    "SELECT EXISTS(SELECT 1 FROM object_accounting
                                     WHERE storage_id=?1 AND object_key=?2)",
                                    params![replacement, key],
                                    |row| row.get::<_, i64>(0),
                                )
                                .map_err(CatalogError::from)?
                                != 0;
                            let transferred_bytes: Option<i64> = tx
                                .query_row(
                                    "SELECT bytes FROM object_accounting
                                     WHERE storage_id=?1 AND object_key=?2",
                                    params![storage_id, key],
                                    |row| row.get(0),
                                )
                                .optional()
                                .map_err(CatalogError::from)?;
                            if let Some(transferred_bytes) = transferred_bytes {
                                tx.execute(
                                    "UPDATE documents
                                     SET counted_size=MAX(size,counted_size-?2)
                                     WHERE storage_id=?1",
                                    params![storage_id, transferred_bytes],
                                )
                                .map_err(CatalogError::from)?;
                                if !replacement_has_accounting {
                                    tx.execute(
                                        "UPDATE documents SET counted_size=counted_size+?2
                                         WHERE storage_id=?1 AND status='active'",
                                        params![replacement, transferred_bytes],
                                    )
                                    .map_err(CatalogError::from)?;
                                }
                            }
                            if replacement_has_accounting {
                                tx.execute(
                                    "DELETE FROM object_accounting
                                     WHERE storage_id=?1 AND object_key=?2",
                                    params![storage_id, key],
                                )
                                .map_err(CatalogError::from)?;
                            } else {
                                tx.execute(
                                    "UPDATE object_accounting SET storage_id=?2
                                     WHERE storage_id=?1 AND object_key=?3",
                                    params![storage_id, replacement, key],
                                )
                                .map_err(CatalogError::from)?;
                            }
                        }
                        // Coverage no longer protects the erased identity,
                        // but the immutable shared object still contains its
                        // frames. Queue a physical rewrite; the retirement
                        // worker performs it before allowing this row to be
                        // reclaimed.
                        tx.execute(
                            "INSERT INTO journal_retirements
                             (object_key, storage_id, kind, encoded_bytes,
                              payload_bytes, maintenance_bytes, retired_revision,
                              modified_at, first_unreferenced_at, delete_after)
                             VALUES (?1, ?2, 'rewrite', ?3, 0, 0,
                                     (SELECT revision FROM journal_state), ?4, ?4, ?4)
                             ON CONFLICT(object_key) DO UPDATE SET
                               storage_id=excluded.storage_id,
                               kind='rewrite',
                               delete_after=MIN(journal_retirements.delete_after,
                                                excluded.delete_after)",
                            params![key, storage_id, bytes, retired_at],
                        )
                        .map_err(CatalogError::from)?;
                        continue;
                    }
                    tx.execute(
                        "DELETE FROM journal_segments WHERE segment_id = ?1",
                        [&segment_id],
                    )
                    .map_err(CatalogError::from)?;
                    tx.execute(
                        "INSERT INTO journal_retirements
                         (object_key, storage_id, kind, encoded_bytes, payload_bytes,
                          maintenance_bytes, retired_revision, modified_at,
                          first_unreferenced_at, delete_after)
                         VALUES (?1, ?2, 'segment', ?3, 0, 0,
                                 (SELECT revision FROM journal_state), ?4, ?4, ?4)
                         ON CONFLICT(object_key) DO UPDATE SET
                           storage_id=excluded.storage_id,
                           kind='segment',
                           encoded_bytes=excluded.encoded_bytes,
                           delete_after=MIN(journal_retirements.delete_after,
                                            excluded.delete_after)",
                        params![key, storage_id, bytes, retired_at],
                    )
                    .map_err(CatalogError::from)?;
                }
                tx.commit().map_err(CatalogError::from)?;
                Ok(())
            })
            .map_err(JournalError::from)
    }

    pub fn compaction_due(
        &self,
        storage_id: &str,
        epoch: u64,
        sequence: u64,
    ) -> JournalResult<bool> {
        self.catalog
            .with_connection(|connection| {
                let base: Option<i64> = connection
                    .query_row(
                        "SELECT MAX(sequence) FROM journal_bases
                         WHERE storage_id = ?1 AND epoch = ?2",
                        params![storage_id, epoch],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let base = u64::try_from(base.unwrap_or(0)).unwrap_or(0);
                let (segments, bytes): (i64, i64) = connection
                    .query_row(
                        "SELECT COUNT(DISTINCT s.segment_id),
                                COALESCE(SUM(s.encoded_bytes),0)
                         FROM journal_segments s
                         JOIN journal_segment_coverage c ON c.segment_id=s.segment_id
                         WHERE c.storage_id=?1 AND c.epoch=?2
                           AND c.last_sequence>?3 AND c.last_sequence<=?4",
                        params![storage_id, epoch, base, sequence],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(CatalogError::from)?;
                Ok(sequence.saturating_sub(base) >= 64
                    || segments >= 64
                    || bytes >= 32 * 1024 * 1024)
            })
            .map_err(JournalError::from)
    }

    /// Initialize a local deployment without changing its durable writer
    /// generation on every restart.  The generation is created once and then
    /// reused for the lifetime of the local deployment; hosted fencing may
    /// supply a generation from its primary driver instead.
    pub fn initialize_local(&self, deployment_id: &str) -> JournalResult<JournalState> {
        if deployment_id.is_empty() {
            return Err(JournalError::Invalid(
                "journal deployment id is empty".into(),
            ));
        }
        let generation = format!("local-{}", hex::encode(rand::random::<[u8; 16]>()));
        let state = self.catalog.journal_state().map_err(JournalError::from)?;
        if state.deployment_id.is_empty() && state.writer_generation.is_empty() {
            return self.initialize(deployment_id, &generation);
        }
        if state.deployment_id != deployment_id {
            return Err(JournalError::Catalog(CatalogError::Conflict(
                "journal deployment identity changed".into(),
            )));
        }
        Ok(state.into())
    }

    /// Startup must not serve rooms while an object-backed publication has an
    /// unresolved SQL outcome.  The caller can keep the deployment locked and
    /// hand the operation to an explicit reconciliation command.
    pub fn require_recovered(&self) -> JournalResult<()> {
        if let Some(preparation) = self.unresolved_preparation()? {
            return Err(JournalError::Invalid(format!(
                "journal operation {} requires reconciliation before serving",
                preparation.operation_id
            )));
        }
        Ok(())
    }

    /// Reconcile the one in-flight publication left by a crashed writer.
    ///
    /// A complete set of valid immutable segment objects can be replayed with
    /// the original ids and committed transactionally.  If any output is
    /// absent or malformed while the expected head is unchanged, the
    /// operation is a known abort: resolve it and retain orphan metadata for
    /// the deletion worker.  Storage read failures and a changed head remain
    /// ambiguous and keep startup fail-closed.
    pub async fn reconcile_pending(&self, blobs: &dyn BlobStore) -> JournalResult<()> {
        let Some(preparation) = self.unresolved_preparation()? else {
            return Ok(());
        };
        let plan: JournalPlan = serde_json::from_str(&preparation.plan)
            .map_err(|error| JournalError::Corrupt(format!("invalid prepared plan: {error}")))?;
        let state = self.state()?;
        // Commit transactions resolve the preparation and advance the head
        // together.  This branch is for older/catalogue implementations that
        // may have committed the head just before recording the resolution;
        // never leave startup blocked when the durable head already names the
        // operation.
        if state.last_operation_id == preparation.operation_id {
            if preparation.kind == "flush" {
                let mut written = Vec::with_capacity(plan.output_keys.len());
                for (index, key) in plan.output_keys.iter().enumerate() {
                    let body = blobs.get(key).await?;
                    let segment = Segment::decode(&body)?;
                    if !segment_matches_plan(&segment, &plan) {
                        return Err(JournalError::Corrupt(
                            "committed preparation has uncovered segment records".into(),
                        ));
                    }
                    written.push(WrittenSegment {
                        segment_id: format!("{}-{index}", preparation.operation_id),
                        object_key: key.clone(),
                        digest: hex::encode(Sha256::digest(&body)),
                        encoded_bytes: body.len() as i64,
                    });
                }
                if !self.resolve_committed_preparation(&preparation, &written)? {
                    return Err(JournalError::Invalid(format!(
                        "journal operation {} changed the head without matching descriptors",
                        preparation.operation_id
                    )));
                }
            } else {
                self.resolve_preparation(&preparation.operation_id)?;
            }
            return Ok(());
        }
        if state.revision != preparation.expected_revision
            || state.writer_generation != preparation.expected_generation
        {
            return Err(JournalError::Invalid(format!(
                "journal operation {} has an unexpected head",
                preparation.operation_id
            )));
        }
        if preparation.kind == "compact" {
            let mut bodies = Vec::with_capacity(plan.output_keys.len());
            for key in &plan.output_keys {
                match blobs.get(key).await {
                    Ok(body) => bodies.push((key.clone(), body)),
                    Err(BlobError::NotFound) => {
                        self.abort_preparation(&preparation, &plan, &[])?;
                        return Ok(());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let Some((base_key, base_body)) = bodies.first() else {
                return Err(JournalError::Corrupt("compact plan has no base".into()));
            };
            let decoded: RecoveryBaseBody = serde_json::from_slice(base_body).map_err(|error| {
                JournalError::Corrupt(format!("invalid prepared recovery base: {error}"))
            })?;
            if decoded.digest != hex::encode(Sha256::digest(&decoded.payload)) {
                return Err(JournalError::Corrupt(
                    "prepared recovery base payload digest mismatch".into(),
                ));
            }
            let base = RecoveryBase {
                base_id: format!(
                    "{}-{}-{}",
                    decoded.storage_id, decoded.epoch, decoded.sequence
                ),
                storage_id: decoded.storage_id.clone(),
                epoch: decoded.epoch,
                sequence: decoded.sequence,
                object_key: base_key.clone(),
                digest: hex::encode(Sha256::digest(base_body)),
                encoded_bytes: base_body.len() as i64,
                committed_at: preparation.created_at,
            };
            let mut shards = Vec::with_capacity(bodies.len().saturating_sub(1));
            for (key, body) in bodies.iter().skip(1) {
                let shard: ManifestShard = serde_json::from_slice(body).map_err(|error| {
                    JournalError::Corrupt(format!("invalid prepared manifest shard: {error}"))
                })?;
                if shard.object_key != *key || shard.encoded_bytes != body.len() as i64 {
                    return Err(JournalError::Corrupt(
                        "prepared manifest shard identity mismatch".into(),
                    ));
                }
                let digest = shard.digest.clone();
                let mut canonical = shard.clone();
                canonical.digest.clear();
                if hex::encode(Sha256::digest(serde_json::to_vec(&canonical).map_err(
                    |error| JournalError::Corrupt(format!("invalid manifest shard: {error}")),
                )?)) != digest
                {
                    return Err(JournalError::Corrupt(
                        "prepared manifest shard digest mismatch".into(),
                    ));
                }
                shards.push(shard);
            }
            if shards.is_empty() {
                return Err(JournalError::Corrupt(
                    "compact plan has no manifest shard".into(),
                ));
            }
            let retire_segments = self.segment_lengths(&plan.protected_input_keys)?;
            if retire_segments.len() != plan.protected_input_keys.len() {
                self.abort_preparation(&preparation, &plan, &[])?;
                return Ok(());
            }
            self.commit_compaction_shards(
                &preparation.operation_id,
                &base,
                &shards,
                &retire_segments,
            )?;
            return Ok(());
        }
        if preparation.kind != "flush" {
            // Unknown preparation kinds are not silently ignored.  A complete
            // output set is still a safe retry; an incomplete set is a known
            // abort and is queued for cleanup below.
            if plan.output_keys.is_empty() {
                self.abort_preparation(&preparation, &plan, &[])?;
                return Ok(());
            }
        }
        let mut written = Vec::with_capacity(plan.output_keys.len());
        let mut known_abort = false;
        for (index, key) in plan.output_keys.iter().enumerate() {
            match blobs.get(key).await {
                Ok(body) => match Segment::decode(&body) {
                    Ok(segment) if segment_matches_plan(&segment, &plan) => {
                        written.push(WrittenSegment {
                            segment_id: format!("{}-{index}", preparation.operation_id),
                            object_key: key.clone(),
                            digest: hex::encode(Sha256::digest(&body)),
                            encoded_bytes: body.len() as i64,
                        });
                    }
                    Ok(_) | Err(_) => known_abort = true,
                },
                Err(BlobError::NotFound) => known_abort = true,
                Err(error) => return Err(error.into()),
            }
        }
        if !known_abort && written.len() == plan.output_keys.len() {
            self.commit_segments(&preparation.operation_id, &written, crate::util::now_unix())?;
            return Ok(());
        }
        let lengths = written
            .iter()
            .map(|segment| (segment.object_key.clone(), segment.encoded_bytes))
            .collect::<Vec<_>>();
        self.abort_preparation(&preparation, &plan, &lengths)?;
        Ok(())
    }

    pub(super) fn resolve_committed_preparation(
        &self,
        preparation: &JournalPreparation,
        written: &[WrittenSegment],
    ) -> JournalResult<bool> {
        self.catalog
            .with_connection(|connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let state = read_state(&tx).map_err(CatalogError::from)?;
                if state.last_operation_id != preparation.operation_id {
                    return Ok(false);
                }
                let mut statement = tx
                    .prepare(
                        "SELECT object_key, digest, encoded_bytes FROM journal_segments
                         WHERE operation_id = ?1 ORDER BY segment_seq",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([preparation.operation_id.as_str()], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })
                    .map_err(CatalogError::from)?;
                let descriptors = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?;
                if descriptors.len() != written.len()
                    || descriptors.iter().zip(written).any(|(left, right)| {
                        left.0 != right.object_key
                            || left.1 != right.digest
                            || left.2 != right.encoded_bytes
                    })
                {
                    return Ok(false);
                }
                drop(statement);
                tx.execute(
                    "UPDATE journal_preparations SET resolved_at = ?2
                     WHERE operation_id = ?1 AND resolved_at IS NULL",
                    params![preparation.operation_id, crate::util::now_unix()],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                Ok(true)
            })
            .map_err(JournalError::from)
    }

    pub(super) fn resolve_preparation(&self, operation_id: &str) -> JournalResult<()> {
        self.catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE journal_preparations SET resolved_at=?2
                         WHERE operation_id=?1 AND resolved_at IS NULL",
                        params![operation_id, crate::util::now_unix()],
                    )
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .map_err(JournalError::from)
    }

    pub(super) fn abort_preparation(
        &self,
        preparation: &JournalPreparation,
        plan: &JournalPlan,
        known_lengths: &[(String, i64)],
    ) -> JournalResult<()> {
        let result = self
            .catalog
            .with_connection(|connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let state = read_state(&tx).map_err(CatalogError::from)?;
                if state.revision != preparation.expected_revision
                    || state.writer_generation != preparation.expected_generation
                {
                    return Err(CatalogError::Conflict(
                        "journal head changed while aborting preparation".into(),
                    ));
                }
                let retirement_storage = if plan.covered.len() == 1 {
                    plan.covered[0].storage_id.as_str()
                } else {
                    ""
                };
                for key in &plan.output_keys {
                    let encoded_bytes = known_lengths
                        .iter()
                        .find(|(known_key, _)| known_key == key)
                        .map(|(_, length)| *length)
                        .unwrap_or(0);
                    tx.execute(
                        "INSERT INTO journal_retirements
                         (object_key, storage_id, kind, encoded_bytes, payload_bytes,
                          maintenance_bytes, retired_revision, modified_at,
                          first_unreferenced_at, delete_after)
                         VALUES (?1, ?2, 'segment', ?3, 0, 0, ?4, ?5, ?5, ?5)
                         ON CONFLICT(object_key) DO NOTHING",
                        params![
                            key,
                            retirement_storage,
                            encoded_bytes,
                            state.revision,
                            crate::util::now_unix()
                        ],
                    )
                    .map_err(CatalogError::from)?;
                }
                tx.execute(
                    "UPDATE journal_preparations SET resolved_at = ?2
                     WHERE operation_id = ?1 AND resolved_at IS NULL",
                    params![preparation.operation_id, crate::util::now_unix()],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                Ok(())
            })
            .map_err(JournalError::from);
        if result.is_ok() {
            // Reservations are separate from the journal preparation so an
            // object-write failure must explicitly refund every owner that
            // participated in a shared segment/compaction operation.
            let owners: HashSet<&str> = plan
                .covered
                .iter()
                .map(|range| range.storage_id.as_str())
                .filter(|storage_id| !storage_id.is_empty())
                .collect();
            for owner in owners {
                for key in &plan.output_keys {
                    let _ = self.abort_object(owner, &preparation.operation_id, key);
                }
            }
        }
        result
    }

    pub fn state(&self) -> JournalResult<JournalState> {
        self.catalog
            .with_connection(|connection| read_state(connection).map_err(CatalogError::from))
            .map_err(JournalError::from)
    }

    pub fn prepare(
        &self,
        operation_id: &str,
        kind: &str,
        expected_revision: i64,
        expected_generation: &str,
        created_at: i64,
        plan: &JournalPlan,
    ) -> JournalResult<JournalPreparation> {
        if operation_id.is_empty() || kind.is_empty() || expected_revision < 0 || created_at < 0 {
            return Err(JournalError::Invalid("invalid journal preparation".into()));
        }
        let encoded_plan = plan.encoded()?;
        self.catalog
            .with_connection(|connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let state = read_state(&tx).map_err(CatalogError::from)?;
                if state.revision != expected_revision || state.writer_generation != expected_generation {
                    return Err(CatalogError::Conflict("journal head changed".into()));
                }
                let unresolved: Option<String> = tx
                    .query_row(
                        "SELECT operation_id FROM journal_preparations WHERE resolved_at IS NULL LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if unresolved.is_some() {
                    return Err(CatalogError::Conflict("another journal operation is prepared".into()));
                }
                tx.execute(
                    "INSERT INTO journal_preparations
                     (operation_id, kind, expected_revision, expected_generation,
                      created_at, plan, resolved_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                    params![operation_id, kind, expected_revision, expected_generation, created_at, encoded_plan],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                Ok(JournalPreparation {
                    operation_id: operation_id.to_owned(),
                    kind: kind.to_owned(),
                    expected_revision,
                    expected_generation: expected_generation.to_owned(),
                    created_at,
                    plan: encoded_plan,
                })
            })
            .map_err(JournalError::from)
    }

    pub fn commit_segment(
        &self,
        operation_id: &str,
        segment: &WrittenSegment,
        committed_at: i64,
    ) -> JournalResult<(JournalState, i64)> {
        if committed_at < 0 || segment.encoded_bytes < 0 || segment.object_key.is_empty() {
            return Err(JournalError::Invalid("invalid segment metadata".into()));
        }
        self.commit_segments(operation_id, std::slice::from_ref(segment), committed_at)
            .map(|(state, sequences)| (state, sequences[0]))
    }

    /// Commit all segments belonging to one prepared operation in one guarded
    /// head transition.  Object writes must already be durable when this is
    /// called; the returned sequence numbers are the durable journal cursor.
    pub fn commit_segments(
        &self,
        operation_id: &str,
        segments: &[WrittenSegment],
        committed_at: i64,
    ) -> JournalResult<(JournalState, Vec<i64>)> {
        if segments.is_empty() {
            return Err(JournalError::Invalid(
                "cannot commit an empty segment batch".into(),
            ));
        }
        if segments
            .iter()
            .any(|segment| segment.encoded_bytes < 0 || segment.object_key.is_empty())
        {
            return Err(JournalError::Invalid("invalid segment metadata".into()));
        }
        self.catalog
            .with_connection(|connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let state = read_state(&tx).map_err(CatalogError::from)?;
                let preparation: Option<(i64, String)> = tx
                    .query_row(
                        "SELECT expected_revision, expected_generation
                         FROM journal_preparations WHERE operation_id = ?1 AND resolved_at IS NULL",
                        [operation_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                let Some((expected_revision, generation)) = preparation else {
                    return Err(CatalogError::NotFound);
                };
                let plan: JournalPlan = tx
                    .query_row(
                        "SELECT plan FROM journal_preparations
                         WHERE operation_id = ?1 AND resolved_at IS NULL",
                        [operation_id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(CatalogError::from)
                    .and_then(|encoded| {
                        serde_json::from_str(&encoded).map_err(|error| {
                            CatalogError::Invalid(format!("invalid journal plan: {error}"))
                        })
                    })?;
                let storage_id = plan
                    .covered
                    .first()
                    .map(|range| range.storage_id.as_str())
                    .unwrap_or("");
                let epoch = plan.covered.first().map(|range| range.epoch).unwrap_or(0);
                let first_sequence = plan
                    .covered
                    .iter()
                    .map(|range| range.first_sequence)
                    .min()
                    .unwrap_or(0);
                let last_sequence = plan
                    .covered
                    .iter()
                    .map(|range| range.last_sequence)
                    .max()
                    .unwrap_or(0);
                if state.revision != expected_revision || state.writer_generation != generation {
                    return Err(CatalogError::Conflict("journal head changed".into()));
                }
                let mut sequences = Vec::with_capacity(segments.len());
                for (offset, segment) in segments.iter().enumerate() {
                    let segment_seq = state.next_segment_seq + offset as i64;
                    tx.execute(
                        "INSERT INTO journal_segments
                         (segment_id, segment_seq, operation_id, object_key, digest,
                          encoded_bytes, committed_at, storage_id, epoch,
                          first_sequence, last_sequence)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            segment.segment_id,
                            segment_seq,
                            operation_id,
                            segment.object_key,
                            segment.digest,
                            segment.encoded_bytes,
                            committed_at,
                            storage_id,
                            epoch,
                            first_sequence,
                            last_sequence
                        ],
                    )
                    .map_err(CatalogError::from)?;
                    for range in &plan.covered {
                        tx.execute(
                            "INSERT INTO journal_segment_coverage
                             (segment_id, storage_id, epoch, first_sequence, last_sequence)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![
                                segment.segment_id,
                                range.storage_id,
                                range.epoch,
                                range.first_sequence,
                                range.last_sequence,
                            ],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    sequences.push(segment_seq);
                }
                tx.execute(
                    "UPDATE journal_state SET revision = revision + 1,
                     last_operation_id = ?1, next_segment_seq = next_segment_seq + ?3
                     WHERE id = 1 AND revision = ?2",
                    params![operation_id, expected_revision, segments.len() as i64],
                )
                .map_err(CatalogError::from)?;
                tx.execute(
                    "UPDATE journal_preparations SET resolved_at = ?2
                     WHERE operation_id = ?1 AND resolved_at IS NULL",
                    params![operation_id, committed_at],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                let state = read_state(connection).map_err(CatalogError::from)?;
                Ok((state, sequences))
            })
            .map_err(JournalError::from)
    }

    pub fn unresolved_preparation(&self) -> JournalResult<Option<JournalPreparation>> {
        self.catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT operation_id, kind, expected_revision, expected_generation,
                                created_at, plan
                         FROM journal_preparations WHERE resolved_at IS NULL",
                        [],
                        |row| {
                            Ok(JournalPreparation {
                                operation_id: row.get(0)?,
                                kind: row.get(1)?,
                                expected_revision: row.get(2)?,
                                expected_generation: row.get(3)?,
                                created_at: row.get(4)?,
                                plan: row.get(5)?,
                            })
                        },
                    )
                    .optional()
                    .map_err(CatalogError::from)
            })
            .map_err(JournalError::from)
    }

    pub fn recovery_base(&self, storage_id: &str) -> JournalResult<Option<RecoveryBase>> {
        self.catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT base_id, storage_id, epoch, sequence, object_key, digest,
                                encoded_bytes, committed_at
                         FROM journal_bases WHERE storage_id = ?1
                         ORDER BY sequence DESC LIMIT 1",
                        [storage_id],
                        |row| {
                            Ok(RecoveryBase {
                                base_id: row.get(0)?,
                                storage_id: row.get(1)?,
                                epoch: row.get(2)?,
                                sequence: row.get(3)?,
                                object_key: row.get(4)?,
                                digest: row.get(5)?,
                                encoded_bytes: row.get(6)?,
                                committed_at: row.get(7)?,
                            })
                        },
                    )
                    .optional()
                    .map_err(CatalogError::from)
            })
            .map_err(JournalError::from)
    }

    pub fn recovery_bases(&self) -> JournalResult<Vec<RecoveryBase>> {
        self.catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT base_id, storage_id, epoch, sequence, object_key, digest,
                                encoded_bytes, committed_at
                         FROM journal_bases ORDER BY storage_id, sequence LIMIT ?1",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([MAX_MANIFEST_ENTRIES as i64 + 1], |row| {
                        Ok(RecoveryBase {
                            base_id: row.get(0)?,
                            storage_id: row.get(1)?,
                            epoch: row.get(2)?,
                            sequence: row.get(3)?,
                            object_key: row.get(4)?,
                            digest: row.get(5)?,
                            encoded_bytes: row.get(6)?,
                            committed_at: row.get(7)?,
                        })
                    })
                    .map_err(CatalogError::from)?;
                let result = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?;
                if result.len() > MAX_MANIFEST_ENTRIES {
                    return Err(CatalogError::Invalid(
                        "recovery base manifest exceeds configured bound".into(),
                    ));
                }
                Ok(result)
            })
            .map_err(JournalError::from)
    }

    pub fn commit_compaction_shards(
        &self,
        operation_id: &str,
        base: &RecoveryBase,
        shards: &[ManifestShard],
        retire_segments: &[(String, i64)],
    ) -> JournalResult<JournalState> {
        let Some(shard) = shards.first() else {
            return Err(JournalError::Invalid("manifest shard set is empty".into()));
        };
        let base_json = serde_json::to_string(base)
            .map_err(|error| JournalError::Invalid(format!("invalid base metadata: {error}")))?;
        if base_json.len() > MAX_METADATA_BYTES {
            return Err(JournalError::Limit("recovery metadata is too large".into()));
        }
        for shard in shards {
            let shard_json = serde_json::to_string(shard).map_err(|error| {
                JournalError::Invalid(format!("invalid shard metadata: {error}"))
            })?;
            if shard_json.len() > MAX_METADATA_BYTES {
                return Err(JournalError::Limit("manifest shard is too large".into()));
            }
        }
        self.catalog
            .with_connection(|connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let state = read_state(&tx).map_err(CatalogError::from)?;
                let (expected_revision, expected_generation): (i64, String) = tx
                    .query_row(
                        "SELECT expected_revision, expected_generation
                         FROM journal_preparations WHERE operation_id = ?1 AND resolved_at IS NULL",
                        [operation_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(CatalogError::from)?;
                if state.revision != expected_revision
                    || state.writer_generation != expected_generation
                {
                    return Err(CatalogError::Conflict("journal head changed".into()));
                }
                tx.execute(
                    "INSERT INTO journal_bases
                     (base_id, storage_id, epoch, sequence, object_key, digest, encoded_bytes, committed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        base.base_id,
                        base.storage_id,
                        base.epoch,
                        base.sequence,
                        base.object_key,
                        base.digest,
                        base.encoded_bytes,
                        base.committed_at,
                    ],
                )
                .map_err(CatalogError::from)?;
                let old_bases: Vec<(String, String, i64)> = {
                    let mut statement = tx
                        .prepare(
                            "SELECT object_key, storage_id, encoded_bytes FROM journal_bases
                             WHERE storage_id = ?1 AND object_key <> ?2",
                        )
                        .map_err(CatalogError::from)?;
                    let rows = statement
                        .query_map(params![base.storage_id, base.object_key], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                        })
                        .map_err(CatalogError::from)?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?
                };
                let old_shards: Vec<(String, i64)> = {
                    let mut statement = tx
                        .prepare(
                            "SELECT object_key, encoded_bytes FROM journal_manifest_shards
                             WHERE object_key <> ?1",
                        )
                        .map_err(CatalogError::from)?;
                    let rows = statement
                        .query_map([shard.object_key.as_str()], |row| {
                            Ok((row.get(0)?, row.get(1)?))
                        })
                        .map_err(CatalogError::from)?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?
                };
                for (object_key, storage_id, encoded_bytes) in old_bases {
                    tx.execute(
                        "INSERT INTO journal_retirements
                         (object_key, storage_id, kind, encoded_bytes, payload_bytes, maintenance_bytes,
                          retired_revision, modified_at, first_unreferenced_at, delete_after)
                         VALUES (?1, ?2, 'base', ?3, 0, 0, ?4, ?5, ?5, ?5)
                         ON CONFLICT(object_key) DO NOTHING",
                        params![object_key, storage_id, encoded_bytes, state.revision, base.committed_at],
                    )
                    .map_err(CatalogError::from)?;
                }
                for (object_key, encoded_bytes) in old_shards {
                    tx.execute(
                        "INSERT INTO journal_retirements
                         (object_key, storage_id, kind, encoded_bytes, payload_bytes, maintenance_bytes,
                          retired_revision, modified_at, first_unreferenced_at, delete_after)
                         VALUES (?1, '', 'manifest', ?2, 0, 0, ?3, ?4, ?4, ?4)
                         ON CONFLICT(object_key) DO NOTHING",
                        params![object_key, encoded_bytes, state.revision, base.committed_at],
                    )
                    .map_err(CatalogError::from)?;
                }
                tx.execute(
                    "DELETE FROM journal_bases
                     WHERE storage_id = ?1 AND object_key <> ?2",
                    params![base.storage_id, base.object_key],
                )
                .map_err(CatalogError::from)?;
                tx.execute(
                    "DELETE FROM journal_manifest_shards WHERE object_key <> ?1",
                    [shard.object_key.as_str()],
                )
                .map_err(CatalogError::from)?;
                for shard in shards {
                    tx.execute(
                        "INSERT INTO journal_manifest_shards
                         (shard_id, shard_seq, object_key, digest, encoded_bytes, committed_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            shard.shard_id,
                            shard.shard_seq,
                            shard.object_key,
                            shard.digest,
                            shard.encoded_bytes,
                            shard.committed_at,
                        ],
                    )
                    .map_err(CatalogError::from)?;
                }
                for (object_key, encoded_bytes) in retire_segments {
                    tx.execute(
                        "DELETE FROM journal_segment_coverage
                         WHERE segment_id IN (
                           SELECT segment_id FROM journal_segments WHERE object_key = ?1
                         )",
                        [&object_key],
                    )
                    .map_err(CatalogError::from)?;
                    tx.execute(
                        "DELETE FROM journal_segments WHERE object_key = ?1",
                        [object_key],
                    )
                    .map_err(CatalogError::from)?;
                    tx.execute(
                        "INSERT INTO journal_retirements
                         (object_key, storage_id, kind, encoded_bytes, payload_bytes, maintenance_bytes,
                          retired_revision, modified_at, first_unreferenced_at, delete_after)
                         VALUES (?1, ?2, 'segment', ?3, 0, 0, ?4, ?5, ?5, ?5)
                         ON CONFLICT(object_key) DO NOTHING",
                        params![object_key, base.storage_id, encoded_bytes, state.revision, base.committed_at],
                    )
                    .map_err(CatalogError::from)?;
                }
                tx.execute(
                    "UPDATE journal_state SET revision = revision + 1,
                         last_operation_id = ?1, manifest_key = ?2,
                         manifest_digest = ?3, manifest_length = ?4,
                         tail_after = ?5 WHERE id = 1 AND revision = ?6",
                    params![
                        operation_id,
                        shard.object_key,
                        shard.digest,
                        shard.encoded_bytes,
                        base.sequence,
                        expected_revision,
                    ],
                )
                .map_err(CatalogError::from)?;
                tx.execute(
                    "UPDATE journal_preparations SET resolved_at = ?2
                     WHERE operation_id = ?1 AND resolved_at IS NULL",
                    params![operation_id, base.committed_at],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                read_state(connection).map_err(CatalogError::from)
            })
            .map_err(JournalError::from)
    }

    pub fn committed_segments(&self) -> JournalResult<Vec<(String, String)>> {
        self.catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT object_key, digest FROM journal_segments
                         ORDER BY segment_seq LIMIT ?1",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([MAX_MANIFEST_ENTRIES as i64 + 1], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .map_err(CatalogError::from)?;
                let result = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?;
                if result.len() > MAX_MANIFEST_ENTRIES {
                    return Err(CatalogError::Invalid(
                        "journal manifest exceeds configured bound".into(),
                    ));
                }
                Ok(result)
            })
            .map_err(JournalError::from)
    }

    pub(super) fn segment_lengths(&self, keys: &[String]) -> JournalResult<Vec<(String, i64)>> {
        let mut result = Vec::with_capacity(keys.len());
        for key in keys {
            let descriptor = self
                .catalog
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT object_key, encoded_bytes FROM journal_segments
                             WHERE object_key=?1",
                            [key],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                        )
                        .optional()
                        .map_err(CatalogError::from)
                })
                .map_err(JournalError::from)?;
            if let Some(descriptor) = descriptor {
                result.push(descriptor);
            }
        }
        Ok(result)
    }

    /// Return only descriptors whose coverage can contain this document. SQL
    /// pages are bounded; the replay cap prevents a pathological tail from
    /// turning a cold open into an unbounded allocation.
    pub fn replay_descriptors(
        &self,
        storage_id: &str,
        after: Option<(u64, u64)>,
    ) -> JournalResult<Vec<(String, String, i64)>> {
        let mut page_after = -1i64;
        let mut result = Vec::new();
        loop {
            let page = self
                .catalog
                .with_connection(|connection| {
                    let (sql, values): (&str, Vec<rusqlite::types::Value>) = match after {
                        Some((epoch, sequence)) => (
                            "SELECT DISTINCT s.segment_seq, s.object_key, s.digest, s.encoded_bytes
                         FROM journal_segments s
                         JOIN journal_segment_coverage c ON c.segment_id = s.segment_id
                         WHERE (c.storage_id = ?1 OR c.storage_id = '')
                           AND (c.epoch > ?2 OR (c.epoch = ?2 AND c.last_sequence > ?3))
                           AND s.segment_seq > ?4
                         ORDER BY s.segment_seq LIMIT ?5",
                            vec![
                                storage_id.to_owned().into(),
                                (epoch as i64).into(),
                                (sequence as i64).into(),
                                page_after.into(),
                                (REPLAY_PAGE_SIZE as i64).into(),
                            ],
                        ),
                        None => (
                            "SELECT DISTINCT s.segment_seq, s.object_key, s.digest, s.encoded_bytes
                         FROM journal_segments s
                         JOIN journal_segment_coverage c ON c.segment_id = s.segment_id
                         WHERE (c.storage_id = ?1 OR c.storage_id = '')
                           AND s.segment_seq > ?2
                         ORDER BY s.segment_seq LIMIT ?3",
                            vec![
                                storage_id.to_owned().into(),
                                page_after.into(),
                                (REPLAY_PAGE_SIZE as i64).into(),
                            ],
                        ),
                    };
                    let mut statement = connection.prepare(sql).map_err(CatalogError::from)?;
                    let rows = statement
                        .query_map(rusqlite::params_from_iter(values), |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, i64>(3)?,
                            ))
                        })
                        .map_err(CatalogError::from)?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)
                })
                .map_err(JournalError::from)?;
            if page.is_empty() {
                break;
            }
            page_after = page.last().map(|row| row.0).unwrap_or(page_after);
            result.extend(
                page.into_iter()
                    .map(|(_, key, digest, bytes)| (key, digest, bytes)),
            );
            if result.len() > MAX_REPLAY_SEGMENTS {
                return Err(JournalError::Limit(
                    "journal replay tail exceeds configured segment bound".into(),
                ));
            }
        }
        Ok(result)
    }

    pub fn sequence_committed(
        &self,
        storage_id: &str,
        epoch: u64,
        sequence: u64,
    ) -> JournalResult<bool> {
        self.catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM journal_segment_coverage
                           WHERE (storage_id = ?1 OR storage_id = '')
                             AND epoch = ?2 AND first_sequence <= ?3
                             AND last_sequence >= ?3
                         ) OR EXISTS(
                           SELECT 1 FROM journal_bases
                           WHERE storage_id = ?1 AND epoch = ?2 AND sequence >= ?3
                         )",
                        params![storage_id, epoch, sequence],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|value| value != 0)
                    .map_err(CatalogError::from)
            })
            .map_err(JournalError::from)
    }

    pub fn latest_sequence(&self, storage_id: &str, epoch: u64) -> JournalResult<u64> {
        self.catalog
            .with_connection(|connection| {
                let coverage: Option<i64> = connection
                    .query_row(
                        "SELECT MAX(last_sequence) FROM journal_segment_coverage
                         WHERE (storage_id = ?1 OR storage_id = '') AND epoch = ?2",
                        params![storage_id, epoch],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let base: Option<i64> = connection
                    .query_row(
                        "SELECT MAX(sequence) FROM journal_bases
                         WHERE storage_id = ?1 AND epoch = ?2",
                        params![storage_id, epoch],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                Ok(u64::try_from(coverage.unwrap_or(0).max(base.unwrap_or(0))).unwrap_or(0))
            })
            .map_err(JournalError::from)
    }

    pub fn committed_segments_for(
        &self,
        storage_id: &str,
        epoch: u64,
        above_sequence: u64,
    ) -> JournalResult<Vec<(String, String, i64)>> {
        self.catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT DISTINCT s.object_key, s.digest, s.encoded_bytes
                         FROM journal_segments s
                         JOIN journal_segment_coverage c ON c.segment_id = s.segment_id
                         WHERE (c.storage_id = ?1 OR c.storage_id = '') AND c.epoch = ?2
                           AND c.last_sequence > ?3
                         ORDER BY segment_seq LIMIT ?4",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(
                        params![
                            storage_id,
                            epoch,
                            above_sequence,
                            MAX_REPLAY_SEGMENTS as i64 + 1
                        ],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(CatalogError::from)?;
                let result = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?;
                if result.len() > MAX_REPLAY_SEGMENTS {
                    return Err(CatalogError::Invalid(
                        "journal retry scan exceeds configured bound".into(),
                    ));
                }
                Ok(result)
            })
            .map_err(JournalError::from)
    }

    /// Return the tail after a base, including records from later retry
    /// epochs. Epoch transitions reset sequence numbering, so filtering only
    /// by the base epoch would silently discard a valid post-resync update.
    pub fn committed_segments_after(
        &self,
        storage_id: &str,
        base_epoch: u64,
        base_sequence: u64,
    ) -> JournalResult<Vec<(String, String, i64)>> {
        self.catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT DISTINCT s.object_key, s.digest, s.encoded_bytes
                         FROM journal_segments s
                         JOIN journal_segment_coverage c ON c.segment_id = s.segment_id
                         WHERE (c.storage_id = ?1 OR c.storage_id = '')
                           AND (c.epoch > ?2 OR (c.epoch = ?2 AND c.last_sequence > ?3))
                         ORDER BY s.segment_seq LIMIT ?4",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(
                        params![
                            storage_id,
                            base_epoch,
                            base_sequence,
                            MAX_REPLAY_SEGMENTS as i64 + 1
                        ],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(CatalogError::from)?;
                let result = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?;
                if result.len() > MAX_REPLAY_SEGMENTS {
                    return Err(CatalogError::Invalid(
                        "journal replay tail exceeds configured bound".into(),
                    ));
                }
                Ok(result)
            })
            .map_err(JournalError::from)
    }

    pub fn committed_segments_through(
        &self,
        storage_id: &str,
        epoch: u64,
        through_sequence: u64,
    ) -> JournalResult<Vec<(String, String, i64)>> {
        self.catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT DISTINCT s.object_key, s.digest, s.encoded_bytes
                         FROM journal_segments s
                         JOIN journal_segment_coverage c ON c.segment_id = s.segment_id
                         WHERE (c.storage_id = ?1 OR c.storage_id = '')
                           AND (c.epoch < ?2 OR (c.epoch = ?2 AND c.last_sequence > 0
                                               AND c.last_sequence <= ?3))
                           AND NOT EXISTS (
                             SELECT 1 FROM journal_segment_coverage other
                             WHERE other.segment_id = c.segment_id
                               AND NOT (
                                 other.storage_id = ?1 AND other.epoch = ?2
                                 AND other.last_sequence <= ?3
                               )
                               AND NOT EXISTS (
                                 SELECT 1 FROM journal_bases covered
                                 WHERE covered.storage_id = other.storage_id
                                   AND (covered.epoch > other.epoch
                                        OR (covered.epoch = other.epoch
                                            AND covered.sequence >= other.last_sequence))
                               )
                           )
                         ORDER BY s.segment_seq LIMIT ?4",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(
                        params![
                            storage_id,
                            epoch,
                            through_sequence,
                            MAX_REPLAY_SEGMENTS as i64 + 1
                        ],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(CatalogError::from)?;
                let result = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?;
                if result.len() > MAX_REPLAY_SEGMENTS {
                    return Err(CatalogError::Invalid(
                        "journal compaction scan exceeds configured bound".into(),
                    ));
                }
                Ok(result)
            })
            .map_err(JournalError::from)
    }
}
