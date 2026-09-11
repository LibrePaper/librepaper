//! Durable retention operations.
//!
//! Selection is performed by the policy layer, but the catalogue owns the
//! operation identity and the final delete.  This is what makes a preview
//! replay-safe across process restarts and prevents a stale HTTP request from
//! deleting a newly-created checkpoint.

use super::*;

fn retention_event_time(value: &str) -> i64 {
    value
        .parse::<i64>()
        .ok()
        .or_else(|| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .ok()
                .map(|time| time.unix_timestamp())
        })
        .unwrap_or(0)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionJob {
    pub account_id: String,
    pub generation: String,
    pub revision: i64,
    pub status: String,
    pub grace_until: i64,
    pub candidate_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionPass {
    pub generation: String,
    pub removed: Vec<(String, String)>,
    pub blocked: usize,
}

impl Catalog {
    /// Enrol a newly-created Balanced document in routine retention.
    pub fn schedule_document_balanced(
        &self,
        slug: &str,
        now: i64,
        bounds: crate::document::quota::RetentionBounds,
    ) -> CatalogResult<usize> {
        if slug.is_empty() || now < 0 {
            return Err(CatalogError::Invalid(
                "invalid routine retention schedule".into(),
            ));
        }
        // Selection is owner-wide because the account's history budget spans
        // documents (the physical namespace itself is document-scoped).
        // Read the same snapshot shape used by quota preview;
        // the write transaction below still rechecks ownership and policy.
        let (owner, owner_key): (Option<String>, String) = self.with_connection(|connection| connection
            .query_row("SELECT owner_id,owner_key FROM documents WHERE slug=?1 AND status IN ('active','creating') AND pending_publication IS NULL", [slug], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional().map_err(CatalogError::from))?.unwrap_or((None, String::new()));
        let mode: String = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT mode FROM document_retention_policy WHERE slug=?1",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })?;
        if mode != "balanced" {
            return Ok(0);
        }
        if owner.is_none() {
            return self.schedule_ownerless_document_balanced(slug, &owner_key, now, bounds);
        }
        let owner = owner.expect("owner checked above");
        let (revision, payload, preferences) = match self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT revision,payload FROM account_quota_preferences WHERE account_id=?1",
                    [&owner],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)
        })? {
            Some((revision, payload)) => {
                let preferences = serde_json::from_str(&payload).map_err(|_| {
                    CatalogError::Conflict("stored quota preference payload is unreadable".into())
                })?;
                (revision, payload, preferences)
            }
            None => {
                let preferences = crate::document::quota::QuotaPreferences::default();
                let payload = serde_json::to_string(&preferences).map_err(|_| {
                    CatalogError::Invalid("default retention policy cannot be encoded".into())
                })?;
                (0, payload, preferences)
            }
        };
        let usage = self.account_storage_usage(&owner)?;
        let points = self.account_checkpoints(&owner)?;
        let references = self.account_open_annotation_references(&owner)?;
        let modes: std::collections::BTreeMap<String, String> =
            self.with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT d.slug,p.mode FROM documents d
                 JOIN document_retention_policy p ON p.slug=d.slug
                 WHERE d.owner_id=?1 AND d.status IN ('active','creating')",
                )?;
                let rows = statement.query_map([&owner], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()
                    .map_err(CatalogError::from)
            })?;
        let mut grouped = std::collections::BTreeMap::<String, Vec<_>>::new();
        for point in points {
            grouped.entry(point.slug.clone()).or_default().push(point);
        }
        let mut policies = Vec::new();
        let mut age_keys = Vec::new();
        let mut routine_keys = Vec::new();
        let mut order = std::collections::BTreeMap::<(String, String), (i64, i64)>::new();
        for (document, rows) in grouped {
            if modes.get(&document).map(String::as_str) != Some("balanced") {
                continue;
            }
            let points = crate::document::history::Manifest::from_catalog_rows(rows)
                .map_err(CatalogError::Invalid)?
                .checkpoints;
            let refs = references.get(&document).cloned().unwrap_or_default();
            let policy =
                crate::document::quota::select_retained(&points, now, &preferences, &bounds, &refs);
            let newest = points
                .iter()
                .enumerate()
                .max_by_key(|(index, point)| {
                    (
                        retention_event_time(&point.at),
                        point.seq.max(*index as i64),
                        point.sha.clone(),
                    )
                })
                .map(|(_, point)| point.sha.as_str());
            for point in &points {
                order.insert(
                    (document.clone(), point.sha.clone()),
                    (retention_event_time(&point.at), point.seq),
                );
                if !policy.retained.contains(&point.sha) {
                    age_keys.push((document.clone(), point.sha.clone()));
                }
                if newest != Some(point.sha.as_str()) && !policy.protected.contains(&point.sha) {
                    routine_keys.push((document.clone(), point.sha.clone()));
                }
            }
            policies.push((document, points));
        }
        age_keys.sort_by_key(|key| (order.get(key).cloned().unwrap_or_default(), key.clone()));
        routine_keys.sort_by_key(|key| (order.get(key).cloned().unwrap_or_default(), key.clone()));
        routine_keys.retain(|key| !age_keys.contains(key));
        let required = usage
            .history_bytes
            .saturating_sub(preferences.history_budget.target_bytes(bounds.hard_quota));
        let mut selected = age_keys;
        if usage.physical_accounting && required > 0 {
            let mut reclaimable = self.reclaimable_checkpoint_bytes(&selected)?;
            if reclaimable < required {
                for candidate in routine_keys {
                    selected.push(candidate);
                    reclaimable = self.reclaimable_checkpoint_bytes(&selected)?;
                    if reclaimable >= required {
                        break;
                    }
                }
            }
        }
        if selected.is_empty() {
            return Ok(0);
        }
        let mut digest = sha2::Sha256::new();
        digest.update(b"routine-balanced-owner");
        digest.update(owner.as_bytes());
        digest.update(now.div_euclid(300).to_be_bytes());
        for (document, points) in &policies {
            digest.update(document.as_bytes());
            for point in points {
                digest.update(point.sha.as_bytes());
                digest.update(point.seq.to_be_bytes());
                digest.update(point.parent.as_bytes());
                digest.update(point.label.as_bytes());
                digest.update(point.why.as_bytes());
            }
        }
        let generation = hex::encode(digest.finalize());
        let fingerprint = format!("routine:{owner}:{generation}");
        self.immediate(|tx| {
            let current: Option<String> = tx.query_row(
                "SELECT owner_id FROM documents WHERE slug=?1 AND status IN ('active','creating')", [slug], |row| row.get(0)).optional()?;
            if current.as_deref() != Some(owner.as_str()) { return Ok(0); }
            let current_mode: String = tx.query_row(
                "SELECT mode FROM document_retention_policy WHERE slug=?1", [slug], |row| row.get(0))?;
            if current_mode != "balanced" { return Ok(0); }
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM quota_retention_jobs WHERE account_id=?1 AND generation=?2)",
                params![owner, generation], |row| row.get(0))?;
            if exists { return Ok(0); }
            tx.execute(
                "INSERT INTO quota_retention_jobs(account_id,generation,revision,payload,candidate_fingerprint,hard_count_limit,status,grace_until,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,'pending',?7,?7,?7)",
                params![owner, generation, revision, payload, fingerprint,
                    bounds.max_checkpoint_count.map(i64::from).unwrap_or(-1), now],
            )?;
            for (document, sha) in &selected {
                let Some(point) = policies.iter().find_map(|(slug, points)|
                    (slug == document).then(|| points.iter().find(|point| point.sha == *sha)).flatten()) else { continue; };
                tx.execute(
                    "INSERT OR IGNORE INTO quota_retention_candidates
                     (account_id,generation,slug,sha,planned_seq,planned_parent,planned_tree_sha,planned_at,planned_label,planned_why,grace_until,status)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending')",
                    params![owner, generation, document, sha, point.seq, point.parent, point.content_sha(), point.at, point.label, point.why, now],
                )?;
            }
            Ok(1)
        })
    }

    fn schedule_ownerless_document_balanced(
        &self,
        slug: &str,
        owner_key: &str,
        now: i64,
        bounds: crate::document::quota::RetentionBounds,
    ) -> CatalogResult<usize> {
        let points = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.slug,c.sha,c.seq,c.durable_seq,c.tree_sha,c.parent,c.at,c.by,
                        c.by_account,c.why,c.source_format,c.size,c.label,c.git_commit,
                        c.dirty,c.changed
                   FROM checkpoints c JOIN documents d ON d.slug=c.slug
                  WHERE c.slug=?1 AND d.owner_id IS NULL AND d.owner_key=?2
                  ORDER BY c.seq",
            )?;
            let rows = statement.query_map(params![slug, owner_key], |row| {
                Ok(crate::storage::catalog::Checkpoint {
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
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(CatalogError::from)
        })?;
        let points = crate::document::history::Manifest::from_catalog_rows(points)
            .map_err(CatalogError::Invalid)?
            .checkpoints;
        let refs: std::collections::BTreeSet<String> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT revision FROM comments WHERE slug=?1 AND resolved=0 AND revision<>''",
            )?;
            let rows = statement.query_map([slug], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()
                .map_err(CatalogError::from)
        })?;
        let prefs = crate::document::quota::QuotaPreferences::default();
        let decision =
            crate::document::quota::select_retained(&points, now, &prefs, &bounds, &refs);
        let newest = points
            .iter()
            .max_by_key(|point| {
                (
                    retention_event_time(&point.at),
                    point.seq,
                    point.sha.clone(),
                )
            })
            .map(|point| point.sha.clone());
        // Ownerless/new visitor documents have no account quota job to own a
        // retention generation.  Keep this path deliberately small and
        // synchronous: a pseudo account would violate the accounts FK and
        // would leave candidates that the account worker can never execute.
        // The policy decision is only a plan; every row is revalidated below
        // in the same write transaction before its graph/ancestry cleanup.
        let selected: Vec<_> = points
            .iter()
            .filter(|point| {
                newest.as_deref() != Some(point.sha.as_str())
                    && !decision.protected.contains(&point.sha)
                    && !decision.retained.contains(&point.sha)
            })
            .map(|point| (slug.to_string(), point.sha.clone()))
            .take(64)
            .collect();
        if selected.is_empty() {
            return Ok(0);
        }
        let mut planned_parents: std::collections::BTreeMap<_, _> = points
            .iter()
            .map(|point| (point.sha.clone(), point.parent.clone()))
            .collect();
        self.immediate(|tx| {
            let current: Option<(Option<String>, String)> = tx.query_row(
                "SELECT owner_id,owner_key FROM documents
                 WHERE slug=?1 AND status IN ('active','creating')
                   AND pending_publication IS NULL",
                [slug], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
            if current.as_ref().and_then(|row| row.0.as_ref()).is_some()
                || current.as_ref().map(|row| row.1.as_str()) != Some(owner_key)
            {
                return Ok(0);
            }
            let mode: String = tx.query_row(
                "SELECT mode FROM document_retention_policy WHERE slug=?1",
                [slug],
                |row| row.get(0),
            )?;
            if mode != "balanced" {
                return Ok(0);
            }

            let mut removed = 0usize;
            for (_, sha) in &selected {
                let planned = points.iter().find(|point| point.sha == *sha).expect("selected point");
                let current: Option<(i64, String, String, String, String, String)> = tx
                    .query_row(
                        "SELECT c.seq,c.parent,COALESCE(NULLIF(c.tree_sha,''),c.sha),c.at,c.label,c.why
                         FROM checkpoints c JOIN documents d ON d.slug=c.slug
                         WHERE c.slug=?1 AND c.sha=?2 AND d.owner_id IS NULL
                           AND d.owner_key=?3 AND d.status IN ('active','creating')
                           AND d.pending_publication IS NULL",
                        params![slug, sha, owner_key],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
                    )
                    .optional()?;
                let Some((seq, parent, tree_sha, at, label, why)) = current else {
                    continue;
                };
                if seq != planned.seq
                    || planned_parents.get(sha) != Some(&parent)
                    || tree_sha != planned.content_sha()
                    || at != planned.at
                    || label != planned.label
                    || why != planned.why
                {
                    continue;
                }
                // Sequence, not wall-clock text, is the catalogue's newest
                // authority.  Protect all object classes that can still be
                // in flight, including a checkpoint tree reservation and
                // source-history leases that have not reached the graph yet.
                let newest: Option<String> = tx
                    .query_row(
                        "SELECT sha FROM checkpoints WHERE slug=?1 ORDER BY seq DESC LIMIT 1",
                        [slug],
                        |row| row.get(0),
                    )
                    .optional()?;
                if newest.as_deref() == Some(sha.as_str()) {
                    continue;
                }
                let lease: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(lease_until),0) FROM checkpoint_retention WHERE slug=?1 AND sha=?2",
                    params![slug, sha],
                    |row| row.get(0),
                )?;
                if lease > now {
                    continue;
                }
                let open_reference: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM comments WHERE slug=?1 AND resolved=0
                                   AND (revision=?2 OR revision=?3))",
                    params![slug, sha, tree_sha],
                    |row| row.get(0),
                )?;
                if open_reference || !planned.label.is_empty() || planned.why == "cli"
                    || planned.why == "publish" || planned.why == "restore"
                    || planned.why == "restored" || planned.why == "accept"
                {
                    continue;
                }
                let storage_id: String = tx.query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1",
                    [slug],
                    |row| row.get(0),
                )?;
                let tree_key = crate::storage::blob::checkpoint_key(&storage_id, sha);
                let busy: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM object_reservations
                                   WHERE storage_id=?1 AND object_key=?2)
                     OR EXISTS(SELECT 1 FROM source_history_write_leases
                               WHERE storage_id=?1 AND object_key=?2)",
                    params![storage_id, tree_key],
                    |row| row.get(0),
                )?;
                if busy {
                    continue;
                }
                let successors: Vec<String> = {
                    let mut statement = tx.prepare(
                        "SELECT sha FROM checkpoints WHERE slug=?1 AND parent=?2
                         ORDER BY seq LIMIT 64",
                    )?;
                    let rows = statement
                        .query_map(params![slug, sha], |row| row.get(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                for successor in successors {
                    planned_parents.insert(successor.clone(), parent.clone());
                    tx.execute(
                        "UPDATE checkpoints SET parent=?3,changed=NULL
                         WHERE slug=?1 AND sha=?2",
                        params![slug, successor, parent],
                    )?;
                    tx.execute(
                        "INSERT OR IGNORE INTO checkpoint_retention
                         (slug,sha,original_parent,ancestry_gap) VALUES(?1,?2,?3,1)",
                        params![slug, successor, sha],
                    )?;
                    tx.execute(
                        "UPDATE checkpoint_retention SET ancestry_gap=1
                         WHERE slug=?1 AND sha=?2",
                        params![slug, successor],
                    )?;
                }
                let tree_bytes: i64 = tx.query_row(
                    "SELECT COALESCE((SELECT bytes FROM object_accounting
                                      WHERE storage_id=?1 AND object_key=?2),0)",
                    params![storage_id, tree_key],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "DELETE FROM checkpoints WHERE slug=?1 AND sha=?2",
                    params![slug, sha],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO pending_deletes
                     (slug,object_key,bytes,queued_at,delete_after)
                     VALUES(?1,?2,?3,?4,?4)",
                    params![slug, tree_key, tree_bytes, now],
                )?;
                removed += 1;
            }
            Ok(removed)
        })
    }

    pub fn retention_metadata_range(
        &self,
        slug: &str,
        first: i64,
        last: i64,
    ) -> CatalogResult<std::collections::HashMap<String, (String, bool)>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT r.sha,r.original_parent,r.ancestry_gap FROM checkpoint_retention r JOIN checkpoints c ON c.slug=r.slug AND c.sha=r.sha WHERE r.slug=?1 AND c.seq>=?2 AND c.seq<=?3")?;
            let rows = statement.query_map(params![slug, first, last], |row| Ok((row.get(0)?, (row.get(1)?, row.get::<_, i64>(2)? != 0))))?;
            rows.collect::<rusqlite::Result<_>>().map_err(CatalogError::from)
        })
    }

    pub fn checkpoint_retention_metadata(
        &self,
        slug: &str,
        sha: &str,
    ) -> CatalogResult<Option<(String, bool)>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT original_parent,ancestry_gap FROM checkpoint_retention
             WHERE slug=?1 AND sha=?2",
                    params![slug, sha],
                    |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }
}

