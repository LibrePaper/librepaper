//! The on-disk format: records, the bounded segments that hold them, and the
//! cursor that reads them back.

use super::*;

pub const SEGMENT_FORMAT: u16 = 2;

pub(super) const LEGACY_SEGMENT_FORMAT: u16 = 1;

pub const SEGMENT_MAGIC: &[u8; 4] = b"KJNL";

pub const MAX_SEGMENT_BYTES: usize = 4 * 1024 * 1024;

// A complete Yjs snapshot can be close to the configured 4 MiB segment cap.
// Keep the record bound just below the segment bound so valid documents do
// not fail journal persistence solely because their snapshot exceeds 1 MiB.
pub const MAX_RECORD_BYTES: usize = MAX_SEGMENT_BYTES - 1024;

pub const MAX_RECORD_CHUNK_BYTES: usize = 512 * 1024;

pub const MAX_RECORDS_PER_SEGMENT: usize = 4096;

pub const MAX_RECORDS_PER_DOCUMENT: usize = 1024;

pub const MAX_METADATA_BYTES: usize = 256 * 1024;

/// One immutable update in a segment. `payload` is an opaque CRDT update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalRecord {
    pub format_version: u16,
    pub storage_id: String,
    pub sequence: u64,
    pub retry_id: String,
    pub epoch: u64,
    pub fragment_index: u32,
    pub fragment_count: u32,
    pub payload: Vec<u8>,
    pub digest: String,
    pub chunk_digest: String,
}

