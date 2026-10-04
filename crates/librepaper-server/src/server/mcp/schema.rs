//! The advertised JSON schemas also validate incoming tool arguments.
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
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

fn compile(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::validator_for(schema).map_err(|error| error.to_string())
}

fn check(validator: &jsonschema::Validator, args: &Value) -> Result<(), String> {
    match validator.iter_errors(args).next() {
        Some(error) => Err(error.to_string()),
        None => Ok(()),
    }
}

/// One compiled validator per tool, built once from the tool's argument
/// schema with its `$ref`s expanded, so the compiled form needs no resolver.
fn validators() -> &'static HashMap<String, jsonschema::Validator> {
    static VALIDATORS: OnceLock<HashMap<String, jsonschema::Validator>> = OnceLock::new();
    VALIDATORS.get_or_init(|| {
        tools()
            .iter()
            .map(|tool| {
                let name = tool["name"].as_str().expect("tool name").to_string();
                let mut schema = tool["inputSchema"].clone();
                inline_refs(&mut schema);
                let validator = compile(&schema).expect("checked-in MCP tool schema compiles");
                (name, validator)
            })
            .collect()
    })
}

/// Validates a tool call's arguments against that tool's advertised schema.
pub(super) fn validate_tool(name: &str, args: &Value) -> Result<(), String> {
    match validators().get(name) {
        Some(validator) => check(validator, args),
        None => Err(format!("unknown tool {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn read_accepts_rendered_quotes_without_source_coordinates() {
        let mut args = json!({"queries":[{"kind":"source","selection":{"exact":"Selected words","position":null},"revision":"current"}]});
        assert!(validate_tool("document_read", &args).is_ok());
        args["queries"][0]["selection"]["position"] = json!("invalid");
        assert!(validate_tool("document_read", &args).is_err());
    }
    #[test]
    fn tool_schemas_reject_typos_and_wrong_preconditions() {
        let valid = json!({"view_id":"v","operation":{"epoch":"e","id":"v2.1700000000000.0123456789abcdef0123456789abcdef"},"patches":[{"range_id":"r","replacement":"😀\n"}]});
        assert!(validate_tool("document_propose", &valid).is_ok());
        let mut typo = valid.clone();
        typo["revision"] = json!("unexpected");
        assert!(validate_tool("document_propose", &typo).is_err());
        let mut missing = valid;
        missing.as_object_mut().expect("object").remove("operation");
        assert!(validate_tool("document_propose", &missing).is_err());
    }

    #[test]
    fn pattern_is_enforced() {
        let mut args = json!({"view_id":"v","operation":{"epoch":"e","id":"v2.1700000000000.0123456789abcdef0123456789abcdef"},"patches":[{"range_id":"r","replacement":"x"}]});
        assert!(validate_tool("document_propose", &args).is_ok());
        args["operation"]["id"] = json!("not-a-valid-key");
        assert!(validate_tool("document_propose", &args).is_err());
    }

    #[test]
    fn number_type_accepts_numbers_and_bounds() {
        let validator = compile(&json!({"type":"number","minimum":0,"maximum":10})).unwrap();
        assert!(check(&validator, &json!(2.5)).is_ok());
        assert!(check(&validator, &json!(3)).is_ok());
        assert!(check(&validator, &json!(11.5)).is_err());
        assert!(check(&validator, &json!("3")).is_err());
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
