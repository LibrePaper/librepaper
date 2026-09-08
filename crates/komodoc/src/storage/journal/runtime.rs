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
    /// The one persistence policy this runtime enforces: `E` for what it will
    /// accept, `Q` through the coordinator, and `M` through `memory`.
    pub(super) persistence: crate::config::PersistenceLimits,
    pub(super) memory: Arc<MemoryBudget>,
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
    pub(super) async fn release(&mut self) -> JournalResult<()> {
        if !self.released {
            let catalog = self.catalog.clone();
            let job_id = self.job_id.clone();
            let slug = self.slug.clone();
            let bytes = self.bytes;
            catalog
                .execute_operation(job_id.len() + slug.len() + 64, move |catalog| {
                    catalog.release_maintenance(&job_id, &slug, bytes, crate::util::now_unix())
                })
                .await
                .map_err(JournalError::from)?;
            self.released = true;
        }
        Ok(())
    }
}

impl Drop for MaintenanceBorrow {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // Every error and cancellation path out of `compact` reaches this
        // refund, and a `Drop` has nowhere to await, so the release is handed
        // to a blocking thread instead of taking the connection on a runtime
        // worker.  It is service-owned from that point: the borrow is keyed by
        // its own job id and the release is conditional on that row, so it can
        // never refund a later compaction's headroom, and a release lost to
        // process exit leaves only a durable `maintenance_jobs` row that the
        // next reconciliation clears by the same id.
        let catalog = self.catalog.clone();
        let job_id = std::mem::take(&mut self.job_id);
        let slug = std::mem::take(&mut self.slug);
        let bytes = self.bytes;
        let release = move || {
            let _ = catalog.release_maintenance(&job_id, &slug, bytes, crate::util::now_unix());
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(release);
            }
            Err(_) => release(),
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
        Self::new_with_policy(
            catalog,
            blobs,
            deployment_id,
            limits,
            crate::config::PersistenceLimits::default(),
            owner_limit,
            total_limit,
        )
    }

    /// The constructor a deployment uses: it validates the persistence policy
    /// before anything can be appended, so a configuration that could accept
    /// work it can never durably save fails at startup rather than at the
    /// first oversized save.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_policy(
        catalog: Arc<Catalog>,
        blobs: Arc<dyn BlobStore>,
        deployment_id: impl Into<String>,
        limits: CoordinatorLimits,
        persistence: crate::config::PersistenceLimits,
        owner_limit: i64,
        total_limit: i64,
    ) -> JournalResult<Arc<Self>> {
        if owner_limit < -1 || total_limit < -1 {
            return Err(JournalError::Invalid("invalid journal quota limits".into()));
        }
        persistence.validate().map_err(JournalError::Invalid)?;
        if limits.max_queued_bytes < persistence.max_encoded_snapshot_bytes {
            return Err(JournalError::Invalid(
                "the journal queue cannot hold one maximum snapshot".into(),
            ));
        }
        let memory = MemoryBudget::new(persistence.max_staging_bytes);
        Ok(Arc::new(Self {
            persistence,
            memory,
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

    /// The persistence policy every caller of this runtime shares.
    pub fn persistence(&self) -> crate::config::PersistenceLimits {
        self.persistence
    }

    /// Payload bytes queued and executing against `Q`, in that order. What
    /// tests assert on, and what a diagnostic would report.
    pub async fn payload_bytes_in_flight(&self) -> (usize, usize) {
        let coordinator = self.coordinator.lock().await;
        (coordinator.queued_bytes(), coordinator.executing_bytes())
    }

    /// The memory admission `append` and `compact` charge their copies to.
    /// Exposed so tests can read its counters and so a caller that stages a
    /// snapshot of its own can be charged against the same budget.
    pub fn memory(&self) -> Arc<MemoryBudget> {
        Arc::clone(&self.memory)
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

    pub async fn compaction_due_async(
        &self,
        storage_id: String,
        epoch: u64,
        sequence: u64,
    ) -> JournalResult<bool> {
        self.store
            .compaction_due_async(storage_id, epoch, sequence)
            .await
    }

    pub async fn latest_sequence_async(
        &self,
        storage_id: String,
        epoch: u64,
    ) -> JournalResult<u64> {
        self.store.latest_sequence_async(storage_id, epoch).await
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
        self.store
            .retire_storage_async(storage_id.to_owned(), retired_at)
            .await
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
        if storage_id.len() > MAX_JOURNAL_IDENTITY_BYTES {
            return Err(JournalError::Invalid(
                "journal storage identity is past the framing limit".into(),
            ));
        }
        // `E`, at the last place before a payload becomes durable work. The
        // room refuses an oversized candidate before applying it; this is the
        // backstop for every other caller, so nothing can be acknowledged
        // that a recovery base could not carry back.
        if payload.len() > self.persistence.max_encoded_snapshot_bytes {
            return Err(JournalError::Limit(format!(
                "a snapshot of {} bytes is past the {} byte encoded ceiling",
                payload.len(),
                self.persistence.max_encoded_snapshot_bytes
            )));
        }
        // `M`, before the first large copy. The permit is owned for the whole
        // operation and released by Drop, so a cancelled append returns it
        // without an await in its unwind.
        let _memory = self
            .memory
            .acquire(crate::config::PersistenceLimits::staging_cost(
                payload.len(),
            ))
            .await?;
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
        match self.store.unresolved_preparation_async().await {
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
                // Charged as executing while the seal still holds the queue
                // mutex, so the round's bytes never leave `Q` unaccounted
                // between the queue and its object I/O. `_executing` is
                // released by Drop on every path out of this function,
                // cancellation included.
                Ok(segments) => {
                    let executing = coordinator.begin_executing(&segments);
                    Ok((segments, executing))
                }
                Err(error) => {
                    coordinator.remove_identity(&identity.0, identity.1, identity.2);
                    Err(error)
                }
            }
        };
        let (segments, _executing) = match seal_result {
            Ok(sealed) => sealed,
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
        let state = match self.store.state_async().await {
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
        if let Err(error) = self
            .store
            .prepare_async(
                operation_id.clone(),
                "flush",
                state.revision,
                state.writer_generation.clone(),
                crate::util::now_unix(),
                &plan,
            )
            .await
        {
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
            .commit_segments_async(
                operation_id.clone(),
                written.clone(),
                crate::util::now_unix(),
            )
            .await;
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
                if let Err(error) = self
                    .store
                    .commit_object_async(
                        record.storage_id.clone(),
                        operation_id.clone(),
                        written_segment.object_key.clone(),
                        "journal_segment".to_string(),
                        written_segment.digest.clone(),
                    )
                    .await
                {
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
        if self
            .store
            .unresolved_preparation_async()
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return None;
        }
        let sequences = self
            .store
            .operation_sequences(operation_id.to_owned())
            .await
            .ok()?;
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
        if !self
            .store
            .sequence_committed(storage_id.to_owned(), epoch, sequence)
            .await?
        {
            return Ok(None);
        }
        let expected_digest = hex::encode(Sha256::digest(payload));
        let candidates = self
            .store
            .committed_segments_for(storage_id.to_owned(), epoch, sequence.saturating_sub(1))
            .await?;
        if candidates.is_empty() {
            // The record has already been folded into a recovery base. Keep
            // the exact payload check for the base cursor itself; older
            // cursors are closed by compaction and must not silently accept a
            // different payload merely because the sequence is covered.
            let Some(base) = self.store.recovery_base(storage_id.to_owned()).await? else {
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
            self.store
                .reserve_object_async(
                    owner.to_owned(),
                    operation_id.to_owned(),
                    key.clone(),
                    "journal_segment",
                    body.len() as i64,
                    self.owner_limit,
                    self.total_limit,
                )
                .await?;
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
        if storage_id.len() > MAX_JOURNAL_IDENTITY_BYTES {
            return Err(JournalError::Invalid(
                "journal storage identity is past the framing limit".into(),
            ));
        }
        if payload.len() > self.persistence.max_encoded_snapshot_bytes {
            return Err(JournalError::Limit(format!(
                "a recovery base of {} bytes is past the {} byte encoded ceiling",
                payload.len(),
                self.persistence.max_encoded_snapshot_bytes
            )));
        }
        // Compaction holds the payload, the framed base body, and the
        // manifest shards at once, and it overlaps whatever appends are
        // already in flight. Admitted against the same budget, before the
        // first of those copies is made.
        let _memory = self
            .memory
            .acquire(crate::config::PersistenceLimits::staging_cost(
                payload.len(),
            ))
            .await?;
        let _publication = self.publication.lock().await;
        if self.store.unresolved_preparation_async().await?.is_some() {
            self.store.reconcile_pending(self.blobs.as_ref()).await?;
            self.reconcile_object_reservations().await?;
        }
        let state = self.store.state_async().await?;
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
            .committed_segments_through(storage_id.to_owned(), epoch, sequence)
            .await?;
        let retire_segments: Vec<(String, i64)> = descriptors
            .iter()
            .map(|(key, _, encoded_bytes)| (key.clone(), *encoded_bytes))
            .collect();
        let retired_keys: std::collections::HashSet<&str> = retire_segments
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        let mut bases = self.store.recovery_bases().await?;
        bases.retain(|existing| existing.storage_id != storage_id);
        bases.push(base.clone());
        let tail: Vec<String> = self
            .store
            .committed_segments_async()
            .await?
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
                .execute_operation(storage_id.len() + 64, {
                    let storage_id = storage_id.to_owned();
                    move |catalog| catalog.slug_by_storage_id(&storage_id)
                })
                .await
                .map_err(JournalError::from)?
            {
                let job_id = format!("maintenance-{operation_id}");
                self.store
                    .catalog
                    .execute_operation(job_id.len() + slug.len() + 64, {
                        let job_id = job_id.clone();
                        let slug = slug.clone();
                        let committed_at = base.committed_at;
                        move |catalog| {
                            catalog.reserve_maintenance(
                                &job_id,
                                &slug,
                                maintenance_bytes,
                                JOURNAL_MAINTENANCE_RESERVE_BYTES,
                                committed_at,
                            )
                        }
                    })
                    .await
                    // The reserve is shared with every other compaction, so
                    // being turned away by it is capacity, not a document
                    // that can never be compacted.
                    .map_err(|error| {
                        JournalError::Busy(format!(
                            "the compaction maintenance reserve is full: {error}"
                        ))
                    })?;
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
        let preparation = match self
            .store
            .prepare_async(
                operation_id.clone(),
                "compact",
                state.revision,
                state.writer_generation.clone(),
                base.committed_at,
                &plan,
            )
            .await
        {
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
        if let Err(error) = self
            .store
            .reserve_object_async(
                storage_id.to_owned(),
                operation_id.clone(),
                base_key.clone(),
                "journal_base",
                base_bytes.len() as i64,
                compaction_owner_limit,
                compaction_total_limit,
            )
            .await
        {
            let _ = self
                .store
                .abort_object_async(
                    storage_id.to_owned(),
                    operation_id.clone(),
                    base_key.clone(),
                )
                .await;
            let _ = self
                .store
                .abort_preparation(preparation.clone(), plan.clone(), Vec::new())
                .await;
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
            if let Err(error) = self
                .store
                .reserve_object_async(
                    storage_id.to_owned(),
                    operation_id.clone(),
                    shard.object_key.clone(),
                    "journal_manifest",
                    shard_bytes.len() as i64,
                    compaction_owner_limit,
                    compaction_total_limit,
                )
                .await
            {
                let _ = self
                    .store
                    .abort_preparation(preparation.clone(), plan.clone(), accounting_keys.clone())
                    .await;
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
        let committed = match self
            .store
            .commit_compaction_shards(
                operation_id.clone(),
                base.clone(),
                finalized,
                retire_segments.clone(),
            )
            .await
        {
            Ok(state) => state,
            Err(error) => {
                let _ = self.store.reconcile_pending(self.blobs.as_ref()).await;
                let _ = self.reconcile_object_reservations().await;
                return Err(error);
            }
        };
        for (key, _) in &accounting_keys {
            if let Err(error) = self
                .store
                .commit_object_async(
                    storage_id.to_owned(),
                    operation_id.clone(),
                    key.clone(),
                    if key == &base_key {
                        "journal_base".to_string()
                    } else {
                        "journal_manifest".to_string()
                    },
                    String::new(),
                )
                .await
            {
                let _ = self.reconcile_object_reservations().await;
                return Err(error);
            }
        }
        if let Some(borrow) = maintenance_borrow.as_mut() {
            borrow.release().await?;
        }
        Ok(committed)
    }
}
