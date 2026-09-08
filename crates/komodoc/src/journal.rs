//! Durable local edit-journal primitives.
//!
//! The journal deliberately has no knowledge of rooms or Yjs.  It owns the
//! bounded record/segment format and the small SQL state transition used by a
//! coordinator.  Object writes are performed before `commit_segment`; callers
//! must therefore treat a failed commit as an unknown outcome and reconcile by
//! operation id before retrying.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::blob::{
    journal_base_key, journal_manifest_key, journal_segment_key, BlobError, BlobStore,
};
use crate::catalog::{Catalog, CatalogError};

pub const SEGMENT_FORMAT: u16 = 2;
const LEGACY_SEGMENT_FORMAT: u16 = 1;
pub const SEGMENT_MAGIC: &[u8; 4] = b"KJNL";
pub const MAX_SEGMENT_BYTES: usize = 4 * 1024 * 1024;
// A complete Yjs snapshot can be close to the configured 4 MiB segment cap.
// Keep the record bound just below the segment bound so valid documents do
// not fail journal persistence solely because their snapshot exceeds 1 MiB.
pub const MAX_RECORD_BYTES: usize = MAX_SEGMENT_BYTES - 1024;
pub const MAX_RECORD_CHUNK_BYTES: usize = 512 * 1024;
pub const MAX_REPLAY_SEGMENTS: usize = 16_384;
pub const REPLAY_PAGE_SIZE: usize = 128;
pub const MAX_MANIFEST_ENTRIES: usize = 65_536;
pub const MAX_RECORDS_PER_SEGMENT: usize = 4096;
pub const MAX_RECORDS_PER_DOCUMENT: usize = 1024;
pub const MAX_METADATA_BYTES: usize = 256 * 1024;
const JOURNAL_MAINTENANCE_RESERVE_BYTES: i64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum JournalError {
    Invalid(String),
    Corrupt(String),
    Limit(String),
    Storage(String),
    Catalog(CatalogError),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid journal request: {message}"),
            Self::Corrupt(message) => write!(f, "corrupt journal segment: {message}"),
            Self::Limit(message) => write!(f, "journal limit exceeded: {message}"),
            Self::Storage(message) => write!(f, "journal storage error: {message}"),
            Self::Catalog(error) => write!(f, "journal catalogue error: {error}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<CatalogError> for JournalError {
    fn from(error: CatalogError) -> Self {
        Self::Catalog(error)
    }
}

impl From<BlobError> for JournalError {
    fn from(error: BlobError) -> Self {
        Self::Storage(error.to_string())
    }
}

pub type JournalResult<T> = Result<T, JournalError>;

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
    fn new_fragment(
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

    fn chunked(
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
        10 + self
            .records
            .iter()
            .map(|record| {
                2 + 2
                    + 8
                    + 8
                    + 2
                    + 2
                    + if record.format_version == SEGMENT_FORMAT {
                        8
                    } else {
                        0
                    }
                    + 4
                    + record.storage_id.len()
                    + record.retry_id.len()
                    + 64
                    + if record.format_version == SEGMENT_FORMAT {
                        2 + 64
                    } else {
                        0
                    }
                    + record.payload.len()
            })
            .sum::<usize>()
    }

    pub fn validate(&self) -> JournalResult<()> {
        if self.records.is_empty() {
            return Err(JournalError::Invalid("empty segment".into()));
        }
        if self.records.len() > MAX_RECORDS_PER_SEGMENT {
            return Err(JournalError::Limit("too many records in segment".into()));
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
        bytes.extend_from_slice(&SEGMENT_FORMAT.to_le_bytes());
        bytes.extend_from_slice(&(self.records.len() as u32).to_le_bytes());
        for record in &self.records {
            put_u16(&mut bytes, record.format_version);
            put_bytes_u16(&mut bytes, record.storage_id.as_bytes())?;
            put_u64(&mut bytes, record.sequence);
            put_u64(&mut bytes, record.epoch);
            if record.format_version == SEGMENT_FORMAT {
                put_u32(&mut bytes, record.fragment_index);
                put_u32(&mut bytes, record.fragment_count);
            }
            put_bytes_u16(&mut bytes, record.retry_id.as_bytes())?;
            let digest = record.digest.as_bytes();
            if digest.len() != 64 {
                return Err(JournalError::Invalid("digest must be sha256 hex".into()));
            }
            put_bytes_u16(&mut bytes, digest)?;
            if record.format_version == SEGMENT_FORMAT {
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

    fn decode_filtered(bytes: &[u8], filter: Option<&str>) -> JournalResult<Self> {
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

fn put_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn put_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn put_bytes_u16(bytes: &mut Vec<u8>, value: &[u8]) -> JournalResult<()> {
    let len = u16::try_from(value.len())
        .map_err(|_| JournalError::Limit("field exceeds u16 framing limit".into()))?;
    put_u16(bytes, len);
    bytes.extend_from_slice(value);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn take(&mut self, length: usize) -> JournalResult<&'a [u8]> {
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
    fn u16(&mut self) -> JournalResult<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("length checked"),
        ))
    }
    fn u32(&mut self) -> JournalResult<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("length checked"),
        ))
    }
    fn u64(&mut self) -> JournalResult<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("length checked"),
        ))
    }
    fn bytes_u16(&mut self) -> JournalResult<Vec<u8>> {
        let length = self.u16()? as usize;
        Ok(self.take(length)?.to_vec())
    }
    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[derive(Clone, Debug)]
pub struct CoordinatorLimits {
    pub max_queued_bytes: usize,
    pub max_queued_records: usize,
    pub max_segment_bytes: usize,
    pub max_records_per_segment: usize,
}

impl Default for CoordinatorLimits {
    fn default() -> Self {
        Self {
            max_queued_bytes: 16 * MAX_SEGMENT_BYTES,
            max_queued_records: MAX_RECORDS_PER_SEGMENT * 16,
            max_segment_bytes: MAX_SEGMENT_BYTES,
            max_records_per_segment: MAX_RECORDS_PER_SEGMENT,
        }
    }
}

/// A single-process queue.  It performs no object or SQL I/O while holding
/// its mutex (the queue itself is intentionally not internally synchronized),
/// so a caller can move sealed segments to an async worker safely.
#[derive(Debug)]
pub struct JournalCoordinator {
    limits: CoordinatorLimits,
    queued: VecDeque<JournalRecord>,
    queued_bytes: usize,
}