impl JournalRecord {
    pub fn new(
        storage_id: impl Into<String>,
        sequence: u64,
        retry_id: impl Into<String>,
        epoch: u64,
        payload: Vec<u8>,
    ) -> JournalResult<Self> {
        Self::new_fragment(storage_id, sequence, retry_id, epoch, payload, 0, 1, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_fragment(
        storage_id: impl Into<String>,
        sequence: u64,
        retry_id: impl Into<String>,
        epoch: u64,
        payload: Vec<u8>,
        fragment_index: u32,
        fragment_count: u32,
        digest: Option<String>,
    ) -> JournalResult<Self> {
        let chunk_digest = hex::encode(Sha256::digest(&payload));
        let record = Self {
            format_version: SEGMENT_FORMAT,
            storage_id: storage_id.into(),
            sequence,
            retry_id: retry_id.into(),
            epoch,
            fragment_index,
            fragment_count,
            digest: hex::encode(Sha256::digest(&payload)),
            chunk_digest,
            payload,
        };
        let mut record = record;
        if let Some(digest) = digest {
            record.digest = digest;
        }
        record.validate()?;
        Ok(record)
    }

    pub(super) fn chunked(
        storage_id: &str,
        sequence: u64,
        retry_id: &str,
        epoch: u64,
        payload: Vec<u8>,
    ) -> JournalResult<Vec<Self>> {
        let digest = hex::encode(Sha256::digest(&payload));
        let count = u32::try_from(payload.len().div_ceil(MAX_RECORD_CHUNK_BYTES))
            .map_err(|_| JournalError::Limit("too many record chunks".into()))?;
        (0..count)
            .map(|index| {
                let start = index as usize * MAX_RECORD_CHUNK_BYTES;
                let end = (start + MAX_RECORD_CHUNK_BYTES).min(payload.len());
                Self::new_fragment(
                    storage_id,
                    sequence,
                    retry_id,
                    epoch,
                    payload[start..end].to_vec(),
                    index,
                    count,
                    Some(digest.clone()),
                )
            })
            .collect()
    }

    pub fn validate(&self) -> JournalResult<()> {
        if self.format_version != SEGMENT_FORMAT && self.format_version != LEGACY_SEGMENT_FORMAT {
            return Err(JournalError::Invalid(format!(
                "unsupported record version {}",
                self.format_version
            )));
        }
        if self.storage_id.is_empty() || self.storage_id.len() > u16::MAX as usize {
            return Err(JournalError::Invalid("invalid storage id".into()));
        }
        if self.sequence == 0 {
            return Err(JournalError::Invalid("sequence must be positive".into()));
        }
        if self.fragment_count == 0 || self.fragment_index >= self.fragment_count {
            return Err(JournalError::Invalid("invalid record fragment".into()));
        }
        if self.retry_id.is_empty() || self.retry_id.len() > u16::MAX as usize {
            return Err(JournalError::Invalid("invalid retry id".into()));
        }
        if self.payload.is_empty() || self.payload.len() > MAX_RECORD_BYTES {
            return Err(JournalError::Limit("record payload is too large".into()));
        }
        let chunk_digest = hex::encode(Sha256::digest(&self.payload));
        if self.format_version == SEGMENT_FORMAT && self.chunk_digest != chunk_digest {
            return Err(JournalError::Corrupt("record digest mismatch".into()));
        }
        if self.digest.len() != 64 {
            return Err(JournalError::Corrupt("invalid record digest".into()));
        }
        if self.fragment_count == 1 && self.digest != chunk_digest {
            return Err(JournalError::Corrupt("record digest mismatch".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub records: Vec<JournalRecord>,
}

/// Exact encoded size of the first journal segment for a full-state payload.
/// Admission uses this before object I/O; it mirrors `JournalRuntime::append`'s
/// chunking and framing, including storage/retry identities and digests.
pub fn initial_segment_bytes(storage_id: &str, payload: &[u8]) -> JournalResult<usize> {
    let retry_id = format!("room-{storage_id}-0-1");
    let records = JournalRecord::chunked(storage_id, 1, &retry_id, 0, payload.to_vec())?;
    Segment::new(records).map(|segment| segment.encoded_len())
}

impl Segment {
    pub fn new(records: Vec<JournalRecord>) -> JournalResult<Self> {
        let segment = Self { records };
        segment.validate()?;
        Ok(segment)
    }

    pub fn encoded_len(&self) -> usize {
        SEGMENT_HEADER_BYTES + self.records.iter().map(encoded_record_len).sum::<usize>()
    }

    pub fn validate(&self) -> JournalResult<()> {
        if self.records.is_empty() {
            return Err(JournalError::Invalid("empty segment".into()));
        }
        if self.records.len() > MAX_RECORDS_PER_SEGMENT {
            return Err(JournalError::Limit("too many records in segment".into()));
        }
        let Some(segment_format) = self.records.first().map(|record| record.format_version) else {
            return Err(JournalError::Invalid("empty segment".into()));
        };
        if self
            .records
            .iter()
            .any(|record| record.format_version != segment_format)
        {
            return Err(JournalError::Invalid(
                "legacy and current records cannot share a segment".into(),
            ));
        }
        if self.encoded_len() > MAX_SEGMENT_BYTES {
            return Err(JournalError::Limit("segment is too large".into()));
        }
        let mut per_document = HashMap::<&str, usize>::new();
        for record in &self.records {
            record.validate()?;
            let count = per_document.entry(&record.storage_id).or_default();
            *count += 1;
            if *count > MAX_RECORDS_PER_DOCUMENT {
                return Err(JournalError::Limit(
                    "too many records for one document".into(),
                ));
            }
        }
        Ok(())
    }

    /// Encode a segment with fixed-width lengths.  JSON is intentionally not
    /// used for payload framing: recovery can skip neither malformed lengths
    /// nor an allocation larger than the configured segment bound.
    pub fn encode(&self) -> JournalResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = Vec::with_capacity(self.encoded_len());
        bytes.extend_from_slice(SEGMENT_MAGIC);
        let segment_format = self.records[0].format_version;
        bytes.extend_from_slice(&segment_format.to_le_bytes());
        bytes.extend_from_slice(&(self.records.len() as u32).to_le_bytes());
        for record in &self.records {
            put_u16(&mut bytes, record.format_version);
            put_bytes_u16(&mut bytes, record.storage_id.as_bytes())?;
            put_u64(&mut bytes, record.sequence);
            put_u64(&mut bytes, record.epoch);
            if segment_format == SEGMENT_FORMAT {
                put_u32(&mut bytes, record.fragment_index);
                put_u32(&mut bytes, record.fragment_count);
            }
            put_bytes_u16(&mut bytes, record.retry_id.as_bytes())?;
            let digest = record.digest.as_bytes();
            if digest.len() != 64 {
                return Err(JournalError::Invalid("digest must be sha256 hex".into()));
            }
            put_bytes_u16(&mut bytes, digest)?;
            if segment_format == SEGMENT_FORMAT {
                let chunk_digest = record.chunk_digest.as_bytes();
                if chunk_digest.len() != 64 {
                    return Err(JournalError::Invalid(
                        "chunk digest must be sha256 hex".into(),
                    ));
                }
                put_bytes_u16(&mut bytes, chunk_digest)?;
            }
            let payload_len = u32::try_from(record.payload.len())
                .map_err(|_| JournalError::Limit("payload length overflow".into()))?;
            put_u32(&mut bytes, payload_len);
            bytes.extend_from_slice(&record.payload);
        }
        debug_assert_eq!(bytes.len(), self.encoded_len());
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> JournalResult<Self> {
        Self::decode_filtered(bytes, None)
    }

    /// Decode framing and integrity for the whole segment while retaining
    /// payloads for one document only. Shared segments therefore do not
    /// allocate unrelated users' snapshots during recovery.
    pub fn decode_for(bytes: &[u8], storage_id: &str) -> JournalResult<Self> {
        Self::decode_filtered(bytes, Some(storage_id))
    }

    pub(super) fn decode_filtered(bytes: &[u8], filter: Option<&str>) -> JournalResult<Self> {
        if bytes.len() > MAX_SEGMENT_BYTES {
            return Err(JournalError::Limit("segment is too large".into()));
        }
        let mut cursor = Cursor::new(bytes);
        if cursor.take(4)? != SEGMENT_MAGIC {
            return Err(JournalError::Corrupt("bad segment magic".into()));
        }
        let segment_format = cursor.u16()?;
        if segment_format != SEGMENT_FORMAT && segment_format != LEGACY_SEGMENT_FORMAT {
            return Err(JournalError::Corrupt("unsupported segment version".into()));
        }
        let count = cursor.u32()? as usize;
        if count == 0 || count > MAX_RECORDS_PER_SEGMENT {
            return Err(JournalError::Corrupt("invalid record count".into()));
        }
        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            let format_version = cursor.u16()?;
            if format_version != segment_format {
                return Err(JournalError::Corrupt(
                    "record format does not match segment format".into(),
                ));
            }
            let storage_id = String::from_utf8(cursor.bytes_u16()?)
                .map_err(|_| JournalError::Corrupt("storage id is not utf-8".into()))?;
            let sequence = cursor.u64()?;
            let epoch = cursor.u64()?;
            let (fragment_index, fragment_count) = if segment_format == SEGMENT_FORMAT {
                (cursor.u32()?, cursor.u32()?)
            } else {
                (0, 1)
            };
            let retry_id = String::from_utf8(cursor.bytes_u16()?)
                .map_err(|_| JournalError::Corrupt("retry id is not utf-8".into()))?;
            let digest = String::from_utf8(cursor.bytes_u16()?)
                .map_err(|_| JournalError::Corrupt("digest is not utf-8".into()))?;
            let chunk_digest = if segment_format == SEGMENT_FORMAT {
                String::from_utf8(cursor.bytes_u16()?)
                    .map_err(|_| JournalError::Corrupt("chunk digest is not utf-8".into()))?
            } else {
                digest.clone()
            };
            let payload_len = cursor.u32()? as usize;
            if payload_len == 0 || payload_len > MAX_RECORD_BYTES {
                return Err(JournalError::Corrupt("invalid payload length".into()));
            }
            let payload = cursor.take(payload_len)?.to_vec();
            let record = JournalRecord {
                format_version,
                storage_id,
                sequence,
                retry_id,
                epoch,
                fragment_index,
                fragment_count,
                payload,
                digest,
                chunk_digest,
            };
            record.validate()?;
            if filter.is_none_or(|storage_id| storage_id == record.storage_id) {
                records.push(record);
            }
        }
        if !cursor.is_empty() {
            return Err(JournalError::Corrupt("trailing segment bytes".into()));
        }
        if records.is_empty() && filter.is_some() {
            Ok(Self { records })
        } else {
            Self::new(records)
        }
    }

    pub fn records_for(&self, storage_id: &str, above_sequence: u64) -> Vec<&JournalRecord> {
        self.records
            .iter()
            .filter(|record| record.storage_id == storage_id && record.sequence > above_sequence)
            .collect()
    }
}

pub(super) fn put_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub const SEGMENT_HEADER_BYTES: usize = 10;

/// The longest storage identity the journal will frame. Record framing puts
/// the storage id and a retry id derived from it into every fragment, so a
/// bound on those is what makes the "a framed record fits a segment" check in
/// [`crate::config::PersistenceLimits::validate`] arithmetic rather than a
/// guess. It is far above any identity this deployment mints; a longer one is
/// refused at append rather than discovered when a segment fails to seal.
pub const MAX_JOURNAL_IDENTITY_BYTES: usize = 256;

/// The framing one current-format record adds around its payload, for a
/// storage identity of `identity` bytes. The retry id `append` derives is the
/// storage id plus a fixed `room-<epoch>-<sequence>` decoration, which is
/// what the second identity allowance covers. It mirrors
/// [`encoded_record_len`]; keeping the two together is what stops admission
/// from drifting when a framing field is added.
pub const fn record_framing_bytes(identity: usize) -> usize {
    2 + 2 + identity + 8 + 8 + 8 + 2 + (identity + 64) + 2 + 64 + 2 + 64 + 4
}

/// Return the exact number of bytes emitted for one record by `encode`.
/// Keeping this beside the encoder prevents queue admission from drifting
/// when a framing field is added.
pub(super) fn encoded_record_len(record: &JournalRecord) -> usize {
    2 + 2
        + record.storage_id.len()
        + 8
        + 8
        + if record.format_version == SEGMENT_FORMAT {
            8
        } else {
            0
        }
        + 2
        + record.retry_id.len()
        + 2
        + 64
        + if record.format_version == SEGMENT_FORMAT {
            2 + 64
        } else {
            0
        }
        + 4
        + record.payload.len()
}

pub(super) fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn put_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn put_bytes_u16(bytes: &mut Vec<u8>, value: &[u8]) -> JournalResult<()> {
    let len = u16::try_from(value.len())
        .map_err(|_| JournalError::Limit("field exceeds u16 framing limit".into()))?;
    put_u16(bytes, len);
    bytes.extend_from_slice(value);
    Ok(())
}

pub(super) struct Cursor<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) offset: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub(super) fn take(&mut self, length: usize) -> JournalResult<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| JournalError::Corrupt("length overflow".into()))?;
        if end > self.bytes.len() {
            return Err(JournalError::Corrupt("truncated segment".into()));
        }
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }
    pub(super) fn u16(&mut self) -> JournalResult<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("length checked"),
        ))
    }
    pub(super) fn u32(&mut self) -> JournalResult<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("length checked"),
        ))
    }
    pub(super) fn u64(&mut self) -> JournalResult<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("length checked"),
        ))
    }
    pub(super) fn bytes_u16(&mut self) -> JournalResult<Vec<u8>> {
        let length = self.u16()? as usize;
        Ok(self.take(length)?.to_vec())
    }
    pub(super) fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}
