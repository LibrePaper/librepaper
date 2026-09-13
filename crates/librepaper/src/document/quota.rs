//! Bounded, deterministic checkpoint retention. Quota limits are deployment policy;
//! preferences never grant additional storage or permission to remove protected history.
use super::history::Checkpoint;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PREFERENCES_VERSION: u32 = 2;
pub const RETENTION_POLICY_VERSION: u32 = 2;
pub const DEFAULT_WARNING_THRESHOLDS: &[u8] = &[75, 90];
pub const DEFAULT_MAX_AGE_MS: i64 = 30 * 86_400_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomRetention {
    pub max_routine_count: Option<u32>,
    pub max_age_ms: Option<i64>,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for CustomRetention {
    fn default() -> Self {
        Self {
            max_routine_count: None,
            max_age_ms: None,
            extra: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPreferences {
    pub version: u32,
    pub retention_profile: String,
    pub retention_policy_version: u32,
    #[serde(default)]
    pub custom_retention: Option<CustomRetention>,
    pub display_timezone: String,
    pub warning_thresholds: Vec<u8>,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}
impl Default for QuotaPreferences {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            retention_profile: "default".into(),
            retention_policy_version: RETENTION_POLICY_VERSION,
            custom_retention: None,
            display_timezone: "UTC".into(),
            warning_thresholds: DEFAULT_WARNING_THRESHOLDS.to_vec(),
            extra: std::collections::BTreeMap::new(),
        }
    }
}
impl QuotaPreferences {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PREFERENCES_VERSION
            || self.retention_policy_version != RETENTION_POLICY_VERSION
        {
            return Err("unsupported retention preference version".into());
        }
        if !matches!(
            self.retention_profile.as_str(),
            "default" | "manual" | "custom"
        ) {
            return Err("retention profile must be default, manual, or custom".into());
        }
        if (self.retention_profile == "custom") != self.custom_retention.is_some() {
            return Err("custom retention limits require the custom profile".into());
        }
        if let Some(custom) = &self.custom_retention {
            if custom.max_routine_count.is_some_and(|n| n > 4096)
                || custom.max_age_ms.is_some_and(|age| age < 0)
            {
                return Err("invalid custom retention limits".into());
            }
        }
        if self.display_timezone.len() > 128
            || self.display_timezone.parse::<chrono_tz::Tz>().is_err()
        {
            return Err("display timezone must be a supported IANA timezone name".into());
        }
        if self.warning_thresholds.is_empty()
            || self.warning_thresholds.len() > 4
            || self.warning_thresholds.iter().any(|n| *n == 0 || *n > 100)
            || self
                .warning_thresholds
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err("warning thresholds must be increasing percentages from 1 to 100".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionBounds {
    pub max_routine_count: Option<u32>,
    pub max_age_ms: Option<i64>,
    /// Admission ceiling only; never overrides a retained root.
    pub max_checkpoint_count: Option<u32>,
    pub hard_quota: i64,
}
impl Default for RetentionBounds {
    fn default() -> Self {
        Self {
            max_routine_count: None,
            max_age_ms: None,
            max_checkpoint_count: Some(4096),
            hard_quota: 100 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveRetention {
    pub profile: String,
    pub policy_version: u32,
    pub max_routine_count: Option<u32>,
    pub max_age_ms: Option<i64>,
    pub explanation: Vec<String>,
    pub safe_mode: bool,
}
fn narrower<T: Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, None) => a,
        (None, b) => b,
    }
}
pub fn effective_retention(
    preferences: &QuotaPreferences,
    bounds: &RetentionBounds,
) -> EffectiveRetention {
    let valid = preferences.validate().is_ok();
    let (count, age) = match preferences.retention_profile.as_str() {
        "default" => (Some(50), Some(DEFAULT_MAX_AGE_MS)),
        "custom" => preferences
            .custom_retention
            .as_ref()
            .map(|p| (p.max_routine_count, p.max_age_ms))
            .unwrap_or((None, None)),
        _ => (None, None),
    };
    let manual = preferences.retention_profile == "manual";
    EffectiveRetention {
        profile: preferences.retention_profile.clone(),
        policy_version: preferences.retention_policy_version,
        max_routine_count: if manual {
            None
        } else {
            narrower(count, bounds.max_routine_count)
        },
        max_age_ms: if manual {
            None
        } else {
            narrower(age, bounds.max_age_ms)
        },
        explanation: if valid {
            Vec::new()
        } else {
            vec!["Unknown policy: automatic deletion is disabled.".into()]
        },
        safe_mode: !valid,
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
/// Wire checkpoints still expose a printable timestamp; persisted v2 times are milliseconds.
fn event_time(point: &Checkpoint) -> Option<i64> {
    point.at.parse::<i64>().ok().or_else(|| {
        time::OffsetDateTime::parse(&point.at, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|time| i64::try_from(time.unix_timestamp_nanos() / 1_000_000).ok())
    })
}
/// `now` is Unix milliseconds; references are checkpoint event IDs, never tree digests.
/// Selection is advisory. The catalog rechecks roots and persisted grace before deletion.
pub fn select_retained(
    checkpoints: &[Checkpoint],
    now: i64,
    preferences: &QuotaPreferences,
    bounds: &RetentionBounds,
    open_references: &BTreeSet<String>,
) -> RetentionDecision {
    let policy = effective_retention(preferences, bounds);
    let mut decision = RetentionDecision {
        retained: BTreeSet::new(),
        removed: Vec::new(),
        protected: BTreeSet::new(),
        policy_version: policy.policy_version,
    };
    let mut ordered: Vec<_> = checkpoints.iter().collect();
    ordered.sort_by(|a, b| (b.seq, &b.sha).cmp(&(a.seq, &a.sha)));
    let newest = ordered.first().map(|point| point.sha.as_str());
    let mut routine = 0u32;
    for point in ordered {
        let protected = Some(point.sha.as_str()) == newest
            || !point.label.is_empty()
            || open_references.contains(&point.sha);
        if protected {
            decision.protected.insert(point.sha.clone());
        }
        let at = event_time(point);
        let keep = if protected || policy.safe_mode || policy.profile == "manual" || at.is_none() {
            true
        } else {
            let rank = routine;
            routine += 1;
            policy.max_routine_count.is_none_or(|limit| rank < limit)
                && policy.max_age_ms.is_none_or(|age| {
                    now.saturating_sub(at.expect("timestamp checked above")) <= age
                })
        };
        if keep {
            decision.retained.insert(point.sha.clone());
        } else {
            decision.removed.push(point.sha.clone());
        }
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(seq: i64, at: i64) -> Checkpoint {
        Checkpoint {
            sha: seq.to_string(),
            seq,
            at: at.to_string(),
            ..Default::default()
        }
    }
    #[test]
    fn defaults_require_both_count_and_age() {
        let mut points: Vec<_> = (1..=60).map(|n| point(n, DEFAULT_MAX_AGE_MS)).collect();
        points[30].at = "0".into();
        let decision = select_retained(
            &points,
            DEFAULT_MAX_AGE_MS + 1,
            &QuotaPreferences::default(),
            &RetentionBounds::default(),
            &BTreeSet::new(),
        );
        assert!(decision.retained.contains("60"));
        assert!(!decision.retained.contains("31"));
        assert!(!decision.retained.contains("1"));
        assert!(decision.retained.contains("59"));
    }
    #[test]
    fn roots_survive_zero_limits_and_hard_pressure() {
        let mut points = vec![point(1, 0), point(2, 0), point(3, 0), point(4, 0)];
        points[0].label = "keep".into();
        let preferences = QuotaPreferences {
            retention_profile: "custom".into(),
            custom_retention: Some(CustomRetention {
                max_routine_count: Some(0),
                max_age_ms: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let decision = select_retained(
            &points,
            100,
            &preferences,
            &RetentionBounds {
                max_checkpoint_count: Some(1),
                ..Default::default()
            },
            &BTreeSet::from(["2".into()]),
        );
        assert_eq!(
            decision.retained,
            BTreeSet::from(["1".into(), "2".into(), "4".into()])
        );
        assert_eq!(decision.removed, vec!["3"]);
    }
    #[test]
    fn manual_and_invalid_policies_keep_all_history() {
        for profile in ["manual", "unknown"] {
            let preferences = QuotaPreferences {
                retention_profile: profile.into(),
                ..Default::default()
            };
            assert!(select_retained(
                &[point(1, 0), point(2, 0)],
                i64::MAX,
                &preferences,
                &RetentionBounds::default(),
                &BTreeSet::new()
            )
            .removed
            .is_empty());
        }
    }
    #[test]
    fn ordering_uses_event_sequence_despite_clock_regression() {
        let decision = select_retained(
            &[point(1, 100), point(2, 0)],
            i64::MAX,
            &QuotaPreferences::default(),
            &RetentionBounds::default(),
            &BTreeSet::new(),
        );
        assert_eq!(decision.retained, BTreeSet::from(["2".into()]));
    }
}
