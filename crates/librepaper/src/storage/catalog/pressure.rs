//! Hard-quota pressure planning.
//!
//! Pressure is a planning operation, not a promise that bytes have already
//! been freed.  In particular, rows in `pending_deletes` remain charged until
//! the object store confirms deletion.  The planner therefore only emits a
//! candidate set whose *cumulative* unique reclaim reaches the current
//! deficit, and refuses to invent reclaim from logical tree sizes.

use super::*;
use std::collections::BTreeMap;

const HARD_PRESSURE_PREFIX: &str = "hard-pressure:";
const MAX_PRESSURE_CANDIDATES: usize = 50_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardPressurePlan {
    pub account_id: String,
    pub deficit: i64,
    pub reclaimable_bytes: i64,
    pub routine: Vec<(String, String)>,
    pub protected: Vec<(String, String)>,
    pub feasible: bool,
}

impl HardPressurePlan {
    pub fn candidates(&self) -> Vec<(String, String)> {
        self.routine
            .iter()
            .chain(self.protected.iter())
            .cloned()
            .collect()
    }
}

fn event_time(value: &str) -> i64 {
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

/// Find the shortest deterministic prefix whose union reclaim reaches the
/// deficit.  Reclaim is monotonic as candidates are added, so binary search
/// keeps a pathological history from turning pressure handling into an
/// unbounded O(n²) query loop.  The callback must count pending/deferred
/// deletion rows as still charged.
fn minimal_prefix<F>(
    candidates: &[(String, String)],
    deficit: i64,
    mut reclaim: F,
) -> CatalogResult<(Vec<(String, String)>, i64)>
where
    F: FnMut(&[(String, String)]) -> CatalogResult<i64>,
{
    if deficit <= 0 {
        return Ok((Vec::new(), 0));
    }
    if candidates.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let mut low = 1usize;
    let mut high = candidates.len();
    let all = reclaim(candidates)?;
    if all < deficit {
        return Ok((candidates.to_vec(), all.max(0)));
    }
    while low < high {
        let middle = low + (high - low) / 2;
        let amount = reclaim(&candidates[..middle])?;
        if amount >= deficit {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let selected = candidates[..low].to_vec();
    let amount = reclaim(&selected)?.max(0);
    Ok((selected, amount))
}

impl Catalog {
    /// Return a bounded, deterministic page of documents with superseded
    /// rendering registrations. The caller owns the cursor so cold-document
    /// cleanup can make progress without loading the whole catalogue.
    pub fn stale_rendering_slugs(
        &self,
        after_slug: &str,
        limit: u32,
    ) -> CatalogResult<Vec<String>> {
        let limit = i64::from(limit.clamp(1, 64));
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT d.slug
                   FROM documents d JOIN renderings r ON r.slug=d.slug
                  WHERE d.status IN ('active','creating') AND d.slug>?1
                  GROUP BY d.slug
                 HAVING COUNT(*)>1
                  ORDER BY d.slug LIMIT ?2",
            )?;
            let rows = statement.query_map(params![after_slug, limit], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()
                .map_err(CatalogError::from)
        })
    }

    /// Plan hard-quota eviction account-wide.  Routine candidates are
    /// exhausted before preferentially protected history is considered; the
    /// newest checkpoint, live roots and leased objects are excluded by the
    /// candidate/reference accounting path.
    pub fn plan_hard_pressure(
        &self,
        account_id: &str,
        hard_quota: i64,
        now: i64,
    ) -> CatalogResult<HardPressurePlan> {
        self.plan_hard_pressure_for_growth(account_id, hard_quota, 0, now)
    }

    /// Plan enough reclaim for the current charge plus a prospective write.
    /// The write is intentionally not reserved or charged by this planner;
    /// its admission transaction remains authoritative after GC succeeds.
    pub fn plan_hard_pressure_for_growth(
        &self,
        account_id: &str,
        hard_quota: i64,
        required_growth: i64,
        now: i64,
    ) -> CatalogResult<HardPressurePlan> {
        if account_id.is_empty() || hard_quota < 0 || required_growth < 0 || now < 0 {
            return Err(CatalogError::Invalid(
                "invalid hard pressure request".into(),
            ));
        }
        let usage = self.account_storage_usage(account_id)?;
        let deficit = usage
            .charged_bytes
            .saturating_add(required_growth)
            .saturating_sub(hard_quota)
            .max(0);
        let mut plan = HardPressurePlan {
            account_id: account_id.to_owned(),
            deficit,
            reclaimable_bytes: 0,
            routine: Vec::new(),
            protected: Vec::new(),
            feasible: deficit == 0,
        };
        let pressure_in_flight: bool = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM quota_retention_jobs
                     WHERE account_id=?1 AND pressure=1
                       AND status IN ('pending','grace','running'))
                       OR EXISTS(SELECT 1 FROM pending_deletes p
                                 JOIN documents d ON d.slug=p.slug
                                 WHERE d.owner_id=?1)",
                    [account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })?;
        if pressure_in_flight {
            // Do not stack another destructive snapshot while prior source
            // or artifact deletion is still charged. Admission callers
            // receive a retryable pressure response instead.
            return Ok(plan);
        }
        if deficit == 0 || !usage.physical_accounting {
            // A legacy/unverified physical ledger is a conservative admission
            // guard, never evidence on which destructive byte pruning may be
            // based.
            return Ok(plan);
        }

        let preferences = self
            .quota_preferences(account_id)?
            .and_then(|record| serde_json::from_str(&record.payload).ok())
            .unwrap_or_default();
        let references = self.account_open_annotation_references(account_id)?;
        let points = self.account_checkpoints(account_id)?;
        let mut by_document = BTreeMap::<String, Vec<_>>::new();
        for point in points {
            by_document
                .entry(point.slug.clone())
                .or_default()
                .push(point);
        }

        let mut routine = Vec::<(String, String, i64, i64)>::new();
        let mut protected = Vec::<(String, String, i64, i64)>::new();
        for (slug, mut document_points) in by_document {
            if document_points.is_empty() {
                continue;
            }
            document_points
                .sort_by_key(|point| (point.seq, event_time(&point.at), point.sha.clone()));
            let document_points =
                crate::document::history::Manifest::from_catalog_rows(document_points)
                    .map_err(CatalogError::Invalid)?
                    .checkpoints;
            let refs = references.get(&slug).cloned().unwrap_or_default();
            let decision = crate::document::quota::select_retained(
                &document_points,
                now,
                &preferences,
                &crate::document::quota::RetentionBounds {
                    hard_quota,
                    ..Default::default()
                },
                &refs,
            );
            let newest = document_points
                .iter()
                .max_by_key(|point| (event_time(&point.at), point.seq, point.sha.clone()))
                .map(|point| point.sha.clone());
            for point in document_points {
                if newest.as_deref() == Some(point.sha.as_str()) {
                    continue;
                }
                let key = (slug.clone(), point.sha.clone());
                let order = (event_time(&point.at), point.seq);
                if decision.protected.contains(&point.sha) {
                    protected.push((key.0, key.1, order.0, order.1));
                } else {
                    // This includes both age-thinned and currently retained
                    // routine winners.  Hard pressure is global and must be
                    // able to thin the latter after the former are exhausted.
                    routine.push((key.0, key.1, order.0, order.1));
                }
            }
        }
        routine.sort_by_key(|(slug, sha, at, seq)| (*at, *seq, slug.clone(), sha.clone()));
        protected.sort_by_key(|(slug, sha, at, seq)| (*at, *seq, slug.clone(), sha.clone()));
        if routine.len().saturating_add(protected.len()) > MAX_PRESSURE_CANDIDATES {
            // A bounded planner must not silently select only a prefix and
            // claim the account is feasible.  Let the caller retry after an
            // operator-sized history thinning pass.
            return Ok(plan);
        }
        let routine_keys: Vec<_> = routine.into_iter().map(|(s, h, _, _)| (s, h)).collect();
        // Older renderings are a cheaper, regenerable pressure tier than any
        // protected source event.  Until the publication collector has
        // retired them, defer protected-history eviction rather than
        // violating that order.  The latest successful bundle is never an
        // eligible pressure candidate.
        let stale_renderings: i64 = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM renderings r JOIN documents d ON d.slug=r.slug
                     WHERE d.owner_id=?1 AND d.status IN ('active','creating')
                       AND r.bytes>0
                       AND r.published_seq < (SELECT COALESCE(MAX(r2.published_seq),0)
                                              FROM renderings r2 WHERE r2.slug=r.slug)",
                    [account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })?;
        if stale_renderings > 0 {
            // Derived publication cleanup is a mandatory tier before any
            // source checkpoint eviction. The room retention hook can verify
            // that a latest PDF is physically available before retiring the
            // superseded registration; this catalogue-only planner cannot.
            // Defer rather than silently deleting source history first.
            return Ok(plan);
        }
        let protected_keys: Vec<_> = if stale_renderings == 0 {
            protected.into_iter().map(|(s, h, _, _)| (s, h)).collect()
        } else {
            Vec::new()
        };
        let (selected_routine, routine_reclaim) = minimal_prefix(&routine_keys, deficit, |set| {
            self.reclaimable_checkpoint_bytes(set)
        })?;
        if routine_reclaim >= deficit {
            plan.reclaimable_bytes = routine_reclaim;
            plan.routine = selected_routine;
            plan.feasible = routine_reclaim > 0;
            return Ok(plan);
        }
        let mut all = routine_keys.clone();
        all.extend(protected_keys.iter().cloned());
        let (selected, total_reclaim) =
            minimal_prefix(&all, deficit, |set| self.reclaimable_checkpoint_bytes(set))?;
        plan.reclaimable_bytes = total_reclaim;
        let routine_len = routine_keys.len();
        if selected.len() > routine_len {
            plan.routine = routine_keys;
            plan.protected = selected[routine_len..].to_vec();
        } else {
            plan.routine = selected;
        }
        plan.feasible = total_reclaim >= deficit && total_reclaim > 0;
        Ok(plan)
    }

    /// Enqueue an immediate, idempotent hard-pressure job.  The job stores a
    /// candidate snapshot, while the retention worker rechecks ownership and
    /// leases before every delete.  No quota bytes are released here.
    pub fn schedule_hard_pressure(
        &self,
        account_id: &str,
        hard_quota: i64,
        now: i64,
    ) -> CatalogResult<Option<RetentionJob>> {
        self.schedule_hard_pressure_for_growth(account_id, hard_quota, 0, now)
    }

    pub fn schedule_hard_pressure_for_growth(
        &self,
        account_id: &str,
        hard_quota: i64,
        required_growth: i64,
        now: i64,
    ) -> CatalogResult<Option<RetentionJob>> {
        self.schedule_hard_pressure_for_growth_with_limits(
            account_id,
            hard_quota,
            required_growth,
            None,
            now,
        )
    }

    pub fn schedule_hard_pressure_for_growth_with_limits(
        &self,
        account_id: &str,
        hard_quota: i64,
        required_growth: i64,
        hard_count_limit: Option<u32>,
        now: i64,
    ) -> CatalogResult<Option<RetentionJob>> {
        let plan =
            self.plan_hard_pressure_for_growth(account_id, hard_quota, required_growth, now)?;
        if !plan.feasible || plan.candidates().is_empty() {
            return Ok(None);
        }
        let mut digest = sha2::Sha256::new();
        digest.update(account_id.as_bytes());
        digest.update(hard_quota.to_be_bytes());
        for (slug, sha) in plan.candidates() {
            digest.update(slug.as_bytes());
            digest.update(sha.as_bytes());
        }
        let generation = hex::encode(digest.finalize());
        let target = hard_quota.saturating_sub(required_growth).max(0);
        let count = hard_count_limit.map(i64::from).unwrap_or(-1);
        let generation_key =
            format!("{HARD_PRESSURE_PREFIX}{target}:{hard_quota}:{count}:{generation}");
        let payload = serde_json::to_string(&crate::document::quota::QuotaPreferences::default())
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        let candidates = plan.candidates();
        self.immediate(|tx| {
            let exists: Option<(i64, String, i64)> = tx
                .query_row(
                    "SELECT revision,status,grace_until FROM quota_retention_jobs
                     WHERE account_id=?1 AND generation=?2",
                    params![account_id, generation_key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            if let Some((revision, status, grace_until)) = exists {
                return Ok(Some(RetentionJob {
                    account_id: account_id.into(),
                    generation: generation_key,
                    revision,
                    status,
                    grace_until,
                    candidate_fingerprint: format!(
                        "{HARD_PRESSURE_PREFIX}{target}:{hard_quota}:{count}:{generation}"
                    ),
                }));
            }
            let revision: i64 = tx
                .query_row(
                    "SELECT revision FROM account_quota_preferences WHERE account_id=?1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            let fingerprint =
                format!("{HARD_PRESSURE_PREFIX}{target}:{hard_quota}:{count}:{generation}");
            tx.execute(
                "INSERT INTO quota_retention_jobs
                 (account_id,generation,revision,payload,candidate_fingerprint,
                  hard_count_limit,pressure,status,grace_until,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,1,'pending',?7,?7,?7)",
                params![
                    account_id,
                    generation_key,
                    revision,
                    payload,
                    fingerprint,
                    count,
                    now
                ],
            )?;
            for (ordinal, (slug, sha)) in candidates.iter().enumerate() {
                let Some(row) = tx
                    .query_row(
                        "SELECT seq,parent,COALESCE(NULLIF(tree_sha,''),sha),at,label,why
                         FROM checkpoints c JOIN documents d ON d.slug=c.slug
                         WHERE c.slug=?1 AND c.sha=?2 AND d.owner_id=?3
                           AND d.status IN ('active','creating')",
                        params![slug, sha, account_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, String>(5)?,
                            ))
                        },
                    )
                    .optional()?
                else {
                    continue;
                };
                tx.execute(
                    "INSERT OR IGNORE INTO quota_retention_candidates
                     (account_id,generation,slug,sha,planned_seq,planned_parent,
                      planned_tree_sha,planned_at,planned_label,planned_why,
                      grace_until,pressure_class,pressure_ordinal,status)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'pending')",
                    params![
                        account_id,
                        generation_key,
                        slug,
                        sha,
                        row.0,
                        row.1,
                        row.2,
                        row.3,
                        row.4,
                        row.5,
                        now,
                        if ordinal < plan.routine.len() { 0 } else { 1 },
                        if ordinal < plan.routine.len() {
                            ordinal
                        } else {
                            ordinal - plan.routine.len()
                        }
                    ],
                )?;
            }
            Ok(Some(RetentionJob {
                account_id: account_id.into(),
                generation: generation_key,
                revision,
                status: "pending".into(),
                grace_until: now,
                candidate_fingerprint: fingerprint,
            }))
        })
    }

    /// Convenience hook for admission paths which only have a document slug.
    pub fn schedule_hard_pressure_for_slug(
        &self,
        slug: &str,
        hard_quota: i64,
        now: i64,
    ) -> CatalogResult<Option<RetentionJob>> {
        self.schedule_hard_pressure_for_slug_for_growth(slug, hard_quota, 0, now)
    }

    pub fn schedule_hard_pressure_for_slug_for_growth(
        &self,
        slug: &str,
        hard_quota: i64,
        required_growth: i64,
        now: i64,
    ) -> CatalogResult<Option<RetentionJob>> {
        self.schedule_hard_pressure_for_slug_for_growth_with_limits(
            slug,
            hard_quota,
            required_growth,
            None,
            now,
        )
    }

    pub fn schedule_hard_pressure_for_slug_for_growth_with_limits(
        &self,
        slug: &str,
        hard_quota: i64,
        required_growth: i64,
        hard_count_limit: Option<u32>,
        now: i64,
    ) -> CatalogResult<Option<RetentionJob>> {
        let owner: Option<String> = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT owner_id FROM documents WHERE slug=?1 AND owner_id IS NOT NULL",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })?;
        match owner {
            Some(owner) => self.schedule_hard_pressure_for_growth_with_limits(
                &owner,
                hard_quota,
                required_growth,
                hard_count_limit,
                now,
            ),
            None => Ok(None),
        }
    }
}

