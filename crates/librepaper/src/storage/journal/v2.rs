//! Per-document v2 journal objects.
//!
//! A segment is a physical object owned by one document.  This module keeps
//! the framing code independent from the catalog transaction that registers
//! an object, so callers can perform bounded I/O first and hand the resulting
//! descriptors to the typed `journal_append`/`journal_compact` mutation.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::storage::blob::{v2_object_key, write_v2_object, BlobError, BlobStore, ObjectId, WrittenObject};

use super::{JournalError, JournalRecord, JournalResult, Segment};

pub const JOURNAL_OBJECT_CONTENT_TYPE: &str = "application/vnd.librepaper.journal-segment";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalAppendAdmission {
    pub operation_id: String,
    pub document_id: String,
    pub epoch: u64,
    pub first_sequence: u64,
    pub expected_last_sequence: u64,
    pub source_generation: u64,
    pub writer_generation: String,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalObjectRef {
    pub object_id: String,
    pub storage_key: String,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalCompactionAdmission {
    pub operation_id: String,
    pub document_id: String,
    pub captured_epoch: u64,
    pub captured_sequence: u64,
    pub writer_generation: String,
}

/// SQL-side hooks for the v2 append protocol. The implementation must use an
/// immediate transaction for prepare/commit and must keep the operation row
/// prepared until every object has settled. It also owns object/counter rows;
/// this trait carries descriptors only, never unbounded object lists.
#[async_trait::async_trait]
pub trait V2JournalCatalog: Send + Sync {
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
        limit: usize,
    ) -> Result<Vec<JournalObjectRef>, String>;
    async fn prepare_compaction(
        &self,
        document_id: &str,
        expected_epoch: u64,
        expected_sequence: u64,
    ) -> Result<JournalCompactionAdmission, String>;
    async fn commit_compaction(
        &self,
        admission: JournalCompactionAdmission,
        base: WrittenObject,
        new_epoch: u64,
        new_sequence: u64,
    ) -> Result<(), String>;
    async fn abort_compaction(&self, operation_id: &str) -> Result<(), String>;
}

pub struct V2JournalRuntime<C> {
    catalog: std::sync::Arc<C>,
    blobs: std::sync::Arc<dyn BlobStore>,
}

impl<C> V2JournalRuntime<C>
where
    C: V2JournalCatalog + 'static,
{
    pub fn new(catalog: std::sync::Arc<C>, blobs: std::sync::Arc<dyn BlobStore>) -> Self {
        Self { catalog, blobs }
    }

    pub async fn append(
        &self,
        request: JournalAppendRequest,
        segments: &[Segment],
    ) -> JournalResult<Vec<WrittenJournalObject>> {
        append_segments(self.catalog.as_ref(), self.blobs.as_ref(), request, segments).await
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
        let mut after = 0;
        let mut bodies = Vec::new();
        loop {
            let page = self
                .catalog
                .journal_objects(document_id, epoch, after, 128)
                .await
                .map_err(JournalError::CatalogText)?;
            if page.is_empty() {
                break;
            }
            if page.len() > 128 {
                return Err(JournalError::Corrupt("journal descriptor page exceeded bound".into()));
            }
            let previous_after = after;
            for object in &page {
                if object.epoch != epoch {
                    return Err(JournalError::Corrupt("journal object epoch mismatch".into()));
                }
                bodies.push(
                    self.blobs
                        .get(&object.storage_key)
                        .await
                        .map_err(|error| JournalError::Storage(error.to_string()))?,
                );
                after = after.max(object.last_sequence);
            }
            if after <= previous_after {
                return Err(JournalError::Corrupt("journal descriptor cursor did not advance".into()));
            }
            if page.last().is_some_and(|object| object.last_sequence >= last_sequence) {
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
        let admission = self
            .catalog
            .prepare_compaction(document_id, expected_epoch, expected_sequence)
            .await
            .map_err(JournalError::CatalogText)?;
        let written = match write_v2_object(self.blobs.as_ref(), document_id, base, content_type).await {
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

pub async fn append_segments(
    catalog: &dyn V2JournalCatalog,
    blobs: &dyn BlobStore,
    request: JournalAppendRequest,
    segments: &[Segment],
) -> JournalResult<Vec<WrittenJournalObject>> {
    let admission = catalog
        .prepare_append(request)
        .await
        .map_err(JournalError::CatalogText)?;
    let written = match write_segments(blobs, &admission.document_id, admission.epoch, segments).await {
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

/// Write one complete set of immutable per-document segments. SQL registration
/// is deliberately left to the caller's operation transaction; no caller may
/// acknowledge the append until that transaction succeeds.
pub async fn write_segments(
    blobs: &dyn BlobStore,
    document_id: &str,
    epoch: u64,
    segments: &[Segment],
) -> JournalResult<Vec<WrittenJournalObject>> {
    let mut written = Vec::with_capacity(segments.len());
    for segment in segments {
        let object = DocumentSegment::new(document_id.to_owned(), epoch, segment.clone())?;
        let body = object.encode()?;
        let digest = hex::encode(Sha256::digest(&body));
        let object_id = ObjectId::random();
        let storage_key = v2_object_key(document_id, &object_id)
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        blobs
            .put_new(&storage_key, body.clone(), JOURNAL_OBJECT_CONTENT_TYPE)
            .await
            .map_err(|error| match error {
                BlobError::Conflict => JournalError::Conflict("journal allocation id reused".into()),
                other => JournalError::Storage(other.to_string()),
            })?;
        written.push(WrittenJournalObject {
            object_id,
            storage_key,
            digest,
            byte_length: body.len() as u64,
            epoch,
            first_sequence: object.first_sequence,
            last_sequence: object.last_sequence,
        });
    }
    Ok(written)
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
        let first_part = parts
            .first()
            .ok_or_else(|| JournalError::Corrupt("empty journal fragment group".into()))?;
        let count = first_part.fragment_count;
        let expected_retry_id = first_part.retry_id.clone();
        let expected_digest = first_part.digest.clone();
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
    }
}