impl JournalCoordinator {
    pub fn new(limits: CoordinatorLimits) -> JournalResult<Self> {
        if limits.max_segment_bytes > MAX_SEGMENT_BYTES
            || limits.max_records_per_segment > MAX_RECORDS_PER_SEGMENT
            || limits.max_segment_bytes < 64
        {
            return Err(JournalError::Invalid("invalid coordinator limits".into()));
        }
        Ok(Self {
            limits,
            queued: VecDeque::new(),
            queued_bytes: 0,
        })
    }

    pub fn enqueue(&mut self, record: JournalRecord) -> JournalResult<()> {
        self.enqueue_batch(std::iter::once(record))
    }

    fn enqueue_batch<I>(&mut self, records: I) -> JournalResult<()>
    where
        I: IntoIterator<Item = JournalRecord>,
    {
        let records = records.into_iter().collect::<Vec<_>>();
        let bytes = records
            .iter()
            .map(|record| {
                record.validate()?;
                Ok(record.payload.len())
            })
            .collect::<JournalResult<Vec<_>>>()?
            .into_iter()
            .sum::<usize>();
        if self.queued.len().saturating_add(records.len()) > self.limits.max_queued_records
            || self.queued_bytes.saturating_add(bytes) > self.limits.max_queued_bytes
        {
            return Err(JournalError::Limit("journal queue is full".into()));
        }
        self.queued_bytes = self.queued_bytes.saturating_add(bytes);
        self.queued.extend(records);
        Ok(())
    }

    pub fn queued_records(&self) -> usize {
        self.queued.len()
    }
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    fn contains_identity(&self, storage_id: &str, epoch: u64, sequence: u64) -> bool {
        self.queued.iter().any(|record| {
            record.storage_id == storage_id && record.epoch == epoch && record.sequence == sequence
        })
    }

    fn remove_identity(&mut self, storage_id: &str, epoch: u64, sequence: u64) -> bool {
        let Some(index) = self.queued.iter().position(|record| {
            record.storage_id == storage_id && record.epoch == epoch && record.sequence == sequence
        }) else {
            return false;
        };
        let record = self.queued.remove(index).expect("identity position exists");
        self.queued_bytes = self.queued_bytes.saturating_sub(record.payload.len());
        true
    }

    /// Return sealed work to the queue when object publication failed before
    /// the preparation was committed. The caller must hold exclusive
    /// publication ownership while doing this.
    pub fn requeue(&mut self, mut segments: Vec<Segment>) {
        while let Some(segment) = segments.pop() {
            for record in segment.records.into_iter().rev() {
                self.queued_bytes = self.queued_bytes.saturating_add(record.payload.len());
                self.queued.push_front(record);
            }
        }
    }

    /// Seal as many complete immutable segments as fit.  A final undersized
    /// queue remains queued, allowing the caller to apply the quiet/deadline
    /// policy without acknowledging a partial flush.
    pub fn seal(&mut self, _force: bool) -> JournalResult<Vec<Segment>> {
        let mut segments = Vec::new();
        loop {
            if self.queued.is_empty() {
                break;
            }
            let mut records = Vec::new();
            let mut encoded_size = 10usize;
            while let Some(record) = self.queued.front() {
                let record_size = 2
                    + 2
                    + 8
                    + 8
                    + 2
                    + 2
                    + 4
                    + record.storage_id.len()
                    + record.retry_id.len()
                    + 64
                    + record.payload.len();
                if records.len() >= self.limits.max_records_per_segment
                    || (records.is_empty()
                        && encoded_size + record_size > self.limits.max_segment_bytes)
                    || (!records.is_empty()
                        && encoded_size + record_size > self.limits.max_segment_bytes)
                {
                    break;
                }
                let record = self.queued.pop_front().expect("front exists");
                encoded_size += record_size;
                self.queued_bytes = self.queued_bytes.saturating_sub(record.payload.len());
                records.push(record);
            }
            if records.is_empty() {
                self.requeue(segments);
                return Err(JournalError::Limit("record cannot fit in segment".into()));
            }
            segments.push(Segment::new(records)?);
        }
        Ok(segments)
    }

