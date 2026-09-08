//! Recovery: rebuilding the latest state from a recovery base and the
//! segments after it, and the manifest shards a compaction writes.

use super::*;

pub(super) fn assemble_fragments(fragments: &[JournalRecord]) -> JournalResult<Vec<u8>> {
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
pub(super) struct RecoveryBaseBody {
    pub(super) format_version: u16,
    pub(super) storage_id: String,
    pub(super) epoch: u64,
    pub(super) sequence: u64,
    pub(super) payload: Vec<u8>,
    pub(super) digest: String,
}

const RECOVERY_BASE_MAGIC: &[u8; 4] = b"KJBS";
const RECOVERY_BASE_CODEC_VERSION: u16 = 1;
/// Compaction receives a complete snapshot, which may be larger than one
/// journal record. Keep its aggregate bounded independently of record framing.
pub const MAX_RECOVERY_BASE_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Encode recovery bases as bounded binary objects. Older deployments wrote
/// JSON with `Vec<u8>`'s array representation, so decoding deliberately keeps
/// that representation as a compatibility path.
pub(super) fn encode_recovery_base(body: &RecoveryBaseBody) -> JournalResult<Vec<u8>> {
    validate_recovery_base(body)?;
    let mut encoded = Vec::with_capacity(64 + body.storage_id.len() + body.payload.len());
    encoded.extend_from_slice(RECOVERY_BASE_MAGIC);
    put_u16(&mut encoded, RECOVERY_BASE_CODEC_VERSION);
    put_u16(&mut encoded, body.format_version);
    put_bytes_u16(&mut encoded, body.storage_id.as_bytes())?;
    put_u64(&mut encoded, body.epoch);
    put_u64(&mut encoded, body.sequence);
    put_u32(
        &mut encoded,
        u32::try_from(body.payload.len())
            .map_err(|_| JournalError::Limit("recovery base payload is too large".into()))?,
    );
    put_bytes_u16(&mut encoded, body.digest.as_bytes())?;
    encoded.extend_from_slice(&body.payload);
    if encoded.len() > MAX_RECOVERY_BASE_PAYLOAD_BYTES + 128 {
        return Err(JournalError::Limit("recovery base is too large".into()));
    }
    Ok(encoded)
}

pub(super) fn decode_recovery_base(bytes: &[u8]) -> JournalResult<RecoveryBaseBody> {
    let body = if bytes.starts_with(RECOVERY_BASE_MAGIC) {
        let mut cursor = Cursor::new(bytes);
        if cursor.take(4)? != RECOVERY_BASE_MAGIC {
            return Err(JournalError::Corrupt("bad recovery base magic".into()));
        }
        if cursor.u16()? != RECOVERY_BASE_CODEC_VERSION {
            return Err(JournalError::Corrupt(
                "unsupported recovery base version".into(),
            ));
        }
        let format_version = cursor.u16()?;
        let storage_id = String::from_utf8(cursor.bytes_u16()?)
            .map_err(|_| JournalError::Corrupt("recovery base storage id is not utf-8".into()))?;
        let epoch = cursor.u64()?;
        let sequence = cursor.u64()?;
        let payload_len = cursor.u32()? as usize;
        if payload_len > MAX_RECOVERY_BASE_PAYLOAD_BYTES {
            return Err(JournalError::Limit("recovery base is too large".into()));
        }
        let digest = String::from_utf8(cursor.bytes_u16()?)
            .map_err(|_| JournalError::Corrupt("recovery base digest is not utf-8".into()))?;
        let payload = cursor.take(payload_len)?.to_vec();
        if !cursor.is_empty() {
            return Err(JournalError::Corrupt("trailing recovery base bytes".into()));
        }
        RecoveryBaseBody {
            format_version,
            storage_id,
            epoch,
            sequence,
            payload,
            digest,
        }
    } else {
        serde_json::from_slice(bytes)
            .map_err(|error| JournalError::Corrupt(format!("invalid recovery base: {error}")))?
    };
    validate_recovery_base(&body)?;
    Ok(body)
}

fn validate_recovery_base(body: &RecoveryBaseBody) -> JournalResult<()> {
    if !matches!(body.format_version, LEGACY_SEGMENT_FORMAT | SEGMENT_FORMAT)
        || body.storage_id.is_empty()
        || body.sequence == 0
        || body.payload.is_empty()
        || body.payload.len() > MAX_RECOVERY_BASE_PAYLOAD_BYTES
        || body.digest != hex::encode(Sha256::digest(&body.payload))
    {
        return Err(JournalError::Corrupt(
            "invalid recovery base identity or digest".into(),
        ));
    }
    if body.storage_id.len() > u16::MAX as usize || body.digest.len() > u16::MAX as usize {
        return Err(JournalError::Limit(
            "recovery base metadata is too large".into(),
        ));
    }
    Ok(())
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

pub(super) fn build_manifest_shards(
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
    pub(super) fn encoded(&self) -> JournalResult<String> {
        let encoded = serde_json::to_string(self)
            .map_err(|error| JournalError::Invalid(format!("invalid journal plan: {error}")))?;
        if encoded.len() > MAX_METADATA_BYTES {
            return Err(JournalError::Limit("journal plan is too large".into()));
        }
        Ok(encoded)
    }
}

impl JournalRuntime {
    /// Recover the newest complete-state record for a document from committed
    /// segments.  Segment descriptors come from the catalogue first; an
    /// object-store listing is never treated as an authoritative journal.
    pub async fn recover_latest(&self, storage_id: &str) -> JournalResult<Option<Vec<u8>>> {
        let _publication = self.publication.lock().await;
        self.verify_manifest_chain().await?;
        let mut latest: Option<(u64, u64, Vec<u8>)> = None;
        let base = self.store.recovery_base(storage_id.to_owned()).await?;
        if let Some(base) = base {
            let body = self.blobs.get(&base.object_key).await?;
            if hex::encode(Sha256::digest(&body)) != base.digest {
                return Err(JournalError::Corrupt(format!(
                    "base {} digest does not match catalogue",
                    base.object_key
                )));
            }
            let decoded = decode_recovery_base(&body)?;
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
                .replay_descriptors(storage_id, Some((base.epoch, base.sequence)))
                .await?;
            return self
                .recover_from_descriptors(storage_id, descriptors, latest)
                .await;
        }
        let descriptors = self.store.replay_descriptors(storage_id, None).await?;
        self.recover_from_descriptors(storage_id, descriptors, latest)
            .await
    }

    pub(super) async fn verify_manifest_chain(&self) -> JournalResult<()> {
        let state = self.store.state_async().await?;
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

    pub(super) async fn recover_from_descriptors(
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
