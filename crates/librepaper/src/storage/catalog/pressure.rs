//! Hard quota admission.
//!
//! v2 never schedules a destructive pressure eviction to make a new write
//! fit. Retention is an independent policy worker, and an allocation that
//! exceeds the maintained counters receives a typed refusal.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardPressurePlan {
    pub account_id: String,
    pub deficit: i64,
    pub reclaimable_bytes: i64,
    pub routine: Vec<(String,String)>,
    pub protected: Vec<(String,String)>,
    pub feasible: bool,
}

impl HardPressurePlan { pub fn candidates(&self) -> Vec<(String,String)> { self.routine.iter().chain(self.protected.iter()).cloned().collect() } }

impl Catalog {
    pub fn plan_hard_pressure(&self, account_id: &str, hard_quota: i64, now: i64) -> CatalogResult<HardPressurePlan> { self.plan_hard_pressure_for_growth(account_id,hard_quota,0,now) }

    pub fn plan_hard_pressure_for_growth(&self, account_id: &str, hard_quota: i64, required_growth: i64, now: i64) -> CatalogResult<HardPressurePlan> {
        if account_id.is_empty() || hard_quota < 0 || required_growth < 0 || now < 0 { return Err(CatalogError::Invalid("invalid hard pressure request".into())); }
        let usage = self.account_storage_usage(account_id)?;
        let deficit = usage.charged_bytes.checked_add(required_growth).ok_or_else(|| CatalogError::Invalid("quota arithmetic overflow".into()))?.saturating_sub(hard_quota).max(0);
        Ok(HardPressurePlan { account_id:account_id.into(),deficit,reclaimable_bytes:0,routine:Vec::new(),protected:Vec::new(),feasible:deficit==0 })
    }

    pub fn schedule_hard_pressure(&self, account_id: &str, hard_quota: i64, now: i64) -> CatalogResult<Option<RetentionJob>> { self.schedule_hard_pressure_for_growth(account_id,hard_quota,0,now) }
    pub fn schedule_hard_pressure_for_growth(&self, account_id: &str, hard_quota: i64, required_growth: i64, now: i64) -> CatalogResult<Option<RetentionJob>> { self.schedule_hard_pressure_for_growth_with_limits(account_id,hard_quota,required_growth,None,now) }
    pub fn schedule_hard_pressure_for_growth_with_limits(&self, account_id: &str, hard_quota: i64, required_growth: i64, _hard_count_limit: Option<u32>, now: i64) -> CatalogResult<Option<RetentionJob>> {
        let plan = self.plan_hard_pressure_for_growth(account_id,hard_quota,required_growth,now)?;
        if plan.deficit > 0 { return Err(CatalogError::refused(CatalogRefusal::OwnerBytes,"hard quota exceeded; protected history is not evicted automatically")); }
        Ok(None)
    }

    pub fn schedule_hard_pressure_for_slug(&self, slug: &str, hard_quota: i64, now: i64) -> CatalogResult<Option<RetentionJob>> { self.schedule_hard_pressure_for_slug_for_growth(slug,hard_quota,0,now) }
    pub fn schedule_hard_pressure_for_slug_for_growth(&self, slug: &str, hard_quota: i64, required_growth: i64, now: i64) -> CatalogResult<Option<RetentionJob>> { self.schedule_hard_pressure_for_slug_for_growth_with_limits(slug,hard_quota,required_growth,None,now) }
    pub fn schedule_hard_pressure_for_slug_for_growth_with_limits(&self, slug: &str, hard_quota: i64, required_growth: i64, hard_count_limit: Option<u32>, now: i64) -> CatalogResult<Option<RetentionJob>> {
        let owner: Option<String> = self.with_connection(|connection| connection.query_row("SELECT owner_id FROM documents WHERE slug=?1", [slug], |row| row.get(0)).optional().map_err(CatalogError::from))?;
        let owner = owner.ok_or(CatalogError::NotFound)?;
        self.schedule_hard_pressure_for_growth_with_limits(&owner,hard_quota,required_growth,hard_count_limit,now)
    }
}
