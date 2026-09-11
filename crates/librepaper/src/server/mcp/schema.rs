//! The advertised JSON schemas also validate incoming tool arguments.
use serde_json::{json, Value};
use std::sync::OnceLock;

pub(super) fn tools() -> &'static Vec<Value> {
    static TOOLS: OnceLock<Vec<Value>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        serde_json::from_str(include_str!("tools.json")).expect("checked-in MCP tools JSON")
    })
}

pub(super) fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(choices) = schema["anyOf"].as_array() {
        if choices.iter().any(|s| validate(s, value).is_ok()) {
            return Ok(());
        }
        return Err("value does not match an allowed shape".into());
    }
    let correct_type = match schema["type"].as_str() {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("integer") => value.is_u64() || value.is_i64(),
        Some("boolean") => value.is_boolean(),
        None => true,
        _ => false,
    };
    if !correct_type {
        return Err("incorrect argument type".into());
    }
    if let Some(allowed) = schema["enum"].as_array() {
        if !allowed.contains(value) {
            return Err("unknown argument value".into());
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("missing {name}"));
                }
            }
        }
        for (key, item) in object {
            if let Some(child) = schema["properties"].get(key) {
                validate(child, item).map_err(|e| format!("{key}: {e}"))?;
            } else if schema["additionalProperties"] == false {
                return Err(format!("unknown argument {key}"));
            }
        }
    }
    if let Some(items) = value.as_array() {
        if schema["minItems"]
            .as_u64()
            .is_some_and(|n| items.len() < n as usize)
            || schema["maxItems"]
                .as_u64()
                .is_some_and(|n| items.len() > n as usize)
        {
            return Err("array exceeds its item bounds".into());
        }
        for item in items {
            validate(&schema["items"], item)?;
        }
    }
    if value.as_str().is_some_and(|s| {
        schema["maxLength"]
            .as_u64()
            .is_some_and(|n| s.chars().count() > n as usize)
    }) {
        return Err("string exceeds its length limit".into());
    }
    if let Some(n) = value.as_f64() {
        if schema["minimum"].as_f64().is_some_and(|min| n < min)
            || schema["maximum"].as_f64().is_some_and(|max| n > max)
        {
            return Err("number exceeds its bounds".into());
        }
    }
    Ok(())
}

pub(super) fn metadata() -> Value {
    json!({"io.modelcontextprotocol/serverInfo":{"name":"librepaper","version":crate::VERSION}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_schemas_reject_typos_and_wrong_preconditions() {
        let propose = &tools()
            .iter()
            .find(|t| t["name"] == "document_propose")
            .expect("propose")["inputSchema"];
        let valid = json!({"view_id":"v","operation":{"epoch":"e","id":"i"},"patches":[{"range_id":"r","replacement":"😀\n"}]});
        assert!(validate(propose, &valid).is_ok());
        let mut typo = valid.clone();
        typo["revision"] = json!("unexpected");
        assert!(validate(propose, &typo).is_err());
        let mut missing = valid;
        missing.as_object_mut().expect("object").remove("operation");
        assert!(validate(propose, &missing).is_err());
    }
}
