//! The runtime: appending to the journal, compacting it, and the readers and
//! maintenance reservations that keep retirement honest.

use super::*;

pub const MAX_REPLAY_SEGMENTS: usize = 16_384;

pub const REPLAY_PAGE_SIZE: usize = 128;

pub const MAX_MANIFEST_ENTRIES: usize = 65_536;

pub(super) const JOURNAL_MAINTENANCE_RESERVE_BYTES: i64 = 64 * 1024 * 1024;

/// Production bridge used by Room.  It serializes the prepare/object
/// write/commit protocol while leaving the coordinator responsible for queue
/// limits and segment framing. Known failures are reconciled while holding the
/// gate; only an ambiguous publication remains for startup reconciliation.
pub struct JournalRuntime {
    pub(super) coordinator: AsyncMutex<JournalCoordinator>,
    pub(super) publication: Arc<AsyncMutex<()>>,
    pub(super) pending: AsyncMutex<HashSet<(String, u64, u64)>>,
    pub(super) pending_notify: Notify,
    pub(super) store: Arc<JournalStore>,
    pub(super) blobs: Arc<dyn BlobStore>,
    pub(super) deployment_id: String,
    pub(super) owner_limit: i64,
    pub(super) total_limit: i64,
    pub(super) next_operation: AtomicU64,
}

/// A compaction borrow is scoped to the object-publication attempt. If
/// staging fails or the process returns early, Drop refunds the durable
/// maintenance allocation; a successful publication releases it explicitly
/// after replacement accounting has committed.
pub(super) struct MaintenanceBorrow {
    pub(super) catalog: Arc<Catalog>,
    pub(super) slug: String,
    pub(super) job_id: String,
    pub(super) bytes: i64,
    pub(super) released: bool,
}

impl MaintenanceBorrow {
    pub(super) fn release(&mut self) -> JournalResult<()> {
        if !self.released {
            self.catalog
                .release_maintenance(
                    &self.job_id,
                    &self.slug,
                    self.bytes,
                    crate::util::now_unix(),
                )
                .map_err(JournalError::from)?;
            self.released = true;
        }
        Ok(())
    }
}

impl Drop for MaintenanceBorrow {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.catalog.release_maintenance(
                &self.job_id,
                &self.slug,
                self.bytes,
                crate::util::now_unix(),
            );
        }
    }
}

impl JournalRuntime {
    pub fn new(
        catalog: Arc<Catalog>,
        blobs: Arc<dyn BlobStore>,
        deployment_id: impl Into<String>,
        limits: CoordinatorLimits,
    ) -> JournalResult<Arc<Self>> {
        Self::new_with_limits(catalog, blobs, deployment_id, limits, -1, -1)
    }

    pub fn new_with_limits(
        catalog: Arc<Catalog>,
        blobs: Arc<dyn BlobStore>,
        deployment_id: impl Into<String>,
        limits: CoordinatorLimits,
        owner_limit: i64,
        total_limit: i64,
    ) -> JournalResult<Arc<Self>> {
        if owner_limit < -1 || total_limit < -1 {
            return Err(JournalError::Invalid("invalid journal quota limits".into()));
        }
        Ok(Arc::new(Self {
            coordinator: AsyncMutex::new(JournalCoordinator::new(limits)?),
            publication: catalog.journal_gate.clone(),
            pending: AsyncMutex::new(HashSet::new()),
            pending_notify: Notify::new(),
            store: Arc::new(JournalStore::new(catalog)),
            blobs,
            deployment_id: deployment_id.into(),
            owner_limit,
            total_limit,
            next_operation: AtomicU64::new(1),
        }))
    }

    pub fn latest_sequence(&self, storage_id: &str, epoch: u64) -> JournalResult<u64> {
        self.store.latest_sequence(storage_id, epoch)
    }