impl Catalog {
    /// Persist a preference revision and its exact candidate set in one
    /// transaction. Replaying a generation is a no-op; replaying it with a
    /// different candidate fingerprint is rejected as an operation-key
    /// conflict.
    /// Compatibility entry point for callers that do not carry deployment
    /// hard-count configuration. Such jobs retain the historical `-1`
    /// behavior and therefore enforce only their persisted policy candidates.
    #[allow(clippy::too_many_arguments)]
    pub fn save_quota_preferences_and_schedule(
        &self,
        account_id: &str,
        expected_revision: i64,
        payload: &str,
        generation: &str,
        candidate_fingerprint: &str,
        candidates: &[(String, String)],
        grace_until: i64,
        now: i64,
    ) -> CatalogResult<RetentionJob> {
        self.save_quota_preferences_and_schedule_with_hard_count_limit(
            account_id,
            expected_revision,
            payload,
            generation,
            candidate_fingerprint,
            candidates,
            grace_until,
            now,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_quota_preferences_and_schedule_with_hard_count_limit(
        &self,
        account_id: &str,
        expected_revision: i64,
        payload: &str,
        generation: &str,
        candidate_fingerprint: &str,
        candidates: &[(String, String)],
        grace_until: i64,
        now: i64,
        hard_count_limit: Option<u32>,
    ) -> CatalogResult<RetentionJob> {
        if account_id.is_empty()
            || generation.is_empty()
            || candidate_fingerprint.is_empty()
            || expected_revision < 0
            || grace_until < now
            || now < 0
            || candidates.len() > 100_000
        {
            return Err(CatalogError::Invalid(
                "invalid retention application".into(),
            ));
        }
        self.immediate(|tx| {
            let existing: Option<(i64, String, String, String, i64)> = tx.query_row(
                "SELECT revision,status,candidate_fingerprint,payload,grace_until
                 FROM quota_retention_jobs WHERE account_id=?1 AND generation=?2",
                params![account_id, generation],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional()?;
            if let Some((job_revision, status, fingerprint, old_payload, old_grace)) = existing {
                if expected_revision != job_revision
                    && job_revision != expected_revision.saturating_add(1)
                {
                    return Err(CatalogError::Conflict("retention generation revision is stale".into()));
                }
                if fingerprint != candidate_fingerprint || old_payload != payload {
                    return Err(CatalogError::Conflict("retention generation was reused".into()));
                }
                return Ok(RetentionJob { account_id: account_id.into(), generation: generation.into(),
                    revision: job_revision, status, grace_until: old_grace,
                    candidate_fingerprint: fingerprint });
            }
            let current: Option<i64> = tx.query_row(
                "SELECT revision FROM account_quota_preferences WHERE account_id=?1",
                [account_id], |row| row.get(0)).optional()?;
            if current.unwrap_or(0) != expected_revision {
                return Err(CatalogError::Conflict("quota preference revision is stale".into()));
            }
            let revision = expected_revision.saturating_add(1);
            tx.execute(
                "INSERT INTO account_quota_preferences(account_id,revision,payload,policy_generation,updated_at)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(account_id) DO UPDATE SET revision=excluded.revision,
                 payload=excluded.payload,policy_generation=excluded.policy_generation,updated_at=excluded.updated_at",
                params![account_id, revision, payload, generation, now],
            )?;
            // A newer confirmed preview supersedes an older grace window. Its
            // candidates remain as an audit trail but can no longer become a
            // deletion pass after the owner has chosen a different policy.
            tx.execute(
                "UPDATE quota_retention_jobs SET status='stale',updated_at=?2
                 WHERE account_id=?1 AND status IN ('pending','grace','running')",
                params![account_id, now],
            )?;
            tx.execute(
                "INSERT INTO quota_retention_jobs(account_id,generation,revision,payload,candidate_fingerprint,hard_count_limit,status,grace_until,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,'grace',?7,?8,?8)",
                params![account_id, generation, revision, payload, candidate_fingerprint,
                    hard_count_limit.map(i64::from).unwrap_or(-1), grace_until, now],
            )?;
            for (slug, sha) in candidates {
                let planned: Option<(i64, String, String, String, String, String)> = tx.query_row(
                    "SELECT c.seq,c.parent,COALESCE(NULLIF(c.tree_sha,''),c.sha),c.at,c.label,c.why
                     FROM checkpoints c JOIN documents d ON d.slug=c.slug
                     WHERE c.slug=?1 AND c.sha=?2 AND d.owner_id=?3
                       AND d.status IN ('active','creating')",
                    params![slug, sha, account_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))
                    .optional()?;
                let Some((planned_seq, planned_parent, planned_tree_sha, planned_at, planned_label, planned_why)) = planned else { continue; };
                tx.execute(
                    "INSERT OR IGNORE INTO quota_retention_candidates
                     (account_id,generation,slug,sha,planned_seq,planned_parent,planned_tree_sha,
                      planned_at,planned_label,planned_why,grace_until,status)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending')",
                    params![account_id, generation, slug, sha, planned_seq, planned_parent,
                        planned_tree_sha, planned_at, planned_label, planned_why, grace_until],
                )?;
            }
            Ok(RetentionJob { account_id: account_id.into(), generation: generation.into(),
                revision, status: "grace".into(), grace_until, candidate_fingerprint: candidate_fingerprint.into() })
        })
    }

    /// Apply at most `limit` due candidates. The transaction rechecks the
    /// document owner, newest event and active lease/grace metadata before
    /// every delete. Reparenting invalidates changed-path summaries rather
    /// than manufacturing an authorship edge across the gap.
    pub fn run_retention_pass(&self, now: i64, limit: u32) -> CatalogResult<RetentionPass> {
        self.run_retention_pass_with_limits(now, limit, None, None)
    }

    /// Run retention using the operator configuration currently in force.
    /// A pressure snapshot made under another hard quota is stale, not an
    /// authorization to continue deleting against the old deployment limit.
    pub fn run_retention_pass_with_limits(
        &self,
        now: i64,
        limit: u32,
        current_hard_quota: Option<i64>,
        current_hard_count: Option<u32>,
    ) -> CatalogResult<RetentionPass> {
        if now < 0 {
            return Err(CatalogError::Invalid("negative retention time".into()));
        }
        let limit = i64::from(limit.clamp(1, 500));
        self.immediate(|tx| {
            let job: Option<(String, String, String, i64, String, i64)> = tx.query_row(
                "SELECT generation,account_id,payload,hard_count_limit,candidate_fingerprint,pressure FROM quota_retention_jobs
                 WHERE status IN ('grace','pending','running') AND grace_until<=?1
                 ORDER BY updated_at,generation LIMIT 1", [now],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))).optional()?;
            let Some((generation, account_id, payload, hard_count_limit, candidate_fingerprint, pressure)) = job else {
                return Ok(RetentionPass { generation: String::new(), removed: Vec::new(), blocked: 0 });
            };
            // A pressure job is only a retryable plan.  Re-check the current
            // measured ledger before touching history: pending GC remains
            // charged, and an intervening successful delete may already have
            // brought the owner back under the hard quota.  Unknown physical
            // accounting fails closed rather than turning a stale plan into
            // speculative destruction.
            if pressure != 0 {
                if let Some(current_quota) = current_hard_quota {
                    let planned_quota = crate::storage::catalog::pressure::hard_pressure_original_quota(&candidate_fingerprint);
                    if planned_quota != Some(current_quota) {
                        let blocked = tx.execute(
                            "UPDATE quota_retention_candidates SET status='blocked'
                             WHERE account_id=?1 AND generation=?2 AND status='pending'",
                            params![account_id, generation],
                        )?;
                        tx.execute(
                            "UPDATE quota_retention_jobs SET status='stale',updated_at=?3
                             WHERE account_id=?1 AND generation=?2",
                            params![account_id, generation, now],
                        )?;
                        return Ok(RetentionPass { generation, removed: Vec::new(), blocked });
                    }
                }
                if let Some(current_count) = current_hard_count {
                    let planned_count = crate::storage::catalog::pressure::hard_pressure_count(&candidate_fingerprint);
                    if planned_count != Some(i64::from(current_count)) {
                        let blocked = tx.execute(
                            "UPDATE quota_retention_candidates SET status='blocked'
                             WHERE account_id=?1 AND generation=?2 AND status='pending'",
                            params![account_id, generation],
                        )?;
                        tx.execute(
                            "UPDATE quota_retention_jobs SET status='stale',updated_at=?3
                             WHERE account_id=?1 AND generation=?2",
                            params![account_id, generation, now],
                        )?;
                        return Ok(RetentionPass { generation, removed: Vec::new(), blocked });
                    }
                }
                // A pressure job has no historical count override. If the
                // caller supplies a live count limit, it is enforced by the
                // normal candidate checks below only when the job persisted
                // one; never synthesize a deletion from a changed limit.
                let awaiting_reclamation: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM quota_retention_candidates
                                   WHERE account_id=?1 AND generation=?2 AND status='removed')
                        AND EXISTS(SELECT 1 FROM pending_deletes p JOIN documents d ON d.slug=p.slug
                                   WHERE d.owner_id=?1)",
                    params![account_id, generation], |row| row.get(0),
                )?;
                if awaiting_reclamation {
                    // The selected prefix already promised these bytes.
                    // Failed or slow GC is not permission to destroy another
                    // prefix while that first reclamation is still pending.
                    return Ok(RetentionPass { generation, removed: Vec::new(), blocked: 0 });
                }
                let quota = crate::storage::catalog::pressure::hard_pressure_quota(&candidate_fingerprint);
                let usage = Self::account_storage_usage_on(tx, &account_id)?;
                if quota.is_none() || !usage.physical_accounting || usage.charged_bytes <= quota.unwrap_or(-1) {
                    let blocked = tx.execute(
                        "UPDATE quota_retention_candidates SET status='blocked'
                         WHERE account_id=?1 AND generation=?2 AND status='pending'",
                        params![account_id, generation],
                    )?;
                    tx.execute(
                        "UPDATE quota_retention_jobs SET status='complete',completed_at=?3,updated_at=?3
                         WHERE account_id=?1 AND generation=?2",
                        params![account_id, generation, now],
                    )?;
                    return Ok(RetentionPass { generation, removed: Vec::new(), blocked });
                }
            }
            tx.execute("UPDATE quota_retention_jobs SET status='running',updated_at=?3
                        WHERE account_id=?1 AND generation=?2", params![account_id,generation,now])?;
            let preferences: crate::document::quota::QuotaPreferences = serde_json::from_str(&payload)
                .map_err(|_| CatalogError::Conflict("retention preference payload is unreadable".into()))?;
            // A document may have been deleted or transferred during grace.
            // Such a candidate is already gone or no longer ours; do not
            // leave the job permanently pending because of an inner join.
            if pressure == 0 {
                tx.execute(
                    "UPDATE quota_retention_candidates SET status='blocked'
                     WHERE account_id=?1 AND generation=?2 AND status='pending'
                       AND NOT EXISTS (SELECT 1 FROM checkpoints c JOIN documents d ON d.slug=c.slug
                                       WHERE c.slug=quota_retention_candidates.slug
                                         AND c.sha=quota_retention_candidates.sha
                                         AND d.owner_id=?1 AND d.status IN ('active','creating'))",
                    params![account_id, generation],
                )?;
            }
            let candidate_limit = if pressure != 0 { 50_000 } else { limit };
            let mut statement = tx.prepare(
                "SELECT c.slug,c.sha,q.planned_seq,q.planned_parent,q.planned_tree_sha,
                        q.planned_at,q.planned_label,q.planned_why
                 FROM quota_retention_candidates q
                 JOIN checkpoints c ON c.slug=q.slug AND c.sha=q.sha
                 JOIN documents d ON d.slug=c.slug
                 WHERE q.account_id=?1 AND q.generation=?2 AND q.status='pending'
                   AND q.grace_until<=?3 AND d.owner_id=?1
                 ORDER BY CASE WHEN ?5 != 0 THEN q.pressure_class ELSE 0 END,
                          CASE WHEN ?5 != 0 THEN q.pressure_ordinal ELSE 0 END,
                          c.at,c.seq,c.sha LIMIT ?4")?;
            let mut rows = statement.query_map(params![account_id,generation,now,candidate_limit,pressure], |row| Ok((
                row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,i64>(2)?,
                row.get::<_,String>(3)?, row.get::<_,String>(4)?, row.get::<_,String>(5)?,
                row.get::<_,String>(6)?, row.get::<_,String>(7)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            if pressure != 0 {
                // A pressure plan is all-or-nothing with respect to its
                // snapshot. Revalidate every candidate and its cumulative
                // physical union while this transaction still owns the write
                // lock; a changed newest event, lease, owner, or planned
                // metadata blocks the whole job rather than deleting a safe
                // prefix and then making a stale decision about protected
                // history.
                let pending_count: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM quota_retention_candidates
                     WHERE account_id=?1 AND generation=?2 AND status='pending'",
                    params![account_id, generation],
                    |row| row.get(0),
                )?;
                let mut valid = pending_count == rows.len() as i64;
                let all_keys: Vec<(String, String)> = rows
                    .iter()
                    .map(|row| (row.0.clone(), row.1.clone()))
                    .collect();
                for (slug, sha, planned_seq, planned_parent, planned_tree_sha, planned_at, planned_label, planned_why) in &rows {
                    let current: Option<(i64, String, String, String, String, String)> = tx
                        .query_row(
                            "SELECT c.seq,c.parent,COALESCE(NULLIF(c.tree_sha,''),c.sha),c.at,c.label,c.why
                             FROM checkpoints c JOIN documents d ON d.slug=c.slug
                             WHERE c.slug=?1 AND c.sha=?2 AND d.owner_id=?3
                               AND d.status IN ('active','creating')",
                            params![slug, sha, account_id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
                        )
                        .optional()?;
                    let newest: Option<String> = tx.query_row(
                        "SELECT sha FROM checkpoints WHERE slug=?1 ORDER BY seq DESC LIMIT 1",
                        [slug],
                        |row| row.get(0),
                    ).optional()?;
                    let lease: i64 = tx.query_row(
                        "SELECT COALESCE(MAX(lease_until),0) FROM checkpoint_retention WHERE slug=?1 AND sha=?2",
                        params![slug, sha],
                        |row| row.get(0),
                    )?;
                    valid &= current.as_ref().is_some_and(|current| {
                        current.0 == *planned_seq
                            && current.1 == *planned_parent
                            && current.2 == *planned_tree_sha
                            && current.3 == *planned_at
                            && current.4 == *planned_label
                            && current.5 == *planned_why
                    }) && newest.as_deref() != Some(sha.as_str()) && lease <= now;
                }
                let target = crate::storage::catalog::pressure::hard_pressure_quota(&candidate_fingerprint);
                let usage = Self::account_storage_usage_on(tx, &account_id)?;
                let deficit = target
                    .map(|target| usage.charged_bytes.saturating_sub(target).max(0))
                    .unwrap_or(i64::MAX);
                let reclaim = if valid {
                    Self::reclaimable_checkpoint_bytes_on(tx, &all_keys)?
                } else {
                    0
                };
                if !valid || !usage.physical_accounting || deficit <= 0 || reclaim < deficit || reclaim <= 0 {
                    let blocked = tx.execute(
                        "UPDATE quota_retention_candidates SET status='blocked'
                         WHERE account_id=?1 AND generation=?2 AND status='pending'",
                        params![account_id, generation],
                    )?;
                    tx.execute(
                        "UPDATE quota_retention_jobs SET status='complete',completed_at=?3,updated_at=?3
                         WHERE account_id=?1 AND generation=?2",
                        params![account_id, generation, now],
                    )?;
                    return Ok(RetentionPass { generation, removed: Vec::new(), blocked });
                }
                let mut low = 1usize;
                let mut high = all_keys.len();
                while low < high {
                    let middle = low + (high - low) / 2;
                    if Self::reclaimable_checkpoint_bytes_on(tx, &all_keys[..middle])? >= deficit {
                        high = middle;
                    } else {
                        low = middle + 1;
                    }
                }
                rows.truncate(low);
            }
            let mut removed = Vec::new();
            let mut blocked = 0;
            for (slug, sha, planned_seq, _planned_parent, planned_tree_sha, planned_at, planned_label, planned_why) in &rows {
                let planned_parent: String = tx.query_row("SELECT planned_parent FROM quota_retention_candidates WHERE account_id=?1 AND generation=?2 AND slug=?3 AND sha=?4", params![account_id,generation,slug,sha], |row| row.get(0))?;
                let current: (i64, String, String, String, String, String) = tx.query_row(
                    "SELECT seq,parent,COALESCE(NULLIF(tree_sha,''),sha),at,label,why FROM checkpoints WHERE slug=?1 AND sha=?2",
                    params![slug,sha], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)))?;
                if current.0 != *planned_seq || current.1 != planned_parent || current.2 != *planned_tree_sha
                    || current.3 != *planned_at || current.4 != *planned_label || current.5 != *planned_why {
                    tx.execute("UPDATE quota_retention_candidates SET status='blocked' WHERE account_id=?1 AND generation=?2 AND slug=?3 AND sha=?4", params![account_id,generation,slug,sha])?;
                    blocked += 1;
                    continue;
                }
                let open_reference: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM comments WHERE slug=?1 AND resolved=0
                       AND (revision=?2 OR revision=?3))",
                    params![slug,sha,planned_tree_sha], |row| row.get(0))?;
                let current_count: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM checkpoints WHERE slug=?1", [slug], |row| row.get(0))?;
                let over_hard_count = hard_count_limit >= 0
                    && current_count > hard_count_limit.max(1);
                let milestone = (!planned_label.is_empty() && preferences.milestone_preferences.named)
                    || (preferences.milestone_preferences.cli && planned_why == "cli")
                    || (preferences.milestone_preferences.publish && planned_why == "publish")
                    || (preferences.milestone_preferences.restore && (planned_why == "restore" || planned_why == "restored"))
                    || (preferences.milestone_preferences.accept && planned_why == "accept")
                    || (preferences.milestone_preferences.comment && open_reference);
                // A hard deployment count is the one retention rule that can
                // override a routine milestone.  Without this exception a
                // milestone selected by the count evaluator would always be
                // blocked here, leaving the document permanently over cap.
                if milestone && !over_hard_count && pressure == 0 {
                    tx.execute("UPDATE quota_retention_candidates SET status='blocked' WHERE account_id=?1 AND generation=?2 AND slug=?3 AND sha=?4", params![account_id,generation,slug,sha])?;
                    blocked += 1;
                    continue;
                }
                let newest: Option<String> = tx.query_row(
                    "SELECT sha FROM checkpoints WHERE slug=?1 ORDER BY seq DESC LIMIT 1", [slug], |row| row.get(0)).optional()?;
                if newest.as_deref() == Some(sha) {
                    tx.execute("UPDATE quota_retention_candidates SET status='blocked' WHERE account_id=?1 AND generation=?2 AND slug=?3 AND sha=?4", params![account_id,generation,slug,sha])?;
                    blocked += 1;
                    continue;
                }
                let lease: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(lease_until),0) FROM checkpoint_retention WHERE slug=?1 AND sha=?2", params![slug,sha], |row| row.get(0))?;
                if lease > now {
                    blocked += 1;
                    continue;
                }
                let parent: String = tx.query_row("SELECT parent FROM checkpoints WHERE slug=?1 AND sha=?2", params![slug,sha], |row| row.get(0))?;
                if let Some((successor,)) = tx.query_row(
                    "SELECT sha FROM checkpoints WHERE slug=?1 AND parent=?2 ORDER BY seq LIMIT 1", params![slug,sha], |row| Ok((row.get::<_,String>(0)?,))).optional()? {
                    tx.execute("UPDATE checkpoints SET parent=?3,changed=NULL WHERE slug=?1 AND sha=?2", params![slug,successor,parent])?;
                    // Reparenting by this very job is not an external change
                    // that invalidates its next approved candidate.
                    tx.execute("UPDATE quota_retention_candidates SET planned_parent=?5 WHERE account_id=?1 AND generation=?2 AND slug=?3 AND sha=?4 AND planned_parent=?6", params![account_id,generation,slug,successor,parent,sha])?;
                    tx.execute("INSERT OR IGNORE INTO checkpoint_retention(slug,sha,original_parent,ancestry_gap) VALUES(?1,?2,?3,1)", params![slug,successor,sha])?;
                    tx.execute("UPDATE checkpoint_retention SET ancestry_gap=1 WHERE slug=?1 AND sha=?2", params![slug,successor])?;
                }
                let (storage_id, tree_bytes): (String, i64) = tx.query_row(
                    "SELECT d.storage_id,COALESCE((SELECT bytes FROM object_accounting o WHERE o.storage_id=d.storage_id AND o.object_key='content/'||d.storage_id||'/trees/'||c.sha),0) FROM documents d JOIN checkpoints c ON c.slug=d.slug
                     WHERE c.slug=?1 AND c.sha=?2", params![slug,sha],
                    |row| Ok((row.get(0)?, row.get(1)?)))?;
                tx.execute("DELETE FROM checkpoints WHERE slug=?1 AND sha=?2", params![slug,sha])?;
                // The event tree is immutable and event-addressed. Queue it
                // only after the catalogue row is gone; a failed physical
                // delete remains durable in pending_deletes.
                tx.execute(
                    "INSERT OR IGNORE INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after)
                     VALUES(?1,?2,?3,?4,?4)",
                    params![slug, crate::storage::blob::checkpoint_key(&storage_id, sha), tree_bytes, now],
                )?;
                tx.execute("UPDATE quota_retention_candidates SET status='removed' WHERE account_id=?1 AND generation=?2 AND slug=?3 AND sha=?4", params![account_id,generation,slug,sha])?;
                removed.push((slug.clone(),sha.clone()));
            }
            let pending: i64 = tx.query_row("SELECT COUNT(*) FROM quota_retention_candidates WHERE account_id=?1 AND generation=?2 AND status='pending'", params![account_id,generation], |row| row.get(0))?;
            if pending == 0 {
                let gc_pending: i64 = if pressure != 0 {
                    tx.query_row(
                        "SELECT COUNT(*) FROM pending_deletes p JOIN documents d ON d.slug=p.slug
                         WHERE d.owner_id=?1 AND d.status IN ('active','creating','deleting')",
                        [&account_id],
                        |row| row.get(0),
                    )?
                } else {
                    0
                };
                if gc_pending == 0 {
                    tx.execute("UPDATE quota_retention_jobs SET status='complete',completed_at=?3,updated_at=?3 WHERE account_id=?1 AND generation=?2", params![account_id,generation,now])?;
                }
            }
            Ok(RetentionPass { generation, removed, blocked })
        })
    }

    pub fn retention_job(
        &self,
        account_id: &str,
        generation: &str,
    ) -> CatalogResult<Option<RetentionJob>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT account_id,generation,revision,status,grace_until,candidate_fingerprint
             FROM quota_retention_jobs WHERE account_id=?1 AND generation=?2",
                    params![account_id, generation],
                    |row| {
                        Ok(RetentionJob {
                            account_id: row.get(0)?,
                            generation: row.get(1)?,
                            revision: row.get(2)?,
                            status: row.get(3)?,
                            grace_until: row.get(4)?,
                            candidate_fingerprint: row.get(5)?,
                        })
                    },
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn latest_retention_job(&self, account_id: &str) -> CatalogResult<Option<RetentionJob>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT account_id,generation,revision,status,grace_until,candidate_fingerprint
             FROM quota_retention_jobs WHERE account_id=?1
             ORDER BY revision DESC,created_at DESC,generation DESC LIMIT 1",
                    [account_id],
                    |row| {
                        Ok(RetentionJob {
                            account_id: row.get(0)?,
                            generation: row.get(1)?,
                            revision: row.get(2)?,
                            status: row.get(3)?,
                            grace_until: row.get(4)?,
                            candidate_fingerprint: row.get(5)?,
                        })
                    },
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn last_completed_retention_generation(
        &self,
        account_id: &str,
    ) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT generation FROM quota_retention_jobs
                 WHERE account_id=?1 AND status='complete'
                 ORDER BY revision DESC,created_at DESC,generation DESC LIMIT 1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_ownerless_document_prunes_without_an_account_job() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog
            .create_document(&NewDocument {
                slug: "visitor-doc".into(),
                storage_id: "visitor-storage".into(),
                title: "Visitor document".into(),
                sha: "document-sha".into(),
                created_at: "0".into(),
                published_at: "0".into(),
                updated_at: "0".into(),
                example: false,
                owner_key: "visitor-key".into(),
                owner_id: None,
                status: "active".into(),
                size: 1,
                counted_size: 1,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "README.md".into(),
            })
            .unwrap();
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO document_retention_policy(slug,mode,enrolled_at)
                 VALUES('visitor-doc','balanced',0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        for (seq, sha, parent) in [
            (0, "oldest", ""),
            (1, "middle", "oldest"),
            (2, "newest", "middle"),
        ] {
            catalog
                .insert_checkpoint(&Checkpoint {
                    slug: "visitor-doc".into(),
                    sha: sha.into(),
                    seq,
                    durable_seq: 0,
                    tree_sha: String::new(),
                    parent: parent.into(),
                    at: seq.to_string(),
                    by: String::new(),
                    by_account: None,
                    why: "auto".into(),
                    source_format: "markdown".into(),
                    size: 1,
                    label: String::new(),
                    git_commit: String::new(),
                    dirty: false,
                    changed: None,
                })
                .unwrap();
        }

        let removed = catalog
            .schedule_document_balanced(
                "visitor-doc",
                10_000_000,
                crate::document::quota::RetentionBounds::default(),
            )
            .unwrap();
        assert_eq!(removed, 2);
        assert!(catalog
            .checkpoint("visitor-doc", "oldest")
            .unwrap()
            .is_none());
        assert!(catalog
            .checkpoint("visitor-doc", "newest")
            .unwrap()
            .is_some());
        let ownerless_jobs: i64 = catalog
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM quota_retention_jobs WHERE account_id LIKE 'ownerless:%'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(ownerless_jobs, 0);
    }
}
