//! Fixed checkpoint retention policy used for the entire deployment.

use serde::Serialize;

pub const DEFAULT_ROUTINE_VERSION_COUNT: i32 = 50;
pub const DEFAULT_ROUTINE_VERSION_AGE_DAYS: i32 = 30;
pub const DEFAULT_MAX_CHECKPOINT_COUNT: u32 = 4096;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicy {
    pub routine_version_count: i32,
    pub routine_version_age_days: i64,
    pub named_checkpoints_protected: bool,
    pub max_checkpoint_count: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            routine_version_count: DEFAULT_ROUTINE_VERSION_COUNT,
            routine_version_age_days: i64::from(DEFAULT_ROUTINE_VERSION_AGE_DAYS),
            named_checkpoints_protected: true,
            max_checkpoint_count: DEFAULT_MAX_CHECKPOINT_COUNT,
        }
    }
}

impl RetentionPolicy {
    pub fn fixed() -> Self {
        Self::default()
    }
}