    /// Gate shared by journal publication, manifest reclamation, and journal
    /// retirement. Callers must hold it across reachability reads, immutable
    /// object writes, and the catalogue pointer transition.
    pub fn publication_gate(&self) -> Arc<AsyncMutex<()>> {
        Arc::clone(&self.publication)
    }

    pub fn compaction_due(
        &self,
        storage_id: &str,
        epoch: u64,
        sequence: u64,
    ) -> JournalResult<bool> {
        self.store.compaction_due(storage_id, epoch, sequence)
    }

    pub fn retire_storage(&self, storage_id: &str, retired_at: i64) -> JournalResult<()> {
        self.store.retire_storage(storage_id, retired_at)
    }

    /// Invalidate the derived manifest graph and retire one document while the
    /// same gate excludes publication and journal retirement workers.
    pub async fn retire_storage_with_manifest(
        &self,
        storage_id: &str,
        retired_at: i64,
    ) -> JournalResult<()> {
        let _publication = self.publication.lock().await;
        self.store.retire_storage(storage_id, retired_at)
    }

    pub fn acquire_reader(
        &self,
        reader_id: &str,
        object_key: &str,
        opened_at: i64,
        expires_at: i64,
    ) -> JournalResult<()> {
        self.store
            .catalog
            .acquire_journal_reader(reader_id, object_key, opened_at, expires_at)
            .map_err(JournalError::from)
    }

    pub fn renew_reader(
        &self,
        reader_id: &str,
        heartbeat_at: i64,
        expires_at: i64,
    ) -> JournalResult<bool> {
        self.store
            .catalog
            .renew_journal_reader(reader_id, heartbeat_at, expires_at)
            .map_err(JournalError::from)
    }

    pub fn release_reader(&self, reader_id: &str) -> JournalResult<bool> {
        self.store
            .catalog
            .release_journal_reader(reader_id)
            .map_err(JournalError::from)
    }

    /// Finish or refund journal-object accounting left between the durable
    /// SQL publication and its small accounting transaction. Journal object
    /// keys are operation-unique, so an existing object is an unambiguous
    /// commit; a missing object is a known abort. Other product reservations
    /// are intentionally handled by their owning subsystem.
    pub async fn reconcile_object_reservations(&self) -> JournalResult<()> {
        self.store
            .reconcile_object_reservations(self.blobs.as_ref())
            .await
    }

    pub async fn append(
        &self,
        storage_id: &str,
        sequence: u64,
        payload: Vec<u8>,
    ) -> JournalResult<Vec<i64>> {
        self.append_with_epoch(storage_id, 0, sequence, payload)
            .await
    }

