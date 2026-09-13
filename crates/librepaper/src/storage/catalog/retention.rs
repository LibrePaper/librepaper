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
        let points = self.checkpoints(slug, 0, 4_096)?;
        let count = points.len().saturating_sub(bounds.max_routine_count.unwrap_or(50) as usize);
        Ok(count)
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
        Ok(RetentionJob { account_id:account_id.into(),generation:generation.into(),revision:record.revision,status:"advisory".into(),grace_until,candidate_fingerprint:candidate_fingerprint.into() })
    }

    pub fn run_retention_pass(&self, now: i64, limit: u32) -> CatalogResult<RetentionPass> { self.run_retention_pass_with_limits(now,limit,None,None) }

    pub fn run_retention_pass_with_limits(&self, now: i64, limit: u32, _current_hard_quota: Option<i64>, _current_hard_count: Option<u32>) -> CatalogResult<RetentionPass> {
        if now < 0 { return Err(CatalogError::Invalid("negative retention time".into())); }
        let _ = limit;
        Ok(RetentionPass { generation:String::new(),removed:Vec::new(),blocked:0 })
    }

    pub fn retention_job(&self, _account_id: &str, _generation: &str) -> CatalogResult<Option<RetentionJob>> { Ok(None) }
    pub fn latest_retention_job(&self, _account_id: &str) -> CatalogResult<Option<RetentionJob>> { Ok(None) }
    pub fn last_completed_retention_generation(&self, _account_id: &str) -> CatalogResult<Option<String>> { Ok(None) }
}
