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

/// Truncate to at most `max_bytes`, never splitting a UTF-8 code point. The
/// one bounded-truncation primitive for the assistant module; everything
/// that needs to bound a string for the journal, the sidebar wire, or a
/// permission detail goes through this and [`push_bounded_utf8`].
pub(crate) fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    let mut end = value.len().min(max_bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

/// Append as much of `chunk` as fits within `max_bytes` total on `target`,
/// never splitting a UTF-8 code point.
pub(crate) fn push_bounded_utf8(target: &mut String, chunk: &str, max_bytes: usize) {
    if target.len() >= max_bytes {
        return;
    }
    let mut end = (max_bytes - target.len()).min(chunk.len());
    while end > 0 && !chunk.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&chunk[..end]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_bounds_are_bytes_and_preserve_utf8() {
        let mut output = "a".repeat(5);
        push_bounded_utf8(&mut output, "ééé", 8);
        assert_eq!(output, "aaaaaé");
        assert_eq!(output.len(), 7);
        assert_eq!(truncate_utf8("ééé", 5), "éé");
    }
}
