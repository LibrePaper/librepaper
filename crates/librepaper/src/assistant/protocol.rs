//! Shared, pure assistant wire vocabulary and structural limits.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const MAX_CONTEXT_BYTES: usize = 16 * 1024;
pub(crate) const MAX_EVENT_TEXT_BYTES: usize = 32 * 1024;
pub(crate) const MAX_ANSWER_BYTES: usize = MAX_EVENT_TEXT_BYTES;
pub(crate) const MAX_ID_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskStatus {
    Queued,
    Working,
    NeedsInput,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskStatus {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(Value::String(value.to_owned())).ok()
    }

    pub(crate) fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskKind {
    Proofread,
    Tighten,
    Rewrite,
    Explain,
    Outline,
    Respond,
    Fix,
    Refine,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskScope {
    Selection,
    File,
    Document,
}

pub(crate) fn valid_context(value: &Value) -> bool {
    serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= MAX_CONTEXT_BYTES)
}