    /// Put sealed immutable segments.  This is deliberately separate from
    /// SQL publication; callers commit the returned metadata transactionally.
    pub async fn write_segments(
        &self,
        blobs: &dyn BlobStore,
        deployment_id: &str,
        operation_id: &str,
        segments: &[Segment],
    ) -> JournalResult<Vec<WrittenSegment>> {
        let mut result = Vec::with_capacity(segments.len());
        for (index, segment) in segments.iter().enumerate() {
            let id = format!("{operation_id}-{index}");
            let body = segment.encode()?;
            let digest = hex::encode(Sha256::digest(&body));
            let key = journal_segment_key(deployment_id, &id);
            blobs
                .put(&key, body.clone(), "application/octet-stream")
                .await?;
            result.push(WrittenSegment {
                segment_id: id,
                object_key: key,
                digest,
                encoded_bytes: body.len() as i64,
            });
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenSegment {
    pub segment_id: String,
    pub object_key: String,
    pub digest: String,
    pub encoded_bytes: i64,
}

fn covered_ranges(segments: &[Segment]) -> Vec<CoveredRange> {
    let mut ranges: HashMap<(String, u64), (u64, u64)> = HashMap::new();
    for record in segments.iter().flat_map(|segment| segment.records.iter()) {
        let entry = ranges
            .entry((record.storage_id.clone(), record.epoch))
            .or_insert((record.sequence, record.sequence));
        entry.0 = entry.0.min(record.sequence);
        entry.1 = entry.1.max(record.sequence);
    }
    ranges
        .into_iter()
        .map(
            |((storage_id, epoch), (first_sequence, last_sequence))| CoveredRange {
                storage_id,
                epoch,
                first_sequence,
                last_sequence,
            },
        )
        .collect()
}

fn segment_identities(segments: &[Segment]) -> HashSet<(String, u64, u64)> {
    segments
        .iter()
        .flat_map(|segment| {
            segment
                .records
                .iter()
                .map(|record| (record.storage_id.clone(), record.epoch, record.sequence))
        })
        .collect()
}

fn segment_matches_plan(segment: &Segment, plan: &JournalPlan) -> bool {
    segment.records.iter().all(|record| {
        plan.covered.iter().any(|range| {
            range.storage_id == record.storage_id
                && range.epoch == record.epoch
                && record.sequence >= range.first_sequence
                && record.sequence <= range.last_sequence
        })
    })
}

/// Production bridge used by Room.  It serializes the prepare/object
/// write/commit protocol while leaving the coordinator responsible for queue
/// limits and segment framing.  A failed SQL commit intentionally leaves the
/// preparation unresolved; startup then refuses to serve until an operator or
/// reconciliation worker proves the outcome.
pub struct JournalRuntime {
    coordinator: AsyncMutex<JournalCoordinator>,
    publication: AsyncMutex<()>,
    pending: AsyncMutex<HashSet<(String, u64, u64)>>,
    pending_notify: Notify,
    store: Arc<JournalStore>,
    blobs: Arc<dyn BlobStore>,
    deployment_id: String,
    owner_limit: i64,
    total_limit: i64,
    next_operation: AtomicU64,
}

/// A compaction borrow is scoped to the object-publication attempt. If
/// staging fails or the process returns early, Drop refunds the durable
/// maintenance allocation; a successful publication releases it explicitly
/// after replacement accounting has committed.
struct MaintenanceBorrow {
    catalog: Arc<Catalog>,
    slug: String,
    job_id: String,
    bytes: i64,
    released: bool,
}

impl MaintenanceBorrow {
    fn release(&mut self) -> JournalResult<()> {
        if !self.released {
            self.catalog
                .release_maintenance(
                    &self.job_id,
                    &self.slug,
                    self.bytes,
                    crate::clock::now_unix(),
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
                crate::clock::now_unix(),
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
            publication: AsyncMutex::new(()),
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
        match self
            .committed_payload_status(storage_id, epoch, sequence, &record_payload)
            .await
        {
            Ok(Some(true)) => {
                self.finish_pending(std::iter::once(identity)).await;
                return Ok(Vec::new());
            }
            Ok(Some(false)) => {
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
        let segments = {
            let mut coordinator = self.coordinator.lock().await;
            match coordinator.seal(true) {
                Ok(segments) => segments,
                Err(error) => {
                    self.finish_pending(std::iter::once(identity)).await;
                    return Err(error);
                }
            }
        };
        let identities = segment_identities(&segments);
        let operation_id = format!(
            "room-{}-{}-{}-{}",
            storage_id,
            sequence,
            crate::clock::now_unix(),
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
            crate::clock::now_unix(),
            &plan,
        ) {
            self.coordinator.lock().await.requeue(segments);
            self.finish_pending(identities.into_iter()).await;
            return Err(error);
        }
        let written = match self.write_segments(&operation_id, &segments).await {
            Ok(written) => written,
            Err(error) => {
                // The preparation remains unresolved deliberately: a
                // partial object write is an ambiguous publication and
                // startup must reconcile it before admitting another
                // operation. Keep the records in memory for that retry
                // once reconciliation has established the outcome.
                self.coordinator.lock().await.requeue(segments);
                self.finish_pending(identities.into_iter()).await;
                return Err(error);
            }
        };
        let result = self
            .store
            .commit_segments(&operation_id, &written, crate::clock::now_unix());
        let (_, sequences) = match result {
            Ok(value) => value,
            Err(error) => {
                for (segment, written_segment) in segments.iter().zip(&written) {
                    if let Some(record) = segment.records.first() {
                        let _ = self.store.abort_object(
                            &record.storage_id,
                            &operation_id,
                            &written_segment.object_key,
                        );
                    }
                }
                self.finish_pending(identities.into_iter()).await;
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
                    for (remaining_segment, remaining_written) in segments.iter().zip(&written) {
                        if let Some(remaining_record) = remaining_segment.records.first() {
                            let _ = self.store.abort_object(
                                &remaining_record.storage_id,
                                &operation_id,
                                &remaining_written.object_key,
                            );
                        }
                    }
                    self.finish_pending(identities.into_iter()).await;
                    return Err(error);
                }
            }
        }
        self.finish_pending(identities.into_iter()).await;
        Ok(sequences)
    }

    async fn finish_pending(&self, identities: impl Iterator<Item = (String, u64, u64)>) {
        let mut pending = self.pending.lock().await;
        for identity in identities {
            pending.remove(&identity);
        }
        drop(pending);
        self.pending_notify.notify_waiters();
    }

    async fn committed_payload_status(
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
            let recovered: RecoveryBaseBody = serde_json::from_slice(&body).map_err(|error| {
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

    async fn write_segments(
        &self,
        operation_id: &str,
        segments: &[Segment],
    ) -> JournalResult<Vec<WrittenSegment>> {
        let mut result = Vec::with_capacity(segments.len());
        let mut reservations: Vec<(String, String)> = Vec::with_capacity(segments.len());
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
            if let Err(error) = self.store.reserve_object(
                owner,
                operation_id,
                &key,
                "journal_segment",
                body.len() as i64,
                self.owner_limit,
                self.total_limit,
            ) {
                for (reserved_owner, object_key) in &reservations {
                    let _ = self
                        .store
                        .abort_object(reserved_owner, operation_id, object_key);
                }
                return Err(error);
            }
            reservations.push((owner.to_owned(), key.clone()));
            if let Err(error) = self
                .blobs
                .put(&key, body.clone(), "application/octet-stream")
                .await
            {
                // A failed object write is known abort; release the exact
                // replacement reservation before returning.
                let _ = self.store.abort_object(owner, operation_id, &key);
                for (owner, object_key) in reservations {
                    let _ = self.store.abort_object(&owner, operation_id, &object_key);
                }
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
        let state = self.store.state()?;
        let operation_id = format!(
            "compact-{}-{}-{}-{}",
            storage_id,
            sequence,
            crate::clock::now_unix(),
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
        let base_bytes = serde_json::to_vec(&base_body)
            .map_err(|error| JournalError::Invalid(format!("invalid recovery base: {error}")))?;
        if base_bytes.len() > MAX_SEGMENT_BYTES {
            return Err(JournalError::Limit("recovery base is too large".into()));
        }
        let base_digest = hex::encode(Sha256::digest(&base_bytes));
        let base = RecoveryBase {
            base_id: format!("{storage_id}-{epoch}-{sequence}"),
            storage_id: storage_id.to_owned(),
            epoch,
            sequence,
            object_key: base_key.clone(),
            digest: base_digest,
            encoded_bytes: base_bytes.len() as i64,
            committed_at: crate::clock::now_unix(),
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
        self.store.prepare(
            &operation_id,
            "compact",
            state.revision,
            &state.writer_generation,
            base.committed_at,
            &plan,
        )?;
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
            return Err(error);
        }
        if let Err(error) = self
            .blobs
            .put(&base_key, base_bytes, "application/json")
            .await
        {
            for (key, _) in &accounting_keys {
                let _ = self.store.abort_object(storage_id, &operation_id, key);
            }
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
                    .abort_object(storage_id, &operation_id, &shard.object_key);
                for (key, _) in &accounting_keys {
                    let _ = self.store.abort_object(storage_id, &operation_id, key);
                }
                return Err(error);
            }
            accounting_keys.push((shard.object_key.clone(), shard_bytes.len() as i64));
            if let Err(error) = self
                .blobs
                .put(&shard.object_key, shard_bytes, "application/json")
                .await
            {
                for (key, _) in &accounting_keys {
                    let _ = self.store.abort_object(storage_id, &operation_id, key);
                }
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
                for (key, _) in &accounting_keys {
                    let _ = self.store.abort_object(storage_id, &operation_id, key);
                }
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
                for (remaining_key, _) in &accounting_keys {
                    let _ = self
                        .store
                        .abort_object(storage_id, &operation_id, remaining_key);
                }
                return Err(error);
            }
        }
        if let Some(borrow) = maintenance_borrow.as_mut() {
            borrow.release()?;
        }
        Ok(committed)
    }

    /// Recover the newest complete-state record for a document from committed
    /// segments.  Segment descriptors come from the catalogue first; an
    /// object-store listing is never treated as an authoritative journal.
    pub async fn recover_latest(&self, storage_id: &str) -> JournalResult<Option<Vec<u8>>> {
        self.verify_manifest_chain().await?;
        let mut latest: Option<(u64, u64, Vec<u8>)> = None;
        let base = self.store.recovery_base(storage_id)?;
        if let Some(base) = base {
            let body = self.blobs.get(&base.object_key).await?;
            if hex::encode(Sha256::digest(&body)) != base.digest {
                return Err(JournalError::Corrupt(format!(
                    "base {} digest does not match catalogue",
                    base.object_key
                )));
            }
            let decoded: RecoveryBaseBody = serde_json::from_slice(&body).map_err(|error| {
                JournalError::Corrupt(format!("invalid recovery base: {error}"))
            })?;
            if decoded.storage_id != storage_id
                || decoded.epoch != base.epoch
                || decoded.sequence != base.sequence
                || decoded.digest != hex::encode(Sha256::digest(&decoded.payload))
            {
                return Err(JournalError::Corrupt(
                    "recovery base identity or digest mismatch".into(),
                ));
            }
            latest = Some((decoded.epoch, decoded.sequence, decoded.payload));
            let descriptors = self
                .store
                .replay_descriptors(storage_id, Some((base.epoch, base.sequence)))?;
            return self
                .recover_from_descriptors(storage_id, descriptors, latest)
                .await;
        }
        let descriptors = self.store.replay_descriptors(storage_id, None)?;
        self.recover_from_descriptors(storage_id, descriptors, latest)
            .await
    }

    async fn verify_manifest_chain(&self) -> JournalResult<()> {
        let state = self.store.state()?;
        if state.manifest_key.is_empty() {
            return Ok(());
        }
        let mut key = state.manifest_key;
        let mut visited = std::collections::HashSet::new();
        for index in 0..4096usize {
            if !visited.insert(key.clone()) {
                return Err(JournalError::Corrupt("manifest shard cycle".into()));
            }
            let body = self.blobs.get(&key).await?;
            let shard: ManifestShard = serde_json::from_slice(&body).map_err(|error| {
                JournalError::Corrupt(format!("invalid manifest shard: {error}"))
            })?;
            if shard.object_key != key || shard.encoded_bytes != body.len() as i64 {
                return Err(JournalError::Corrupt(
                    "manifest shard identity or length mismatch".into(),
                ));
            }
            let digest = shard.digest.clone();
            let mut canonical = shard;
            canonical.digest.clear();
            let canonical_bytes = serde_json::to_vec(&canonical).map_err(|error| {
                JournalError::Corrupt(format!("invalid manifest shard: {error}"))
            })?;
            if hex::encode(Sha256::digest(&canonical_bytes)) != digest {
                return Err(JournalError::Corrupt(
                    "manifest shard digest mismatch".into(),
                ));
            }
            if index == 0
                && (body.len() as i64 != state.manifest_length || digest != state.manifest_digest)
            {
                return Err(JournalError::Corrupt(
                    "manifest root does not match journal state".into(),
                ));
            }
            let Some(next) = canonical.next_key else {
                return Ok(());
            };
            key = next;
        }
        Err(JournalError::Limit(
            "manifest shard chain is too long".into(),
        ))
    }

    async fn recover_from_descriptors(
        &self,
        storage_id: &str,
        descriptors: Vec<(String, String, i64)>,
        latest: Option<(u64, u64, Vec<u8>)>,
    ) -> JournalResult<Option<Vec<u8>>> {
        let mut latest_identity: Option<(u64, u64)> = None;
        let mut latest_fragments = Vec::<JournalRecord>::new();
        for (key, expected_digest, _) in descriptors {
            let body = self.blobs.get(&key).await?;
            let digest = hex::encode(Sha256::digest(&body));
            if digest != expected_digest {
                return Err(JournalError::Corrupt(format!(
                    "segment {key} digest does not match catalogue"
                )));
            }
            let segment = Segment::decode_for(&body, storage_id)?;
            for record in segment.records {
                if record.storage_id != storage_id {
                    continue;
                }
                let identity = (record.epoch, record.sequence);
                match latest_identity {
                    Some(previous) if identity < previous => continue,
                    Some(previous) if identity == previous => {
                        if let Some(existing) = latest_fragments
                            .iter()
                            .find(|existing| existing.fragment_index == record.fragment_index)
                        {
                            if existing.digest != record.digest
                                || existing.chunk_digest != record.chunk_digest
                                || existing.payload != record.payload
                            {
                                return Err(JournalError::Corrupt(format!(
                                    "sequence {} in epoch {} has conflicting payloads",
                                    record.sequence, record.epoch
                                )));
                            }
                        } else {
                            latest_fragments.push(record);
                        }
                    }
                    _ => {
                        latest_identity = Some(identity);
                        latest_fragments.clear();
                        latest_fragments.push(record);
                    }
                }
            }
        }
        let tail = latest_identity
            .map(|(epoch, sequence)| {
                assemble_fragments(&latest_fragments).map(|payload| (epoch, sequence, payload))
            })
            .transpose()?;
        Ok(match (latest, tail) {
            (Some((base_epoch, base_sequence, _)), Some((epoch, sequence, payload)))
                if (epoch, sequence) > (base_epoch, base_sequence) =>
            {
                Some(payload)
            }
            (Some((_, _, payload)), _) => Some(payload),
            (None, Some((_, _, payload))) => Some(payload),
            (None, None) => None,
        })
    }
}

fn assemble_fragments(fragments: &[JournalRecord]) -> JournalResult<Vec<u8>> {
    let Some(first) = fragments.first() else {
        return Err(JournalError::Corrupt("empty record fragment set".into()));
    };
    let count = first.fragment_count as usize;
    if fragments.len() != count
        || fragments.iter().any(|fragment| {
            fragment.fragment_count as usize != count || fragment.digest != first.digest
        })
    {
        return Err(JournalError::Corrupt("incomplete record fragments".into()));
    }
    let mut ordered = fragments.to_vec();
    ordered.sort_by_key(|fragment| fragment.fragment_index);
    if ordered
        .iter()
        .enumerate()
        .any(|(index, fragment)| fragment.fragment_index as usize != index)
    {
        return Err(JournalError::Corrupt(
            "record fragment indexes have a gap".into(),
        ));
    }
    let payload = ordered
        .into_iter()
        .flat_map(|fragment| fragment.payload)
        .collect::<Vec<_>>();
    if hex::encode(Sha256::digest(&payload)) != first.digest {
        return Err(JournalError::Corrupt(
            "record fragment digest mismatch".into(),
        ));
    }
    Ok(payload)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalPlan {
    pub version: u16,
    pub output_keys: Vec<String>,
    pub covered: Vec<CoveredRange>,
    pub protected_input_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryBase {
    pub base_id: String,
    pub storage_id: String,
    pub epoch: u64,
    pub sequence: u64,
    pub object_key: String,
    pub digest: String,
    pub encoded_bytes: i64,
    pub committed_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestShard {
    pub shard_id: String,
    pub shard_seq: u64,
    pub object_key: String,
    pub digest: String,
    pub encoded_bytes: i64,
    pub committed_at: i64,
    /// The manifest root is the first shard; the chain keeps the complete
    /// bounded shard set discoverable without an unbounded root document.
    #[serde(default)]
    pub next_key: Option<String>,
    pub bases: Vec<RecoveryBase>,
    pub segments: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct RecoveryBaseBody {
    format_version: u16,
    storage_id: String,
    epoch: u64,
    sequence: u64,
    payload: Vec<u8>,
    digest: String,
}

/// Manifest digests cover the canonical descriptor with its digest field
/// blanked. This avoids a self-referential hash while retaining an
/// independently verifiable catalogue pointer.
pub(crate) fn finalize_manifest_shard(
    mut shard: ManifestShard,
) -> JournalResult<(ManifestShard, Vec<u8>)> {
    shard.digest.clear();
    shard.encoded_bytes = 0;
    for _ in 0..3 {
        shard.digest.clear();
        let canonical = serde_json::to_vec(&shard)
            .map_err(|error| JournalError::Invalid(format!("invalid manifest shard: {error}")))?;
        shard.digest = hex::encode(Sha256::digest(&canonical));
        let encoded = serde_json::to_vec(&shard)
            .map_err(|error| JournalError::Invalid(format!("invalid manifest shard: {error}")))?;
        shard.encoded_bytes = encoded.len() as i64;
    }
    let encoded = serde_json::to_vec(&shard)
        .map_err(|error| JournalError::Invalid(format!("invalid manifest shard: {error}")))?;
    if encoded.len() > MAX_METADATA_BYTES {
        return Err(JournalError::Limit("manifest shard is too large".into()));
    }
    Ok((shard, encoded))
}

fn build_manifest_shards(
    deployment_id: &str,
    root_seq: u64,
    committed_at: i64,
    bases: Vec<RecoveryBase>,
    segments: Vec<String>,
) -> JournalResult<Vec<ManifestShard>> {
    let mut chunks: Vec<(Vec<RecoveryBase>, Vec<String>)> = Vec::new();
    let mut current = (Vec::new(), Vec::new());
    let fits = |candidate: &(Vec<RecoveryBase>, Vec<String>)| {
        let probe = ManifestShard {
            shard_id: "probe".into(),
            shard_seq: root_seq,
            object_key: "probe".into(),
            digest: String::new(),
            encoded_bytes: 0,
            committed_at,
            next_key: None,
            bases: candidate.0.clone(),
            segments: candidate.1.clone(),
        };
        serde_json::to_vec(&probe)
            .map(|encoded| encoded.len() <= MAX_METADATA_BYTES.saturating_sub(512))
            .unwrap_or(false)
    };
    for base in bases {
        let mut candidate = (current.0.clone(), current.1.clone());
        candidate.0.push(base.clone());
        if !current.0.is_empty() || !current.1.is_empty() {
            if !fits(&candidate) {
                chunks.push(current);
                current = (vec![base], Vec::new());
                continue;
            }
        } else if !fits(&candidate) {
            return Err(JournalError::Limit(
                "recovery base exceeds manifest shard limit".into(),
            ));
        }
        current = candidate;
    }
    for segment in segments {
        let mut candidate = (current.0.clone(), current.1.clone());
        candidate.1.push(segment.clone());
        if !current.0.is_empty() || !current.1.is_empty() {
            if !fits(&candidate) {
                chunks.push(current);
                current = (Vec::new(), vec![segment]);
                continue;
            }
        } else if !fits(&candidate) {
            return Err(JournalError::Limit(
                "manifest segment exceeds shard limit".into(),
            ));
        }
        current = candidate;
    }
    if !current.0.is_empty() || !current.1.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push((Vec::new(), Vec::new()));
    }
    let keys: Vec<String> = (0..chunks.len())
        .map(|index| journal_manifest_key(deployment_id, &format!("{root_seq}-{index}")))
        .collect();
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(index, (bases, segments))| ManifestShard {
            shard_id: format!("manifest-{root_seq}-{index}"),
            shard_seq: root_seq.saturating_add(index as u64),
            object_key: keys[index].clone(),
            digest: String::new(),
            encoded_bytes: 0,
            committed_at,
            next_key: keys.get(index + 1).cloned(),
            bases,
            segments,
        })
        .collect())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoveredRange {
    pub storage_id: String,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

impl JournalPlan {
    fn encoded(&self) -> JournalResult<String> {
        let encoded = serde_json::to_string(self)
            .map_err(|error| JournalError::Invalid(format!("invalid journal plan: {error}")))?;
        if encoded.len() > MAX_METADATA_BYTES {
            return Err(JournalError::Limit("journal plan is too large".into()));
        }
        Ok(encoded)
    }
}

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

impl From<crate::catalog::JournalState> for JournalState {
    fn from(state: crate::catalog::JournalState) -> Self {
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
    catalog: Arc<Catalog>,
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
    fn reserve_object(
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
            .reserve_object_change(crate::catalog::ObjectReservationRequest {
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

    fn commit_object(
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

    fn abort_object(
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
            self.commit_segments(
                &preparation.operation_id,
                &written,
                crate::clock::now_unix(),
            )?;
            return Ok(());
        }
        let lengths = written
            .iter()
            .map(|segment| (segment.object_key.clone(), segment.encoded_bytes))
            .collect::<Vec<_>>();
        self.abort_preparation(&preparation, &plan, &lengths)?;
        Ok(())
    }

    fn resolve_committed_preparation(
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
                    params![preparation.operation_id, crate::clock::now_unix()],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                Ok(true)
            })
            .map_err(JournalError::from)
    }

    fn resolve_preparation(&self, operation_id: &str) -> JournalResult<()> {
        self.catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE journal_preparations SET resolved_at=?2
                         WHERE operation_id=?1 AND resolved_at IS NULL",
                        params![operation_id, crate::clock::now_unix()],
                    )
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .map_err(JournalError::from)
    }

    fn abort_preparation(
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
                            crate::clock::now_unix()
                        ],
                    )
                    .map_err(CatalogError::from)?;
                }
                tx.execute(
                    "UPDATE journal_preparations SET resolved_at = ?2
                     WHERE operation_id = ?1 AND resolved_at IS NULL",
                    params![preparation.operation_id, crate::clock::now_unix()],
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

    fn segment_lengths(&self, keys: &[String]) -> JournalResult<Vec<(String, i64)>> {
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

fn read_state(connection: &rusqlite::Connection) -> rusqlite::Result<JournalState> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobStore, FsStore};
    use crate::catalog::NewDocument;
    use crate::maintenance::JournalRetirementWorker;

    #[test]
    fn segment_round_trip_and_digest_validation() {
        let record =
            JournalRecord::new("storage", 1, "retry", 7, b"update".to_vec()).expect("record");
        let segment = Segment::new(vec![record]).expect("segment");
        let encoded = segment.encode().expect("encode");
        assert_eq!(Segment::decode(&encoded).expect("decode"), segment);
        let mut broken = encoded;
        *broken.last_mut().expect("payload") ^= 1;
        assert!(matches!(
            Segment::decode(&broken),
            Err(JournalError::Corrupt(_))
        ));
    }

    #[test]
    fn coordinator_applies_queue_limits_and_seals() {
        let mut coordinator = JournalCoordinator::new(CoordinatorLimits {
            max_queued_bytes: 10,
            max_queued_records: 1,
            max_segment_bytes: MAX_SEGMENT_BYTES,
            max_records_per_segment: MAX_RECORDS_PER_SEGMENT,
        })
        .expect("limits");
        coordinator
            .enqueue(JournalRecord::new("storage", 1, "retry", 1, b"x".to_vec()).expect("record"))
            .expect("enqueue");
        assert!(matches!(
            coordinator.enqueue(
                JournalRecord::new("storage", 2, "retry-2", 1, b"x".to_vec()).expect("record")
            ),
            Err(JournalError::Limit(_))
        ));
        assert_eq!(coordinator.seal(true).expect("seal").len(), 1);
        assert_eq!(coordinator.queued_records(), 0);
    }

    #[test]
    fn journal_head_and_preparation_commit_together() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let store = JournalStore::new(catalog);
        let state = store
            .initialize("deployment", "generation")
            .expect("initialize");
        let plan = JournalPlan {
            version: 1,
            output_keys: vec!["journal/deployment/segments/op-0".into()],
            covered: vec![],
            protected_input_keys: vec![],
        };
        store
            .prepare("op", "flush", state.revision, "generation", 10, &plan)
            .expect("prepare");
        let committed = store
            .commit_segment(
                "op",
                &WrittenSegment {
                    segment_id: "op-0".into(),
                    object_key: "journal/deployment/segments/op-0".into(),
                    digest: "a".repeat(64),
                    encoded_bytes: 3,
                },
                11,
            )
            .expect("commit");
        assert_eq!(committed.0.revision, 1);
        assert_eq!(committed.1, 0);
        assert!(store.unresolved_preparation().expect("read prep").is_none());
    }

    #[tokio::test]
    async fn compaction_replays_base_and_retires_segments() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let runtime = JournalRuntime::new(
            catalog.clone(),
            blobs.clone(),
            "deployment",
            CoordinatorLimits::default(),
        )
        .expect("runtime");
        JournalStore::new(catalog.clone())
            .initialize("deployment", "generation")
            .expect("initialize");

        runtime
            .append("storage", 1, b"state-1".to_vec())
            .await
            .expect("append 1");
        runtime
            .append("storage", 2, b"state-2".to_vec())
            .await
            .expect("append 2");
        assert_eq!(
            runtime.recover_latest("storage").await.expect("recover"),
            Some(b"state-2".to_vec())
        );

        runtime
            .compact("storage", 0, 2, b"state-2".to_vec())
            .await
            .expect("compact");
        assert_eq!(
            runtime
                .recover_latest("storage")
                .await
                .expect("recover base"),
            Some(b"state-2".to_vec())
        );
        assert!(runtime
            .append("storage", 2, b"state-2".to_vec())
            .await
            .expect("retry")
            .is_empty());
        assert!(runtime
            .append("storage", 2, b"different-state".to_vec())
            .await
            .expect_err("compacted retry must compare the retained base digest")
            .to_string()
            .contains("different payload"));
        runtime
            .append_with_epoch("storage", 1, 1, b"state-epoch-1".to_vec())
            .await
            .expect("new epoch");
        assert_eq!(
            runtime
                .recover_latest("storage")
                .await
                .expect("new epoch recovery"),
            Some(b"state-epoch-1".to_vec())
        );
        let store = JournalStore::new(catalog.clone());
        assert_eq!(store.committed_segments().expect("segments").len(), 1);
        let base = store
            .recovery_base("storage")
            .expect("base metadata")
            .expect("base");
        assert!(blobs.get(&base.object_key).await.is_ok());

        let worker = JournalRetirementWorker::new(catalog, blobs.clone(), 16).expect("worker");
        assert_eq!(
            worker
                .run_once(crate::clock::now_unix())
                .await
                .expect("retire"),
            2
        );
        assert!(blobs.get(&base.object_key).await.is_ok());
    }

    #[tokio::test]
    async fn compaction_borrows_maintenance_headroom_at_full_quota() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        catalog
            .create_document(&NewDocument {
                slug: "full".into(),
                storage_id: "full-storage".into(),
                title: "full".into(),
                sha: String::new(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                published_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
                example: false,
                owner_key: "owner".into(),
                owner_id: None,
                status: "active".into(),
                size: 0,
                counted_size: 0,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "README.md".into(),
            })
            .expect("document");
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let runtime = JournalRuntime::new_with_limits(
            catalog.clone(),
            blobs,
            "deployment",
            CoordinatorLimits::default(),
            0,
            0,
        )
        .expect("runtime");
        JournalStore::new(catalog.clone())
            .initialize("deployment", "generation")
            .expect("initialize");
        runtime
            .compact("full-storage", 0, 1, b"full state".to_vec())
            .await
            .expect("compaction borrows reserve");
        let active_jobs: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM maintenance_jobs WHERE status='active'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::catalog::CatalogError::from)
            })
            .expect("maintenance jobs");
        assert_eq!(active_jobs, 0);
    }

    #[tokio::test]
    async fn concurrent_deployment_appends_preserve_per_document_coverage() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let runtime = JournalRuntime::new(
            catalog.clone(),
            blobs,
            "deployment",
            CoordinatorLimits::default(),
        )
        .expect("runtime");
        JournalStore::new(catalog.clone())
            .initialize("deployment", "generation")
            .expect("initialize");
        let (first, second) = tokio::join!(
            runtime.append("one", 1, b"one".to_vec()),
            runtime.append("two", 1, b"two".to_vec()),
        );
        first.expect("first append");
        second.expect("second append");
        assert_eq!(
            runtime.recover_latest("one").await.expect("one recovery"),
            Some(b"one".to_vec())
        );
        assert_eq!(
            runtime.recover_latest("two").await.expect("two recovery"),
            Some(b"two".to_vec())
        );
        let coverage: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM journal_segment_coverage", [], |row| {
                        row.get(0)
                    })
                    .map_err(crate::catalog::CatalogError::from)
            })
            .expect("coverage rows");
        assert_eq!(coverage, 2);
        let segments: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM journal_segments", [], |row| {
                        row.get(0)
                    })
                    .map_err(crate::catalog::CatalogError::from)
            })
            .expect("segment rows");
        assert_eq!(segments, 1, "concurrent rooms should share one segment");
        runtime
            .compact("one", 0, 1, b"one".to_vec())
            .await
            .expect("one compaction");
        assert_eq!(
            runtime
                .recover_latest("two")
                .await
                .expect("two after compaction"),
            Some(b"two".to_vec())
        );
        JournalStore::new(catalog.clone())
            .retire_storage("one", crate::clock::now_unix())
            .expect("retire one");
        assert_eq!(
            runtime
                .recover_latest("two")
                .await
                .expect("two after deletion retirement"),
            Some(b"two".to_vec())
        );
    }

    #[tokio::test]
    async fn shared_segment_rewrite_physically_excludes_erased_identity() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        for (slug, storage_id) in [("one", "one"), ("two", "two"), ("three", "three")] {
            catalog
                .create_document(&NewDocument {
                    slug: slug.into(),
                    storage_id: storage_id.into(),
                    title: slug.into(),
                    sha: String::new(),
                    created_at: "2026-01-01T00:00:00.000Z".into(),
                    published_at: "2026-01-01T00:00:00.000Z".into(),
                    updated_at: "2026-01-01T00:00:00.000Z".into(),
                    example: false,
                    owner_key: "owner".into(),
                    owner_id: None,
                    status: "active".into(),
                    size: 0,
                    counted_size: 0,
                    maintenance_reserved: 0,
                    last_auto_checkpoint_at: 0,
                    source_format: "markdown".into(),
                    main: "README.md".into(),
                })
                .expect("document");
        }
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let runtime = JournalRuntime::new(
            catalog.clone(),
            blobs.clone(),
            "deployment",
            CoordinatorLimits::default(),
        )
        .expect("runtime");
        JournalStore::new(catalog.clone())
            .initialize("deployment", "generation")
            .expect("initialize");
        let (one, two, three) = tokio::join!(
            runtime.append("one", 1, b"one".to_vec()),
            runtime.append("two", 1, b"two".to_vec()),
            runtime.append("three", 1, b"three".to_vec()),
        );
        one.expect("one append");
        two.expect("two append");
        three.expect("three append");
        let store = JournalStore::new(catalog.clone());
        let (segment_key, _) = store
            .committed_segments()
            .expect("segment descriptor")
            .into_iter()
            .next()
            .expect("shared segment");
        let manifest_key = crate::blob::journal_manifest_key("deployment", "reader-test");
        let (manifest, manifest_body) = finalize_manifest_shard(ManifestShard {
            shard_id: "reader-test".into(),
            shard_seq: 1,
            object_key: manifest_key.clone(),
            digest: String::new(),
            encoded_bytes: 0,
            committed_at: crate::clock::now_unix(),
            next_key: None,
            bases: Vec::new(),
            segments: vec![segment_key.clone()],
        })
        .expect("manifest");
        catalog
            .reserve_object_change(crate::catalog::ObjectReservationRequest {
                slug: "two",
                operation_id: "reader-manifest",
                object_key: &manifest_key,
                kind: "journal_manifest",
                new_bytes: manifest_body.len() as i64,
                owner_limit: -1,
                total_limit: -1,
            })
            .expect("manifest reservation");
        blobs
            .put(&manifest_key, manifest_body.clone(), "application/json")
            .await
            .expect("manifest object");
        catalog
            .commit_object_change(
                "two",
                "reader-manifest",
                &manifest_key,
                "journal_manifest",
                &manifest.digest,
            )
            .expect("manifest accounting");
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO journal_manifest_shards
                         (shard_id,shard_seq,object_key,digest,encoded_bytes,committed_at)
                         VALUES (?1,?2,?3,?4,?5,?6)",
                        rusqlite::params![
                            manifest.shard_id,
                            manifest.shard_seq,
                            manifest.object_key,
                            manifest.digest,
                            manifest_body.len() as i64,
                            manifest.committed_at
                        ],
                    )
                    .map_err(crate::catalog::CatalogError::from)?;
                connection
                    .execute(
                        "UPDATE journal_state SET manifest_key=?1,manifest_digest=?2,
                         manifest_length=?3 WHERE id=1",
                        rusqlite::params![
                            manifest_key,
                            manifest.digest,
                            manifest_body.len() as i64
                        ],
                    )
                    .map_err(crate::catalog::CatalogError::from)?;
                Ok(())
            })
            .expect("manifest graph");
        catalog.begin_delete("one").expect("begin delete");
        store
            .retire_storage("one", crate::clock::now_unix())
            .expect("retire one");
        // Queue another erasure before the worker runs. The retirement row is
        // keyed by the shared segment, so the rewrite must remove both erased
        // identities rather than only the most recently queued one.
        catalog.begin_delete("three").expect("begin delete three");
        store
            .retire_storage("three", crate::clock::now_unix())
            .expect("retire three");
        let worker =
            JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 16).expect("worker");
        assert_eq!(
            worker
                .run_once(crate::clock::now_unix())
                .await
                .expect("rewrite"),
            1
        );
        assert_eq!(
            runtime.recover_latest("two").await.expect("recover two"),
            Some(b"two".to_vec())
        );
        let segments = store.committed_segments().expect("segments");
        assert_eq!(segments.len(), 1);
        let body = blobs.get(&segments[0].0).await.expect("rewritten segment");
        let rewritten = Segment::decode(&body).expect("decode rewritten");
        assert!(rewritten
            .records
            .iter()
            .all(|record| record.storage_id == "two"));
        let (current_manifest_key, current_manifest_digest): (String, String) = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT manifest_key,manifest_digest FROM journal_state WHERE id=1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(crate::catalog::CatalogError::from)
            })
            .expect("manifest state");
        assert_ne!(current_manifest_key, manifest_key);
        let current_manifest = blobs
            .get(&current_manifest_key)
            .await
            .expect("rewritten manifest");
        let current_manifest: ManifestShard =
            serde_json::from_slice(&current_manifest).expect("manifest json");
        assert_eq!(current_manifest.digest, current_manifest_digest);
        assert!(current_manifest.segments.contains(&segments[0].0));
        assert!(!current_manifest.segments.contains(&segment_key));
        let counted: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT counted_size FROM documents WHERE storage_id='two'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::catalog::CatalogError::from)
            })
            .expect("counted bytes");
        let accounted: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COALESCE(SUM(bytes),0) FROM object_accounting
                         WHERE storage_id='two'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::catalog::CatalogError::from)
            })
            .expect("accounted bytes");
        assert_eq!(counted, accounted);
        assert!(counted >= body.len() as i64);
    }

    #[tokio::test]
    async fn restart_reconciles_complete_and_aborted_preparations() {
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let store = JournalStore::new(catalog.clone());
        store
            .initialize("deployment", "generation")
            .expect("initialize");

        let record =
            JournalRecord::new("storage", 1, "retry", 0, b"state".to_vec()).expect("record");
        let segment = Segment::new(vec![record]).expect("segment");
        let body = segment.encode().expect("segment bytes");
        let key = journal_segment_key("deployment", "complete-0");
        blobs
            .put(&key, body, "application/octet-stream")
            .await
            .expect("write segment");
        let plan = JournalPlan {
            version: 1,
            output_keys: vec![key],
            covered: vec![CoveredRange {
                storage_id: "storage".into(),
                epoch: 0,
                first_sequence: 1,
                last_sequence: 1,
            }],
            protected_input_keys: Vec::new(),
        };
        store
            .prepare("complete", "flush", 0, "generation", 1, &plan)
            .expect("prepare complete");
        store
            .reconcile_pending(blobs.as_ref())
            .await
            .expect("reconcile complete");
        assert!(store
            .unresolved_preparation()
            .expect("preparation")
            .is_none());
        assert_eq!(store.state().expect("state").revision, 1);

        let abort_plan = JournalPlan {
            version: 1,
            output_keys: vec![journal_segment_key("deployment", "abort-0")],
            covered: vec![CoveredRange {
                storage_id: "storage".into(),
                epoch: 0,
                first_sequence: 2,
                last_sequence: 2,
            }],
            protected_input_keys: Vec::new(),
        };
        store
            .prepare("abort", "flush", 1, "generation", 2, &abort_plan)
            .expect("prepare abort");
        store
            .reconcile_pending(blobs.as_ref())
            .await
            .expect("reconcile abort");
        assert!(store
            .unresolved_preparation()
            .expect("preparation")
            .is_none());
        let retired: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM journal_retirements WHERE object_key LIKE '%abort-0'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::catalog::CatalogError::from)
            })
            .expect("retirement");
        assert_eq!(retired, 1);
    }

    #[tokio::test]
    async fn restarted_cursor_rejects_conflicting_payload_and_accepts_next_sequence() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let runtime = JournalRuntime::new(
            catalog.clone(),
            blobs.clone(),
            "deployment",
            CoordinatorLimits::default(),
        )
        .expect("runtime");
        JournalStore::new(catalog.clone())
            .initialize("deployment", "generation")
            .expect("initialize");
        runtime
            .append("storage", 1, b"state-1".to_vec())
            .await
            .expect("first append");
        let restarted =
            JournalRuntime::new(catalog, blobs, "deployment", CoordinatorLimits::default())
                .expect("restarted runtime");
        assert_eq!(restarted.latest_sequence("storage", 0).expect("cursor"), 1);
        assert!(restarted
            .append("storage", 1, b"state-1".to_vec())
            .await
            .expect("idempotent retry")
            .is_empty());
        assert!(matches!(
            restarted
                .append("storage", 1, b"different".to_vec())
                .await,
            Err(JournalError::Invalid(message)) if message.contains("different payload")
        ));
        restarted
            .append("storage", 2, b"state-2".to_vec())
            .await
            .expect("next sequence");
        assert_eq!(
            restarted.recover_latest("storage").await.expect("recovery"),
            Some(b"state-2".to_vec())
        );
    }

    #[tokio::test]
    async fn large_snapshot_is_chunked_and_recovered_at_the_record_boundary() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let runtime = JournalRuntime::new(
            catalog.clone(),
            blobs,
            "deployment",
            CoordinatorLimits::default(),
        )
        .expect("runtime");
        JournalStore::new(catalog)
            .initialize("deployment", "generation")
            .expect("initialize");
        let payload = (0..(MAX_RECORD_CHUNK_BYTES * 2 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        runtime
            .append("large", 1, payload.clone())
            .await
            .expect("large append");
        assert_eq!(
            runtime.recover_latest("large").await.expect("recovery"),
            Some(payload)
        );
    }

    #[test]
    fn manifest_metadata_is_partitioned_into_bounded_shards() {
        let bases = (0..3_000)
            .map(|index| RecoveryBase {
                base_id: format!("base-{index}"),
                storage_id: format!("storage-{index}"),
                epoch: 0,
                sequence: index + 1,
                object_key: format!("journal/deployment/bases/storage-{index}/0-{}", index + 1),
                digest: "a".repeat(64),
                encoded_bytes: 128,
                committed_at: 1,
            })
            .collect();
        let shards = build_manifest_shards("deployment", 1, 1, bases, Vec::new())
            .expect("partition manifest");
        assert!(shards.len() > 1);
        for shard in shards {
            let (_, bytes) = finalize_manifest_shard(shard).expect("bounded shard");
            assert!(bytes.len() <= MAX_METADATA_BYTES);
        }
    }
}
