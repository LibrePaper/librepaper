//! The projection corpus, from the Rust side.
//!
//! `web/tests/unit/projection.mjs` reads the same file and asserts the same
//! expectations, so the two implementations are held equal by trees and
//! digests rather than by two people reading one specification
//! (SPEC-server-is-a-log §4.4, §14.2).
//!
//! `LIBREPAPER_REGENERATE_FIXTURES=1 cargo test -p librepaper-document-core`
//! rewrites the `expect` blocks in place. Read the diff before committing it:
//! this test is only worth what the expectations say.

use std::collections::BTreeMap;

use librepaper_document_core::{paths::Rules, project, Projection};
use loro::{LoroDoc, LoroText, LoroValue};
use serde_json::{json, Map, Value};

fn corpus_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/tests/fixtures/projection.json")
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn insert_value(map: &loro::LoroMap, key: &str, value: &Value) {
    match value {
        Value::String(text) => {
            map.insert(key, text.as_str()).unwrap();
        }
        Value::Bool(flag) => {
            map.insert(key, *flag).unwrap();
        }
        Value::Number(number) => {
            map.insert(key, number.as_i64().unwrap_or_default())
                .unwrap();
        }
        Value::Null => {
            map.insert(key, LoroValue::Null).unwrap();
        }
        other => panic!("fixture value {other} has no Loro spelling"),
    }
}

fn build(case: &Value) -> LoroDoc {
    let doc = LoroDoc::new();
    for root in librepaper_document_core::ROOTS {
        let _ = doc.get_map(root);
    }
    let files = doc.get_map("files");
    for entry in case["files"].as_array().into_iter().flatten() {
        let id = entry["id"].as_str().expect("a files entry names an id");
        match entry.get("text") {
            Some(Value::String(body)) => {
                let text = files.insert_container(id, LoroText::new()).unwrap();
                if !body.is_empty() {
                    text.insert(0, body).unwrap();
                }
            }
            _ => insert_value(&files, id, &entry["value"]),
        }
    }
    let paths = doc.get_map("paths");
    for entry in case["paths"].as_array().into_iter().flatten() {
        let id = entry["id"].as_str().expect("a paths entry names an id");
        match entry.get("path") {
            Some(path) => insert_value(&paths, id, path),
            None => insert_value(&paths, id, &entry["value"]),
        }
    }
    let assets = doc.get_map("assets");
    for entry in case["assets"].as_array().into_iter().flatten() {
        let path = entry["path"]
            .as_str()
            .expect("an assets entry names a path");
        insert_value(&assets, path, &entry["value"]);
    }
    let meta = doc.get_map("meta");
    for entry in case["meta"].as_array().into_iter().flatten() {
        let key = entry["key"].as_str().expect("a meta entry names a key");
        insert_value(&meta, key, &entry["value"]);
    }
    doc
}

/// The expectation block, as the corpus spells it. Diagnostics carry no `why`
/// here: the sentence is shown to a person and the two implementations word
/// it for their own callers, while the tag and the subject are the contract.
fn expectation(projection: &Projection) -> Value {
    let files: Vec<Value> = projection
        .files
        .iter()
        .map(|(path, entry)| {
            json!([
                path,
                {"kind": entry.kind, "id": entry.id, "digest": entry.digest, "bytes": entry.bytes}
            ])
        })
        .collect();
    let diagnostics: Vec<Value> = projection
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let mut value = serde_json::to_value(diagnostic).expect("a diagnostic serializes");
            if let Some(map) = value.as_object_mut() {
                map.remove("why");
            }
            value
        })
        .collect();
    json!({
        "main": projection.main,
        "mainId": projection.main_id,
        "files": files,
        "diagnostics": diagnostics,
        "digest": projection.digest(),
    })
}

#[test]
fn the_shared_corpus_projects_the_same_way_on_both_sides() {
    let raw = std::fs::read_to_string(corpus_path()).expect("the projection corpus is readable");
    let mut corpus: Value = serde_json::from_str(&raw).expect("the projection corpus is JSON");
    let text = strings(&corpus["rules"], "text_extensions");
    let asset = strings(&corpus["rules"], "asset_extensions");
    let derived = strings(&corpus["rules"], "derived_extensions");
    let rules = Rules {
        text: &text,
        asset: &asset,
        derived: &derived,
        max_path: corpus["rules"]["max_path"].as_u64().unwrap_or(255) as usize,
        max_segments: librepaper_document_core::paths::MAX_SEGMENTS,
    };

    let regenerate = std::env::var("LIBREPAPER_REGENERATE_FIXTURES").is_ok();
    let mut produced: BTreeMap<usize, Value> = BTreeMap::new();
    let cases = corpus["cases"].as_array().expect("cases is a list").clone();
    for (index, case) in cases.iter().enumerate() {
        let name = case["name"].as_str().unwrap_or("<unnamed>");
        let doc = build(case);
        let projected = project(&doc, &rules);
        let actual = expectation(&projected.projection);
        // Every placed text is readable at its emitted path, which is what a
        // render and an export depend on.
        for (path, entry) in &projected.projection.files {
            if entry.kind == "text" {
                assert!(
                    projected.texts.contains_key(path),
                    "{name}: {path} is a text in the tree with no body beside it"
                );
            }
        }
        if regenerate {
            produced.insert(index, actual);
        } else {
            let expected = case
                .get("expect")
                .unwrap_or_else(|| panic!("{name}: no expectation; regenerate the corpus"));
            assert_eq!(
                &actual, expected,
                "{name}: the projection moved. If that is intended, regenerate the corpus."
            );
        }
    }

    if regenerate {
        let list = corpus["cases"].as_array_mut().expect("cases is a list");
        for (index, value) in produced {
            let entry = list[index].as_object_mut().expect("a case is an object");
            entry.insert("expect".into(), value);
        }
        // Key order is the corpus's own; `serde_json::Map` preserves insertion
        // order only with the `preserve_order` feature, so `expect` is appended
        // by hand above and the file is written pretty so the diff is readable.
        let _: &Map<String, Value> = corpus.as_object().expect("the corpus is an object");
        std::fs::write(
            corpus_path(),
            format!("{}\n", serde_json::to_string_pretty(&corpus).unwrap()),
        )
        .expect("the corpus is writable");
        panic!("the corpus was regenerated; re-run without LIBREPAPER_REGENERATE_FIXTURES");
    }
}
