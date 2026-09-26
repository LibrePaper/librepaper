//! The advertised JSON schemas also validate incoming tool arguments.
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

/// The checked-in file's own shape: one `operation`-object schema shared by
/// every tool that needs it, referenced from each tool's `inputSchema` by
/// `{"$ref":"#/$defs/NAME"}`.
#[derive(Deserialize)]
struct ToolsFile {
    #[serde(rename = "$defs")]
    defs: Value,
    tools: Vec<Value>,
}

fn tools_file() -> &'static ToolsFile {
    static FILE: OnceLock<ToolsFile> = OnceLock::new();
    FILE.get_or_init(|| {
        serde_json::from_str(include_str!("tools.json")).expect("checked-in MCP tools JSON")
    })
}

/// The tools as authored, `$ref`s and all: what `tools/call` looks a tool up
/// by name in and validates its arguments against.
pub(super) fn tools() -> &'static Vec<Value> {
    &tools_file().tools
}

/// The tools as served to the model: every `$ref` expanded inline, since a
/// client reads one schema per tool and does not resolve JSON Schema
/// references itself, and with `operation`/`target_operation` removed from
/// every tool's properties and required list. The bridge injects a mutating
/// call's operation identity itself now, from the epoch it remembered off
/// the model's last document_read; a model that never sees the field cannot
/// get its epoch or id wrong. The server's own validation schema (`tools()`)
/// is unchanged: the wire contract the bridge and the server keep between
/// themselves still requires it.
pub(super) fn served_tools() -> &'static Vec<Value> {
    static SERVED: OnceLock<Vec<Value>> = OnceLock::new();
    SERVED.get_or_init(|| {
        tools()
            .iter()
            .cloned()
            .map(|mut tool| {
                inline_refs(&mut tool["inputSchema"]);
                strip_operation_fields(&mut tool["inputSchema"]);
                tool
            })
            .collect()
    })
}

fn strip_operation_fields(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    // Operation receipts are the bridge's business now; a model has no
    // operation key to look one up with, so document_result offers it only
    // the candidate and render lookups, and says which one it is asking for
    // rather than falling through to the operation default.
    let mut kind_required = false;
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        properties.remove("operation");
        properties.remove("target_operation");
        if let Some(kinds) = properties
            .get_mut("kind")
            .and_then(|kind| kind.get_mut("enum"))
            .and_then(Value::as_array_mut)
        {
            kinds.retain(|kind| kind != "operation");
            kind_required = true;
        }
    }
    if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|name| name != "operation" && name != "target_operation");
        if kind_required && !required.iter().any(|name| name == "kind") {
            required.push(Value::from("kind"));
        }
    }
}

fn inline_refs(schema: &mut Value) {
    if let Some(name) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
    {
        if let Some(resolved) = tools_file().defs.get(name).cloned() {
            *schema = resolved;
        }
    }
    match schema {
        Value::Object(object) => {
            for value in object.values_mut() {
                inline_refs(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                inline_refs(item);
            }
        }
        _ => {}
    }
}

pub(super) fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(name) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
    {
        let resolved = tools_file()
            .defs
            .get(name)
            .ok_or_else(|| format!("unknown schema definition {name}"))?;
        return validate(resolved, value);
    }
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
        Some("null") => value.is_null(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn read_accepts_rendered_quotes_without_source_coordinates() {
        let read = &tools()
            .iter()
            .find(|tool| tool["name"] == "document_read")
            .unwrap()["inputSchema"];
        let mut args = json!({"queries":[{"kind":"source","selection":{"exact":"Selected words","position":null},"revision":"current"}]});
        assert!(validate(read, &args).is_ok());
        args["queries"][0]["selection"]["position"] = json!("invalid");
        assert!(validate(read, &args).is_err());
    }
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

    /// The served schema omits `operation`/`target_operation` from every
    /// tool: the bridge injects a mutating call's identity now, so a model
    /// never sees the field and cannot get it wrong. The validation schema
    /// `tools()` is untouched: the bridge-to-server contract still requires
    /// it.
    #[test]
    fn served_tools_omit_the_operation_fields_the_bridge_now_injects() {
        for tool in served_tools() {
            let properties = &tool["inputSchema"]["properties"];
            assert!(properties.get("operation").is_none(), "{}", tool["name"]);
            assert!(
                properties.get("target_operation").is_none(),
                "{}",
                tool["name"]
            );
            let required = tool["inputSchema"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(!required.iter().any(|name| name == "operation"));
        }
        let result = served_tools()
            .iter()
            .find(|t| t["name"] == "document_result")
            .expect("result");
        assert_eq!(
            result["inputSchema"]["properties"]["kind"]["enum"],
            json!(["candidate", "render"])
        );
        assert!(result["inputSchema"]["required"]
            .as_array()
            .expect("required")
            .contains(&json!("kind")));
        // The validation schema is unaffected: the wire contract between the
        // bridge and the server still requires the field.
        let propose = &tools()
            .iter()
            .find(|t| t["name"] == "document_propose")
            .expect("propose")["inputSchema"];
        assert!(propose["properties"]["operation"].is_object());
    }
}
