//! Per-document v2 journal objects.
//!
//! A segment is a physical object owned by one document.  This module keeps
//! the framing code independent from the catalog transaction that registers
//! an object, so callers can perform bounded I/O first and hand the resulting
//! descriptors to the typed `journal_append`/`journal_compact` mutation.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::storage::blob::{v2_object_key, BlobError, BlobStore, ObjectId};

use super::{JournalError, JournalRecord, JournalResult, Segment};

pub const JOURNAL_OBJECT_CONTENT_TYPE: &str = "application/vnd.librepaper.journal-segment";

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
        if parts.len() != count as usize
            || parts.iter().any(|part| {
                part.fragment_count != count
                    || part.retry_id != first_part.retry_id
                    || part.digest != first_part.digest
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
        if hex::encode(Sha256::digest(&payload)) != first_part.digest {
            return Err(JournalError::Corrupt("complete journal record digest mismatch".into()));
        }
        recovered.push(JournalRecord::new(
            document_id,
            sequence,
            first_part.retry_id.clone(),
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
