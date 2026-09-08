//! The coordinator: the queue of records waiting to be sealed into segments,
//! and the limits that decide when a segment is full.

use super::*;

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
    pub(super) limits: CoordinatorLimits,
    pub(super) queued: VecDeque<JournalRecord>,
    pub(super) queued_bytes: usize,
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

    pub(super) fn enqueue_batch<I>(&mut self, records: I) -> JournalResult<()>
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

    pub(super) fn contains_identity(&self, storage_id: &str, epoch: u64, sequence: u64) -> bool {
        self.queued.iter().any(|record| {
            record.storage_id == storage_id && record.epoch == epoch && record.sequence == sequence
        })
    }

    pub(super) fn remove_identity(&mut self, storage_id: &str, epoch: u64, sequence: u64) -> bool {
        let mut removed = false;
        let mut retained = VecDeque::with_capacity(self.queued.len());
        while let Some(record) = self.queued.pop_front() {
            if record.storage_id == storage_id
                && record.epoch == epoch
                && record.sequence == sequence
            {
                self.queued_bytes = self.queued_bytes.saturating_sub(record.payload.len());
                removed = true;
            } else {
                retained.push_back(record);
            }
        }
        self.queued = retained;
        removed
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
            let mut per_document = HashMap::<String, usize>::new();
            while let Some(record) = self.queued.front() {
                let document_count = per_document.get(&record.storage_id).copied().unwrap_or(0);
                let record_size = encoded_record_len(record);
                if records.len() >= self.limits.max_records_per_segment
                    || document_count >= MAX_RECORDS_PER_DOCUMENT
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
                *per_document.entry(record.storage_id.clone()).or_default() += 1;
                records.push(record);
            }
            if records.is_empty() {
                self.requeue(segments);
                return Err(JournalError::Limit("record cannot fit in segment".into()));
            }
            let segment = Segment { records };
            if let Err(error) = segment.validate() {
                segments.push(segment);
                self.requeue(segments);
                return Err(error);
            }
            segments.push(segment);
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

pub(super) fn covered_ranges(segments: &[Segment]) -> Vec<CoveredRange> {
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

pub(super) fn segment_identities(segments: &[Segment]) -> HashSet<(String, u64, u64)> {
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

pub(super) fn segment_matches_plan(segment: &Segment, plan: &JournalPlan) -> bool {
    segment.records.iter().all(|record| {
        plan.covered.iter().any(|range| {
            range.storage_id == record.storage_id
                && range.epoch == record.epoch
                && record.sequence >= range.first_sequence
                && record.sequence <= range.last_sequence
        })
    })
}
