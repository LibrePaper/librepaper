//! What a paged request asks for: a clamped page size and the typed keyset
//! cursor of the templates listing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A page size from a query value. A missing or malformed value is
/// `default`, and the result is never below 1 or above `max`.
pub(super) fn clamp_limit(raw: Option<&str>, default: u32, max: u32) -> u32 {
    raw.and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default)
        .clamp(1, max)
}

/// The `next_cursor` of the templates listing, as the browser sends it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TemplatesCursor {
    pub after_updated: String,
    pub after_id: String,
}

impl TemplatesCursor {
    /// The cursor a request carries in its query, when it has both fields.
    pub fn from_query(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        Some(Self {
            after_updated: get("after_updated")?,
            after_id: get("after_id")?,
        })
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("a cursor of two strings encodes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limits_are_defaulted_and_clamped() {
        assert_eq!(clamp_limit(None, 200, 200), 200);
        assert_eq!(clamp_limit(Some("abc"), 64, 200), 64);
        assert_eq!(clamp_limit(Some("-5"), 64, 200), 64);
        assert_eq!(clamp_limit(Some("0"), 64, 200), 1);
        assert_eq!(clamp_limit(Some("1"), 64, 200), 1);
        assert_eq!(clamp_limit(Some("150"), 64, 200), 150);
        assert_eq!(clamp_limit(Some("5000"), 50, 100), 100);
        assert_eq!(clamp_limit(Some("99999999999"), 50, 100), 50);
    }

    #[test]
    fn the_templates_cursor_keeps_its_wire_fields() {
        let cursor = TemplatesCursor {
            after_updated: "2026-01-02T03:04:05Z".into(),
            after_id: "id-1".into(),
        };
        let wire = json!({"after_updated": "2026-01-02T03:04:05Z", "after_id": "id-1"});
        assert_eq!(cursor.to_json(), wire);
        assert_eq!(
            serde_json::from_value::<TemplatesCursor>(wire).unwrap(),
            cursor
        );
        let query = |name: &str| match name {
            "after_updated" => Some("2026-01-02T03:04:05Z".to_string()),
            "after_id" => Some("id-1".to_string()),
            _ => None,
        };
        assert_eq!(TemplatesCursor::from_query(query), Some(cursor));
        assert_eq!(TemplatesCursor::from_query(|_| None), None);
    }
}
