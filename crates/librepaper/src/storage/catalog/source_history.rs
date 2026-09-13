//! The durable reference graph for encoded source history.
//!
//! Checkpoint trees keep the logical file digest and remain the source of
//! truth.  This graph records the physical recipe/chunk representation so a
//! retained checkpoint can be pruned without guessing which shared objects
//! are still needed.  Object-store I/O is deliberately outside this module;
//! rows are committed before a writer lease is released and deletion is
//! handed to `pending_deletes` so failed physical deletes remain charged.

use super::*;

/// One encoded source file to publish with a checkpoint.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceHistoryRecord {
    pub file_digest: String,
    pub recipe_key: String,
    pub recipe_digest: String,
    pub codec: i64,
    pub uncompressed_bytes: i64,
    pub recipe_bytes: i64,
    pub objects: Vec<SourceHistoryObject>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceHistoryObject {
    pub object_key: String,
    pub kind: String,
    pub bytes: i64,
}

impl SourceHistoryRecord {
    /// Convert the worker output into the catalogue's compact representation.
    /// Encoded objects are all addressed as chunks, including the whole-file
    /// zstd fallback; this keeps the legacy `blobs/` sweep from deleting a
    /// newly published object behind the graph's back.
    pub fn from_encoded(
        storage_id: &str,
        encoded: &crate::storage::encoding::EncodedSource,
    ) -> Result<Self, CatalogError> {
        let file_digest = hex::encode(encoded.file_digest);
        let recipe_key = crate::storage::blob::content_recipe_key(storage_id, &file_digest);
        let recipe_digest = hex::encode(encoded.recipe_digest());
        let mut objects = vec![SourceHistoryObject {
            object_key: recipe_key.clone(),
            kind: "source_recipe".to_string(),
            bytes: i64::try_from(encoded.recipe_bytes.len())
                .map_err(|_| CatalogError::Invalid("recipe is too large".into()))?,
        }];
        // `EncodedSource::objects` contains only chunks emitted by this
        // encoding job; chunks found in the object ledger are deliberately
        // omitted there.  The recipe, however, names every chunk needed to
        // reconstruct the file.  Record every recipe edge so pruning an old
        // checkpoint can never mistake a reused chunk for dead data.
        let emitted_sizes: std::collections::HashMap<[u8; 32], i64> = encoded
            .objects
            .iter()
            .map(|object| {
                Ok((
                    object.digest,
                    i64::try_from(object.encoded.len())
                        .map_err(|_| CatalogError::Invalid("encoded object is too large".into()))?,
                ))
            })
            .collect::<Result<_, CatalogError>>()?;
        let mut seen = std::collections::HashSet::new();
        for reference in &encoded.recipe.chunks {
            if !seen.insert(reference.digest) {
                continue;
            }
            objects.push(SourceHistoryObject {
                object_key: crate::storage::blob::content_chunk_key(
                    storage_id,
                    &hex::encode(reference.digest),
                ),
                kind: "source_chunk".to_string(),
                // A zero is an unresolved sentinel for a reused chunk. The
                // publication transaction must resolve it from the durable
                // object ledger before inserting the graph edge.
                bytes: emitted_sizes.get(&reference.digest).copied().unwrap_or(0),
            });
        }
        Ok(Self {
            file_digest,
            recipe_key,
            recipe_digest,
            codec: encoded.codec() as i64,
            uncompressed_bytes: i64::try_from(encoded.uncompressed_len)
                .map_err(|_| CatalogError::Invalid("source is too large".into()))?,
            recipe_bytes: i64::try_from(encoded.recipe_bytes.len())
                .map_err(|_| CatalogError::Invalid("recipe is too large".into()))?,
            objects,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceHistoryLease {
    pub storage_id: String,
    pub operation_id: String,
    pub object_key: String,
    pub bytes: i64,
    pub created_at: i64,
    pub expires_at: i64,
}

impl Catalog {
    /// Return measured sizes for canonical v2 source objects.
    pub fn source_history_object_sizes(
        &self,
        storage_id: &str,
        object_keys: &[String],
    ) -> CatalogResult<std::collections::HashMap<String, i64>> {
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        if object_keys.len() > 256 || object_keys.iter().any(|key| key.is_empty()) {
            return Err(CatalogError::Invalid(
                "invalid source-history accounting lookup".into(),
            ));
        }
        self.with_connection(|connection| {
            let mut result = std::collections::HashMap::new();
            for key in object_keys {
                let (owner, bytes): (String, i64) = connection
                    .query_row(
                        "SELECT document_id,COALESCE(byte_length, reserved_bytes) FROM objects
                     WHERE document_id=?1 AND storage_key=?2 AND state='available'",
                        params![document_id.as_str(), key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                    .ok_or(CatalogError::NotFound)?;
                if owner != document_id.as_str() {
                    return Err(CatalogError::Invalid(
                        "source object crossed document namespace".into(),
                    ));
                }
                result.insert(key.clone(), bytes);
            }
            Ok(result)
        })
    }

    /// Return a durable source recipe and its directly addressed chunks. v2
    /// stores the file digest in the recipe's logical_digest column; the
    /// checkpoint edge table remains the source of truth for complete trees.
    pub fn source_history_record(
        &self,
        storage_id: &str,
        file_digest: &str,
    ) -> CatalogResult<Option<SourceHistoryRecord>> {
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        if file_digest.len() != 64
            || !file_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(CatalogError::Invalid(
                "source file digest must be lowercase SHA-256 hex".into(),
            ));
        }
        self.with_connection(|connection| {
            let recipe: Option<(String, String, String, i64, i64)> = connection
                .query_row(
                    "SELECT id,storage_key,digest,COALESCE(byte_length,0),encoding_version
                 FROM objects WHERE document_id=?1 AND kind='source_recipe'
                   AND logical_digest=?2 AND state='available' ORDER BY id LIMIT 1",
                    params![document_id.as_str(), file_digest],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some((recipe_id, recipe_key, recipe_digest, recipe_bytes, codec)) = recipe else {
                return Ok(None);
            };
            let mut statement = connection
                .prepare(
                    "SELECT DISTINCT o.storage_key,o.kind,COALESCE(o.byte_length,0)
                 FROM checkpoint_objects edge
                 JOIN checkpoint_objects recipe_edge
                   ON recipe_edge.document_id=edge.document_id
                  AND recipe_edge.checkpoint_id=edge.checkpoint_id
                  AND recipe_edge.object_id=?2
                 JOIN objects o
                   ON o.document_id=edge.document_id AND o.id=edge.object_id
                 WHERE edge.document_id=?1 AND o.kind='source_chunk' AND o.state='available'
                 ORDER BY o.storage_key LIMIT 16384",
                )
                .map_err(CatalogError::from)?;
            let objects = statement
                .query_map(params![document_id.as_str(), recipe_id], |row| {
                    Ok(SourceHistoryObject {
                        object_key: row.get(0)?,
                        kind: row.get(1)?,
                        bytes: row.get(2)?,
                    })
                })
                .map_err(CatalogError::from)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(CatalogError::from)?;
            Ok(Some(SourceHistoryRecord {
                file_digest: file_digest.to_owned(),
                recipe_key,
                recipe_digest,
                codec,
                uncompressed_bytes: 0,
                recipe_bytes,
                objects,
            }))
        })
    }

    /// Register operation-owned write leases for already allocated v2
    /// objects. Storage keys are matched exactly, so a document cannot lease
    /// a sibling namespace by sharing a textual prefix.
    pub fn begin_source_history_lease(
        &self,
        storage_id: &str,
        operation_id: &str,
        objects: &[SourceHistoryObject],
        created_at: i64,
        expires_at: i64,
    ) -> CatalogResult<()> {
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let operation = OperationId::new(operation_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        if objects.is_empty()
            || objects.len() > MAX_CHECKPOINT_OBJECTS
            || created_at < 0
            || expires_at <= created_at
        {
            return Err(CatalogError::Invalid("invalid source-history lease".into()));
        }
        self.immediate(|tx| {
            let writer_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            let (state, op_generation): (String, String) = tx.query_row(
                "SELECT state,writer_generation FROM operations WHERE document_id=?1 AND id=?2",
                params![document_id.as_str(), operation.as_str()], |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(CatalogError::from)?;
            if state != "prepared" || op_generation != writer_generation {
                return Err(CatalogError::Conflict("source operation is not prepared in the current writer generation".into()));
            }
            for object in objects {
                if object.bytes < 0 || object.kind.is_empty() {
                    return Err(CatalogError::Invalid("invalid source-history object lease".into()));
                }
                let (kind, state, measured): (String, String, Option<i64>) = tx.query_row(
                    "SELECT kind,state,byte_length FROM objects WHERE document_id=?1 AND storage_key=?2",
                    params![document_id.as_str(), object.object_key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                ).optional().map_err(CatalogError::from)?
                    .ok_or(CatalogError::NotFound)?;
                if kind != object.kind || !matches!(state.as_str(), "allocated" | "available") {
                    return Err(CatalogError::Conflict("source object kind or state changed".into()));
                }
                if measured.is_some_and(|value| value != object.bytes) {
                    return Err(CatalogError::Conflict("source object size changed".into()));
                }
                let previous: Option<(i64, String, i64)> = tx.query_row(
                    "SELECT expires_at,writer_generation,created_at FROM object_leases
                     WHERE document_id=?1 AND object_id=(SELECT id FROM objects WHERE document_id=?1 AND storage_key=?2)
                       AND holder_id=?3 AND purpose='write' AND operation_id=?4",
                    params![document_id.as_str(), object.object_key, operation.as_str(), operation.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                ).optional().map_err(CatalogError::from)?;
                if let Some((old_expiry, old_generation, old_created)) = previous {
                    if old_expiry != expires_at || old_generation != writer_generation || old_created != created_at {
                        return Err(CatalogError::Conflict("source-history lease was reused with a different object".into()));
                    }
                    continue;
                }
                tx.execute(
                    "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at)
                     SELECT ?1,id,?3,'write',?3,?4,?5,?6 FROM objects
                     WHERE document_id=?1 AND storage_key=?2",
                    params![document_id.as_str(), object.object_key, operation.as_str(), writer_generation, created_at, expires_at],
                ).map_err(CatalogError::from)?;
            }
            Ok(())
        })
    }

    /// Release completed operation-owned source leases.
    pub fn finish_source_history_lease(
        &self,
        storage_id: &str,
        operation_id: &str,
    ) -> CatalogResult<()> {
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let operation = OperationId::new(operation_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        self.immediate(|tx| {
            tx.execute("DELETE FROM object_leases WHERE document_id=?1 AND operation_id=?2 AND purpose='write'", params![document_id.as_str(), operation.as_str()]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Extend operation-owned source leases only while the operation and
    /// writer generation remain current and every lease is still alive.
    pub fn renew_source_history_lease(
        &self,
        storage_id: &str,
        operation_id: &str,
        now: i64,
        expires_at: i64,
    ) -> CatalogResult<()> {
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let operation = OperationId::new(operation_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        if now < 0 || expires_at <= now {
            return Err(CatalogError::Invalid(
                "invalid source-history lease renewal".into(),
            ));
        }
        self.immediate(|tx| {
            let generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            let active: i64 = tx.query_row(
                "SELECT count(*) FROM object_leases l JOIN operations o ON o.document_id=l.document_id AND o.id=l.operation_id
                 WHERE l.document_id=?1 AND l.operation_id=?2 AND l.purpose='write' AND o.state='prepared'
                   AND l.writer_generation=?3 AND l.expires_at>?4",
                params![document_id.as_str(), operation.as_str(), generation, now], |r| r.get(0),
            ).map_err(CatalogError::from)?;
            if active == 0 { return Err(CatalogError::NotFound); }
            let changed = tx.execute(
                "UPDATE object_leases SET expires_at=?1 WHERE document_id=?2 AND operation_id=?3
                 AND purpose='write' AND writer_generation=?4 AND expires_at>?5",
                params![expires_at, document_id.as_str(), operation.as_str(), generation, now],
            ).map_err(CatalogError::from)?;
            if changed == 0 { return Err(CatalogError::Conflict("source-history lease has expired".into())); }
            Ok(())
        })
    }

    pub(crate) fn require_active_source_history_lease_tx(
        tx: &Transaction<'_>,
        storage_id: &str,
        operation_id: &str,
    ) -> CatalogResult<()> {
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let operation = OperationId::new(operation_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM object_leases l JOIN operations o
                 ON o.document_id=l.document_id AND o.id=l.operation_id
             WHERE l.document_id=?1 AND l.operation_id=?2 AND l.purpose='write'
               AND o.state='prepared' AND l.writer_generation=(SELECT writer_generation FROM server_state WHERE id=1)
               AND l.expires_at>?3)",
            params![document_id.as_str(), operation.as_str(), unix_millis()], |r| r.get(0),
        ).map_err(CatalogError::from)?;
        if !active {
            return Err(CatalogError::Conflict(
                "source-history writer lease is missing or expired".into(),
            ));
        }
        Ok(())
    }

    /// Remove expired write leases in a bounded transaction. Expiration never
    /// changes object accounting; the owning operation must settle or abort.
    pub fn expire_source_history_leases(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        if now < 0 || limit == 0 {
            return Err(CatalogError::Invalid(
                "invalid source-history lease sweep".into(),
            ));
        }
        self.immediate(|tx| {
            let mut statement = tx.prepare(
                "SELECT document_id,object_id,holder_id FROM object_leases
                 WHERE purpose='write' AND expires_at<=?1
                 ORDER BY expires_at,document_id,object_id,holder_id LIMIT ?2",
            ).map_err(CatalogError::from)?;
            let keys = statement.query_map(params![now, i64::from(limit.min(10_000))], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            }).map_err(CatalogError::from)?.collect::<rusqlite::Result<Vec<_>>>().map_err(CatalogError::from)?;
            drop(statement);
            let mut removed = 0u32;
            for (document_id, object_id, holder_id) in keys {
                removed = removed.saturating_add(tx.execute(
                    "DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND holder_id=?3 AND purpose='write' AND expires_at<=?4",
                    params![document_id, object_id, holder_id, now],
                ).map_err(CatalogError::from)? as u32);
            }
            Ok(removed)
        })
    }

    /// Attach already allocated v2 source objects to a checkpoint. Physical
    /// recipe decoding belongs to the storage worker; this boundary records
    /// only exact canonical object IDs and never recreates the removed v1
    /// source-history tables.
    pub(crate) fn insert_source_history_tx(
        tx: &Transaction<'_>,
        storage_id: &str,
        checkpoint_sha: &str,
        records: &[SourceHistoryRecord],
    ) -> CatalogResult<()> {
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let checkpoint_id = CheckpointId::new(checkpoint_sha.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        if records.len() > MAX_CHECKPOINT_OBJECTS {
            return Err(CatalogError::Invalid(
                "source-history closure is too large".into(),
            ));
        }
        for record in records {
            if record.file_digest.len() != 64
                || !record
                    .file_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || record.recipe_key.is_empty()
                || record.recipe_digest.len() != 64
                || !record
                    .recipe_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || record.codec < 0
                || record.uncompressed_bytes < 0
                || record.recipe_bytes < 0
            {
                return Err(CatalogError::Invalid(
                    "invalid source-history record metadata".into(),
                ));
            }
            for object in &record.objects {
                let (object_id, kind, state, byte_length): (String, String, String, Option<i64>) = tx.query_row(
                    "SELECT id,kind,state,byte_length FROM objects WHERE document_id=?1 AND storage_key=?2",
                    params![document_id.as_str(), object.object_key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
                if kind != object.kind || state != "available" {
                    return Err(CatalogError::Conflict(
                        "source-history object is not settled".into(),
                    ));
                }
                if object.bytes < 0 || byte_length != Some(object.bytes) && object.bytes != 0 {
                    return Err(CatalogError::Conflict(
                        "source-history object size changed".into(),
                    ));
                }
                tx.execute(
                    "INSERT OR IGNORE INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,?2,?3)",
                    params![document_id.as_str(), checkpoint_id.as_str(), object_id],
                ).map_err(CatalogError::from)?;
            }
        }
        Ok(())
    }

    /// Delete bounded v2 checkpoints through the same retention/protection
    /// fence as the worker. Object edges and GC deadlines are updated by the
    /// typed checkpoint transaction; the old source-history graph is gone.
    pub fn delete_checkpoints_with_source_history(
        &self,
        slug: &str,
        shas: &[String],
        queued_at: i64,
        delete_after: i64,
    ) -> CatalogResult<Vec<String>> {
        if slug.is_empty() || shas.is_empty() || queued_at < 0 || delete_after < queued_at {
            return Err(CatalogError::Invalid(
                "invalid source-history pruning".into(),
            ));
        }
        let document_id = self
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT id FROM documents WHERE slug=?1 AND status <> 'deleting'",
                        [slug],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)
            })?
            .ok_or(CatalogError::NotFound)?;
        let document_id =
            DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let now = UnixMillis::new(delete_after)?;
        let mut removed = Vec::new();
        for sha in shas.iter().take(32) {
            let checkpoint_id =
                CheckpointId::new(sha.clone()).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if self.delete_v2_checkpoint(&document_id, &checkpoint_id, now)? {
                removed.push(sha.clone());
            }
        }
        Ok(removed)
    }
}
