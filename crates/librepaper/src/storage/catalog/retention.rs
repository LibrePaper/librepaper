//! Advisory v2 retention over immutable checkpoint events.
//!
//! Retention candidates are deliberately not durable rows. A preview is a
//! bounded read and an apply rechecks labels, annotations, roots, grace, and
//! current policy in the final checkpoint transaction.

use super::*;
use std::collections::HashMap;

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
            let (document_id, mode): (String,String) = tx.query_row("SELECT id,retention_mode FROM documents WHERE slug=?1 AND status='active'", [slug], |r| Ok((r.get(0)?,r.get(1)?))).map_err(CatalogError::from)?;
            if mode == "manual" { return Ok(0); }
            let max_count = i64::from(bounds.max_routine_count.unwrap_or(50));
            let mut rows = tx.prepare("SELECT id,seq,label,created_at FROM checkpoints WHERE document_id=?1 ORDER BY seq DESC").map_err(CatalogError::from)?;
            let checkpoints: Vec<(String,i64,Option<String>,i64)> = rows.query_map([&document_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(CatalogError::from)?.collect::<Result<Vec<_>,_>>().map_err(CatalogError::from)?;
            let mut scheduled = 0usize;
            for (index,(checkpoint_id,_seq,label,created_at)) in checkpoints.iter().enumerate() {
                let age_due = bounds.max_age_ms.is_some_and(|age| now.saturating_sub(*created_at) >= age);
                if (!age_due && index < max_count as usize) || label.is_some() { continue; }
                let protected: i64 = tx.query_row("SELECT count(*) FROM annotations WHERE document_id=?1 AND protected_checkpoint_id=?2", params![document_id,checkpoint_id], |r| r.get(0)).map_err(CatalogError::from)?;
                if protected != 0 { continue; }
                let due = now.checked_add(900_000).ok_or_else(|| CatalogError::Invalid("retention grace overflow".into()))?;
                tx.execute("UPDATE checkpoints SET eligible_after=CASE WHEN eligible_after IS NULL OR eligible_after<?1 THEN ?1 ELSE eligible_after END WHERE document_id=?2 AND id=?3", params![due,document_id,checkpoint_id]).map_err(CatalogError::from)?;
                scheduled += 1;
            }
            let next_due: Option<i64> = tx.query_row("SELECT min(eligible_after) FROM checkpoints WHERE document_id=?1 AND eligible_after IS NOT NULL", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET retention_due_at=COALESCE(?1,0),retention_revision=retention_revision+1,updated_at=max(updated_at,?2) WHERE id=?3", params![next_due,now,document_id]).map_err(CatalogError::from)?;
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
        let limit = limit.clamp(1, 256) as i64;
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