    /// Append one complete-state record for an epoch.  A retry of an already
    /// committed `(storage, epoch, sequence)` is a no-op: callers can safely
    /// retry after an unknown object/SQL outcome without creating a second
    /// durable record for the same acknowledgement cursor.
    pub async fn append_with_epoch(
        &self,
        storage_id: &str,
        epoch: u64,
        sequence: u64,
        payload: Vec<u8>,
    ) -> JournalResult<Vec<i64>> {
        if storage_id.is_empty() || sequence == 0 {
            return Err(JournalError::Invalid(
                "invalid journal append identity".into(),
            ));
        }
        let identity = (storage_id.to_owned(), epoch, sequence);
        loop {
            let wait = {
                let mut pending = self.pending.lock().await;
                if pending.insert(identity.clone()) {
                    None
                } else {
                    Some(self.pending_notify.notified())
                }
            };
            if let Some(wait) = wait {
                wait.await;
                continue;
            }
            break;
        }
        let retry_id = format!("room-{storage_id}-{epoch}-{sequence}");
        let records = match JournalRecord::chunked(storage_id, sequence, &retry_id, epoch, payload)
        {
            Ok(records) => records,
            Err(error) => {
                self.finish_pending(std::iter::once(identity)).await;
                return Err(error);
            }
        };
        let record_payload = records
            .iter()
            .flat_map(|record| record.payload.iter().copied())
            .collect::<Vec<_>>();
        {
            let mut coordinator = self.coordinator.lock().await;
            if !coordinator.contains_identity(storage_id, epoch, sequence) {
                if let Err(error) = coordinator.enqueue_batch(records) {
                    self.finish_pending(std::iter::once(identity)).await;
                    return Err(error);
                }
            }
        }
        // Give other ready writers one scheduling turn to enqueue before the
        // first writer seals the deployment-wide round.  The publication gate
        // remains the serialization point; this yield only widens the batch
        // at the cheap in-memory boundary.
        tokio::task::yield_now().await;
        // Enqueue before taking the publication gate.  Object I/O happens
        // after sealing, so callers that arrive during that I/O can join the
        // next seal (or the current one if the coordinator has not sealed
        // yet) instead of every room producing a one-record segment.
        let _publication = self.publication.lock().await;
        match self.store.unresolved_preparation() {
            Ok(Some(_)) => {
                if let Err(error) = self.store.reconcile_pending(self.blobs.as_ref()).await {
                    self.coordinator.lock().await.remove_identity(
                        &identity.0,
                        identity.1,
                        identity.2,
                    );
                    self.finish_pending(std::iter::once(identity)).await;
                    return Err(error);
                }
                if let Err(error) = self.reconcile_object_reservations().await {
                    self.coordinator.lock().await.remove_identity(
                        &identity.0,
                        identity.1,
                        identity.2,
                    );
                    self.finish_pending(std::iter::once(identity)).await;
                    return Err(error);
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.coordinator
                    .lock()
                    .await
                    .remove_identity(&identity.0, identity.1, identity.2);
                self.finish_pending(std::iter::once(identity)).await;
                return Err(error);
            }
        }
        match self
            .committed_payload_status(storage_id, epoch, sequence, &record_payload)
            .await
        {
            Ok(Some(true)) => {
                self.coordinator
                    .lock()
                    .await
                    .remove_identity(storage_id, epoch, sequence);
                self.finish_pending(std::iter::once(identity)).await;
                return Ok(Vec::new());
            }
            Ok(Some(false)) => {
                self.coordinator
                    .lock()
                    .await
                    .remove_identity(storage_id, epoch, sequence);
                self.finish_pending(std::iter::once(identity)).await;
                return Err(JournalError::Invalid(format!(
                    "journal sequence {storage_id}/{epoch}/{sequence} has a different payload"
                )));
            }
            Ok(None) => {}
            Err(error) => {
                self.finish_pending(std::iter::once(identity)).await;
                return Err(error);
            }
        }
        let seal_result = {
            let mut coordinator = self.coordinator.lock().await;
            match coordinator.seal(true) {
                Ok(segments) => Ok(segments),
                Err(error) => {
                    coordinator.remove_identity(&identity.0, identity.1, identity.2);
                    Err(error)
                }
            }
        };
        let segments = match seal_result {
            Ok(segments) => segments,
            Err(error) => {
                self.finish_pending(std::iter::once(identity)).await;
                return Err(error);
            }
        };
        let identities = segment_identities(&segments);
        let operation_id = format!(
            "room-{}-{}-{}-{}",
            storage_id,
            sequence,
            crate::util::now_unix(),
            self.next_operation.fetch_add(1, Ordering::Relaxed)
        );
        let state = match self.store.state() {
            Ok(state) => state,
            Err(error) => {
                self.coordinator.lock().await.requeue(segments);
                self.finish_pending(identities.into_iter()).await;
                return Err(error);
            }
        };
        let output_keys = (0..segments.len())
            .map(|index| {
                journal_segment_key(&self.deployment_id, &format!("{operation_id}-{index}"))
            })
            .collect();
        let plan = JournalPlan {
            version: 1,
            output_keys,
            covered: covered_ranges(&segments),
            protected_input_keys: Vec::new(),
        };
        if let Err(error) = self.store.prepare(
            &operation_id,
            "flush",
            state.revision,
            &state.writer_generation,
            crate::util::now_unix(),
            &plan,
        ) {
            self.coordinator.lock().await.requeue(segments);
            self.finish_pending(identities.into_iter()).await;
            return Err(error);
        }
        let written = match self.write_segments(&operation_id, &segments).await {
            Ok(written) => written,
            Err(error) => {
                let _ = self
                    .recover_failed_flush(&operation_id, segments, &identity)
                    .await;
                self.finish_pending(identities.into_iter()).await;
                return Err(error);
            }
        };
        let result = self
            .store
            .commit_segments(&operation_id, &written, crate::util::now_unix());
        let (_, sequences) = match result {
            Ok(value) => value,
            Err(error) => {
                let recovered = self
                    .recover_failed_flush(&operation_id, segments, &identity)
                    .await;
                self.finish_pending(identities.into_iter()).await;
                if let Some(sequences) = recovered {
                    return Ok(sequences);
                }
                return Err(error);
            }
        };
        for (segment, written_segment) in segments.iter().zip(&written) {
            if let Some(record) = segment.records.first() {
                if let Err(error) = self.store.commit_object(
                    &record.storage_id,
                    &operation_id,
                    &written_segment.object_key,
                    "journal_segment",
                    &written_segment.digest,
                ) {
                    // The SQL head is already durable. Keep reservations in
                    // place and let the object-accounting reconciler finish
                    // them after a transient catalogue failure.
                    let _ = self.reconcile_object_reservations().await;
                    self.finish_pending(identities.into_iter()).await;
                    return Err(error);
                }
            }
        }
        self.finish_pending(identities.into_iter()).await;
        Ok(sequences)
    }

    pub(super) async fn finish_pending(
        &self,
        identities: impl Iterator<Item = (String, u64, u64)>,
    ) {
        let mut pending = self.pending.lock().await;
        for identity in identities {
            pending.remove(&identity);
        }
        drop(pending);
        self.pending_notify.notify_waiters();
    }

    /// Reconcile a failed flush while the publication gate is still held.
    /// Known aborts return the unaffected records to the queue and discard the
    /// identity whose request was rejected. Ambiguous failures keep the
    /// preparation and durable reservations for startup reconciliation.
    async fn recover_failed_flush(
        &self,
        operation_id: &str,
        segments: Vec<Segment>,
        failed_identity: &(String, u64, u64),
    ) -> Option<Vec<i64>> {
        if self
            .store
            .reconcile_pending(self.blobs.as_ref())
            .await
            .is_err()
        {
            return None;
        }
        if self.store.unresolved_preparation().ok().flatten().is_some() {
            return None;
        }
        let sequences = self.store.operation_sequences(operation_id).ok()?;
        if !sequences.is_empty() {
            return Some(sequences);
        }
        let mut coordinator = self.coordinator.lock().await;
        coordinator.requeue(vec![Segment {
            records: segments
                .into_iter()
                .flat_map(|segment| segment.records)
                .collect(),
        }]);
        coordinator.remove_identity(&failed_identity.0, failed_identity.1, failed_identity.2);
        None
    }

    pub(super) async fn committed_payload_status(
        &self,
        storage_id: &str,
        epoch: u64,
        sequence: u64,
        payload: &[u8],
    ) -> JournalResult<Option<bool>> {
        if !self.store.sequence_committed(storage_id, epoch, sequence)? {
            return Ok(None);
        }
        let expected_digest = hex::encode(Sha256::digest(payload));
        let candidates =
            self.store
                .committed_segments_for(storage_id, epoch, sequence.saturating_sub(1))?;
        if candidates.is_empty() {
            // The record has already been folded into a recovery base. Keep
            // the exact payload check for the base cursor itself; older
            // cursors are closed by compaction and must not silently accept a
            // different payload merely because the sequence is covered.
            let Some(base) = self.store.recovery_base(storage_id)? else {
                return Err(JournalError::Corrupt(format!(
                    "journal sequence {storage_id}/{epoch}/{sequence} is covered without a base"
                )));
            };
            if base.epoch != epoch || base.sequence != sequence {
                return Ok(Some(false));
            }
            let body = self.blobs.get(&base.object_key).await?;
            if hex::encode(Sha256::digest(&body)) != base.digest {
                return Err(JournalError::Corrupt(format!(
                    "recovery base digest mismatch for {}",
                    base.object_key
                )));
            }
            let recovered = decode_recovery_base(&body).map_err(|error| {
                JournalError::Corrupt(format!(
                    "invalid recovery base {}: {error}",
                    base.object_key
                ))
            })?;
            if recovered.storage_id != storage_id
                || recovered.epoch != epoch
                || recovered.sequence != sequence
                || recovered.digest != hex::encode(Sha256::digest(&recovered.payload))
            {
                return Err(JournalError::Corrupt(format!(
                    "recovery base metadata mismatch for {}",
                    base.object_key
                )));
            }
            return Ok(Some(recovered.payload == payload));
        }
        let mut fragments = Vec::new();
        for (key, digest, _) in candidates {
            let body = self.blobs.get(&key).await?;
            if hex::encode(Sha256::digest(&body)) != digest {
                return Err(JournalError::Corrupt(format!(
                    "journal segment digest mismatch for {key}"
                )));
            }
            let segment = Segment::decode_for(&body, storage_id)?;
            fragments.extend(segment.records.into_iter().filter(|record| {
                record.storage_id == storage_id
                    && record.epoch == epoch
                    && record.sequence == sequence
            }));
        }
        if !fragments.is_empty() {
            let reconstructed = assemble_fragments(&fragments)?;
            return Ok(Some(
                hex::encode(Sha256::digest(&reconstructed)) == expected_digest
                    && reconstructed == payload,
            ));
        }
        Err(JournalError::Corrupt(format!(
            "journal sequence {storage_id}/{epoch}/{sequence} is covered but absent"
        )))
    }

    pub(super) async fn write_segments(
        &self,
        operation_id: &str,
        segments: &[Segment],
    ) -> JournalResult<Vec<WrittenSegment>> {
        let mut result = Vec::with_capacity(segments.len());
        for (index, segment) in segments.iter().enumerate() {
            let id = format!("{operation_id}-{index}");
            let body = segment.encode()?;
            let digest = hex::encode(Sha256::digest(&body));
            let key = journal_segment_key(&self.deployment_id, &id);
            let owner = segment
                .records
                .first()
                .map(|record| record.storage_id.as_str())
                .ok_or_else(|| JournalError::Invalid("empty journal segment".into()))?;
            // Earlier segments may already be durable. Keep reservations
            // until reconciliation checks the output set and queues cleanup.
            self.store.reserve_object(
                owner,
                operation_id,
                &key,
                "journal_segment",
                body.len() as i64,
                self.owner_limit,
                self.total_limit,
            )?;
            if let Err(error) = self
                .blobs
                .put(&key, body.clone(), "application/octet-stream")
                .await
            {
                // The object store may have accepted the write before
                // reporting an error. Leave reservations durable until the
                // preparation reconciler proves which objects exist.
                return Err(JournalError::from(error));
            }
            result.push(WrittenSegment {
                segment_id: id,
                object_key: key,
                digest,
                encoded_bytes: body.len() as i64,
            });
        }
        Ok(result)
    }

    /// Write a complete recovery base and atomically advance the catalogue's
    /// manifest pointer.  Segments through `sequence` are retired only by the
    /// same SQL transition; the maintenance worker deletes their objects later.
    pub async fn compact(
        &self,
        storage_id: &str,
        epoch: u64,
        sequence: u64,
        payload: Vec<u8>,
    ) -> JournalResult<JournalState> {
        if storage_id.is_empty() || sequence == 0 || payload.is_empty() {
            return Err(JournalError::Invalid(
                "invalid journal compaction identity".into(),
            ));
        }
        let _publication = self.publication.lock().await;
        if self.store.unresolved_preparation()?.is_some() {
            self.store.reconcile_pending(self.blobs.as_ref()).await?;
            self.reconcile_object_reservations().await?;
        }
        let state = self.store.state()?;
        let operation_id = format!(
            "compact-{}-{}-{}-{}",
            storage_id,
            sequence,
            crate::util::now_unix(),
            self.next_operation.fetch_add(1, Ordering::Relaxed)
        );
        let base_key = journal_base_key(
            &self.deployment_id,
            storage_id,
            &format!("{epoch}-{sequence}"),
        );
        let payload_digest = hex::encode(Sha256::digest(&payload));
        let base_body = RecoveryBaseBody {
            format_version: SEGMENT_FORMAT,
            storage_id: storage_id.to_owned(),
            epoch,
            sequence,
            payload,
            digest: payload_digest,
        };
        let base_bytes = encode_recovery_base(&base_body)?;
        let base_digest = hex::encode(Sha256::digest(&base_bytes));
        let base = RecoveryBase {
            base_id: format!("{storage_id}-{epoch}-{sequence}"),
            storage_id: storage_id.to_owned(),
            epoch,
            sequence,
            object_key: base_key.clone(),
            digest: base_digest,
            encoded_bytes: base_bytes.len() as i64,
            committed_at: crate::util::now_unix(),
        };
        let descriptors = self
            .store
            .committed_segments_through(storage_id, epoch, sequence)?;
        let retire_segments: Vec<(String, i64)> = descriptors
            .iter()
            .map(|(key, _, encoded_bytes)| (key.clone(), *encoded_bytes))
            .collect();
        let retired_keys: std::collections::HashSet<&str> = retire_segments
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        let mut bases = self.store.recovery_bases()?;
        bases.retain(|existing| existing.storage_id != storage_id);
        bases.push(base.clone());
        let tail: Vec<String> = self
            .store
            .committed_segments()?
            .into_iter()
            .map(|(key, _)| key)
            .filter(|key| !retired_keys.contains(key.as_str()))
            .collect();
        let shard_seq = u64::try_from(state.revision.saturating_add(1)).unwrap_or(u64::MAX);
        let shards = build_manifest_shards(
            &self.deployment_id,
            shard_seq,
            base.committed_at,
            bases,
            tail,
        )?;
        let manifest_bytes = shards
            .iter()
            .map(|shard| {
                finalize_manifest_shard(shard.clone()).map(|(_, bytes)| bytes.len() as i64)
            })
            .try_fold(0i64, |total, bytes| {
                bytes.map(|bytes| total.saturating_add(bytes))
            })?;
        let maintenance_bytes = (base_bytes.len() as i64).saturating_add(manifest_bytes);
        if maintenance_bytes > JOURNAL_MAINTENANCE_RESERVE_BYTES {
            return Err(JournalError::Limit(
                "compaction exceeds the maintenance reserve".into(),
            ));
        }
        let shard_keys: Vec<String> = shards
            .iter()
            .map(|shard| shard.object_key.clone())
            .collect();
        let plan = JournalPlan {
            version: 1,
            output_keys: std::iter::once(base_key.clone())
                .chain(shard_keys.iter().cloned())
                .collect(),
            covered: vec![CoveredRange {
                storage_id: storage_id.to_owned(),
                epoch,
                first_sequence: sequence,
                last_sequence: sequence,
            }],
            protected_input_keys: retire_segments.iter().map(|(key, _)| key.clone()).collect(),
        };
        let mut maintenance_borrow = if maintenance_bytes > 0 {
            if let Some(slug) = self
                .store
                .catalog
                .slug_by_storage_id(storage_id)
                .map_err(JournalError::from)?
            {
                let job_id = format!("maintenance-{operation_id}");
                self.store
                    .catalog
                    .reserve_maintenance(
                        &job_id,
                        &slug,
                        maintenance_bytes,
                        JOURNAL_MAINTENANCE_RESERVE_BYTES,
                        base.committed_at,
                    )
                    .map_err(JournalError::from)?;
                Some(MaintenanceBorrow {
                    catalog: self.store.catalog.clone(),
                    slug,
                    job_id,
                    bytes: maintenance_bytes,
                    released: false,
                })
            } else {
                None
            }
        } else {
            None
        };
        // Reserve temporary maintenance headroom before creating the durable
        // preparation. A quota rejection therefore cannot strand a global
        // unresolved operation.
        let preparation = match self.store.prepare(
            &operation_id,
            "compact",
            state.revision,
            &state.writer_generation,
            base.committed_at,
            &plan,
        ) {
            Ok(preparation) => preparation,
            Err(error) => {
                drop(maintenance_borrow);
                return Err(error);
            }
        };
        // A maintenance borrow is the explicit headroom for compaction's
        // temporary copies. Ordinary quota checks must not charge that peak
        // a second time while the borrow is active.
        let (compaction_owner_limit, compaction_total_limit) = if maintenance_borrow.is_some() {
            (-1, -1)
        } else {
            (self.owner_limit, self.total_limit)
        };
        let mut accounting_keys = vec![(base_key.clone(), base_bytes.len() as i64)];
        if let Err(error) = self.store.reserve_object(
            storage_id,
            &operation_id,
            &base_key,
            "journal_base",
            base_bytes.len() as i64,
            compaction_owner_limit,
            compaction_total_limit,
        ) {
            let _ = self
                .store
                .abort_object(storage_id, &operation_id, &base_key);
            let _ = self.store.abort_preparation(&preparation, &plan, &[]);
            return Err(error);
        }
        if let Err(error) = self
            .blobs
            .put(&base_key, base_bytes, "application/octet-stream")
            .await
        {
            let _ = self.store.reconcile_pending(self.blobs.as_ref()).await;
            let _ = self.reconcile_object_reservations().await;
            return Err(JournalError::from(error));
        }
        let mut finalized = Vec::with_capacity(shards.len());
        for shard in shards {
            let (shard, shard_bytes) = finalize_manifest_shard(shard)?;
            if let Err(error) = self.store.reserve_object(
                storage_id,
                &operation_id,
                &shard.object_key,
                "journal_manifest",
                shard_bytes.len() as i64,
                compaction_owner_limit,
                compaction_total_limit,
            ) {
                let _ = self
                    .store
                    .abort_preparation(&preparation, &plan, &accounting_keys);
                return Err(error);
            }
            accounting_keys.push((shard.object_key.clone(), shard_bytes.len() as i64));
            if let Err(error) = self
                .blobs
                .put(&shard.object_key, shard_bytes, "application/json")
                .await
            {
                let _ = self.store.reconcile_pending(self.blobs.as_ref()).await;
                let _ = self.reconcile_object_reservations().await;
                return Err(JournalError::from(error));
            }
            finalized.push(shard);
        }
        let committed = match self.store.commit_compaction_shards(
            &operation_id,
            &base,
            &finalized,
            &retire_segments,
        ) {
            Ok(state) => state,
            Err(error) => {
                let _ = self.store.reconcile_pending(self.blobs.as_ref()).await;
                let _ = self.reconcile_object_reservations().await;
                return Err(error);
            }
        };
        for (key, _) in &accounting_keys {
            if let Err(error) = self.store.commit_object(
                storage_id,
                &operation_id,
                key,
                if key == &base_key {
                    "journal_base"
                } else {
                    "journal_manifest"
                },
                "",
            ) {
                let _ = self.reconcile_object_reservations().await;
                return Err(error);
            }
        }
        if let Some(borrow) = maintenance_borrow.as_mut() {
            borrow.release()?;
        }
        Ok(committed)
    }
}
