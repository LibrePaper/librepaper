//! Account history preferences and the deterministic retention evaluator.
//!
//! The evaluator is deliberately independent of the object store.  It selects
//! immutable checkpoint events; a storage/catalogue implementation can then
//! calculate references and reclaimable bytes before deleting the selected
//! events.  Keeping those concerns separate also makes previews safe when the
//! physical accounting backend is temporarily unavailable.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::history::Checkpoint;

pub const PREFERENCES_VERSION: u32 = 1;
pub const BALANCED_POLICY_VERSION: u32 = 1;
pub const DEFAULT_WARNING_THRESHOLDS: &[u8] = &[75, 90];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryBudget {
    pub kind: BudgetKind,
    pub value: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BudgetKind {
    Percent,
    Bytes,
}

impl Default for HistoryBudget {
    fn default() -> Self {
        Self {
            kind: BudgetKind::Percent,
            value: 60,
        }
    }
}

impl HistoryBudget {
    pub fn validate(&self) -> Result<(), String> {
        match self.kind {
            BudgetKind::Percent if !(0..=100).contains(&self.value) => {
                Err("history budget percentage must be between 0 and 100".into())
            }
            BudgetKind::Bytes if self.value < 0 => {
                Err("history budget bytes cannot be negative".into())
            }
            _ => Ok(()),
        }
    }

    pub fn target_bytes(&self, hard_quota: i64) -> i64 {
        match self.kind {
            BudgetKind::Percent => hard_quota.saturating_mul(self.value).saturating_div(100),
            BudgetKind::Bytes => self.value.min(hard_quota.max(0)),
        }
        .max(0)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MilestonePreferences {
    pub named: bool,
    pub cli: bool,
    pub publish: bool,
    pub restore: bool,
    pub accept: bool,
    pub comment: bool,
}

impl Default for MilestonePreferences {
    fn default() -> Self {
        Self {
            named: true,
            cli: true,
            publish: true,
            restore: true,
            accept: true,
            comment: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetentionTier {
    /// Exclusive upper age bound. `None` is the final, unbounded tier.
    pub max_age_seconds: Option<i64>,
    pub bucket_seconds: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomRetention {
    pub tiers: Vec<RetentionTier>,
    #[serde(default)]
    pub max_routine_count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPreferences {
    pub version: u32,
    pub history_budget: HistoryBudget,
    pub retention_profile: String,
    pub retention_policy_version: u32,
    #[serde(default)]
    pub custom_retention: Option<CustomRetention>,
    pub display_timezone: String,
    pub milestone_preferences: MilestonePreferences,
    pub warning_thresholds: Vec<u8>,
    #[serde(default)]
    pub document_overrides: serde_json::Map<String, serde_json::Value>,
    /// Preserve fields introduced by a newer server during a read/write.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for QuotaPreferences {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            history_budget: HistoryBudget::default(),
            retention_profile: "balanced".into(),
            retention_policy_version: BALANCED_POLICY_VERSION,
            custom_retention: None,
            display_timezone: "UTC".into(),
            milestone_preferences: MilestonePreferences::default(),
            warning_thresholds: DEFAULT_WARNING_THRESHOLDS.to_vec(),
            document_overrides: serde_json::Map::new(),
            extra: serde_json::Map::new(),
        }
    }
}

impl QuotaPreferences {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PREFERENCES_VERSION {
            return Err(format!("unsupported preference version {}", self.version));
        }
        if !matches!(
            self.retention_profile.as_str(),
            "balanced"
                | "moreRecoveryPoints"
                | "more-recovery-points"
                | "useLessStorage"
                | "use-less-storage"
                | "custom"
        ) {
            return Err("unsupported retention profile".into());
        }
        if self.retention_policy_version != BALANCED_POLICY_VERSION {
            return Err("unsupported retention policy version".into());
        }
        self.history_budget.validate()?;
        if self.display_timezone.len() > 128
            || self.display_timezone.parse::<chrono_tz::Tz>().is_err()
        {
            return Err("display timezone must be a supported IANA timezone name".into());
        }
        if self.warning_thresholds.is_empty()
            || self.warning_thresholds.len() > 4
            || self
                .warning_thresholds
                .windows(2)
                .any(|window| window[0] >= window[1])
            || self
                .warning_thresholds
                .iter()
                .any(|threshold| *threshold == 0 || *threshold > 100)
        {
            return Err("warning thresholds must be increasing percentages from 1 to 100".into());
        }
        if self.custom_retention.is_some() && self.retention_profile != "custom" {
            return Err("custom retention requires the custom profile".into());
        }
        if self.retention_profile == "custom" && self.custom_retention.is_none() {
            return Err("the custom profile requires retention tiers".into());
        }
        if let Some(custom) = &self.custom_retention {
            validate_custom_retention(custom)?;
        }
        Ok(())
    }
}

fn validate_custom_retention(custom: &CustomRetention) -> Result<(), String> {
    if custom.tiers.is_empty() || custom.tiers.len() > 8 {
        return Err("custom retention must contain between 1 and 8 tiers".into());
    }
    let mut previous_age = 0;
    let mut previous_bucket = 0;
    for (index, tier) in custom.tiers.iter().enumerate() {
        if tier.bucket_seconds <= 0 || tier.bucket_seconds > 31_536_000 {
            return Err(format!("custom tier {index} has an invalid bucket width"));
        }
        // Supported widths are deliberately bounded and UTC aligned.  This
        // prevents a preference from becoming an unbounded cron language.
        if ![
            300, 600, 900, 1_800, 3_600, 7_200, 10_800, 21_600, 43_200, 86_400,
        ]
        .contains(&tier.bucket_seconds)
        {
            return Err(format!("custom tier {index} is not UTC aligned"));
        }
        if tier.bucket_seconds < previous_bucket {
            return Err("custom bucket widths must be nondecreasing with age".into());
        }
        previous_bucket = tier.bucket_seconds;
        if let Some(age) = tier.max_age_seconds {
            if age <= previous_age {
                return Err("custom tier age bounds must increase".into());
            }
            previous_age = age;
        } else if index + 1 != custom.tiers.len() {
            return Err("only the final custom tier may be unbounded".into());
        }
    }
    if custom.max_routine_count == Some(0) {
        return Err("custom routine count must be positive".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveRetention {
    pub profile: String,
    pub policy_version: u32,
    pub tiers: Vec<RetentionTier>,
    pub max_routine_count: Option<u32>,
    pub explanation: Vec<String>,
    pub safe_mode: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionBounds {
    pub max_retention_seconds: Option<i64>,
    pub min_bucket_seconds: i64,
    pub max_routine_count: Option<u32>,
    /// Deployment hard cap. `Some(0)` has the explicit meaning "newest only";
    /// `None` means uncapped.
    pub max_checkpoint_count: Option<u32>,
    pub hard_quota: i64,
}

impl Default for RetentionBounds {
    fn default() -> Self {
        Self {
            max_retention_seconds: None,
            min_bucket_seconds: 300,
            max_routine_count: None,
            max_checkpoint_count: None,
            hard_quota: 100 * 1024 * 1024,
        }
    }
}

fn preset(name: &str) -> (u32, Vec<RetentionTier>) {
    match name {
        "moreRecoveryPoints" | "more-recovery-points" => (
            1,
            vec![
                RetentionTier {
                    max_age_seconds: Some(3_600),
                    bucket_seconds: 300,
                },
                RetentionTier {
                    max_age_seconds: Some(86_400),
                    bucket_seconds: 1_800,
                },
                RetentionTier {
                    max_age_seconds: Some(604_800),
                    bucket_seconds: 10_800,
                },
                RetentionTier {
                    max_age_seconds: None,
                    bucket_seconds: 86_400,
                },
            ],
        ),
        "useLessStorage" | "use-less-storage" => (
            1,
            vec![
                RetentionTier {
                    max_age_seconds: Some(3_600),
                    bucket_seconds: 900,
                },
                RetentionTier {
                    max_age_seconds: Some(86_400),
                    bucket_seconds: 7_200,
                },
                RetentionTier {
                    max_age_seconds: Some(604_800),
                    bucket_seconds: 43_200,
                },
                RetentionTier {
                    max_age_seconds: None,
                    bucket_seconds: 172_800,
                },
            ],
        ),
        _ => (
            BALANCED_POLICY_VERSION,
            vec![
                RetentionTier {
                    max_age_seconds: Some(3_600),
                    bucket_seconds: 300,
                },
                RetentionTier {
                    max_age_seconds: Some(86_400),
                    bucket_seconds: 3_600,
                },
                RetentionTier {
                    max_age_seconds: Some(604_800),
                    bucket_seconds: 21_600,
                },
                RetentionTier {
                    max_age_seconds: None,
                    bucket_seconds: 86_400,
                },
            ],
        ),
    }
}

pub fn effective_retention(
    preferences: &QuotaPreferences,
    bounds: &RetentionBounds,
) -> EffectiveRetention {
    let known = matches!(
        preferences.retention_profile.as_str(),
        "balanced"
            | "moreRecoveryPoints"
            | "more-recovery-points"
            | "useLessStorage"
            | "use-less-storage"
            | "custom"
    );
    let profile_name = if known {
        preferences.retention_profile.as_str()
    } else {
        "balanced"
    };
    let (preset_version, mut tiers) = preset(profile_name);
    let mut explanation = Vec::new();
    if !known {
        explanation.push("This retention profile is newer than the server; thinning is read-only until it is understood.".into());
    }
    let mut policy_version = preferences.retention_policy_version.max(preset_version);
    if preferences.retention_profile == "custom" {
        if let Some(custom) = &preferences.custom_retention {
            tiers = custom.tiers.clone();
            policy_version = preferences.retention_policy_version;
        } else {
            explanation.push("Custom policy was missing; Balanced is used safely.".into());
        }
    }
    if bounds.min_bucket_seconds > 0 {
        for tier in &mut tiers {
            if tier.bucket_seconds < bounds.min_bucket_seconds {
                tier.bucket_seconds = bounds.min_bucket_seconds;
                explanation.push("A deployment minimum widened one or more bucket widths.".into());
            }
        }
    }
    if let Some(max_age) = bounds.max_retention_seconds {
        for tier in &mut tiers {
            tier.max_age_seconds = tier.max_age_seconds.map(|age| age.min(max_age));
        }
        explanation.push("The deployment maximum retention duration applies.".into());
    }
    let custom_max = preferences
        .custom_retention
        .as_ref()
        .and_then(|custom| custom.max_routine_count);
    let max_routine_count = match (custom_max, bounds.max_routine_count) {
        (Some(custom), Some(bound)) => Some(custom.min(bound)),
        (Some(custom), None) => Some(custom),
        (None, Some(bound)) => Some(bound),
        (None, None) => None,
    };
    EffectiveRetention {
        profile: profile_name.to_string(),
        policy_version,
        tiers,
        max_routine_count,
        explanation,
        safe_mode: !known,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionDecision {
    pub retained: BTreeSet<String>,
    pub removed: Vec<String>,
    pub protected: BTreeSet<String>,
    pub policy_version: u32,
}

fn event_time(point: &Checkpoint) -> i64 {
    point
        .at
        .parse::<i64>()
        .ok()
        .or_else(|| {
            time::OffsetDateTime::parse(&point.at, &time::format_description::well_known::Rfc3339)
                .ok()
                .map(|time| time.unix_timestamp())
        })
        .unwrap_or(0)
}

/// Select deterministic retained events. `open_references` contains event or
/// tree identities referenced by open annotations/suggestions.
pub fn select_retained(
    checkpoints: &[Checkpoint],
    now: i64,
    preferences: &QuotaPreferences,
    bounds: &RetentionBounds,
    open_references: &BTreeSet<String>,
) -> RetentionDecision {
    let effective = effective_retention(preferences, bounds);
    if effective.safe_mode {
        return RetentionDecision {
            retained: checkpoints.iter().map(|point| point.sha.clone()).collect(),
            removed: Vec::new(),
            protected: BTreeSet::new(),
            policy_version: effective.policy_version,
        };
    }
    let mut retained = BTreeSet::new();
    let mut protected = BTreeSet::new();
    let Some((_, newest)) = checkpoints.iter().enumerate().max_by_key(|(index, point)| {
        (
            event_time(point),
            point.seq.max(*index as i64),
            point.sha.clone(),
        )
    }) else {
        return RetentionDecision {
            retained,
            removed: Vec::new(),
            protected,
            policy_version: effective.policy_version,
        };
    };
    let newest_sha = newest.sha.clone();
    for point in checkpoints {
        let milestone = (!point.label.is_empty() && preferences.milestone_preferences.named)
            || (preferences.milestone_preferences.cli && point.why == "cli")
            || (preferences.milestone_preferences.publish && point.why == "publish")
            || (preferences.milestone_preferences.restore
                && (point.why == "restore" || point.why == "restored"))
            || (preferences.milestone_preferences.accept && point.why == "accept")
            || (preferences.milestone_preferences.comment && open_references.contains(&point.sha));
        if milestone {
            protected.insert(point.sha.clone());
            retained.insert(point.sha.clone());
        }
    }
    let mut winners = BTreeMap::<(i64, i64), usize>::new();
    // A tree-only annotation reference protects the newest matching event,
    // not every restore/event that happens to point at that tree.
    for tree in open_references {
        if !preferences.milestone_preferences.comment {
            break;
        }
        if let Some((_, point)) = checkpoints
            .iter()
            .enumerate()
            .filter(|(_, point)| point.content_sha() == tree)
            .max_by_key(|(index, point)| (event_time(point), *index, point.sha.clone()))
        {
            protected.insert(point.sha.clone());
            retained.insert(point.sha.clone());
        }
    }
    for (index, point) in checkpoints.iter().enumerate() {
        if protected.contains(&point.sha) {
            continue;
        }
        let age = now.saturating_sub(event_time(point)).max(0);
        let tier = effective
            .tiers
            .iter()
            .find(|tier| tier.max_age_seconds.is_none_or(|bound| age < bound));
        let Some(tier) = tier else { continue };
        let bucket = event_time(point).div_euclid(tier.bucket_seconds);
        let key = (tier.bucket_seconds, bucket);
        // The array order is catalogue sequence for callers that materialize
        // a manifest. A later timestamp wins; the sequence and SHA make ties
        // total without depending on hash-map iteration order.
        let wins = winners.get(&key).is_none_or(|previous| {
            let old = &checkpoints[*previous];
            (
                event_time(point),
                point.seq.max(index as i64),
                point.sha.as_str(),
            ) > (
                event_time(old),
                old.seq.max(*previous as i64),
                old.sha.as_str(),
            )
        });
        if wins {
            if let Some(previous) = winners.insert(key, index) {
                retained.remove(&checkpoints[previous].sha);
            }
            retained.insert(point.sha.clone());
        }
    }
    if !effective.safe_mode {
        retained.insert(newest_sha.clone());
    } else {
        retained.extend(checkpoints.iter().map(|point| point.sha.clone()));
    }
    if let Some(limit) = effective.max_routine_count {
        let mut routine: Vec<_> = retained
            .iter()
            .filter(|sha| !protected.contains(*sha))
            .filter_map(|sha| {
                checkpoints
                    .iter()
                    .find(|point| point.sha.as_str() == (*sha).as_str())
            })
            .collect();
        routine.sort_by_key(|point| (event_time(point), point.sha.clone()));
        let excess = routine.len().saturating_sub(limit as usize);
        for point in routine.into_iter().take(excess) {
            // The newest checkpoint is an irreducible root even when a
            // deployment's count cap is lower than its routine winners.
            if point.sha != newest_sha {
                retained.remove(&point.sha);
            }
        }
    }
    if let Some(limit) = bounds.max_checkpoint_count {
        let mut all: Vec<_> = retained
            .iter()
            .filter_map(|sha| {
                checkpoints
                    .iter()
                    .find(|point| point.sha.as_str() == sha.as_str())
            })
            .filter(|point| point.sha != newest_sha)
            .collect();
        all.sort_by_key(|point| {
            (
                protected.contains(&point.sha),
                event_time(point),
                point.seq,
                point.sha.clone(),
            )
        });
        let excess = retained.len().saturating_sub(limit.max(1) as usize);
        for point in all.into_iter().take(excess) {
            retained.remove(&point.sha);
        }
    }
    let removed = checkpoints
        .iter()
        .filter(|point| !retained.contains(&point.sha))
        .map(|point| point.sha.clone())
        .collect();
    RetentionDecision {
        retained,
        removed,
        protected,
        policy_version: effective.policy_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(sha: &str, at: i64, why: &str) -> Checkpoint {
        Checkpoint {
            sha: sha.into(),
            at: at.to_string(),
            why: why.into(),
            ..Default::default()
        }
    }

    #[test]
    fn balanced_uses_utc_five_minute_buckets_and_protects_milestones() {
        let mut named = point("named", 1, "automatic");
        named.label = "Milestone".into();
        let points = vec![
            point("old", 3_599, "automatic"),
            point("new", 3_899, "automatic"),
            named,
        ];
        let mut prefs = QuotaPreferences {
            display_timezone: "America/Toronto".into(),
            ..QuotaPreferences::default()
        };
        prefs.milestone_preferences.named = true;
        let decision = select_retained(
            &points,
            4_000,
            &prefs,
            &RetentionBounds::default(),
            &BTreeSet::new(),
        );
        assert!(decision.retained.contains("new"));
        assert!(decision.retained.contains("named"));
    }

    #[test]
    fn budget_percentage_follows_hard_quota() {
        assert_eq!(HistoryBudget::default().target_bytes(100), 60);
    }

    #[test]
    fn hard_count_is_total_including_newest_and_can_remove_protected() {
        let points = vec![
            point("old", 1, "publish"),
            point("middle", 2, "cli"),
            point("new", 3, "automatic"),
        ];
        for limit in [0, 1, 2] {
            let bounds = RetentionBounds {
                max_checkpoint_count: Some(limit),
                ..Default::default()
            };
            let result = select_retained(
                &points,
                10,
                &QuotaPreferences::default(),
                &bounds,
                &BTreeSet::new(),
            );
            assert_eq!(result.retained.len(), limit.max(1) as usize);
            assert!(result.retained.contains("new"));
        }
    }

    #[test]
    fn invalid_timezone_and_missing_custom_tiers_are_rejected() {
        let mut preferences = QuotaPreferences {
            display_timezone: "Not/A-Timezone".into(),
            ..Default::default()
        };
        assert!(preferences.validate().is_err());
        preferences.display_timezone = "America/Toronto".into();
        assert!(preferences.validate().is_ok());
        preferences.retention_profile = "custom".into();
        assert!(preferences.validate().is_err());
    }

    #[test]
    fn display_timezone_never_changes_utc_bucket_winners() {
        let points = vec![point("a", 300, "automatic"), point("b", 301, "automatic")];
        let utc = QuotaPreferences::default();
        let mut local = utc.clone();
        local.display_timezone = "America/Toronto".into();
        let a = select_retained(
            &points,
            900,
            &utc,
            &RetentionBounds::default(),
            &BTreeSet::new(),
        );
        let b = select_retained(
            &points,
            900,
            &local,
            &RetentionBounds::default(),
            &BTreeSet::new(),
        );
        assert_eq!(a.retained, b.retained);
    }

    #[test]
    fn open_tree_reference_protects_only_newest_matching_event() {
        let mut old = point("old", 10, "automatic");
        old.tree_sha = "tree".into();
        let mut new = point("new", 20, "automatic");
        new.tree_sha = "tree".into();
        let refs = BTreeSet::from([String::from("tree")]);
        let result = select_retained(
            &[old, new],
            100_000,
            &QuotaPreferences::default(),
            &RetentionBounds::default(),
            &refs,
        );
        assert!(result.protected.contains("new"));
        assert!(!result.protected.contains("old"));
    }

    #[test]
    fn unknown_profile_enters_non_destructive_safe_mode() {
        let preferences = QuotaPreferences {
            retention_profile: "future-profile".into(),
            ..QuotaPreferences::default()
        };
        let result = select_retained(
            &[point("old", 1, "automatic"), point("new", 2, "automatic")],
            10_000,
            &preferences,
            &RetentionBounds::default(),
            &BTreeSet::new(),
        );
        assert!(result.removed.is_empty());
    }
}
