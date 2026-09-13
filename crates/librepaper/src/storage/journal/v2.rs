//! Per-document v2 journal objects.
//!
//! A segment is a physical object owned by one document.  This module keeps
//! the framing code independent from the catalog transaction that registers
//! an object, so callers can perform bounded I/O first and hand the resulting
//! descriptors to the typed `journal_append`/`journal_compact` mutation.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::storage::blob::{v2_object_key, write_v2_object_with_id, BlobError, BlobStore, ObjectId, WrittenObject};

use super::{JournalError, JournalRecord, JournalResult, Segment};

pub const JOURNAL_OBJECT_CONTENT_TYPE: &str = "application/vnd.librepaper.journal-segment";

/// Document-facing journal boundary used by rooms. Implementations own the
/// catalog admission and the immutable segment/base writes; rooms only supply
/// a document id and a complete encoded snapshot. All cursor reads are async
/// because v2 cursors come from bounded catalog pages rather than a shared
/// deployment manifest.
#[async_trait::async_trait]
pub trait DocumentJournal: Send + Sync {
    async fn latest_sequence(&self, document_id: &str, epoch: u64) -> JournalResult<u64>;
    async fn recover_latest(&self, document_id: &str) -> JournalResult<Option<Vec<u8>>>;
    async fn append(&self, document_id: &str, sequence: u64, body: Vec<u8>) -> JournalResult<()>;
    async fn compaction_due(&self, document_id: &str, epoch: u64, sequence: u64) -> JournalResult<bool>;
    async fn compact(&self, document_id: &str, epoch: u64, sequence: u64, body: Vec<u8>) -> JournalResult<()>;
    async fn payload_bytes_in_flight(&self) -> (usize, usize);
    fn memory(&self) -> std::sync::Arc<crate::storage::journal::MemoryBudget>;
}