pub(crate) fn hard_pressure_quota(value: &str) -> Option<i64> {
    value
        .strip_prefix(HARD_PRESSURE_PREFIX)?
        .split_once(':')?
        .0
        .parse()
        .ok()
}

pub(crate) fn hard_pressure_original_quota(value: &str) -> Option<i64> {
    value
        .strip_prefix(HARD_PRESSURE_PREFIX)?
        .split(':')
        .nth(1)?
        .parse()
        .ok()
}

pub(crate) fn hard_pressure_count(value: &str) -> Option<i64> {
    value
        .strip_prefix(HARD_PRESSURE_PREFIX)?
        .split(':')
        .nth(2)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::minimal_prefix;
    use crate::storage::catalog::{Account, Checkpoint, NewDocument};

    #[test]
    fn minimal_prefix_is_bounded_and_keeps_shared_union() {
        let candidates = vec![("d".into(), "a".into()), ("d".into(), "b".into())];
        let (selected, bytes) = minimal_prefix(&candidates, 10, |set| {
            Ok(if set.len() == 1 { 0 } else { 12 })
        })
        .unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(bytes, 12);
    }

    #[test]
    fn catalog_pressure_includes_prospective_growth_in_deficit() {
        let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
        catalog
            .upsert_account(&Account {
                id: "acct-pressure".into(),
                provider: "test".into(),
                handle: "pressure".into(),
                name: String::new(),
                email: String::new(),
                first_seen: "0".into(),
                last_seen: "0".into(),
                plan: "test".into(),
                status: "active".into(),
                session_generation: "generation".into(),
                erasure_cursor: None,
            })
            .unwrap();
        catalog
            .create_document(&NewDocument {
                slug: "pressure-doc".into(),
                storage_id: "pressure-storage".into(),
                title: "Pressure".into(),
                sha: "head".into(),
                created_at: "0".into(),
                published_at: "0".into(),
                updated_at: "0".into(),
                example: false,
                owner_key: String::new(),
                owner_id: Some("acct-pressure".into()),
                status: "active".into(),
                size: 1,
                counted_size: 1,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "README.md".into(),
            })
            .unwrap();
        for (seq, sha) in [(0, "old"), (1, "new")] {
            catalog
                .insert_checkpoint(&Checkpoint {
                    slug: "pressure-doc".into(),
                    sha: sha.into(),
                    seq,
                    durable_seq: seq,
                    tree_sha: sha.into(),
                    parent: String::new(),
                    at: seq.to_string(),
                    by: String::new(),
                    by_account: None,
                    why: "automatic".into(),
                    source_format: "markdown".into(),
                    size: 1,
                    label: String::new(),
                    git_commit: String::new(),
                    dirty: false,
                    changed: None,
                })
                .unwrap();
        }
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version)
                     VALUES(?1,?2,'checkpoint_tree',100,'test'),
                           (?1,?3,'checkpoint_tree',100,'test')",
                    rusqlite::params![
                        "pressure-storage",
                        crate::storage::blob::checkpoint_key("pressure-storage", "old"),
                        crate::storage::blob::checkpoint_key("pressure-storage", "new")
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let usage = catalog.account_storage_usage("acct-pressure").unwrap();
        let plan = catalog
            .plan_hard_pressure_for_growth("acct-pressure", usage.charged_bytes, 50, 100)
            .unwrap();
        assert_eq!(plan.deficit, 50);
        assert!(plan.feasible);
        assert_eq!(plan.protected, Vec::<(String, String)>::new());
        assert_eq!(plan.routine, vec![("pressure-doc".into(), "old".into())]);
    }

    #[test]
    fn pressure_job_is_stale_when_operator_quota_changes() {
        let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
        catalog
            .upsert_account(&Account {
                id: "acct-stale".into(),
                provider: "test".into(),
                handle: "stale".into(),
                name: String::new(),
                email: String::new(),
                first_seen: "0".into(),
                last_seen: "0".into(),
                plan: "test".into(),
                status: "active".into(),
                session_generation: "generation".into(),
                erasure_cursor: None,
            })
            .unwrap();
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO quota_retention_jobs
                     (account_id,generation,revision,payload,candidate_fingerprint,
                      hard_count_limit,pressure,status,grace_until,created_at,updated_at)
                     VALUES('acct-stale','hard-pressure:0:100:2:gen',0,?1,
                            'hard-pressure:0:100:2:gen',2,1,'pending',0,0,0)",
                    [
                        serde_json::to_string(&crate::document::quota::QuotaPreferences::default())
                            .unwrap(),
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let pass = catalog
            .run_retention_pass_with_limits(1, 10, Some(200), Some(3))
            .unwrap();
        assert_eq!(pass.removed.len(), 0);
        assert_eq!(
            catalog
                .retention_job("acct-stale", "hard-pressure:0:100:2:gen")
                .unwrap()
                .unwrap()
                .status,
            "stale"
        );
    }

    #[test]
    fn stale_renderings_defer_source_pressure_until_cold_cleanup() {
        let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
        catalog
            .upsert_account(&Account {
                id: "acct-rendering-pressure".into(),
                provider: "test".into(),
                handle: "rendering-pressure".into(),
                name: String::new(),
                email: String::new(),
                first_seen: "0".into(),
                last_seen: "0".into(),
                plan: "test".into(),
                status: "active".into(),
                session_generation: "generation".into(),
                erasure_cursor: None,
            })
            .unwrap();
        catalog
            .create_document(&NewDocument {
                slug: "rendering-pressure-doc".into(),
                storage_id: "rendering-pressure-storage".into(),
                title: "Pressure".into(),
                sha: "head".into(),
                created_at: "0".into(),
                published_at: "0".into(),
                updated_at: "0".into(),
                example: false,
                owner_key: String::new(),
                owner_id: Some("acct-rendering-pressure".into()),
                status: "active".into(),
                size: 1,
                counted_size: 1,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "README.md".into(),
            })
            .unwrap();
        for (seq, sha) in [(0, "old"), (1, "new")] {
            catalog
                .insert_checkpoint(&Checkpoint {
                    slug: "rendering-pressure-doc".into(),
                    sha: sha.into(),
                    seq,
                    durable_seq: seq,
                    tree_sha: sha.into(),
                    parent: String::new(),
                    at: seq.to_string(),
                    by: String::new(),
                    by_account: None,
                    why: "automatic".into(),
                    source_format: "markdown".into(),
                    size: 1,
                    label: String::new(),
                    git_commit: String::new(),
                    dirty: false,
                    changed: None,
                })
                .unwrap();
        }
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version)
                     VALUES(?1,?2,'checkpoint_tree',100,'test'),
                           (?1,?3,'checkpoint_tree',100,'test')",
                    rusqlite::params![
                        "rendering-pressure-storage",
                        crate::storage::blob::checkpoint_key("rendering-pressure-storage", "old"),
                        crate::storage::blob::checkpoint_key("rendering-pressure-storage", "new")
                    ],
                )?;
                connection.execute(
                    "INSERT INTO renderings
                     (slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes,published_seq)
                     VALUES(?1,'old','0','test','','','','10',0,0,1),
                           (?1,'new','1','test','','','','10',0,0,2)",
                    ["rendering-pressure-doc"],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            catalog.stale_rendering_slugs("", 8).unwrap(),
            vec!["rendering-pressure-doc".to_string()]
        );
        let usage = catalog
            .account_storage_usage("acct-rendering-pressure")
            .unwrap();
        let plan = catalog
            .plan_hard_pressure_for_growth("acct-rendering-pressure", usage.charged_bytes, 50, 100)
            .unwrap();
        assert!(!plan.feasible);
        assert!(plan.routine.is_empty());
        assert!(plan.protected.is_empty());
    }
}
