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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::catalog::tests::{account, document};
    use crate::storage::catalog::{Catalog, Checkpoint};
    use crate::storage::encoding;
    use std::collections::HashSet;

    #[test]
    fn reused_recipe_chunks_are_graph_edges_with_accounted_sizes() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.upsert_account(&account()).unwrap();
        catalog.create_document(&document()).unwrap();

        let source: Vec<u8> = (0..32_768).map(|position| (position % 251) as u8).collect();
        let plan =
            encoding::plan_source_with_profile(&source, encoding::EncodingProfile::default())
                .unwrap();
        let all_chunks: HashSet<_> = plan
            .recipe
            .chunks
            .iter()
            .map(|chunk| chunk.digest)
            .collect();
        let encoded = encoding::encode_source_from_plan(&source, plan, &all_chunks).unwrap();
        assert!(
            encoded.objects.is_empty(),
            "the fixture must exercise reuse"
        );

        let record = SourceHistoryRecord::from_encoded("storage-1", &encoded).unwrap();
        let chunk_objects: Vec<_> = record
            .objects
            .iter()
            .filter(|object| object.kind == "source_chunk")
            .collect();
        assert_eq!(chunk_objects.len(), all_chunks.len());
        assert!(chunk_objects.iter().all(|object| object.bytes == 0));

        catalog
            .with_connection(|connection| {
                for object in &chunk_objects {
                    connection.execute(
                        "INSERT INTO object_accounting
                         (storage_id,object_key,kind,bytes,version)
                         VALUES(?1,?2,'source_chunk',37,'test')",
                        rusqlite::params!["storage-1", object.object_key],
                    )?;
                }
                Ok(())
            })
            .unwrap();

        let first = Checkpoint {
            slug: "doc".into(),
            sha: "checkpoint-reused".into(),
            seq: -1,
            durable_seq: 0,
            tree_sha: "checkpoint-reused".into(),
            parent: String::new(),
            at: "2026-01-01T00:00:00Z".into(),
            by: String::new(),
            why: "test".into(),
            source_format: "markdown".into(),
            size: source.len() as i64,
            label: String::new(),
            git_commit: String::new(),
            dirty: false,
            changed: None,
            by_account: None,
        };
        catalog
            .insert_checkpoints_atomic_with_sources(
                std::slice::from_ref(&first),
                None,
                std::slice::from_ref(&record),
            )
            .unwrap();
        let mut second = first.clone();
        second.sha = "checkpoint-reused-2".into();
        catalog
            .insert_checkpoints_atomic_with_sources(
                std::slice::from_ref(&second),
                None,
                std::slice::from_ref(&record),
            )
            .unwrap();

        let stored = catalog
            .source_history_record("storage-1", &hex::encode(encoded.file_digest))
            .unwrap()
            .unwrap();
        let stored_chunks: Vec<_> = stored
            .objects
            .iter()
            .filter(|object| object.kind == "source_chunk")
            .collect();
        assert_eq!(stored_chunks.len(), all_chunks.len());
        assert!(stored_chunks.iter().all(|object| object.bytes == 37));

        catalog.delete_checkpoint("doc", &first.sha).unwrap();
        let pending = catalog.due_deletes(crate::util::now_unix(), 100).unwrap();
        assert!(pending
            .iter()
            .all(|object| { !object.object_key.starts_with("content/storage-1/chunks/") }));
        catalog.delete_checkpoint("doc", &second.sha).unwrap();
        let pending = catalog.due_deletes(crate::util::now_unix(), 100).unwrap();
        assert!(pending
            .iter()
            .any(|object| { object.object_key.starts_with("content/storage-1/chunks/") }));
    }

    #[test]
    fn shared_chunk_from_an_earlier_new_file_has_a_known_size() {
        let profile = encoding::EncodingProfile::default();
        let first: Vec<u8> = (0usize..131_072)
            .map(|position| (position.wrapping_mul(31) % 251) as u8)
            .collect();
        let first_plan = encoding::plan_source_with_profile(&first, profile).unwrap();
        assert!(first_plan.recipe.chunks.len() > 1);

        // Change only after the first planned chunk. FastCDC's prefix is
        // deterministic, so the second file necessarily shares that first
        // physical chunk while still having a different file digest.
        let cut = first_plan.recipe.chunks[0].length as usize;
        let mut second = first.clone();
        for byte in &mut second[cut..] {
            *byte = byte.wrapping_add(1);
        }
        let second_plan = encoding::plan_source_with_profile(&second, profile).unwrap();
        let shared = first_plan.recipe.chunks[0].digest;
        assert!(second_plan
            .recipe
            .chunks
            .iter()
            .any(|chunk| chunk.digest == shared));

        let first_encoded =
            encoding::encode_source_from_plan(&first, first_plan.clone(), &HashSet::new()).unwrap();
        let emitted_size = first_encoded
            .objects
            .iter()
            .find(|object| object.digest == shared)
            .map(|object| object.encoded.len() as i64)
            .expect("the shared chunk is emitted by the first file");
        let second_existing = [shared].into_iter().collect();
        let second_encoded =
            encoding::encode_source_from_plan(&second, second_plan, &second_existing).unwrap();
        assert!(second_encoded
            .objects
            .iter()
            .all(|object| object.digest != shared));

        // This is the same union used by the checkpoint writer: emitted
        // chunks from earlier files are known before leases are built, even
        // though they are not in object_accounting until upload completes.
        let mut known_sizes = std::collections::HashMap::new();
        known_sizes.insert(
            crate::storage::blob::content_chunk_key("storage-1", &hex::encode(shared)),
            emitted_size,
        );
        let second_record =
            SourceHistoryRecord::from_encoded("storage-1", &second_encoded).unwrap();
        let reused = second_record
            .objects
            .iter()
            .find(|object| object.object_key.ends_with(&hex::encode(shared)))
            .expect("the second recipe keeps the shared edge");
        assert_eq!(reused.bytes, 0);
        assert_eq!(known_sizes.get(&reused.object_key), Some(&emitted_size));
    }

    #[test]
    fn lease_renewal_cannot_resurrect_an_expired_operation() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.upsert_account(&account()).unwrap();
        catalog.create_document(&document()).unwrap();
        let object = SourceHistoryObject {
            object_key: "content/storage-1/chunks/heartbeat".into(),
            kind: "source_chunk".into(),
            bytes: 7,
        };
        catalog
            .begin_source_history_lease("storage-1", "heartbeat", &[object], 10, 20)
            .unwrap();
        catalog
            .renew_source_history_lease("storage-1", "heartbeat", 11, 30)
            .unwrap();
        assert!(matches!(
            catalog.renew_source_history_lease("storage-1", "heartbeat", 30, 40),
            Err(CatalogError::Conflict(_))
        ));
    }

    #[test]
    fn pending_asset_objects_wait_for_read_lease() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.upsert_account(&account()).unwrap();
        catalog.create_document(&document()).unwrap();
        let objects = vec![SourceHistoryObject {
            object_key: "content/storage-1/assets/figure".into(),
            kind: "asset".into(),
            bytes: 29,
        }];
        catalog
            .begin_source_history_lease("storage-1", "restore-read", &objects, 10, 100)
            .unwrap();
        for object in &objects {
            catalog
                .queue_delete(&PendingDelete {
                    slug: "doc".into(),
                    object_key: object.object_key.clone(),
                    bytes: object.bytes,
                    queued_at: 20,
                    delete_after: 20,
                })
                .unwrap();
        }
        let held = catalog.due_deletes(21, 100).unwrap();
        assert!(
            held.is_empty(),
            "active restore lease must block both object kinds"
        );

        catalog
            .finish_source_history_lease("storage-1", "restore-read")
            .unwrap();
        let released = catalog.due_deletes(21, 100).unwrap();
        assert_eq!(released.len(), objects.len());
    }

    #[test]
    fn expired_lease_rejects_checkpoint_publication_in_transaction() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.upsert_account(&account()).unwrap();
        catalog.create_document(&document()).unwrap();
        let now = crate::util::now_unix();
        catalog
            .begin_source_history_lease(
                "storage-1",
                "expired-publication",
                &[SourceHistoryObject {
                    object_key: "content/storage-1/trees/expired".into(),
                    kind: "checkpoint_tree".into(),
                    bytes: 1,
                }],
                now.saturating_sub(2),
                now.saturating_sub(1),
            )
            .unwrap();
        let checkpoint = Checkpoint {
            slug: "doc".into(),
            sha: "expired-checkpoint".into(),
            seq: -1,
            durable_seq: 0,
            tree_sha: "expired-checkpoint".into(),
            parent: String::new(),
            at: "2026-01-01T00:00:00Z".into(),
            by: String::new(),
            why: "test".into(),
            source_format: "markdown".into(),
            size: 0,
            label: String::new(),
            git_commit: String::new(),
            dirty: false,
            changed: None,
            by_account: None,
        };
        assert!(matches!(
            catalog.insert_checkpoints_atomic_with_sources_and_lease(
                std::slice::from_ref(&checkpoint),
                None,
                &[],
                Some("expired-publication"),
            ),
            Err(CatalogError::Conflict(_))
        ));
        assert!(catalog
            .checkpoint("doc", "expired-checkpoint")
            .unwrap()
            .is_none());
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
    /// Return measured compressed sizes for the requested durable source
    /// chunks.  The caller supplies a bounded batch of keys derived from the
    /// files being encoded; never enumerate the whole document namespace just
    /// to decide whether one checkpoint's chunks can be reused.
    pub fn source_history_object_sizes(
        &self,
        storage_id: &str,
        object_keys: &[String],
    ) -> CatalogResult<std::collections::HashMap<String, i64>> {
        if storage_id.is_empty() || object_keys.len() > 256 {
            return Err(CatalogError::Invalid(
                "invalid source-history accounting lookup".into(),
            ));
        }
        if object_keys.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        self.with_connection(|connection| {
            let placeholders = (2..=object_keys.len() + 1)
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let query = format!(
                "SELECT object_key,bytes FROM object_accounting
                 WHERE storage_id=?1 AND object_key IN ({placeholders})"
            );
            let mut statement = connection.prepare(&query)?;
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(object_keys.len() + 1);
            values.push(&storage_id);
            values.extend(object_keys.iter().map(|key| key as &dyn rusqlite::ToSql));
            let rows = statement
                .query_map(rusqlite::params_from_iter(values), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.into_iter().collect())
        })
    }

    /// Return a previously committed physical representation for a complete
    /// file digest.  This is the fast path for unchanged files: a checkpoint
    /// links the existing recipe instead of recompressing its source.
    pub fn source_history_record(
        &self,
        storage_id: &str,
        file_digest: &str,
    ) -> CatalogResult<Option<SourceHistoryRecord>> {
        self.with_connection(|connection| {
            let Some((recipe_key, recipe_digest, codec, uncompressed_bytes, recipe_bytes)) =
                connection
                    .query_row(
                        "SELECT recipe_key,recipe_digest,codec,uncompressed_bytes,recipe_bytes
                         FROM source_history_encodings
                         WHERE storage_id=?1 AND file_digest=?2",
                        params![storage_id, file_digest],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, i64>(4)?,
                            ))
                        },
                    )
                    .optional()?
            else {
                return Ok(None);
            };
            let mut statement = connection.prepare(
                "SELECT object_key,kind,bytes FROM source_history_objects
                 WHERE storage_id=?1 AND file_digest=?2 ORDER BY object_key",
            )?;
            let objects = statement
                .query_map(params![storage_id, file_digest], |row| {
                    Ok(SourceHistoryObject {
                        object_key: row.get(0)?,
                        kind: row.get(1)?,
                        bytes: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(SourceHistoryRecord {
                file_digest: file_digest.to_string(),
                recipe_key,
                recipe_digest,
                codec,
                uncompressed_bytes,
                recipe_bytes,
                objects,
            }))
        })
    }

    /// Register every intended object before the first upload.  Repeating an
    /// operation with the same object sizes is idempotent; changing its plan
    /// is refused so a retry cannot silently weaken a lease.
    pub fn begin_source_history_lease(
        &self,
        storage_id: &str,
        operation_id: &str,
        objects: &[SourceHistoryObject],
        created_at: i64,
        expires_at: i64,
    ) -> CatalogResult<()> {
        let object_prefix = crate::storage::blob::content_prefix(storage_id);
        if storage_id.is_empty()
            || operation_id.is_empty()
            || created_at < 0
            || expires_at < created_at
            || objects.iter().any(|object| {
                object.object_key.is_empty()
                    || !object.object_key.starts_with(&object_prefix)
                    || object.kind.is_empty()
                    || object.bytes < 0
            })
        {
            return Err(CatalogError::Invalid("invalid source-history lease".into()));
        }
        self.immediate(|tx| {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE storage_id=?1 AND status IN ('creating','active'))",
                [storage_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(CatalogError::NotFound);
            }
            for object in objects {
                let pending: bool = tx.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM pending_deletes p
                         JOIN documents d ON d.slug=p.slug
                         WHERE d.storage_id=?1 AND p.object_key=?2)",
                    params![storage_id, object.object_key],
                    |row| row.get(0),
                )?;
                if pending {
                    // The deletion worker may already have admitted its
                    // object-store operation. Removing this row cannot
                    // cancel that in-flight I/O, even for an asset.
                    return Err(CatalogError::Conflict(
                        "source-history object is queued for deletion".into(),
                    ));
                }
                let previous: Option<(i64, i64)> = tx
                    .query_row(
                        "SELECT bytes,expires_at FROM source_history_write_leases
                         WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                        params![storage_id, operation_id, object.object_key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((old_bytes, old_expiry)) = previous {
                    if old_bytes != object.bytes || old_expiry != expires_at {
                        return Err(CatalogError::Conflict(
                            "source-history lease was reused with a different object".into(),
                        ));
                    }
                    continue;
                }
                tx.execute(
                    "INSERT INTO source_history_write_leases
                     (storage_id,operation_id,object_key,bytes,created_at,expires_at)
                     VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        storage_id,
                        operation_id,
                        object.object_key,
                        object.bytes,
                        created_at,
                        expires_at
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// Release a completed writer lease.  An expired/failed operation is
    /// intentionally left for the repair pass to remove after checking that
    /// no committed graph edge adopted its objects.
    pub fn finish_source_history_lease(
        &self,
        storage_id: &str,
        operation_id: &str,
    ) -> CatalogResult<()> {
        if storage_id.is_empty() || operation_id.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid source-history lease id".into(),
            ));
        }
        self.immediate(|tx| {
            tx.execute(
                "DELETE FROM source_history_write_leases
                 WHERE storage_id=?1 AND operation_id=?2",
                params![storage_id, operation_id],
            )?;
            Ok(())
        })
    }

    /// Extend an existing lease only while every row for the operation is
    /// still alive.  In particular, an expired operation cannot be resurrected
    /// by a delayed heartbeat after maintenance has removed its rows.
    pub fn renew_source_history_lease(
        &self,
        storage_id: &str,
        operation_id: &str,
        now: i64,
        expires_at: i64,
    ) -> CatalogResult<()> {
        if storage_id.is_empty() || operation_id.is_empty() || now < 0 || expires_at <= now {
            return Err(CatalogError::Invalid(
                "invalid source-history lease renewal".into(),
            ));
        }
        self.immediate(|tx| {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM source_history_write_leases
                     WHERE storage_id=?1 AND operation_id=?2)",
                params![storage_id, operation_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(CatalogError::NotFound);
            }
            let expired: bool = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM source_history_write_leases
                     WHERE storage_id=?1 AND operation_id=?2 AND expires_at<=?3)",
                params![storage_id, operation_id, now],
                |row| row.get(0),
            )?;
            if expired {
                return Err(CatalogError::Conflict(
                    "source-history lease has expired".into(),
                ));
            }
            tx.execute(
                "UPDATE source_history_write_leases SET expires_at=?3
                 WHERE storage_id=?1 AND operation_id=?2 AND expires_at>?4",
                params![storage_id, operation_id, expires_at, now],
            )?;
            Ok(())
        })
    }

    /// Require a still-live lease inside a caller's publication transaction.
    /// This closes the heartbeat/commit boundary: a lease that expired while
    /// object I/O was in flight cannot be adopted after its rows were swept.
    pub(crate) fn require_active_source_history_lease_tx(
        tx: &Transaction<'_>,
        storage_id: &str,
        operation_id: &str,
    ) -> CatalogResult<()> {
        if storage_id.is_empty() || operation_id.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid source-history lease id".into(),
            ));
        }
        let active: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM source_history_write_leases
                 WHERE storage_id=?1 AND operation_id=?2)
             AND NOT EXISTS(
                 SELECT 1 FROM source_history_write_leases
                 WHERE storage_id=?1 AND operation_id=?2 AND expires_at<=unixepoch())",
            params![storage_id, operation_id],
            |row| row.get(0),
        )?;
        if !active {
            return Err(CatalogError::Conflict(
                "source-history writer lease is missing or expired".into(),
            ));
        }
        Ok(())
    }

    /// Remove abandoned leases only after their durable deadline.  The caller
    /// may then run source-history GC; active leases are never treated as
    /// orphan roots.
    pub fn expire_source_history_leases(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        if now < 0 || limit == 0 {
            return Err(CatalogError::Invalid(
                "invalid source-history lease sweep".into(),
            ));
        }
        self.immediate(|tx| {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS source_history_gc_encoding_state (
                     id INTEGER PRIMARY KEY CHECK(id=1),
                     storage_cursor TEXT,
                     file_cursor TEXT,
                     updated_at INTEGER NOT NULL
                 );
                 INSERT OR IGNORE INTO source_history_gc_encoding_state
                     (id,updated_at) VALUES (1,0);",
            )?;
            // Capture the lease targets before removing the rows.  A failed
            // upload has no source_history_objects row yet, so scanning only
            // that graph after DELETE leaks its physical object forever.
            // Keep the operation id in the snapshot so the delete remains
            // exact if a writer retries with the same object key.
            let mut statement = tx.prepare(
                "SELECT l.storage_id,l.operation_id,l.object_key,l.bytes,d.slug
                 FROM source_history_write_leases l
                 JOIN documents d ON d.storage_id=l.storage_id
                 WHERE l.expires_at<=?1
                 ORDER BY l.expires_at,l.storage_id,l.operation_id,l.object_key
                 LIMIT ?2",
            )?;
            let expired = statement
                .query_map(params![now, i64::from(limit.min(10_000))], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);

            let mut removed = 0u32;
            for (storage_id, operation_id, object_key, bytes, slug) in &expired {
                removed = removed.saturating_add(tx.execute(
                    "DELETE FROM source_history_write_leases
                         WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                    params![storage_id, operation_id, object_key],
                )? as u32);

                // Read leases can name a retained checkpoint tree.  Keep that
                // tree alive while its checkpoint row remains active, while
                // still reclaiming a writer's tree if publication failed
                // before the checkpoint was committed.
                let content_prefix = crate::storage::blob::content_prefix(storage_id);
                let tree_object = object_key.starts_with(&format!("{content_prefix}trees/"));
                let source_object = object_key.starts_with(&format!("{content_prefix}chunks/"))
                    || object_key.starts_with(&format!("{content_prefix}recipes/"));
                if !source_object && !tree_object {
                    continue;
                }
                let retained: bool = if tree_object {
                    tx.query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM checkpoints c
                             JOIN documents d ON d.slug=c.slug
                             WHERE d.storage_id=?1 AND d.status='active'
                               AND ?2 = 'content/' || d.storage_id || '/trees/' ||
                                   c.sha)",
                        params![storage_id, object_key],
                        |row| row.get(0),
                    )?
                } else {
                    tx.query_row(
                        "SELECT EXISTS(
                             SELECT 1
                             FROM source_history_objects o
                             JOIN source_history_checkpoint_files r
                               ON r.storage_id=o.storage_id AND r.file_digest=o.file_digest
                             WHERE o.storage_id=?1 AND o.object_key=?2)",
                        params![storage_id, object_key],
                        |row| row.get(0),
                    )?
                };
                let leased: bool = tx.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM source_history_write_leases
                         WHERE storage_id=?1 AND object_key=?2)",
                    params![storage_id, object_key],
                    |row| row.get(0),
                )?;
                if !retained && !leased {
                    tx.execute(
                        "INSERT INTO pending_deletes
                         (slug,object_key,bytes,queued_at,delete_after)
                         VALUES(?1,?2,?3,?4,?4)
                         ON CONFLICT(slug,object_key) DO UPDATE SET
                           bytes=excluded.bytes,
                           delete_after=MIN(pending_deletes.delete_after,excluded.delete_after)",
                        params![slug, object_key, bytes, now],
                    )?;
                }
            }

            let batch_limit = i64::from(limit.min(10_000));
            // Encoding rows own their physical object rows with ON DELETE
            // CASCADE. Delete only a bounded page of encodings, and drain
            // their object rows individually. A row is removed only when its
            // physical key is queued for deletion or another encoding still
            // owns that key; an unvisited physical object is never erased by
            // a cascading delete.
            let (encoding_storage_cursor, encoding_file_cursor): (Option<String>, Option<String>) =
                tx.query_row(
                    "SELECT storage_cursor,file_cursor
                     FROM source_history_gc_encoding_state WHERE id=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
            let encoding_storage_cursor = encoding_storage_cursor.as_deref().unwrap_or("");
            let encoding_file_cursor = encoding_file_cursor.as_deref().unwrap_or("");
            let mut statement = tx.prepare(
                "SELECT storage_id,file_digest
                 FROM source_history_encodings
                 WHERE (storage_id,file_digest) > (?1,?2)
                 ORDER BY storage_id,file_digest LIMIT 1",
            )?;
            let encoding_page = statement
                .query_map(
                    params![encoding_storage_cursor, encoding_file_cursor],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for (storage_id, file_digest) in &encoding_page {
                let retained: bool = tx.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM source_history_checkpoint_files
                         WHERE storage_id=?1 AND file_digest=?2)",
                    params![storage_id, file_digest],
                    |row| row.get(0),
                )?;
                let leased: bool = tx.query_row(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM source_history_objects o
                         JOIN source_history_write_leases l
                           ON l.storage_id=o.storage_id AND l.object_key=o.object_key
                         WHERE o.storage_id=?1 AND o.file_digest=?2)",
                    params![storage_id, file_digest],
                    |row| row.get(0),
                )?;
                if retained || leased {
                    continue;
                }
                let object_limit = batch_limit;
                let slug: String = tx.query_row(
                    "SELECT slug FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )?;
                let mut objects = tx.prepare(
                    "SELECT object_key,bytes FROM source_history_objects
                     WHERE storage_id=?1 AND file_digest=?2
                     ORDER BY object_key LIMIT ?3",
                )?;
                let object_keys = objects
                    .query_map(params![storage_id, file_digest, object_limit], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(objects);
                for (object_key, bytes) in &object_keys {
                    let leased: bool = tx.query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM source_history_write_leases
                             WHERE storage_id=?1 AND object_key=?2)",
                        params![storage_id, object_key],
                        |row| row.get(0),
                    )?;
                    let shared: bool = tx.query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM source_history_objects
                             WHERE storage_id=?1 AND object_key=?2
                               AND file_digest<>?3)",
                        params![storage_id, object_key, file_digest],
                        |row| row.get(0),
                    )?;
                    if leased {
                        continue;
                    }
                    if !shared {
                        // This cleanup pass also discovers orphan keys. Queue
                        // each selected key before removing its graph row so
                        // the physical delete cursor cannot miss it.
                        tx.execute(
                            "INSERT INTO pending_deletes
                             (slug,object_key,bytes,queued_at,delete_after)
                             VALUES(?1,?2,?3,?4,?4)
                             ON CONFLICT(slug,object_key) DO UPDATE SET
                               bytes=excluded.bytes,
                               delete_after=MIN(pending_deletes.delete_after,excluded.delete_after)",
                            params![slug, object_key, bytes, now],
                        )?;
                    }
                    // Remove only rows whose physical key is covered by the
                    // delete queue or another encoding. This lets a very
                    // large encoding drain over several bounded passes
                    // without relying on a cascading delete to do unbounded
                    // work.
                    tx.execute(
                        "DELETE FROM source_history_objects
                         WHERE storage_id=?1 AND file_digest=?2 AND object_key=?3",
                        params![storage_id, file_digest, object_key],
                    )?;
                }
                let has_objects: bool = tx.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM source_history_objects
                         WHERE storage_id=?1 AND file_digest=?2)",
                    params![storage_id, file_digest],
                    |row| row.get(0),
                )?;
                if !has_objects {
                    tx.execute(
                        "DELETE FROM source_history_encodings
                         WHERE storage_id=?1 AND file_digest=?2",
                        params![storage_id, file_digest],
                    )?;
                }
            }
            let (next_encoding_storage, next_encoding_file) = if encoding_page.is_empty() {
                (None, None)
            } else {
                encoding_page
                    .last()
                    .map(|(storage, file)| (Some(storage.clone()), Some(file.clone())))
                    .unwrap_or((None, None))
            };
            tx.execute(
                "UPDATE source_history_gc_encoding_state
                 SET storage_cursor=?1,file_cursor=?2,updated_at=?3 WHERE id=1",
                params![next_encoding_storage, next_encoding_file, now],
            )?;
            Ok(removed)
        })
    }

    /// Insert encoding rows and checkpoint edges in the same transaction as a
    /// checkpoint publication.  The operation is idempotent for repeated
    /// retries of a checkpoint and rejects an encoding identity collision.
    pub(crate) fn insert_source_history_tx(
        tx: &Transaction<'_>,
        storage_id: &str,
        checkpoint_sha: &str,
        records: &[SourceHistoryRecord],
    ) -> CatalogResult<()> {
        if storage_id.is_empty() || checkpoint_sha.is_empty() {
            return Err(CatalogError::Invalid("invalid source-history edge".into()));
        }
        for record in records {
            if record.file_digest.is_empty()
                || !record
                    .recipe_key
                    .starts_with(&crate::storage::blob::content_prefix(storage_id))
                || record.recipe_key.is_empty()
                || record.recipe_digest.is_empty()
                || record.codec <= 0
                || record.uncompressed_bytes < 0
                || record.recipe_bytes < 0
                || record.objects.iter().any(|object| {
                    object.object_key.is_empty() || object.kind.is_empty() || object.bytes < 0
                })
            {
                return Err(CatalogError::Invalid(
                    "invalid source-history encoding".into(),
                ));
            }
            let existing: Option<(String, String, i64, i64, i64)> = tx
                .query_row(
                    "SELECT recipe_key,recipe_digest,codec,uncompressed_bytes,recipe_bytes
                     FROM source_history_encodings WHERE storage_id=?1 AND file_digest=?2",
                    params![storage_id, record.file_digest],
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
                .optional()?;
            if let Some((recipe_key, recipe_digest, codec, uncompressed, recipe_bytes)) = existing {
                if (recipe_key, recipe_digest, codec, uncompressed, recipe_bytes)
                    != (
                        record.recipe_key.clone(),
                        record.recipe_digest.clone(),
                        record.codec,
                        record.uncompressed_bytes,
                        record.recipe_bytes,
                    )
                {
                    return Err(CatalogError::Conflict(
                        "source-history digest has a different encoding".into(),
                    ));
                }
            } else {
                tx.execute(
                    "INSERT INTO source_history_encodings
                     (storage_id,file_digest,recipe_key,recipe_digest,codec,
                      uncompressed_bytes,recipe_bytes,created_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,unixepoch())",
                    params![
                        storage_id,
                        record.file_digest,
                        record.recipe_key,
                        record.recipe_digest,
                        record.codec,
                        record.uncompressed_bytes,
                        record.recipe_bytes
                    ],
                )?;
            }
            for object in &record.objects {
                if !object
                    .object_key
                    .starts_with(&crate::storage::blob::content_prefix(storage_id))
                {
                    return Err(CatalogError::Invalid(
                        "source-history object is outside document namespace".into(),
                    ));
                }
                let object_bytes = if object.kind == "source_chunk" && object.bytes == 0 {
                    // A zero length is the in-memory marker emitted for a
                    // reused chunk. Resolve it from measured accounting in
                    // this publication transaction; accepting the marker
                    // would make the graph look complete while undercharging
                    // and could later queue a live chunk with no useful size.
                    let measured: Option<i64> = tx
                        .query_row(
                            "SELECT bytes FROM object_accounting
                             WHERE storage_id=?1 AND object_key=?2",
                            params![storage_id, object.object_key],
                            |row| row.get(0),
                        )
                        .optional()?;
                    let Some(measured) = measured.filter(|bytes| *bytes > 0) else {
                        return Err(CatalogError::Conflict(
                            "reused source chunk has no durable accounting".into(),
                        ));
                    };
                    measured
                } else {
                    object.bytes
                };
                let pending: bool = tx.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM pending_deletes p
                         JOIN documents d ON d.slug=p.slug
                         WHERE d.storage_id=?1 AND p.object_key=?2)",
                    params![storage_id, object.object_key],
                    |row| row.get(0),
                )?;
                if pending {
                    return Err(CatalogError::Conflict(
                        "source-history object is queued for deletion".into(),
                    ));
                }
                tx.execute(
                    "INSERT INTO source_history_objects
                     (storage_id,file_digest,object_key,kind,bytes)
                     VALUES(?1,?2,?3,?4,?5)
                     ON CONFLICT(storage_id,file_digest,object_key) DO UPDATE SET
                       kind=excluded.kind,bytes=excluded.bytes",
                    params![
                        storage_id,
                        record.file_digest,
                        object.object_key,
                        object.kind,
                        object_bytes
                    ],
                )?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO source_history_checkpoint_files
                 (storage_id,checkpoint_sha,file_digest) VALUES(?1,?2,?3)",
                params![storage_id, checkpoint_sha, record.file_digest],
            )?;
        }
        Ok(())
    }

    /// Remove checkpoint edges and queue encoded objects that have no retained
    /// checkpoint edge or active write lease.  The queue is the durable handoff
    /// to the existing deletion worker; object accounting is not released
    /// until that worker confirms physical deletion.
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
        self.immediate(|tx| {
            let storage_id: String = tx.query_row(
                "SELECT storage_id FROM documents WHERE slug=?1",
                [slug],
                |row| row.get(0),
            )?;
            let mut removed = Vec::new();
            for sha in shas {
                let changed = tx.execute(
                    "DELETE FROM checkpoints WHERE slug=?1 AND sha=?2",
                    params![slug, sha],
                )?;
                if changed == 0 {
                    continue;
                }
                removed.push(sha.clone());
                tx.execute(
                    "DELETE FROM source_history_checkpoint_files
                     WHERE storage_id=?1 AND checkpoint_sha=?2",
                    params![storage_id, sha],
                )?;
            }
            let mut statement = tx.prepare(
                "SELECT o.object_key,o.bytes
                 FROM source_history_objects o
                 JOIN source_history_encodings e
                   ON e.storage_id=o.storage_id AND e.file_digest=o.file_digest
                 WHERE o.storage_id=?1
                   AND NOT EXISTS (SELECT 1
                                  FROM source_history_objects o2
                                  JOIN source_history_checkpoint_files r
                                    ON r.storage_id=o2.storage_id
                                   AND r.file_digest=o2.file_digest
                                  WHERE o2.storage_id=o.storage_id
                                    AND o2.object_key=o.object_key)
                   AND NOT EXISTS (SELECT 1 FROM source_history_write_leases l
                                  WHERE l.storage_id=o.storage_id AND l.object_key=o.object_key)",
            )?;
            let objects = statement
                .query_map([storage_id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for (object_key, bytes) in objects {
                tx.execute(
                    "INSERT INTO pending_deletes
                     (slug,object_key,bytes,queued_at,delete_after)
                     VALUES(?1,?2,?3,?4,?5)
                     ON CONFLICT(slug,object_key) DO UPDATE SET
                       bytes=excluded.bytes,delete_after=MIN(pending_deletes.delete_after,excluded.delete_after)",
                    params![slug, object_key, bytes, queued_at, delete_after],
                )?;
            }
            tx.execute(
                "DELETE FROM source_history_encodings
                 WHERE storage_id=?1
                   AND NOT EXISTS (SELECT 1 FROM source_history_checkpoint_files r
                                  WHERE r.storage_id=source_history_encodings.storage_id
                                    AND r.file_digest=source_history_encodings.file_digest)
                   AND NOT EXISTS (SELECT 1
                                  FROM source_history_objects o
                                  JOIN source_history_write_leases l
                                    ON l.storage_id=o.storage_id
                                   AND l.object_key=o.object_key
                                  WHERE o.storage_id=source_history_encodings.storage_id
                                    AND o.file_digest=source_history_encodings.file_digest)",
                [storage_id],
            )?;
            Ok(removed)
        })
    }
}