/// Shared bounded memory for a document journal implementation. The v2
/// runtime uses it for source snapshot staging and payload admission; keeping
/// this boundary on the room-facing trait prevents unbounded cancellation
/// retries from bypassing persistence limits.
pub fn document_journal_memory() -> std::sync::Arc<crate::storage::journal::MemoryBudget> {
    crate::storage::journal::MemoryBudget::new(64 * 1024 * 1024)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalAppendAdmission {
    pub operation_id: String,
    pub document_id: String,
    pub epoch: u64,
    pub first_sequence: u64,
    pub expected_last_sequence: u64,
    pub source_generation: u64,
    pub writer_generation: String,
    pub allocations: Vec<JournalObjectAllocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalAppendRequest {
    pub document_id: String,
    pub actor_key: String,
    pub request_key: String,
    pub expected_source_generation: u64,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub parts: Vec<JournalPartAdmission>,
    /// Already admitted immutable source/asset objects referenced by this
    /// append. They become live roots in the same transaction that
    /// acknowledges the journal append.
    pub dependencies: Vec<JournalDependency>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalDependency {
    pub object_id: ObjectId,
    pub kind: String,
    pub digest: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalPartAdmission {
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub digest: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalObjectAllocation {
    pub object_id: ObjectId,
    pub storage_key: String,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalObjectRef {
    pub object_id: String,
    pub storage_key: String,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub digest: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalHead {
    pub epoch: u64,
    pub sequence: u64,
    pub source_generation: u64,
    pub base_sequence: u64,
    pub base: Option<JournalObjectRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalCompactionAdmission {
    pub operation_id: String,
    pub document_id: String,
    pub captured_epoch: u64,
    pub captured_sequence: u64,
    pub captured_source_generation: u64,
    pub writer_generation: String,
    pub base_allocation: JournalObjectAllocation,
}

/// SQL-side hooks for the v2 append protocol. The implementation must use an
/// immediate transaction for prepare/commit and must keep the operation row
/// prepared until every object has settled. It also owns object/counter rows;
/// this trait carries descriptors only, never unbounded object lists.
#[async_trait::async_trait]
pub trait V2JournalCatalog: Send + Sync {
    fn physical_namespace(&self) -> usize {
        self as *const Self as *const () as usize
    }
    async fn journal_head(&self, document_id: &str) -> Result<JournalHead, String>;
    async fn prepare_append(&self, request: JournalAppendRequest)
        -> Result<JournalAppendAdmission, String>;
    async fn commit_append(
        &self,
        admission: JournalAppendAdmission,
        objects: Vec<WrittenJournalObject>,
    ) -> Result<(), String>;
    async fn abort_append(&self, operation_id: &str) -> Result<(), String>;
    async fn journal_objects(
        &self,
        document_id: &str,
        epoch: u64,
        after_sequence: u64,
        after_object_id: &str,
        limit: usize,
    ) -> Result<Vec<JournalObjectRef>, String>;
    async fn prepare_compaction(
        &self,
        document_id: &str,
        expected_epoch: u64,
        expected_sequence: u64,
        byte_length: u64,
        digest: String,
    ) -> Result<JournalCompactionAdmission, String>;
    async fn commit_compaction(
        &self,
        admission: JournalCompactionAdmission,
        base: WrittenObject,
        new_epoch: u64,
        new_sequence: u64,
    ) -> Result<(), String>;
    async fn abort_compaction(&self, operation_id: &str) -> Result<(), String>;
    async fn journal_compaction_due(
        &self,
        document_id: &str,
        epoch: u64,
        sequence: u64,
    ) -> Result<bool, String>;
}

pub struct V2JournalRuntime<C> {
    catalog: Arc<C>,
    blobs: Arc<dyn BlobStore>,
    memory: Arc<crate::storage::journal::MemoryBudget>,
    max_encoded_snapshot_bytes: usize,
    executing_bytes: AtomicUsize,
}

impl<C> V2JournalRuntime<C>
where
    C: V2JournalCatalog + 'static,
{
    pub fn new(catalog: std::sync::Arc<C>, blobs: std::sync::Arc<dyn BlobStore>) -> Self {
        Self::with_persistence(catalog, blobs, crate::config::PersistenceLimits::default())
            .expect("default persistence limits are valid")
    }

    pub fn with_memory(
        catalog: Arc<C>,
        blobs: Arc<dyn BlobStore>,
        memory: Arc<crate::storage::journal::MemoryBudget>,
    ) -> Self {
        Self {
            catalog,
            blobs,
            max_encoded_snapshot_bytes: memory.capacity(),
            memory,
            executing_bytes: AtomicUsize::new(0),
        }
    }

    pub fn with_persistence(
        catalog: Arc<C>,
        blobs: Arc<dyn BlobStore>,
        persistence: crate::config::PersistenceLimits,
    ) -> JournalResult<Self> {
        persistence.validate().map_err(JournalError::Invalid)?;
        Ok(Self {
            catalog,
            blobs,
            max_encoded_snapshot_bytes: persistence.max_encoded_snapshot_bytes,
            memory: crate::storage::journal::MemoryBudget::new(persistence.max_staging_bytes),
            executing_bytes: AtomicUsize::new(0),
        })
    }

    pub async fn append(
        &self,
        request: JournalAppendRequest,
        segments: &[Segment],
    ) -> JournalResult<Vec<WrittenJournalObject>> {
        append_segments(self.catalog.as_ref(), self.blobs.clone(), request, segments).await
    }

    /// Read all catalogued segment descriptors in bounded pages and fail
    /// closed if an acknowledged range is missing or an object disappears.
    pub async fn recover(
        &self,
        document_id: &str,
        epoch: u64,
        first_sequence: u64,
        last_sequence: u64,
    ) -> JournalResult<Vec<JournalRecord>> {
        if first_sequence == 0 || last_sequence < first_sequence {
            return Err(JournalError::Invalid("invalid journal recovery range".into()));
        }
        // Keep one aggregate permit for every compressed body and decoded
        // fragment retained by this recovery.  A per-object permit released
        // before inserting records into `bodies` would account only the I/O
        // buffer while allowing the acknowledged range to grow without a
        // process-wide memory charge.
        let _recovery_permit = self
            .memory
            .acquire(crate::config::PersistenceLimits::staging_cost(
                self.max_encoded_snapshot_bytes,
            ))
            .await?;
        let mut after = (first_sequence.saturating_sub(1), String::new());
        let mut bodies = Vec::new();
        let mut body_bytes = 0usize;
        loop {
            let page = self
                .catalog
                .journal_objects(document_id, epoch, after.0, &after.1, 128)
                .await
                .map_err(JournalError::CatalogText)?;
            if page.is_empty() {
                break;
            }
            if page.len() > 128 {
                return Err(JournalError::Corrupt("journal descriptor page exceeded bound".into()));
            }
            let previous_after = after.clone();
            for object in &page {
                if object.epoch != epoch {
                    return Err(JournalError::Corrupt("journal object epoch mismatch".into()));
                }
                if object.last_sequence < first_sequence || object.first_sequence > last_sequence {
                    return Err(JournalError::Corrupt("journal object lies outside acknowledged range".into()));
                }
                let bytes = self
                    .blobs
                    .get(&object.storage_key)
                    .await
                    .map_err(|error| JournalError::Storage(error.to_string()))?;
                verify_object_bytes(object, &bytes)?;
                body_bytes = body_bytes.saturating_add(bytes.len());
                if body_bytes > self.max_encoded_snapshot_bytes {
                    return Err(JournalError::Limit(
                        "journal recovery exceeds the configured encoded ceiling".into(),
                    ));
                }
                bodies.push(bytes);
                after = (object.last_sequence, object.object_id.clone());
            }
            if after <= previous_after {
                return Err(JournalError::Corrupt("journal descriptor cursor did not advance".into()));
            }
            if page.last().is_some_and(|object| object.last_sequence > last_sequence)
                || (page.len() < 128
                    && page.last().is_some_and(|object| object.last_sequence == last_sequence))
            {
                break;
            }
        }
        recover_records(bodies, document_id, epoch, Some(first_sequence), Some(last_sequence))
    }

    pub async fn compact(
        &self,
        document_id: &str,
        expected_epoch: u64,
        expected_sequence: u64,
        base: Vec<u8>,
        content_type: &str,
    ) -> JournalResult<WrittenObject> {
        let base_body = crate::storage::journal::RecoveryBaseBody {
            format_version: super::SEGMENT_FORMAT,
            storage_id: document_id.to_owned(),
            epoch: expected_epoch.saturating_add(1),
            sequence: expected_sequence,
            digest: hex::encode(Sha256::digest(&base)),
            payload: base,
        };
        let encoded_base = crate::storage::journal::encode_recovery_base(&base_body)?;
        let digest = hex::encode(Sha256::digest(&encoded_base));
        let admission = self
            .catalog
            .prepare_compaction(document_id, expected_epoch, expected_sequence, encoded_base.len() as u64, digest)
            .await
            .map_err(JournalError::CatalogText)?;
        let written = match guarded_write_v2_object(
            self.blobs.clone(),
            self.catalog.physical_namespace(),
            document_id,
            admission.base_allocation.object_id.clone(),
            encoded_base,
            content_type,
        ).await {
            Ok(written) => written,
            Err(error) => {
                let _ = self.catalog.abort_compaction(&admission.operation_id).await;
                return Err(JournalError::Storage(error.to_string()));
            }
        };
        if let Err(error) = self
            .catalog
            .commit_compaction(admission.clone(), written.clone(), expected_epoch.saturating_add(1), expected_sequence)
            .await
        {
            return Err(JournalError::CatalogText(error));
        }
        Ok(written)
    }
}

impl<C> V2JournalRuntime<C>
where
    C: V2JournalCatalog + 'static,
{
    async fn latest_sequence_v2(&self, document_id: &str, _epoch: u64) -> JournalResult<u64> {
        self.catalog
            .journal_head(document_id)
            .await
            .map(|head| head.sequence)
            .map_err(JournalError::CatalogText)
    }

    async fn recover_latest_v2(&self, document_id: &str) -> JournalResult<Option<Vec<u8>>> {
        let head = self
            .catalog
            .journal_head(document_id)
            .await
            .map_err(JournalError::CatalogText)?;
        if head.sequence == 0 && head.base.is_none() {
            return Ok(None);
        }
        let document = crate::document::session::new_doc();
        let mut has_state = false;
        if let Some(reference) = &head.base {
            if reference.epoch > head.epoch
                || reference.last_sequence != head.base_sequence
                || reference.first_sequence != head.base_sequence
            {
                return Err(JournalError::Corrupt("journal base does not match document head".into()));
            }
            let expected_bytes = usize::try_from(reference.byte_length)
                .map_err(|_| JournalError::Limit("journal base length exceeds memory accounting".into()))?;
            let permit = self
                .memory
                .acquire(crate::config::PersistenceLimits::staging_cost(expected_bytes))
                .await?;
            let bytes = self
                .blobs
                .get(&reference.storage_key)
                .await
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            verify_object_bytes(reference, &bytes)?;
            let decoded = crate::storage::journal::decode_recovery_base(&bytes)?;
            if decoded.storage_id != document_id
                || decoded.epoch != reference.epoch
                || decoded.sequence != reference.last_sequence
            {
                return Err(JournalError::Corrupt("journal base identity does not match catalog".into()));
            }
            crate::document::session::apply_update(&document, &decoded.payload)
                .map_err(|error| JournalError::Corrupt(format!("journal base update is invalid: {error}")))?;
            drop(permit);
            has_state = true;
        }
        if head.sequence == head.base_sequence {
            return if has_state {
                Ok(Some(crate::document::session::encode_state(&document)))
            } else {
                Err(JournalError::Corrupt("journal head range has no state".into()))
            };
        }
        let first = head.base_sequence.saturating_add(1).max(1);
        self.recover_into_document(document_id, head.epoch, first, head.sequence, &document)
            .await?;
        if !has_state && first > head.sequence {
            return Err(JournalError::Corrupt("journal head range has no state".into()));
        }
        Ok(Some(crate::document::session::encode_state(&document)))
    }

}

impl<C> V2JournalRuntime<C>
where
    C: V2JournalCatalog + 'static,
{
    /// Stream acknowledged segments into the CRDT document. The descriptor
    /// cursor is `(last_sequence, object_id)`, so fragments sharing a sequence
    /// cannot disappear when a page boundary cuts through that sequence. Only
    /// one physical object and the final fragment group are retained at once.
    async fn recover_into_document(
        &self,
        document_id: &str,
        epoch: u64,
        first_sequence: u64,
        last_sequence: u64,
        document: &yrs::Doc,
    ) -> JournalResult<()> {
        let mut cursor = (first_sequence.saturating_sub(1), String::new());
        let mut pending = BTreeMap::<u64, Vec<JournalRecord>>::new();
        let mut pending_bytes = 0usize;
        let mut next_sequence = first_sequence;
        // Retain the aggregate permit until every decoded fragment has either
        // been applied or the recovery returns.  This covers pending groups
        // between descriptor pages as well as the current decoded segment.
        let _recovery_permit = self
            .memory
            .acquire(crate::config::PersistenceLimits::staging_cost(
                self.max_encoded_snapshot_bytes,
            ))
            .await?;
        loop {
            let page = self
                .catalog
                .journal_objects(document_id, epoch, cursor.0, &cursor.1, 128)
                .await
                .map_err(JournalError::CatalogText)?;
            if page.is_empty() {
                break;
            }
            for reference in &page {
                if reference.epoch != epoch
                    || reference.last_sequence < first_sequence
                    || reference.first_sequence > last_sequence
                    || reference.last_sequence > last_sequence
                {
                    return Err(JournalError::Corrupt("journal object lies outside acknowledged range".into()));
                }
                let bytes = self
                    .blobs
                    .get(&reference.storage_key)
                    .await
                    .map_err(|error| JournalError::Storage(error.to_string()))?;
                verify_object_bytes(reference, &bytes)?;
                let decoded = decode_segment(&bytes, document_id, epoch)?;
                if decoded.first_sequence != reference.first_sequence
                    || decoded.last_sequence != reference.last_sequence
                {
                    return Err(JournalError::Corrupt("journal descriptor range does not match its object".into()));
                }
                for record in decoded.segment.records {
                    if record.sequence < first_sequence || record.sequence > last_sequence {
                        return Err(JournalError::Corrupt("journal record lies outside acknowledged range".into()));
                    }
                    pending_bytes = pending_bytes.saturating_add(record.payload.len());
                    pending.entry(record.sequence).or_default().push(record);
                }
                pending_bytes = pending_bytes.saturating_sub(flush_recovered_records(
                    &mut pending,
                    reference.last_sequence.saturating_sub(1),
                    document_id,
                    epoch,
                    &mut next_sequence,
                    document,
                )?);
                if pending_bytes > self.max_encoded_snapshot_bytes {
                    return Err(JournalError::Limit("journal fragment group exceeds the configured recovery ceiling".into()));
                }
            }
            let terminal = page.last().map(|reference| reference.last_sequence).unwrap_or(0);
            let last = page.last().expect("nonempty page");
            let next_cursor = (last.last_sequence, last.object_id.clone());
            if next_cursor <= cursor {
                return Err(JournalError::Corrupt("journal descriptor cursor did not advance".into()));
            }
            cursor = next_cursor;
            if terminal >= last_sequence && page.len() < 128 {
                break;
            }
        }
        pending_bytes = pending_bytes.saturating_sub(flush_recovered_records(
            &mut pending,
            last_sequence,
            document_id,
            epoch,
            &mut next_sequence,
            document,
        )?);
        if next_sequence != last_sequence.saturating_add(1) || !pending.is_empty() {
            return Err(JournalError::Corrupt("acknowledged journal range is missing".into()));
        }
        let _ = pending_bytes;
        Ok(())
    }
}

#[async_trait::async_trait]
impl<C> DocumentJournal for V2JournalRuntime<C>
where
    C: V2JournalCatalog + 'static,
{

    async fn latest_sequence(&self, document_id: &str, epoch: u64) -> JournalResult<u64> {
        self.latest_sequence_v2(document_id, epoch).await
    }

    async fn recover_latest(&self, document_id: &str) -> JournalResult<Option<Vec<u8>>> {
        self.recover_latest_v2(document_id).await
    }

    async fn append(&self, document_id: &str, sequence: u64, body: Vec<u8>) -> JournalResult<()> {
        if body.len() > self.max_encoded_snapshot_bytes {
            return Err(JournalError::Limit("journal snapshot exceeds the configured encoded ceiling".into()));
        }
        let head = self
            .catalog
            .journal_head(document_id)
            .await
            .map_err(JournalError::CatalogText)?;
        if sequence != head.sequence.saturating_add(1) {
            return Err(JournalError::Conflict("journal append is not the next sequence".into()));
        }
        let payload_bytes = body.len();
        let permit = self
            .memory
            .acquire(crate::config::PersistenceLimits::staging_cost(payload_bytes))
            .await?;
        self.executing_bytes.fetch_add(payload_bytes, Ordering::Relaxed);
        let result = async {
            let retry_id = format!("room-{document_id}-{sequence}");
            let records = JournalRecord::chunked(document_id, sequence, &retry_id, head.epoch, body)?;
            let segment = Segment::new(records)?;
            let request = JournalAppendRequest {
                document_id: document_id.to_owned(),
                actor_key: "room".into(),
                request_key: format!("room-{document_id}-{sequence}"),
                expected_source_generation: head.source_generation,
                epoch: head.epoch,
                first_sequence: sequence,
                last_sequence: sequence,
                parts: Vec::new(),
                dependencies: Vec::new(),
            };
            self.append(request, &[segment]).await.map(|_| ())
        }
        .await;
        self.executing_bytes.fetch_sub(payload_bytes, Ordering::Relaxed);
        drop(permit);
        result
    }

    async fn compaction_due(&self, document_id: &str, _epoch: u64, sequence: u64) -> JournalResult<bool> {
        let head = self
            .catalog
            .journal_head(document_id)
            .await
            .map_err(JournalError::CatalogText)?;
        self.catalog
            .journal_compaction_due(document_id, head.epoch, sequence)
            .await
            .map_err(JournalError::CatalogText)
    }

    async fn compact(&self, document_id: &str, _epoch: u64, sequence: u64, body: Vec<u8>) -> JournalResult<()> {
        if body.len() > self.max_encoded_snapshot_bytes {
            return Err(JournalError::Limit("journal base exceeds the configured encoded ceiling".into()));
        }
        let payload_bytes = body.len();
        let permit = self
            .memory
            .acquire(crate::config::PersistenceLimits::staging_cost(payload_bytes))
            .await?;
        self.executing_bytes.fetch_add(payload_bytes, Ordering::Relaxed);
        let result = async {
            let head = self
                .catalog
                .journal_head(document_id)
                .await
                .map_err(JournalError::CatalogText)?;
            if sequence != head.sequence {
                return Err(JournalError::Conflict("journal compaction cursor changed".into()));
            }
            self.compact(
                document_id,
                head.epoch,
                sequence,
                body,
                "application/vnd.librepaper.journal-base",
            )
            .await
            .map(|_| ())
        }
        .await;
        self.executing_bytes.fetch_sub(payload_bytes, Ordering::Relaxed);
        drop(permit);
        result
    }

    async fn payload_bytes_in_flight(&self) -> (usize, usize) {
        (0, self.executing_bytes.load(Ordering::Relaxed))
    }

    fn memory(&self) -> Arc<crate::storage::journal::MemoryBudget> {
        Arc::clone(&self.memory)
    }
}

fn flush_recovered_records(
    pending: &mut BTreeMap<u64, Vec<JournalRecord>>,
    through: u64,
    document_id: &str,
    epoch: u64,
    next_sequence: &mut u64,
    document: &yrs::Doc,
) -> JournalResult<usize> {
    let mut released_bytes = 0usize;
    let sequences = pending
        .range(..=through)
        .map(|(&sequence, _)| sequence)
        .collect::<Vec<_>>();
    for sequence in sequences {
        let parts = pending
            .remove(&sequence)
            .ok_or_else(|| JournalError::Corrupt("journal fragment group disappeared".into()))?;
        released_bytes = released_bytes.saturating_add(
            parts.iter().map(|part| part.payload.len()).sum::<usize>(),
        );
        if sequence != *next_sequence {
            return Err(JournalError::Corrupt("journal sequence gap".into()));
        }
        let first = parts
            .first()
            .ok_or_else(|| JournalError::Corrupt("empty journal fragment group".into()))?;
        let count = first.fragment_count;
        let retry_id = first.retry_id.clone();
        let digest = first.digest.clone();
        if parts.len() != count as usize
            || parts.iter().any(|part| {
                part.fragment_count != count
                    || part.retry_id != retry_id
                    || part.digest != digest
                    || part.epoch != epoch
            })
        {
            return Err(JournalError::Corrupt("incomplete journal record fragments".into()));
        }
        let mut parts = parts;
        parts.sort_by_key(|part| part.fragment_index);
        if parts.iter().enumerate().any(|(index, part)| part.fragment_index as usize != index) {
            return Err(JournalError::Corrupt("journal fragment index gap".into()));
        }
        let payload = parts
            .iter()
            .flat_map(|part| part.payload.iter().copied())
            .collect::<Vec<_>>();
        if hex::encode(Sha256::digest(&payload)) != digest {
            return Err(JournalError::Corrupt("complete journal record digest mismatch".into()));
        }
        let record = JournalRecord::new(document_id, sequence, retry_id, epoch, payload)?;
        crate::document::session::apply_update(document, &record.payload)
            .map_err(|error| JournalError::Corrupt(format!("journal update {sequence} is invalid: {error}")))?;
        *next_sequence = (*next_sequence).saturating_add(1);
    }
    Ok(released_bytes)
}

fn replay_complete_state(
    base: Option<Vec<u8>>,
    records: Vec<JournalRecord>,
) -> JournalResult<Option<Vec<u8>>> {
    if base.is_none() && records.is_empty() {
        return Err(JournalError::Corrupt("journal head range has no state".into()));
    }
    let document = crate::document::session::new_doc();
    if let Some(base) = base {
        crate::document::session::apply_update(&document, &base)
            .map_err(|error| JournalError::Corrupt(format!("journal base update is invalid: {error}")))?;
    }
    for record in records {
        crate::document::session::apply_update(&document, &record.payload).map_err(|error| {
            JournalError::Corrupt(format!("journal update {} is invalid: {error}", record.sequence))
        })?;
    }
    Ok(Some(crate::document::session::encode_state(&document)))
}

fn verify_object_bytes(reference: &JournalObjectRef, bytes: &[u8]) -> JournalResult<()> {
    if bytes.len() as u64 != reference.byte_length
        || hex::encode(Sha256::digest(bytes)) != reference.digest
    {
        return Err(JournalError::Corrupt("journal object digest or length mismatch".into()));
    }
    Ok(())
}

pub async fn append_segments(
    catalog: &dyn V2JournalCatalog,
    blobs: Arc<dyn BlobStore>,
    request: JournalAppendRequest,
    segments: &[Segment],
) -> JournalResult<Vec<WrittenJournalObject>> {
    let encoded = segments
        .iter()
        .map(|segment| {
            let object = DocumentSegment::new(request.document_id.clone(), request.epoch, segment.clone())?;
            let body = object.encode()?;
            let digest = hex::encode(Sha256::digest(&body));
            Ok((object, body, digest))
        })
        .collect::<JournalResult<Vec<_>>>()?;
    let mut request = request;
    request.parts = encoded
        .iter()
        .map(|(object, body, digest)| JournalPartAdmission {
            epoch: object.epoch,
            first_sequence: object.first_sequence,
            last_sequence: object.last_sequence,
            digest: digest.clone(),
            byte_length: body.len() as u64,
        })
        .collect();
    let admission = catalog
        .prepare_append(request)
        .await
        .map_err(JournalError::CatalogText)?;
    let written = match write_encoded_segments(blobs, catalog.physical_namespace(), &admission, encoded).await {
        Ok(written) => written,
        Err(error) => {
            let _ = catalog.abort_append(&admission.operation_id).await;
            return Err(error);
        }
    };
    let contiguous = !written.is_empty()
        && written.first().is_some_and(|object| object.first_sequence == admission.first_sequence)
        && written.last().is_some_and(|object| object.last_sequence == admission.expected_last_sequence)
        && written.windows(2).all(|objects| {
            objects[1].first_sequence <= objects[0].last_sequence.saturating_add(1)
        });
    if !contiguous {
        let _ = catalog.abort_append(&admission.operation_id).await;
        return Err(JournalError::Conflict("journal append range changed during staging".into()));
    }
    if let Err(error) = catalog.commit_append(admission.clone(), written.clone()).await {
        return Err(JournalError::CatalogText(error));
    }
    Ok(written)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSegment {
    pub document_id: String,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub segment: Segment,
}

impl DocumentSegment {
    pub fn new(document_id: impl Into<String>, epoch: u64, segment: Segment) -> JournalResult<Self> {
        let document_id = document_id.into();
        if document_id.is_empty() {
            return Err(JournalError::Invalid("journal document id is empty".into()));
        }
        if segment.records.iter().any(|record| record.storage_id != document_id) {
            return Err(JournalError::Invalid(
                "journal segment contains another document's record".into(),
            ));
        }
        if segment.records.iter().any(|record| record.epoch != epoch) {
            return Err(JournalError::Invalid("journal segment mixes epochs".into()));
        }
        let mut sequences = segment.records.iter().map(|record| record.sequence).collect::<Vec<_>>();
        sequences.sort_unstable();
        sequences.dedup();
        let first_sequence = *sequences
            .first()
            .ok_or_else(|| JournalError::Invalid("empty journal segment".into()))?;
        let last_sequence = sequences
            .last()
            .copied()
            .ok_or_else(|| JournalError::Invalid("empty journal segment".into()))?;
        Ok(Self { document_id, epoch, first_sequence, last_sequence, segment })
    }

    pub fn encode(&self) -> JournalResult<Vec<u8>> {
        self.segment.encode()
    }

    pub fn digest(&self) -> JournalResult<String> {
        Ok(hex::encode(Sha256::digest(&self.encode()?)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenJournalObject {
    pub object_id: ObjectId,
    pub storage_key: String,
    pub digest: String,
    pub byte_length: u64,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

async fn write_encoded_segments(
    blobs: Arc<dyn BlobStore>,
    namespace: usize,
    admission: &JournalAppendAdmission,
    encoded: Vec<(DocumentSegment, Vec<u8>, String)>,
) -> JournalResult<Vec<WrittenJournalObject>> {
    if encoded.len() != admission.allocations.len() {
        return Err(JournalError::Conflict("catalog returned the wrong journal allocation count".into()));
    }
    let mut written = Vec::with_capacity(encoded.len());
    for ((object, body, digest), allocation) in encoded.into_iter().zip(&admission.allocations) {
        let expected_key = v2_object_key(&admission.document_id, &allocation.object_id)
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        if allocation.epoch != object.epoch
            || allocation.first_sequence != object.first_sequence
            || allocation.last_sequence != object.last_sequence
            || allocation.storage_key != expected_key
        {
            return Err(JournalError::Conflict("journal allocation does not match encoded segment".into()));
        }
        guarded_write_v2_object(
            blobs.clone(),
            namespace,
            &admission.document_id,
            allocation.object_id.clone(),
            body.clone(),
            JOURNAL_OBJECT_CONTENT_TYPE,
        )
            .await?;
        written.push(WrittenJournalObject {
            object_id: allocation.object_id.clone(),
            storage_key: allocation.storage_key.clone(),
            digest,
            byte_length: body.len() as u64,
            epoch: object.epoch,
            first_sequence: object.first_sequence,
            last_sequence: object.last_sequence,
        });
    }
    Ok(written)
}

/// A detached PUT cannot be cancelled with the room request that initiated
/// it. If the caller disappears after admission, startup recovery can still
/// observe and settle the immutable object by digest.
async fn guarded_write_v2_object(
    blobs: Arc<dyn BlobStore>,
    namespace: usize,
    document_id: &str,
    object_id: ObjectId,
    body: Vec<u8>,
    content_type: &str,
) -> JournalResult<WrittenObject> {
    let document_id = document_id.to_owned();
    let content_type = content_type.to_owned();
    let object_key = object_id.as_str().to_owned();
    if !crate::storage::v2_catalog::register_physical_guard(namespace, &document_id, &object_key) {
        return Err(JournalError::Conflict(
            "journal allocation already has a physical write in flight".into(),
        ));
    }
    let object_key_for_task = object_key.clone();
    let document_id_for_task = document_id.clone();
    tokio::spawn(async move {
        let result = write_v2_object_with_id(
            blobs.as_ref(),
            &document_id,
            object_id,
            body,
            &content_type,
        )
        .await;
        if let Ok(written) = &result {
            crate::storage::v2_catalog::complete_physical_guard(namespace, &document_id, written);
        } else {
            crate::storage::v2_catalog::fail_physical_guard(
                namespace,
                &document_id,
                &object_key,
                result.as_ref().err().map(ToString::to_string).as_deref().unwrap_or("journal PUT failed"),
            );
        }
        result
    })
    .await
    .map_err(|error| {
        crate::storage::v2_catalog::fail_physical_guard(
            namespace,
            &document_id_for_task,
            &object_key_for_task,
            &format!("guarded journal PUT task failed: {error}"),
        );
        JournalError::Storage(format!("guarded journal PUT task failed: {error}"))
    })?
    .map_err(|error| match error {
        BlobError::Conflict => JournalError::Conflict("journal allocation id reused".into()),
        other => JournalError::Storage(other.to_string()),
    })
}

/// Decode and validate all record fragments in one object. A segment may have
/// multiple records, but all records belong to the same document and epoch.
pub fn decode_segment(
    bytes: &[u8],
    document_id: &str,
    epoch: u64,
) -> JournalResult<DocumentSegment> {
    let segment = Segment::decode(bytes)?;
    DocumentSegment::new(document_id.to_owned(), epoch, segment)
}

/// Recover complete records from an ordered set of segment objects. Fragment
/// groups are checked before replay; missing fragments and sequence gaps are
/// corruption, never an empty-room fallback.
pub fn recover_records(
    segments: impl IntoIterator<Item = Vec<u8>>,
    document_id: &str,
    epoch: u64,
    expected_first: Option<u64>,
    expected_last: Option<u64>,
) -> JournalResult<Vec<JournalRecord>> {
    let mut fragments = BTreeMap::<u64, Vec<JournalRecord>>::new();
    for bytes in segments {
        let decoded = decode_segment(&bytes, document_id, epoch)?;
        for record in decoded.segment.records {
            fragments.entry(record.sequence).or_default().push(record);
        }
    }
    if fragments.is_empty() {
        if expected_first.is_some() || expected_last.is_some() {
            return Err(JournalError::Corrupt("acknowledged journal range is missing".into()));
        }
        return Ok(Vec::new());
    }
    let first = fragments
        .keys()
        .next()
        .copied()
        .ok_or_else(|| JournalError::Corrupt("empty journal fragment map".into()))?;
    let last = fragments
        .keys()
        .next_back()
        .copied()
        .ok_or_else(|| JournalError::Corrupt("empty journal fragment map".into()))?;
    if expected_first.is_some_and(|value| value != first)
        || expected_last.is_some_and(|value| value != last)
    {
        return Err(JournalError::Corrupt("journal object range does not match catalog".into()));
    }
    let mut recovered = Vec::with_capacity(fragments.len());
    for sequence in first..=last {
        let parts = fragments
            .remove(&sequence)
            .ok_or_else(|| JournalError::Corrupt("journal sequence gap".into()))?;
        let (count, expected_retry_id, expected_digest) = {
            let first_part = parts
                .first()
                .ok_or_else(|| JournalError::Corrupt("empty journal fragment group".into()))?;
            (
                first_part.fragment_count,
                first_part.retry_id.clone(),
                first_part.digest.clone(),
            )
        };
        if parts.len() != count as usize
            || parts.iter().any(|part| {
                part.fragment_count != count
                    || part.retry_id != expected_retry_id
                    || part.digest != expected_digest
                    || part.epoch != epoch
            })
        {
            return Err(JournalError::Corrupt("incomplete journal record fragments".into()));
        }
        let mut parts = parts;
        parts.sort_by_key(|part| part.fragment_index);
        if parts.iter().enumerate().any(|(index, part)| part.fragment_index as usize != index) {
            return Err(JournalError::Corrupt("journal fragment index gap".into()));
        }
        let payload = parts.iter().flat_map(|part| part.payload.iter().copied()).collect::<Vec<_>>();
        if hex::encode(Sha256::digest(&payload)) != expected_digest {
            return Err(JournalError::Corrupt("complete journal record digest mismatch".into()));
        }
        recovered.push(JournalRecord::new(
            document_id,
            sequence,
            expected_retry_id,
            epoch,
            payload,
        )?);
    }
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(document_id: &str, sequence: u64, payload: &[u8]) -> JournalRecord {
        JournalRecord::new(document_id, sequence, format!("retry-{sequence}"), 3, payload.to_vec())
            .expect("test record is valid")
    }

    #[test]
    fn segment_rejects_cross_document_records() {
        let segment = Segment::new(vec![record("doc-a", 1, b"a"), record("doc-b", 2, b"b")])
            .expect("segment framing is valid");
        assert!(DocumentSegment::new("doc-a", 3, segment).is_err());
    }

    #[test]
    fn recovery_rejects_a_missing_sequence() {
        let one = Segment::new(vec![record("doc-a", 1, b"a")]).expect("segment framing is valid");
        let three = Segment::new(vec![record("doc-a", 3, b"c")]).expect("segment framing is valid");
        let bytes = [one.encode().expect("encoding"), three.encode().expect("encoding")];
        assert!(recover_records(bytes, "doc-a", 3, Some(1), Some(3)).is_err());
    }

    #[test]
    fn object_ids_and_keys_are_allocation_scoped() {
        let first = ObjectId::random();
        let second = ObjectId::random();
        assert_ne!(first, second);
        let key = v2_object_key("stable-document", &first).expect("valid object key");
        assert!(key.starts_with("v2/documents/stable-document/objects/"));
        assert!(crate::storage::blob::validate_v2_object_key(&key).is_ok());
        assert!(crate::storage::blob::parse_v2_object_key(&key).is_ok());
        assert!(crate::storage::blob::validate_v2_object_key(&format!("{key}/extra")).is_err());
    }

    #[test]
    fn recovery_composes_base_and_every_committed_update() {
        use yrs::{GetString, Text, Transact};

        let document = crate::document::session::new_doc();
        let text = document.get_or_insert_text("body");
        {
            let mut transaction = document.transact_mut();
            text.insert(&mut transaction, 0, "base");
        }
        let base = crate::document::session::encode_state(&document);
        let vector = crate::document::session::encode_vector(&document);
        {
            let mut transaction = document.transact_mut();
            text.insert(&mut transaction, 4, " + tail");
        }
        let update = crate::document::session::encode_diff(&document, &vector).expect("diff");
        let records = vec![
            JournalRecord::new("doc", 1, "retry-1", 0, base).expect("base record"),
            JournalRecord::new("doc", 2, "retry-2", 0, update).expect("tail record"),
        ];
        let recovered = replay_complete_state(
            Some(records[0].payload.clone()),
            records[1..].to_vec(),
        )
        .expect("recovery")
        .expect("state");
        let restored = crate::document::session::new_doc();
        crate::document::session::apply_update(&restored, &recovered).expect("restored update");
        let restored_text = restored
            .get_or_insert_text("body")
            .get_string(&restored.transact());
        assert_eq!(restored_text, "base + tail");
    }
}
