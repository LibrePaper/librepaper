//! Legacy shared-journal recovery runtime, retained for converter tests.

use super::*;

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
