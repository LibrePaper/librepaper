//! Accounts: the row an identity becomes, its sessions, and erasing one.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl Catalog {
    /// Durable references held by unresolved annotations/suggestions. They
    /// are keyed by document so account-wide retention cannot accidentally
    /// protect a same-named revision in another document.
    pub fn account_open_annotation_references(
        &self,
        account_id: &str,
    ) -> CatalogResult<BTreeMap<String, BTreeSet<String>>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.slug,c.revision FROM comments c JOIN documents d ON d.slug=c.slug
                 WHERE d.owner_id=?1 AND d.status IN ('active','creating')
                   AND c.resolved=0 AND c.revision<>'' ORDER BY c.slug,c.revision",
            )?;
            let rows = statement.query_map([account_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut references = BTreeMap::new();
            for row in rows {
                let (slug, revision) = row?;
                references
                    .entry(slug)
                    .or_insert_with(BTreeSet::new)
                    .insert(revision);
            }
            Ok(references)
        })
    }

    pub fn account_checkpoints(&self, account_id: &str) -> CatalogResult<Vec<Checkpoint>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.slug,c.sha,c.seq,c.durable_seq,c.tree_sha,c.parent,c.at,c.by,
                        c.by_account,c.why,c.source_format,c.size,c.label,c.git_commit,
                        c.dirty,c.changed
                 FROM checkpoints c JOIN documents d ON d.slug=c.slug
                 WHERE d.owner_id=?1 AND d.status IN ('active','creating')
                 ORDER BY c.slug,c.seq",
            )?;
            let rows = statement.query_map([account_id], |row| {
                Ok(Checkpoint {
                    slug: row.get(0)?,
                    sha: row.get(1)?,
                    seq: row.get(2)?,
                    durable_seq: row.get(3)?,
                    tree_sha: row.get(4)?,
                    parent: row.get(5)?,
                    at: row.get(6)?,
                    by: row.get(7)?,
                    by_account: row.get(8)?,
                    why: row.get(9)?,
                    source_format: row.get(10)?,
                    size: row.get(11)?,
                    label: row.get(12)?,
                    git_commit: row.get(13)?,
                    dirty: row.get::<_, i64>(14)? != 0,
                    changed: row.get(15)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(CatalogError::from)
        })
    }

    /// Return the owner's charged physical storage.
    ///
    /// The rows below are deliberately combined by `(storage_id, object_key)`
    /// before they are summed.  A source-history object can be present in the
    /// graph, in an in-flight lease, and in the deletion queue at the same
    /// time; those are three catalogue views of one physical object, not
    /// three charges.  The same rule also handles the migration period where
    /// a source object is represented by both the legacy object ledger and the
    /// new source-history graph.
    ///
    /// `checkpoints.size` is never used here.  It is the uncompressed logical
    /// tree size and is not evidence of either an encoded object or a
    /// reclaimable allocation.
    pub fn account_storage_usage(&self, account_id: &str) -> CatalogResult<AccountStorageUsage> {
        self.with_connection(|connection| Self::account_storage_usage_on(connection, account_id))
    }

    /// Connection-scoped form used by admission transactions.  Keeping the
    /// physical evaluator below the catalogue lock boundary avoids a nested
    /// connection lock while retaining exactly the same object/metadata
    /// categories as the account status endpoint.
    pub(super) fn account_storage_usage_on(
        connection: &Connection,
        account_id: &str,
    ) -> CatalogResult<AccountStorageUsage> {
        #[derive(Clone)]
        struct PhysicalObject {
            storage_id: String,
            key: String,
            kind: String,
            bytes: i64,
            /// A source/tree object named by the newest checkpoint is
            /// also needed by the live document.  It is charged in the
            /// live category and not again as history.
            live_root: bool,
        }

        let (document_count, checkpoint_count): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*),
                            (SELECT COUNT(*) FROM checkpoints c
                             JOIN documents cd ON cd.slug=c.slug
                             WHERE cd.owner_id=?1
                               AND cd.status IN ('active','creating','deleting'))
                     FROM documents
                     WHERE owner_id=?1 AND status IN ('active','creating','deleting')",
                [account_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(CatalogError::from)?;

        // Keep this query as a UNION ALL and deduplicate in Rust.  SQL
        // UNION cannot detect a catalogue repair disagreement where the
        // same key has two different recorded lengths; retaining that
        // signal lets the caller avoid claiming verified accounting.
        let mut statement = connection.prepare(
            "SELECT d.storage_id, o.object_key, o.kind, o.bytes, 0
                   FROM documents d JOIN object_accounting o
                     ON o.storage_id=d.storage_id
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')
                 UNION ALL
                 SELECT d.storage_id, o.object_key, o.kind, o.bytes,
                        EXISTS(
                          SELECT 1
                            FROM source_history_checkpoint_files newest
                            JOIN checkpoints c
                              ON c.slug=d.slug AND c.sha=newest.checkpoint_sha
                           WHERE newest.storage_id=o.storage_id
                             AND newest.file_digest=o.file_digest
                             AND c.seq=(SELECT MAX(c2.seq) FROM checkpoints c2
                                        WHERE c2.slug=d.slug))
                   FROM documents d JOIN source_history_objects o
                     ON o.storage_id=d.storage_id
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')
                 UNION ALL
                 SELECT d.storage_id, l.object_key, 'source_lease', l.bytes, 0
                   FROM documents d JOIN source_history_write_leases l
                     ON l.storage_id=d.storage_id
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')
                 UNION ALL
                 SELECT d.storage_id, r.object_key, 'object_reservation', r.new_bytes, 0
                   FROM documents d JOIN object_reservations r
                     ON r.storage_id=d.storage_id
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')
                 UNION ALL
                 SELECT d.storage_id, p.object_key, 'pending_delete', p.bytes, 0
                   FROM documents d JOIN pending_deletes p ON p.slug=d.slug
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')
                 UNION ALL
                 SELECT d.storage_id, r.object_key, 'asset_reference', r.bytes, 0
                   FROM documents d JOIN checkpoint_asset_refs r
                     ON r.storage_id=d.storage_id
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
        )?;
        let rows = statement.query_map([account_id], |row| {
            Ok(PhysicalObject {
                storage_id: row.get(0)?,
                key: row.get(1)?,
                kind: row.get(2)?,
                bytes: row.get(3)?,
                live_root: row.get::<_, i64>(4)? != 0,
            })
        })?;
        let mut objects = BTreeMap::<(String, String), PhysicalObject>::new();
        let mut verified = true;
        for row in rows {
            let object = row?;
            let key = (object.storage_id.clone(), object.key.clone());
            if let Some(previous) = objects.get_mut(&key) {
                if previous.bytes != object.bytes {
                    // Queue/lease lengths are plans copied into the
                    // catalogue.  A committed object ledger or source graph
                    // length is measured, while an ordinary reservation is
                    // the prospective replacement and must contribute its
                    // positive delta.  Taking the maximum keeps an old
                    // measured object charged until its replacement commits,
                    // and prevents a reservation overwrite from hiding the
                    // bytes admission already promised.
                    let previous_planned = matches!(
                        previous.kind.as_str(),
                        "object_reservation" | "pending" | "pending_delete" | "source_lease"
                    );
                    let object_planned = matches!(
                        object.kind.as_str(),
                        "object_reservation" | "pending" | "pending_delete" | "source_lease"
                    );
                    if previous_planned || object_planned {
                        previous.bytes = previous.bytes.max(object.bytes);
                    } else {
                        // Two measured catalogue records disagree.  Keep the
                        // conservative value but do not report verification.
                        verified = false;
                        previous.bytes = previous.bytes.max(object.bytes);
                    }
                }
                previous.live_root |= object.live_root;
            } else {
                objects.insert(key, object);
            }
        }
        drop(statement);

        fn class(kind: &str, key: &str, live_root: bool, historical_asset: bool) -> &'static str {
            if live_root {
                return "live";
            }
            if historical_asset && (kind == "asset" || key.contains("/assets/")) {
                return "history";
            }
            if kind == "asset" || key.contains("/assets/") {
                return "asset";
            }
            if kind == "rendering" || kind == "publish" || key.contains("/renderings/") {
                return "publication";
            }
            if kind.starts_with("source_")
                || kind == "text"
                || key.contains("/chunks/")
                || key.contains("/recipes/")
                || key.contains("/blobs/")
                || key.contains("/trees/")
            {
                return "history";
            }
            // Sessions, journals, room state, Quarto state, and any
            // future unclassified durable object belong to the live
            // footprint until a dedicated category is introduced.
            "live"
        }

        let mut live_bytes = 0i64;
        let mut history_bytes = 0i64;
        let mut asset_bytes = 0i64;
        let mut publication_bytes = 0i64;
        let mut object_bytes = 0i64;
        let mut latest_trees = BTreeSet::<(String, String)>::new();
        let mut statement = connection.prepare(
            "SELECT d.storage_id,
                        'content/'||d.storage_id||'/trees/'||c.sha
                   FROM documents d JOIN checkpoints c ON c.slug=d.slug
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')
                    AND c.seq=(SELECT MAX(c2.seq) FROM checkpoints c2 WHERE c2.slug=c.slug)",
        )?;
        let rows = statement.query_map([account_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            latest_trees.insert(row?);
        }
        drop(statement);
        let mut historical_assets = BTreeSet::<(String, String)>::new();
        let mut live_assets = BTreeSet::<(String, String)>::new();
        let mut statement = connection.prepare(
            "SELECT d.slug,
                    EXISTS(
                      SELECT 1 FROM checkpoints c
                       WHERE c.slug=d.slug
                         AND NOT EXISTS(
                           SELECT 1 FROM checkpoint_asset_sets s
                            WHERE s.storage_id=d.storage_id
                              AND s.checkpoint_sha=c.sha
                         )),
                    d.pending_publication IS NOT NULL,
                    (
                      COALESCE((
                        SELECT MAX(j.last_sequence)
                          FROM journal_segment_coverage j
                         WHERE j.storage_id=d.storage_id OR j.storage_id=''
                      ),0) > COALESCE((
                        SELECT MAX(c.durable_seq) FROM checkpoints c WHERE c.slug=d.slug
                      ),0)
                      OR COALESCE((
                        SELECT MAX(b.sequence)
                          FROM journal_bases b
                         WHERE b.storage_id=d.storage_id OR b.storage_id=''
                      ),0) > COALESCE((
                        SELECT MAX(c.durable_seq) FROM checkpoints c WHERE c.slug=d.slug
                      ),0)
                    )
               FROM documents d
              WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
        )?;
        let rows = statement.query_map([account_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        let mut safe_documents = BTreeSet::new();
        for row in rows {
            let (slug, legacy, pending_publication, journal_ahead) = row?;
            if !legacy && !pending_publication && !journal_ahead {
                safe_documents.insert(slug);
            }
        }
        drop(statement);
        let mut statement = connection.prepare(
            "SELECT r.storage_id,r.object_key,d.slug,
                    r.checkpoint_sha=(SELECT c.sha FROM checkpoints c
                                      WHERE c.slug=d.slug ORDER BY c.seq DESC LIMIT 1)
               FROM checkpoint_asset_refs r
               JOIN documents d ON d.storage_id=r.storage_id
              WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
        )?;
        let rows = statement.query_map([account_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (storage_id, object_key, slug, newest) = row?;
            let object = (storage_id, object_key);
            if newest || !safe_documents.contains(&slug) {
                live_assets.insert(object);
            } else {
                historical_assets.insert(object);
            }
        }
        drop(statement);
        for object in objects.values() {
            if object.bytes < 0 {
                verified = false;
                continue;
            }
            object_bytes = object_bytes.saturating_add(object.bytes);
            let live_root = object.live_root
                || latest_trees.contains(&(object.storage_id.clone(), object.key.clone()));
            let object_id = (object.storage_id.clone(), object.key.clone());
            let historical_asset =
                historical_assets.contains(&object_id) && !live_assets.contains(&object_id);
            match class(&object.kind, &object.key, live_root, historical_asset) {
                "history" => history_bytes = history_bytes.saturating_add(object.bytes),
                "asset" => asset_bytes = asset_bytes.saturating_add(object.bytes),
                "publication" => publication_bytes = publication_bytes.saturating_add(object.bytes),
                _ => live_bytes = live_bytes.saturating_add(object.bytes),
            }
        }

        // Catalogue records have no backend page-size meaning.  This is
        // the deterministic serialized-record charge used for quota
        // attribution: text columns contribute their UTF-8 byte lengths,
        // while integer fields contribute their fixed 64-bit wire width.
        // SQLite page slack and indexes remain deployment overhead.
        let (metadata_total, history_metadata) =
            Self::account_catalogue_metadata_bytes(connection, account_id)?;
        history_bytes = history_bytes.saturating_add(history_metadata);
        let metadata_bytes = metadata_total.saturating_sub(history_metadata);
        // Every serialized catalogue record is charged exactly once;
        // history records are exposed in `history_bytes` for the soft
        // target, while `metadata_bytes` keeps the category totals
        // disjoint from the physical object total.
        let charged_bytes = object_bytes.saturating_add(metadata_total);

        // A legacy document with a non-zero admission ledger but no
        // measured object/lease/queue rows cannot be represented by this
        // query.  Preserve the safe migration signal rather than
        // claiming that an inferred logical value is physical.
        let unrepresented_ledger: i64 = connection.query_row(
            "SELECT COALESCE(SUM(MAX(0,counted_size-size)),0)
                   FROM documents d
                  WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')
                    AND NOT EXISTS (SELECT 1 FROM object_accounting o
                                    WHERE o.storage_id=d.storage_id)
                    AND NOT EXISTS (SELECT 1 FROM source_history_objects o
                                    WHERE o.storage_id=d.storage_id)",
            [account_id],
            |row| row.get(0),
        )?;
        if unrepresented_ledger > 0 {
            verified = false;
        }
        // A catalogue row without any measured physical object is a
        // legacy reservation, not proof that its physical footprint is
        // zero.  Keep admission on the conservative counted-size path
        // until at least one object/graph measurement exists.
        let measured_objects: i64 = connection.query_row(
                "SELECT
                    (SELECT COUNT(*) FROM object_accounting o JOIN documents d ON d.storage_id=o.storage_id
                     WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting'))+
                    (SELECT COUNT(*) FROM source_history_objects o JOIN documents d ON d.storage_id=o.storage_id
                     WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting'))",
                [account_id], |row| row.get(0),
            )?;
        if document_count > 0 && measured_objects == 0 {
            verified = false;
        }
        Ok(AccountStorageUsage {
            charged_bytes,
            live_bytes,
            history_bytes,
            asset_bytes,
            publication_bytes,
            metadata_bytes,
            document_count,
            checkpoint_count,
            physical_accounting: verified,
        })
    }

    /// Return the amount admission should charge for one owner while the
    /// caller already holds the catalogue transaction.  Unknown legacy
    /// layouts deliberately fall back to their existing reservation rather
    /// than turning an unverified physical measurement into authority.
    pub(super) fn owner_admission_bytes_on(
        connection: &Connection,
        owner_id: Option<&str>,
        owner_key: &str,
    ) -> CatalogResult<(i64, bool)> {
        let Some(owner_id) = owner_id else {
            let bytes = connection
                .query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents
                 WHERE owner_id IS NULL AND owner_key=?1",
                    [owner_key],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            return Ok((bytes, false));
        };
        let usage = Self::account_storage_usage_on(connection, owner_id)?;
        if usage.physical_accounting {
            let counted_headroom: i64 = connection
                .query_row(
                    "SELECT COALESCE(SUM(MAX(counted_size-size,0)),0)
                 FROM documents WHERE owner_id=?1
                   AND status IN ('active','creating','deleting')",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let pending_headroom: i64 = connection
                .query_row(
                    "SELECT COALESCE(SUM(MAX(counted_size-size,0)),0)
                 FROM documents WHERE owner_id=?1 AND pending_publication IS NOT NULL
                   AND status IN ('active','creating','deleting')",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let object_reservation_delta: i64 = connection
                .query_row(
                    "SELECT COALESCE(SUM(MAX(r.new_bytes-r.old_bytes,0)),0)
                 FROM object_reservations r JOIN documents d ON d.storage_id=r.storage_id
                WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let pending_object_reservation_delta: i64 = connection
                .query_row(
                    "SELECT COALESCE(SUM(MAX(r.new_bytes-r.old_bytes,0)),0)
                 FROM object_reservations r JOIN documents d ON d.storage_id=r.storage_id
                WHERE d.owner_id=?1 AND d.pending_publication IS NOT NULL
                  AND d.status IN ('active','creating','deleting')",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let edit_headroom: i64 = connection
                .query_row(
                    "SELECT COALESCE(SUM(e.pending_bytes+e.writing_bytes),0)
                 FROM room_edit_reservations e JOIN documents d ON d.storage_id=e.storage_id
                 WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            // `reserve` and `reserve_document_bytes` predate the object
            // reservation table and only grow counted_size.  Preserve those
            // bytes, while removing the two kinds of headroom represented by
            // explicit rows above so they are not charged a second time.
            let generic_headroom = counted_headroom
                .saturating_sub(pending_headroom)
                .saturating_sub(object_reservation_delta);
            Ok((
                usage
                    .charged_bytes
                    // Object reservations are already represented by the
                    // prospective maximum in `usage`; only the publication
                    // peak not covered by one of those reservations remains
                    // additional headroom.
                    .saturating_add(
                        pending_headroom.saturating_sub(pending_object_reservation_delta),
                    )
                    .saturating_add(edit_headroom)
                    .saturating_add(generic_headroom),
                true,
            ))
        } else {
            let bytes: i64 = connection
                .query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents
                 WHERE owner_id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            // Unverified legacy accounting may be larger, but it must not
            // erase measured objects/metadata already known for this owner.
            Ok((bytes.max(usage.charged_bytes), false))
        }
    }

    /// Deployment-wide counterpart to [`owner_admission_bytes_on`].  It is
    /// intentionally computed in the same transaction as the prospective
    /// reservation, so concurrent writers cannot bypass the physical limit.
    pub(super) fn deployment_admission_bytes_on(
        connection: &Connection,
    ) -> CatalogResult<(i64, bool)> {
        // Account-owned rows are grouped by durable owner id.  The owner key
        // is a legacy/display identity and can differ between that owner's
        // documents; grouping by both would charge the same account twice.
        // Anonymous rows have no durable id, so their owner key remains the
        // identity for the fallback bucket.
        let mut statement = connection.prepare(
            "SELECT owner_id, MIN(owner_key) FROM documents
               WHERE status IN ('active','creating','deleting') AND owner_id IS NOT NULL
             GROUP BY owner_id
             UNION ALL
             SELECT NULL, owner_key FROM documents
               WHERE status IN ('active','creating','deleting') AND owner_id IS NULL
             GROUP BY owner_key",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
        })?;
        let owners = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let mut total = 0i64;
        let mut verified = true;
        for (owner_id, owner_key) in owners {
            let (bytes, known) =
                Self::owner_admission_bytes_on(connection, owner_id.as_deref(), &owner_key)?;
            total = total.saturating_add(bytes);
            verified &= known;
        }
        Ok((total, verified))
    }

    /// Physical counterpart of the room allowance. The current document is
    /// excluded using its durable reservation so edits may replace its own
    /// bytes; the owner/deployment totals still include every other lifecycle
    /// row and all measured metadata.
    pub fn physical_room_for(
        &self,
        slug: &str,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<Option<i64>> {
        self.with_connection(|connection| {
            let current: Option<(Option<String>, String, i64)> = connection.query_row(
                "SELECT owner_id,owner_key,counted_size FROM documents
                 WHERE slug=?1 AND status='active'",
                [slug], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(CatalogError::from)?;
            let Some((owner_id, owner_key, counted)) = current else { return Ok(None); };
            let (owner_total, owner_known) = Self::owner_admission_bytes_on(
                connection, owner_id.as_deref(), &owner_key,
            )?;
            let (deployment_total, deployment_known) = Self::deployment_admission_bytes_on(connection)?;
            if !owner_known || !deployment_known {
                // Legacy/unmeasured rows retain the established reservation
                // behavior; physical status must never be inferred here.
                let owner_total: i64 = if let Some(owner) = owner_id.as_deref() {
                    connection.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id=?1", [owner], |row| row.get(0)).map_err(CatalogError::from)?
                } else {
                    connection.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id IS NULL AND owner_key=?1", [&owner_key], |row| row.get(0)).map_err(CatalogError::from)?
                };
                let total: i64 = connection.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents", [], |row| row.get(0)).map_err(CatalogError::from)?;
                return Ok(Some(
                    total_limit
                        .saturating_sub(total.saturating_sub(counted))
                        .min(owner_limit.saturating_sub(owner_total.saturating_sub(counted)))
                        .max(0),
                ));
            }
            Ok(Some(
                total_limit
                    .saturating_sub(deployment_total.saturating_sub(counted))
                    .min(owner_limit.saturating_sub(owner_total.saturating_sub(counted)))
                    .max(0),
            ))
        })
    }

    /// Re-evaluate a graph write after all prospective checkpoint and
    /// source-history rows have been inserted into `connection`.  This is
    /// intentionally called before the surrounding transaction commits: a
    /// rejected graph rolls back its metadata edges together with the
    /// checkpoint, so a reused object cannot bypass quota merely because it
    /// needed no new blob allocation.
    pub(super) fn enforce_physical_quota_on(
        connection: &Connection,
        slug: &str,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<()> {
        let (owner_id, owner_key): (Option<String>, String) = connection
            .query_row(
                "SELECT owner_id,owner_key FROM documents
                 WHERE slug=?1 AND status IN ('active','creating','deleting')",
                [slug],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(CatalogError::from)?
            .ok_or(CatalogError::NotFound)?;
        let (owner_bytes, owner_known) =
            Self::owner_admission_bytes_on(connection, owner_id.as_deref(), &owner_key)?;
        let (deployment_bytes, deployment_known) = Self::deployment_admission_bytes_on(connection)?;
        // An unmeasured legacy row has no trustworthy physical value.  Its
        // established counted/admission reservation remains authoritative;
        // do not turn an unknown measurement into a guessed rejection.
        if owner_known && owner_limit >= 0 && owner_bytes > owner_limit {
            return Err(CatalogError::Conflict(
                "owner storage quota exceeded by catalogue metadata".into(),
            ));
        }
        if deployment_known && total_limit >= 0 && deployment_bytes > total_limit {
            return Err(CatalogError::Conflict(
                "deployment storage quota exceeded by catalogue metadata".into(),
            ));
        }
        Ok(())
    }

    /// Deterministic catalogue-record charge for one owner's durable rows.
    /// This intentionally excludes SQLite page slack, indexes, and shared
    /// database overhead; those are deployment measurements, not per-object
    /// owner bytes.
    fn account_catalogue_metadata_bytes(
        connection: &Connection,
        account_id: &str,
    ) -> CatalogResult<(i64, i64)> {
        let mut total = 0i64;
        let mut history = 0i64;
        let queries = [
            (
                "SELECT COALESCE(SUM(
                length(CAST(c.slug AS BLOB))+length(CAST(c.sha AS BLOB))+
                length(CAST(c.tree_sha AS BLOB))+length(CAST(c.parent AS BLOB))+
                length(CAST(c.at AS BLOB))+length(CAST(c.by AS BLOB))+
                length(CAST(c.why AS BLOB))+length(CAST(c.source_format AS BLOB))+
                length(CAST(c.label AS BLOB))+length(CAST(c.git_commit AS BLOB))+
                length(CAST(COALESCE(c.changed,'') AS BLOB))+
                length(CAST(COALESCE(c.by_account,'') AS BLOB))+56),0)
             FROM checkpoints c JOIN documents d ON d.slug=c.slug
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                true,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(e.storage_id AS BLOB))+length(CAST(e.file_digest AS BLOB))+
                length(CAST(e.recipe_key AS BLOB))+length(CAST(e.recipe_digest AS BLOB))+32),0)
             FROM source_history_encodings e JOIN documents d ON d.storage_id=e.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                true,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(o.storage_id AS BLOB))+length(CAST(o.file_digest AS BLOB))+
                length(CAST(o.object_key AS BLOB))+length(CAST(o.kind AS BLOB))+8),0)
             FROM source_history_objects o JOIN documents d ON d.storage_id=o.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                true,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(r.storage_id AS BLOB))+length(CAST(r.checkpoint_sha AS BLOB))+
                length(CAST(r.file_digest AS BLOB))),0)
             FROM source_history_checkpoint_files r JOIN documents d ON d.storage_id=r.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                true,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(o.storage_id AS BLOB))+length(CAST(o.object_key AS BLOB))+
                length(CAST(o.kind AS BLOB))+length(CAST(o.version AS BLOB))+8),0)
             FROM object_accounting o JOIN documents d ON d.storage_id=o.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                false,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(p.slug AS BLOB))+length(CAST(p.object_key AS BLOB))+16),0)
             FROM pending_deletes p JOIN documents d ON d.slug=p.slug
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                false,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(l.storage_id AS BLOB))+length(CAST(l.operation_id AS BLOB))+
                length(CAST(l.object_key AS BLOB))+24),0)
             FROM source_history_write_leases l JOIN documents d ON d.storage_id=l.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                false,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(r.storage_id AS BLOB))+length(CAST(r.operation_id AS BLOB))+
                length(CAST(r.object_key AS BLOB))+16),0)
             FROM object_reservations r JOIN documents d ON d.storage_id=r.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                false,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(r.storage_id AS BLOB))+length(CAST(r.checkpoint_sha AS BLOB))+
                length(CAST(r.object_key AS BLOB))+8),0)
             FROM checkpoint_asset_refs r JOIN documents d ON d.storage_id=r.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                false,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(s.storage_id AS BLOB))+length(CAST(s.checkpoint_sha AS BLOB))+8),0)
             FROM checkpoint_asset_sets s JOIN documents d ON d.storage_id=s.storage_id
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                false,
            ),
            (
                "SELECT COALESCE(SUM(
                length(CAST(r.slug AS BLOB))+length(CAST(r.tree_sha AS BLOB))+
                length(CAST(r.at AS BLOB))+length(CAST(r.backend AS BLOB))+
                length(CAST(r.engine AS BLOB))+length(CAST(r.release AS BLOB))+
                length(CAST(r.tools AS BLOB))+32),0)
             FROM renderings r JOIN documents d ON d.slug=r.slug
             WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                false,
            ),
        ];
        for (query, is_history) in queries {
            let bytes: i64 = connection
                .query_row(query, [account_id], |row| row.get(0))
                .map_err(CatalogError::from)?;
            let bytes = bytes.max(0);
            total = total.saturating_add(bytes);
            if is_history {
                history = history.saturating_add(bytes);
            }
        }
        Ok((total, history))
    }

    /// Compute the bytes that would become unreachable after removing a
    /// proposed checkpoint set.  This is intentionally a catalogue-only
    /// calculation: object-store listings are not authoritative for a
    /// concurrent publication and a checkpoint's logical `size` is not a
    /// physical byte measure.
    #[allow(clippy::type_complexity)]
    pub fn reclaimable_checkpoint_bytes(
        &self,
        candidates: &[(String, String)],
    ) -> CatalogResult<i64> {
        self.with_connection(|connection| {
            Self::reclaimable_checkpoint_bytes_on(connection, candidates)
        })
    }

    /// Transaction-scoped counterpart used by hard-pressure workers.  The
    /// caller must hold the same immediate transaction that will delete the
    /// candidate rows; otherwise a lease or newest-checkpoint change could
    /// invalidate the estimate between planning and deletion.
    #[allow(clippy::type_complexity)]
    pub(super) fn reclaimable_checkpoint_bytes_on(
        connection: &Connection,
        candidates: &[(String, String)],
    ) -> CatalogResult<i64> {
        if candidates.is_empty() {
            return Ok(0);
        }
        let candidate_set: BTreeSet<(String, String)> = candidates.iter().cloned().collect();
        let candidate_slugs: BTreeSet<&str> =
            candidates.iter().map(|(slug, _)| slug.as_str()).collect();
        let candidate_slugs_json = serde_json::to_string(&candidate_slugs)
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        let mut object_bytes = BTreeMap::<(String, String), i64>::new();
        let mut pending = BTreeSet::<(String, String)>::new();
        let mut statement = connection.prepare(
            "SELECT d.storage_id,o.object_key,o.bytes
                   FROM documents d JOIN json_each(?1) wanted ON wanted.value=d.slug
                   JOIN object_accounting o
                     ON o.storage_id=d.storage_id
                  WHERE d.status IN ('active','creating','deleting')
                 UNION ALL
                 SELECT d.storage_id,o.object_key,o.bytes
                   FROM documents d JOIN json_each(?1) wanted ON wanted.value=d.slug
                   JOIN source_history_objects o
                     ON o.storage_id=d.storage_id
                  WHERE d.status IN ('active','creating','deleting')
                 UNION ALL
                 SELECT d.storage_id,p.object_key,p.bytes
                   FROM documents d JOIN json_each(?1) wanted ON wanted.value=d.slug
                   JOIN pending_deletes p ON p.slug=d.slug
                  WHERE d.status IN ('active','creating','deleting')",
        )?;
        // The physical map may include other documents, but only
        // objects named by the supplied candidate set are returned as
        // reclaimable below.
        let rows = statement.query_map([candidate_slugs_json.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (storage_id, key, bytes) = row?;
            object_bytes
                .entry((storage_id, key))
                .and_modify(|old| *old = (*old).max(bytes))
                .or_insert(bytes);
        }
        drop(statement);

        // Asset references are durable per-checkpoint roots, not a property
        // of the newest tree alone. A missing set marker means pre-migration
        // history; never infer that such a checkpoint had no assets.
        let mut asset_sets_known = true;
        for (slug, sha) in &candidate_set {
            let known: bool = connection.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM checkpoint_asset_sets s
                      JOIN documents d ON d.storage_id=s.storage_id
                     WHERE d.slug=?1 AND s.checkpoint_sha=?2
                 )",
                params![slug, sha],
                |row| row.get(0),
            )?;
            // A retained pre-migration checkpoint may name the same physical
            // asset without a durable root row.  Do not claim that asset is
            // reclaimable merely because the selected checkpoint is measured.
            let document_fully_measured: bool = connection.query_row(
                "SELECT NOT EXISTS(
                     SELECT 1 FROM checkpoints c
                      JOIN documents d ON d.slug=c.slug
                     WHERE c.slug=?1
                       AND d.status IN ('active','creating','deleting')
                       AND NOT EXISTS(
                           SELECT 1 FROM checkpoint_asset_sets s
                            WHERE s.storage_id=d.storage_id
                              AND s.checkpoint_sha=c.sha
                       )
                 )",
                [slug],
                |row| row.get(0),
            )?;
            asset_sets_known &= known && document_fully_measured;
        }
        let mut asset_references = BTreeMap::<(String, String), BTreeSet<(String, String)>>::new();
        if asset_sets_known {
            // Never reclaim the newest checkpoint's assets: the live room may
            // have changed since its last durable tree, and the catalogue has
            // no independent current-tree asset root to prove otherwise.
            for (slug, sha) in &candidate_set {
                let newest: Option<String> = connection
                    .query_row(
                        "SELECT c.sha FROM checkpoints c
                          WHERE c.slug=?1 ORDER BY c.seq DESC LIMIT 1",
                        [slug],
                        |row| row.get(0),
                    )
                    .optional()?;
                let pending_publication: bool = connection.query_row(
                    "SELECT pending_publication IS NOT NULL FROM documents WHERE slug=?1",
                    [slug],
                    |row| row.get(0),
                )?;
                if newest.as_deref() == Some(sha.as_str()) || pending_publication {
                    let mut statement = connection.prepare(
                        "SELECT object_key FROM checkpoint_asset_refs
                          WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=?1)
                            AND checkpoint_sha=?2",
                    )?;
                    let rows = statement.query_map(params![slug, sha], |row| row.get(0))?;
                    for row in rows {
                        let object_key: String = row?;
                        let storage_id: String = connection.query_row(
                            "SELECT storage_id FROM documents WHERE slug=?1",
                            [slug],
                            |row| row.get(0),
                        )?;
                        pending.insert((storage_id, object_key));
                    }
                }
            }
            let mut statement = connection.prepare(
                "SELECT d.storage_id,r.object_key,r.bytes,r.checkpoint_sha,d.slug
                   FROM checkpoint_asset_refs r
                   JOIN documents d ON d.storage_id=r.storage_id
                  WHERE d.status IN ('active','creating','deleting')",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            for row in rows {
                let (storage_id, object_key, bytes, sha, slug) = row?;
                object_bytes
                    .entry((storage_id.clone(), object_key.clone()))
                    .and_modify(|old| *old = (*old).max(bytes))
                    .or_insert(bytes);
                asset_references
                    .entry((storage_id, object_key))
                    .or_default()
                    .insert((slug, sha));
            }
            drop(statement);
        }

        let mut statement = connection.prepare(
            "SELECT d.storage_id,p.object_key
                   FROM documents d JOIN json_each(?1) wanted ON wanted.value=d.slug
                   JOIN pending_deletes p ON p.slug=d.slug
                  WHERE d.status IN ('active','creating','deleting')",
        )?;
        let rows = statement.query_map([candidate_slugs_json.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            pending.insert(row?);
        }
        drop(statement);
        let mut statement = connection.prepare(
            "SELECT l.storage_id,l.object_key FROM source_history_write_leases l
                   JOIN documents d ON d.storage_id=l.storage_id
                   JOIN json_each(?1) wanted ON wanted.value=d.slug
                 UNION ALL
                 SELECT r.storage_id,r.object_key FROM object_reservations r
                   JOIN documents d ON d.storage_id=r.storage_id
                   JOIN json_each(?1) wanted ON wanted.value=d.slug",
        )?;
        let rows = statement.query_map([candidate_slugs_json.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            pending.insert(row?);
        }
        drop(statement);

        // Restrict physical roots to objects named by the candidate
        // checkpoints.  A source object referenced by one retained
        // checkpoint is not reclaimable when another checkpoint is
        // removed, even if the removed event is its first occurrence.
        let mut references = BTreeMap::<(String, String), BTreeSet<(String, String)>>::new();
        let mut statement = connection.prepare(
            "SELECT d.storage_id,r.file_digest,o.object_key,r.checkpoint_sha,d.slug
                   FROM source_history_checkpoint_files r
                   JOIN documents d ON d.storage_id=r.storage_id
                   JOIN json_each(?1) wanted ON wanted.value=d.slug
                   JOIN source_history_objects o
                     ON o.storage_id=r.storage_id AND o.file_digest=r.file_digest
                  WHERE d.status IN ('active','creating','deleting')",
        )?;
        let rows = statement.query_map([candidate_slugs_json.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (storage_id, _file_digest, object_key, sha, slug) = row?;
            references
                .entry((storage_id, object_key))
                .or_default()
                .insert((slug, sha));
        }
        drop(statement);

        let mut reclaimable = BTreeSet::<(String, String)>::new();
        for (object, refs) in references {
            if !refs.is_empty()
                && refs
                    .iter()
                    .all(|reference| candidate_set.contains(reference))
                && !pending.contains(&object)
            {
                reclaimable.insert(object);
            }
        }
        if asset_sets_known {
            for (object, refs) in asset_references {
                if !refs.is_empty()
                    && refs
                        .iter()
                        .all(|reference| candidate_set.contains(reference))
                    && !pending.contains(&object)
                {
                    reclaimable.insert(object);
                }
            }
        }
        for (slug, sha) in &candidate_set {
            let Some((storage_id,)) = connection
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1",
                    [slug],
                    |row| Ok((row.get::<_, String>(0)?,)),
                )
                .optional()
                .map_err(CatalogError::from)?
            else {
                continue;
            };
            let tree_key = crate::storage::blob::checkpoint_key(&storage_id, sha);
            if !pending.contains(&(storage_id.clone(), tree_key.clone())) {
                reclaimable.insert((storage_id, tree_key));
            }
        }

        let mut bytes = 0i64;
        for object in reclaimable {
            if let Some(value) = object_bytes.get(&object) {
                bytes = bytes.saturating_add((*value).max(0));
            }
        }

        // Add only the deterministic metadata of the candidate rows. It
        // is separate from logical tree payload size and is reclaimable
        // even when all source objects are shared with retained events.
        for (slug, sha) in candidate_set {
            let row: Option<(
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                Option<String>,
                Option<String>,
            )> = connection
                .query_row(
                    "SELECT c.slug,c.sha,c.tree_sha,c.parent,c.at,c.by,c.why,
                                c.source_format,c.label,c.git_commit,c.changed,c.by_account
                           FROM checkpoints c JOIN documents d ON d.slug=c.slug
                          WHERE c.slug=?1 AND c.sha=?2
                            AND d.status IN ('active','creating','deleting')",
                    params![slug, sha],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                            row.get(9)?,
                            row.get(10)?,
                            row.get(11)?,
                        ))
                    },
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(values) = row {
                let text_bytes = values.0.len() as i64
                    + values.1.len() as i64
                    + values.2.len() as i64
                    + values.3.len() as i64
                    + values.4.len() as i64
                    + values.5.len() as i64
                    + values.6.len() as i64
                    + values.7.len() as i64
                    + values.8.len() as i64
                    + values.9.len() as i64
                    + values.10.as_deref().unwrap_or_default().len() as i64
                    + values.11.as_deref().unwrap_or_default().len() as i64;
                bytes = bytes.saturating_add(text_bytes.saturating_add(56));
                let asset_metadata: i64 = connection.query_row(
                    "SELECT
                       COALESCE((SELECT SUM(length(CAST(r.storage_id AS BLOB))+
                                           length(CAST(r.checkpoint_sha AS BLOB))+
                                           length(CAST(r.object_key AS BLOB))+8)
                                  FROM checkpoint_asset_refs r
                                 WHERE r.storage_id=(SELECT storage_id FROM documents WHERE slug=?1)
                                   AND r.checkpoint_sha=?2),0)+
                       COALESCE((SELECT SUM(length(CAST(s.storage_id AS BLOB))+
                                           length(CAST(s.checkpoint_sha AS BLOB))+8)
                                  FROM checkpoint_asset_sets s
                                 WHERE s.storage_id=(SELECT storage_id FROM documents WHERE slug=?1)
                                   AND s.checkpoint_sha=?2),0)",
                    params![slug, sha],
                    |row| row.get(0),
                )?;
                bytes = bytes.saturating_add(asset_metadata);
            }
        }
        Ok(bytes)
    }
    /// Read the account owner's saved quota intent.  Missing preferences are
    /// deliberately distinct from a malformed payload: callers can expose a
    /// safe read-only state rather than authorizing destructive thinning.
    pub fn quota_preferences(
        &self,
        account_id: &str,
    ) -> CatalogResult<Option<QuotaPreferencesRecord>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT account_id, revision, payload, policy_generation, updated_at
                     FROM account_quota_preferences WHERE account_id=?1",
                    [account_id],
                    |row| {
                        Ok(QuotaPreferencesRecord {
                            account_id: row.get(0)?,
                            revision: row.get(1)?,
                            payload: row.get(2)?,
                            policy_generation: row.get(3)?,
                            updated_at: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    /// Save account quota intent with optimistic concurrency.  `expected` is
    /// zero for an initial insert; a non-zero value must equal the persisted
    /// revision.  The generation is an immutable operation identity used by
    /// preview/apply and thinning workers.
    pub fn save_quota_preferences(
        &self,
        account_id: &str,
        expected: i64,
        payload: &str,
        policy_generation: &str,
        updated_at: i64,
    ) -> CatalogResult<QuotaPreferencesRecord> {
        if account_id.is_empty() || payload.is_empty() || payload.len() > 65_536 || updated_at < 0 {
            return Err(CatalogError::Invalid(
                "invalid quota preference record".into(),
            ));
        }
        self.immediate(|tx| {
            let current: Option<i64> = tx
                .query_row(
                    "SELECT revision FROM account_quota_preferences WHERE account_id=?1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            match (current, expected) {
                (Some(revision), expected) if revision != expected => {
                    return Err(CatalogError::Conflict("quota preference revision is stale".into()))
                }
                (None, expected) if expected != 0 => {
                    return Err(CatalogError::Conflict("quota preference revision is stale".into()))
                }
                _ => {}
            }
            let revision = current.unwrap_or(0).saturating_add(1);
            let account_active: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active')",
                    [account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !account_active {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "INSERT INTO account_quota_preferences(account_id,revision,payload,policy_generation,updated_at)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(account_id) DO UPDATE SET revision=excluded.revision,
                   payload=excluded.payload, policy_generation=excluded.policy_generation,
                   updated_at=excluded.updated_at",
                params![account_id, revision, payload, policy_generation, updated_at],
            )
            .map_err(CatalogError::from)?;
            Ok(QuotaPreferencesRecord {
                account_id: account_id.to_string(),
                revision,
                payload: payload.to_string(),
                policy_generation: policy_generation.to_string(),
                updated_at,
            })
        })
    }
    /// Insert or refresh a profile.  Lifecycle state and session generation
    /// are never overwritten by a profile refresh.
    pub fn upsert_account(&self, profile: &Account) -> CatalogResult<Account> {
        if profile.id.is_empty() || profile.session_generation.is_empty() {
            return Err(CatalogError::Invalid(
                "account id and generation are required".into(),
            ));
        }
        self.immediate(|tx| {
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT status, session_generation FROM accounts WHERE id = ?1",
                    [&profile.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((status, generation)) = existing {
                if status != "active" {
                    return Err(CatalogError::Conflict(format!(
                        "account is {status} and cannot sign in"
                    )));
                }
                tx.execute(
                    "UPDATE accounts SET provider = ?2, handle = ?3, name = ?4,
                     email = ?5, last_seen = CASE
                       WHEN substr(last_seen, 1, 10) < substr(?6, 1, 10) THEN ?6
                       ELSE last_seen END
                     WHERE id = ?1",
                    params![
                        profile.id,
                        profile.provider,
                        profile.handle,
                        profile.name,
                        profile.email,
                        profile.last_seen
                    ],
                )
                .map_err(CatalogError::from)?;
                return self.account_in_tx(tx, &profile.id, Some(generation));
            }
            tx.execute(
                "INSERT INTO accounts
                 (id, provider, handle, name, email, first_seen, last_seen, plan,
                  status, session_generation, erasure_cursor)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9, NULL)",
                params![
                    profile.id,
                    profile.provider,
                    profile.handle,
                    profile.name,
                    profile.email,
                    profile.first_seen,
                    profile.last_seen,
                    profile.plan,
                    profile.session_generation
                ],
            )
            .map_err(CatalogError::from)?;
            for position in 0..crate::seed::ACCOUNT_EXAMPLE_COUNT {
                tx.execute(
                    "INSERT INTO account_examples (account_id, position, slug) VALUES (?1, ?2, ?3)",
                    params![
                        profile.id,
                        position,
                        format!("starter-{}", crate::util::new_id())
                    ],
                )
                .map_err(CatalogError::from)?;
            }
            self.account_in_tx(tx, &profile.id, None)
        })
    }

    pub fn account(&self, id: &str) -> CatalogResult<Option<Account>> {
        self.with_connection(|connection| {
            Self::account_on(connection, id).map_err(CatalogError::from)
        })
    }

    /// The durable remaining work for this account's first sign-in.
    pub fn pending_account_examples(&self, id: &str) -> CatalogResult<Vec<(usize, String)>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT position, slug FROM account_examples WHERE account_id = ?1 AND completed = 0 ORDER BY position",
            )?;
            let rows = statement.query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(CatalogError::from)
        })
    }

    pub fn complete_account_example(&self, id: &str, position: usize) -> CatalogResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE account_examples SET completed = 1 WHERE account_id = ?1 AND position = ?2",
                params![id, position],
            )?;
            Ok(())
        })
    }

    pub(super) fn account_in_tx(
        &self,
        tx: &Transaction<'_>,
        id: &str,
        _existing_generation: Option<String>,
    ) -> CatalogResult<Account> {
        tx.query_row(
            "SELECT id, provider, handle, name, email, first_seen, last_seen, plan,
                    status, session_generation, erasure_cursor
             FROM accounts WHERE id = ?1",
            [id],
            Self::read_account,
        )
        .map_err(CatalogError::from)
    }

    pub(super) fn account_on(
        connection: &Connection,
        id: &str,
    ) -> rusqlite::Result<Option<Account>> {
        connection
            .query_row(
                "SELECT id, provider, handle, name, email, first_seen, last_seen, plan,
                        status, session_generation, erasure_cursor
                 FROM accounts WHERE id = ?1",
                [id],
                Self::read_account,
            )
            .optional()
    }

    pub(super) fn read_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
        Ok(Account {
            id: row.get(0)?,
            provider: row.get(1)?,
            handle: row.get(2)?,
            name: row.get(3)?,
            email: row.get(4)?,
            first_seen: row.get(5)?,
            last_seen: row.get(6)?,
            plan: row.get(7)?,
            status: row.get(8)?,
            session_generation: row.get(9)?,
            erasure_cursor: row.get(10)?,
        })
    }

    /// Change a session generation in the same authoritative transaction that
    /// marks revocation.  Returns the new generation for cookie invalidation.
    pub fn revoke_sessions(&self, id: &str, new_generation: &str) -> CatalogResult<String> {
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE accounts SET session_generation = ?2 WHERE id = ?1 AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            Ok(new_generation.to_owned())
        })
    }

    /// Mark an account erasing and revoke all sessions atomically.
    pub fn begin_erasure(&self, id: &str, new_generation: &str) -> CatalogResult<()> {
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE accounts SET status = 'erasing', session_generation = ?2,
                     erasure_cursor = NULL WHERE id = ?1 AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            // Withdrawal is part of the lifecycle transition.  The erasure
            // worker still owns physical reclamation and must retain these
            // rows (and their reservations) until object cleanup succeeds.
            // Withdraw every owned lifecycle row, including a creation whose
            // publication receipt is still prepared.  A prepared receipt is
            // an externally visible reservation even though its document is
            // not listed; leaving it behind would let a restart resurrect
            // content for an account that has already been erased.
            tx.execute(
                "UPDATE documents SET status='deleting', pending_publication=NULL
                 WHERE owner_id=?1 AND status IN ('active','creating')",
                [id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status='aborted',
                 result='account erasure withdrew the publication'
                 WHERE status='prepared' AND storage_id IN
                   (SELECT storage_id FROM documents WHERE owner_id=?1 AND status='deleting')",
                [id],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Record qualifying authenticated activity.  Activity is intentionally
    /// monotonic: an imported older timestamp cannot make an account look
    /// recently active, and repeated requests on one UTC day are a no-op.
    pub fn record_activity(&self, id: &str, at: &str) -> CatalogResult<()> {
        if id.is_empty() || at.len() < 10 {
            return Err(CatalogError::Invalid("invalid account activity".into()));
        }
        self.immediate(|tx| {
            let active: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| r.get(0))
                .optional()
                .map_err(CatalogError::from)?;
            if active.as_deref() != Some("active") {
                return Err(CatalogError::Conflict("account is not active".into()));
            }
            tx.execute(
                "INSERT INTO account_activity(account_id, last_qualified_at)
                 VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET last_qualified_at =
                   CASE WHEN account_activity.last_qualified_at < excluded.last_qualified_at
                        THEN excluded.last_qualified_at ELSE account_activity.last_qualified_at END",
                params![id, at],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Persist one bounded erasure cursor.  A worker may safely repeat a
    /// batch after a crash because the cursor update is in the same tx as the
    /// deletions performed by the caller through `with_erasure_batch`.
    pub fn erasure_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if stage.is_empty() || stage.len() > 64 || limit == 0 || limit > 1000 {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: String = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if status != "erasing" {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            tx.execute(
                "INSERT INTO erasure_batches(account_id, stage, cursor, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,
                   cursor=excluded.cursor, updated_at=excluded.updated_at",
                params![id, stage, cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor = ?2 WHERE id = ?1",
                params![id, cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(limit)
        })
    }

    /// Apply one resumable logical-erasure batch.  Physical document cleanup
    /// remains owned by `begin_delete`/`finish_delete`; this method handles
    /// account references in documents that belong to other users.
    pub fn erase_account_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if limit == 0 || limit > 1000 || stage.is_empty() {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id=?1", [id], |r| r.get(0))
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            // These tables deliberately use WITHOUT ROWID primary keys in
            // the catalogue contract.  Advance by immutable primary-key
            // tuples rather than SQLite's hidden rowid: the cursor remains
            // valid across vacuum/backup/restore and a crash can repeat only
            // an already committed keyset batch.
            let cursor_parts = cursor
                .map(|value| {
                    serde_json::from_str::<Vec<String>>(value).map_err(|_| {
                        CatalogError::Invalid("invalid erasure cursor".into())
                    })
                })
                .transpose()?;
            let n_and_cursor = match stage {
                "grants" => {
                    let (after_slug, after_role) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid grants cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, role FROM grants
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND role>?3))
                             ORDER BY slug, role LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_role, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, role) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                            params![slug, role, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, role)| serde_json::json!([slug, role]).to_string()), has_rows)
                }
                "guests" => {
                    let (after_slug, after_link) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid guests cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, link_hash FROM guests
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND link_hash>?3))
                             ORDER BY slug, link_hash LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_link, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, link_hash) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                            params![slug, id, link_hash],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, link)| serde_json::json!([slug, link]).to_string()), has_rows)
                }
                "comments" => {
                    let (after_slug, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid comments cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, id FROM comments
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND id>?3))
                             ORDER BY slug, id LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_id, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, comment_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM comments WHERE slug=?1 AND id=?2 AND author=?3",
                            params![slug, comment_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment_id)| serde_json::json!([slug, comment_id]).to_string()), has_rows)
                }
                "replies" => {
                    let (after_slug, after_comment, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 3 {
                                return Err(CatalogError::Invalid("invalid replies cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str(), parts[2].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", "", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, comment_id, id FROM replies
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND comment_id>?3)
                                    OR (slug=?2 AND comment_id=?3 AND id>?4))
                             ORDER BY slug, comment_id, id LIMIT ?5",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![id, after_slug, after_comment, after_id, i64::from(limit)],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, comment_id, reply_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM replies
                             WHERE slug=?1 AND comment_id=?2 AND id=?3 AND author=?4",
                            params![slug, comment_id, reply_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment, reply)| serde_json::json!([slug, comment, reply]).to_string()), has_rows)
                }
                // Checkpoints are the account's contributions to documents it
                // may not own.  The row itself is retained -- its content,
                // sha, tree_sha, parent, timestamps and label are the
                // document's history, and the event identity other rows point
                // at -- and only the identifying attribution is cleared.
                //
                // Two stages, because there are two ways a row can name this
                // account.  The indexed one is the stable id; the legacy one
                // is a pre-migration row whose `by` literally holds an account
                // id, which is what the erasure query matched before this
                // column existed.  That second match is on the account id, not
                // on a handle: no row is selected because its display name
                // resembles the account's.
                "checkpoints" => {
                    let (after_slug, after_sha) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid checkpoints cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, sha FROM checkpoints
                             WHERE by_account=?1
                               AND (slug>?2 OR (slug=?2 AND sha>?3))
                             ORDER BY slug, sha LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_sha, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, sha) in rows.drain(..) {
                        tx.execute(
                            "UPDATE checkpoints SET by=?4, by_account=NULL
                             WHERE slug=?1 AND sha=?2 AND by_account=?3",
                            params![slug, sha, id, ERASED_ATTRIBUTION],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, sha)| serde_json::json!([slug, sha]).to_string()), has_rows)
                }
                "checkpoints_legacy" => {
                    let (after_slug, after_sha) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid(
                                    "invalid legacy checkpoints cursor".into(),
                                ));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, sha FROM checkpoints
                             WHERE by_account IS NULL AND by=?1
                               AND (slug>?2 OR (slug=?2 AND sha>?3))
                             ORDER BY slug, sha LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_sha, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, sha) in rows.drain(..) {
                        tx.execute(
                            "UPDATE checkpoints SET by=?4
                             WHERE slug=?1 AND sha=?2 AND by_account IS NULL AND by=?3",
                            params![slug, sha, id, ERASED_ATTRIBUTION],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, sha)| serde_json::json!([slug, sha]).to_string()), has_rows)
                }
                _ => return Err(CatalogError::Invalid("unknown erasure stage".into())),
            };
            let n = u32::from(n_and_cursor.1);
            let next_cursor = n_and_cursor.0;
            tx.execute(
                "INSERT INTO erasure_batches(account_id,stage,cursor,updated_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,cursor=excluded.cursor,updated_at=excluded.updated_at",
                params![id, stage, next_cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor=?2 WHERE id=?1",
                params![id, next_cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(n)
        })
    }

    pub fn finish_erasure(&self, id: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::NotFound);
            }
            let owned: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE owner_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            if owned != 0 {
                return Err(CatalogError::Conflict("owned documents remain".into()));
            }
            let references: i64 = tx
                .query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM grants WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM guests WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM comments WHERE author=?1) +
                       (SELECT COUNT(*) FROM replies WHERE author=?1) +
                       (SELECT COUNT(*) FROM checkpoints WHERE by_account=?1) +
                       (SELECT COUNT(*) FROM checkpoints
                        WHERE by_account IS NULL AND by=?1)",
                    [id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if references != 0 {
                return Err(CatalogError::Conflict("account attribution remains".into()));
            }
            tx.execute("DELETE FROM accounts WHERE id = ?1", [id])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Accounts awaiting the bounded erasure worker, ordered by immutable id.
    pub fn erasing_accounts(
        &self,
        after_id: Option<&str>,
        limit: u32,
    ) -> CatalogResult<Vec<String>> {
        let limit = i64::from(limit.clamp(1, 100));
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM accounts WHERE status='erasing' AND (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2"
            ).map_err(CatalogError::from)?;
            let rows = statement.query_map(params![after_id,limit], |row| row.get(0)).map_err(CatalogError::from)?;
            rows.collect::<Result<_,_>>().map_err(CatalogError::from)
        })
    }

    pub fn erasure_stage(&self, id: &str) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn erasure_progress(&self, id: &str) -> CatalogResult<Option<(String, Option<String>)>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage, cursor FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }
}
