//! Advisory v2 retention over immutable checkpoint events.
//!
//! Retention candidates are deliberately not durable rows. A preview is a
//! bounded read and an apply rechecks labels, annotations, roots, grace, and
//! current policy in the final checkpoint transaction.

use super::*;
use std::collections::HashMap;

const RETENTION_GRACE_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentRetentionPayload {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    max_routine_count: Option<u32>,
    #[serde(default)]
    max_age_ms: Option<i64>,
    #[serde(default)]
    evaluation: Option<RetentionEvaluation>,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RetentionEvaluation {
    account_revision: i64,
    document_revision: i64,
    policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RetentionCandidate {
    id: String,
    current: bool,
    labelled: bool,
    protected: bool,
    routine_rank: Option<u32>,
    age_due: bool,
}

/// Calculate eligibility from one bounded checkpoint page.  This function is
/// deliberately independent of SQL so the worker and its adversarial tests
/// share the same rules: roots and protections are removed from the routine
/// count, and either configured limit may make a point eligible.
fn retention_candidates(
    checkpoints: &[RetentionCandidate],
    max_routine_count: Option<u32>,
) -> std::collections::BTreeSet<String> {
    checkpoints
        .iter()
        .filter(|point| {
            !point.current
                && !point.labelled
                && !point.protected
                && (point.age_due || point.routine_rank.is_some_and(|rank| {
                    max_routine_count.is_some_and(|limit| rank >= limit)
                }))
        })
        .map(|point| point.id.clone())
        .collect()
}

fn policy_fingerprint(
    mode: &str,
    account_revision: i64,
    document_revision: i64,
    profile: &str,
    max_routine_count: Option<u32>,
    max_age_ms: Option<i64>,
) -> String {
    format!(
        "v2:{mode}:{account_revision}:{document_revision}:{profile}:{}:{}",
        max_routine_count.map_or_else(|| "none".into(), |value| value.to_string()),
        max_age_ms.map_or_else(|| "none".into(), |value| value.to_string())
    )
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

fn retention_time(value: &str) -> i64 { value.parse().unwrap_or(0) }

impl Catalog {
    pub fn schedule_document_balanced(&self, slug: &str, now: i64, bounds: crate::document::quota::RetentionBounds) -> CatalogResult<usize> {
        if slug.is_empty() || now < 0 { return Err(CatalogError::Invalid("invalid retention schedule".into())); }
        self.immediate(|tx| {
            let (document_id, owner_id, mode, document_revision, retention_json): (String, String, String, i64, String) = tx
                .query_row(
                    "SELECT id,owner_id,retention_mode,retention_revision,retention_json
                     FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .map_err(CatalogError::from)?;

            let (account_revision, account_payload): (i64, String) = tx
                .query_row(
                    "SELECT preferences_revision,preferences_json FROM accounts WHERE id=?1",
                    [&owner_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(CatalogError::from)?;
            let account_preferences = if account_payload == r#"{"version":1}"# {
                crate::document::quota::QuotaPreferences::default()
            } else {
                serde_json::from_str(&account_payload).unwrap_or_else(|_| {
                    // A malformed saved preference is a safe read-only state:
                    // it must never turn into automatic history deletion.
                    crate::document::quota::QuotaPreferences {
                        version: 0,
                        retention_profile: "invalid".into(),
                        retention_policy_version: 0,
                        custom_retention: None,
                        display_timezone: "UTC".into(),
                        warning_thresholds: vec![1],
                    }
                })
            };
            let document_payload: DocumentRetentionPayload = serde_json::from_str(&retention_json)
                .unwrap_or_default();
            let document_override = document_payload.profile.is_some()
                || document_payload.max_routine_count.is_some()
                || document_payload.max_age_ms.is_some();
            let effective = crate::document::quota::effective_retention(&account_preferences, &bounds);
            let (profile, max_routine_count, max_age_ms, safe_mode) = if mode == "manual" {
                ("manual".to_string(), None, None, false)
            } else if document_override {
                let profile = document_payload.profile.clone().unwrap_or_else(|| "custom".into());
                let valid = matches!(profile.as_str(), "default" | "manual" | "custom")
                    && document_payload.max_routine_count.is_none_or(|value| value <= 4_096)
                    && document_payload.max_age_ms.is_none_or(|value| value >= 0);
                if !valid || profile == "manual" {
                    (profile, None, None, !valid)
                } else if profile == "default" {
                    (profile, effective.max_routine_count, effective.max_age_ms, effective.safe_mode)
                } else {
                    (
                        profile,
                        document_payload.max_routine_count.and_then(|value| {
                            bounds.max_routine_count.map_or(Some(value), |cap| Some(value.min(cap)))
                        }),
                        document_payload.max_age_ms.and_then(|value| {
                            bounds.max_age_ms.map_or(Some(value), |cap| Some(value.min(cap)))
                        }),
                        effective.safe_mode,
                    )
                }
            } else {
                (effective.profile.clone(), effective.max_routine_count, effective.max_age_ms, effective.safe_mode)
            };
            let fingerprint = policy_fingerprint(
                &mode,
                account_revision,
                document_revision,
                &profile,
                max_routine_count,
                max_age_ms,
            );
            let previous_evaluation = document_payload.evaluation.clone();
            let policy_changed = previous_evaluation.as_ref().map(|value| value.policy.as_str()) != Some(fingerprint.as_str())
                || previous_evaluation.as_ref().map(|value| value.account_revision) != Some(account_revision)
                || previous_evaluation.as_ref().map(|value| value.document_revision) != Some(document_revision);

            let mut rows = tx.prepare(
                "SELECT c.id,c.seq,c.label,c.created_at,c.eligible_after,
                    CASE WHEN c.id=d.current_checkpoint_id THEN 1 ELSE 0 END,
                    EXISTS(SELECT 1 FROM annotations a
                           WHERE a.document_id=c.document_id AND a.protected_checkpoint_id=c.id
                             AND a.resolved_at IS NULL)
                 FROM checkpoints c JOIN documents d ON d.id=c.document_id
                 WHERE c.document_id=?1 ORDER BY c.seq DESC LIMIT 4096",
            ).map_err(CatalogError::from)?;
            let rows = rows.query_map([&document_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, i64>(5)? != 0,
                    r.get::<_, bool>(6)?,
                ))
            }).map_err(CatalogError::from)?;
            let checkpoints: Vec<_> = rows.collect::<Result<Vec<_>, _>>().map_err(CatalogError::from)?;
            let mut routine_rank = 0u32;
            let mut candidates = Vec::with_capacity(checkpoints.len());
            for (id, _seq, label, created_at, _eligible_after, current, protected) in &checkpoints {
                let labelled = label.is_some();
                let rank = if !current && !labelled && !protected {
                    let rank = routine_rank;
                    routine_rank = routine_rank.checked_add(1).ok_or_else(|| CatalogError::Invalid("retention rank overflow".into()))?;
                    Some(rank)
                } else { None };
                candidates.push(RetentionCandidate {
                    id: id.clone(), current: *current, labelled, protected: *protected,
                    routine_rank: rank,
                    age_due: max_age_ms.is_some_and(|age| now.saturating_sub(*created_at) > age),
                });
            }
            let eligible = if safe_mode || profile == "manual" {
                std::collections::BTreeSet::new()
            } else {
                retention_candidates(&candidates, max_routine_count)
            };
            let due = now.checked_add(RETENTION_GRACE_MS).ok_or_else(|| CatalogError::Invalid("retention grace overflow".into()))?;
            let mut scheduled = 0usize;
            for (id, _seq, _label, _created_at, old_deadline, _current, _protected) in &checkpoints {
                if eligible.contains(id) {
                    let deadline = if policy_changed || old_deadline.is_none() { Some(due) } else { *old_deadline };
                    if old_deadline != &deadline {
                        tx.execute("UPDATE checkpoints SET eligible_after=?1 WHERE document_id=?2 AND id=?3", params![deadline, document_id, id]).map_err(CatalogError::from)?;
                        scheduled += 1;
                    }
                } else if old_deadline.is_some() {
                    tx.execute("UPDATE checkpoints SET eligible_after=NULL WHERE document_id=?1 AND id=?2", params![document_id, id]).map_err(CatalogError::from)?;
                }
            }
            let mut new_payload = document_payload;
            new_payload.version.get_or_insert(1);
            new_payload.evaluation = Some(RetentionEvaluation { account_revision, document_revision, policy: fingerprint });
            let new_json = serde_json::to_string(&new_payload).map_err(|e| CatalogError::Invalid(format!("retention payload: {e}")))?;
            let next_due: Option<i64> = tx.query_row("SELECT min(eligible_after) FROM checkpoints WHERE document_id=?1 AND eligible_after IS NOT NULL", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET retention_json=?1,retention_due_at=COALESCE(?2,0),updated_at=max(updated_at,?3) WHERE id=?4", params![new_json, next_due, now, document_id]).map_err(CatalogError::from)?;
            Ok(scheduled)
        })
    }

    pub fn retention_metadata_range(&self, slug: &str, first: i64, last: i64) -> CatalogResult<HashMap<String,(String,bool)>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,COALESCE(parent_id,''),0 FROM checkpoints c JOIN documents d ON d.id=c.document_id WHERE d.slug=?1 AND c.seq>=?2 AND c.seq<=?3") .map_err(CatalogError::from)?;
            let rows = statement.query_map(params![slug,first,last], |row| Ok((row.get(0)?,(row.get(1)?,row.get::<_,i64>(2)? != 0)))).map_err(CatalogError::from)?;
            rows.collect::<Result<HashMap<_,_>,_>>().map_err(CatalogError::from)
        })
    }

    pub fn checkpoint_retention_metadata(&self, slug: &str, sha: &str) -> CatalogResult<Option<(String,bool)>> {
        self.with_connection(|connection| connection.query_row("SELECT COALESCE(c.parent_id,''),0 FROM checkpoints c JOIN documents d ON d.id=c.document_id WHERE d.slug=?1 AND c.id=?2", params![slug,sha], |row| Ok((row.get(0)?,row.get::<_,i64>(1)? != 0))).optional().map_err(CatalogError::from))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_quota_preferences_and_schedule(&self, account_id: &str, expected_revision: i64, payload: &str, generation: &str, candidate_fingerprint: &str, _candidates: &[(String,String)], grace_until: i64, now: i64) -> CatalogResult<RetentionJob> {
        self.save_quota_preferences_and_schedule_with_hard_count_limit(account_id,expected_revision,payload,generation,candidate_fingerprint,&[],grace_until,now,None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_quota_preferences_and_schedule_with_hard_count_limit(&self, account_id: &str, expected_revision: i64, payload: &str, generation: &str, candidate_fingerprint: &str, _candidates: &[(String,String)], grace_until: i64, now: i64, _hard_count_limit: Option<u32>) -> CatalogResult<RetentionJob> {
        if generation.is_empty() || candidate_fingerprint.is_empty() || grace_until < now { return Err(CatalogError::Invalid("invalid retention application".into())); }
        let record = self.save_quota_preferences(account_id,expected_revision,payload,generation,now)?;
        let slugs: Vec<String> = self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT slug FROM documents WHERE owner_id=?1 AND status IN ('active','creating')").map_err(CatalogError::from)?;
            let rows = statement.query_map([account_id], |row| row.get(0)).map_err(CatalogError::from)?;
            rows.collect::<Result<Vec<_>,_>>().map_err(CatalogError::from)
        })?;
        for slug in slugs { let _ = self.schedule_document_balanced(&slug, now, crate::document::quota::RetentionBounds::default())?; }
        Ok(RetentionJob { account_id:account_id.into(),generation:generation.into(),revision:record.revision,status:"advisory".into(),grace_until,candidate_fingerprint:candidate_fingerprint.into() })
    }

    pub fn run_retention_pass(&self, now: i64, limit: u32) -> CatalogResult<RetentionPass> { self.run_retention_pass_with_limits(now,limit,None,None) }

    pub fn run_retention_pass_with_limits(&self, now: i64, limit: u32, _current_hard_quota: Option<i64>, _current_hard_count: Option<u32>) -> CatalogResult<RetentionPass> {
        if now < 0 { return Err(CatalogError::Invalid("negative retention time".into())); }
        // A retention transaction is bounded to the normative 32 checkpoint
        // rows.  Closure edges are checked before each delete and stop the
        // pass before the 32,768-row transaction budget can be exceeded.
        let limit = limit.clamp(1, 32) as i64;
        let candidates: Vec<(String,String,String,i64)> = self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT c.document_id,c.id,d.slug,(SELECT count(*) FROM checkpoint_objects co WHERE co.document_id=c.document_id AND co.checkpoint_id=c.id) FROM checkpoints c JOIN documents d ON d.id=c.document_id WHERE d.status='active' AND c.eligible_after IS NOT NULL AND c.eligible_after<=?1 AND c.label IS NULL ORDER BY c.eligible_after,c.document_id,c.seq LIMIT ?2").map_err(CatalogError::from)?;
            let rows = statement.query_map(params![now,limit], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(CatalogError::from)?;
            rows.collect::<Result<Vec<_>,_>>().map_err(CatalogError::from)
        })?;
        let mut removed = Vec::new();
        let mut blocked = 0usize;
        let mut edges = 0i64;
        for (document_id,checkpoint_id,slug,count) in candidates {
            if edges.checked_add(count).ok_or_else(|| CatalogError::Invalid("retention edge counter overflow".into()))? > 32_768 { break; }
            let doc = DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            let checkpoint = CheckpointId::new(checkpoint_id).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if self.delete_v2_checkpoint(&doc,&checkpoint,UnixMillis::new(now)?)? { edges += count; removed.push((slug,checkpoint.to_string())); } else { blocked += 1; }
        }
        Ok(RetentionPass { generation:format!("retention:{now}"),removed,blocked })
    }

    pub fn retention_job(&self, _account_id: &str, _generation: &str) -> CatalogResult<Option<RetentionJob>> { Err(CatalogError::Invalid("durable retention jobs were removed in catalog v2".into())) }
    pub fn latest_retention_job(&self, _account_id: &str) -> CatalogResult<Option<RetentionJob>> { Err(CatalogError::Invalid("durable retention jobs were removed in catalog v2".into())) }
    pub fn last_completed_retention_generation(&self, _account_id: &str) -> CatalogResult<Option<String>> { Err(CatalogError::Invalid("durable retention jobs were removed in catalog v2".into())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: &str, rank: Option<u32>, age_due: bool) -> RetentionCandidate {
        RetentionCandidate {
            id: id.into(),
            current: false,
            labelled: false,
            protected: false,
            routine_rank: rank,
            age_due,
        }
    }

    #[test]
    fn routine_limit_counts_only_unprotected_unlabelled_points() {
        let mut labelled = point("label", Some(0), false);
        labelled.labelled = true;
        let mut protected = point("protected", Some(0), false);
        protected.protected = true;
        let mut current = point("current", Some(0), true);
        current.current = true;
        let eligible = retention_candidates(
            &[labelled, protected, current, point("newest", Some(0), false), point("old", Some(1), false)],
            Some(1),
        );
        assert_eq!(eligible, std::collections::BTreeSet::from(["old".into()]));
    }

    #[test]
    fn age_is_an_independent_candidate_bound() {
        let eligible = retention_candidates(&[point("fresh", Some(0), false), point("old", Some(0), true)], Some(50));
        assert_eq!(eligible, std::collections::BTreeSet::from(["old".into()]));
    }

    #[test]
    fn policy_fingerprint_changes_when_account_revision_changes() {
        assert_ne!(
            policy_fingerprint("balanced", 1, 4, "default", Some(50), Some(30)),
            policy_fingerprint("balanced", 2, 4, "default", Some(50), Some(30)),
        );
    }
}
