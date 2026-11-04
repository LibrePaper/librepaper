//! The wire fixtures (SPEC-wire.md section 9). Every file under
//! `wire/v{MAJOR}/` is checked against what the current code produces, and
//! `WIRE_BLESS=1` rewrites the response fixtures when the change is allowed.
//!
//! Request fixtures are hand-written and are never blessed: a request is what
//! a page sends, so changing one is a decision, not a regeneration.

use super::settings_tests::{body_json, job_body, job_request, test_inner};
use super::test_support::bearer;
use super::*;

use std::path::PathBuf;

use axum::http::Uri;

use crate::local::protocol::{
    Capabilities, JobRequest, OutputEntry, Provenance, RendererReport, Tool,
};
use crate::local::render::records;
use crate::local::wire;

const ORIGIN: &str = "https://paper.example";

/// Requests a page sends. Hand-written; `requests_still_decode...` reads them.
const JOB_REQUESTS: &[&str] = &[
    "jobs.post.request.full.json",
    "jobs.post.request.minimal.json",
];
const RENDERER_REQUESTS: &[&str] = &[
    "renderers.put.request.set.json",
    "renderers.put.request.reset.json",
];
/// Replies the current code produces. Blessed with `WIRE_BLESS=1`.
const RESPONSES: &[&str] = &[
    "health.get.response.json",
    "capabilities.get.response.full.json",
    "renderers.put.response.json",
    "jobs.post.response.json",
    "jobs.cancel.post.response.json",
    "jobs.get.response.queued.json",
    "jobs.get.response.running.json",
    "jobs.get.response.done.json",
    "jobs.get.response.failed.json",
    "jobs.get.response.canceled.json",
];

fn wire_root() -> PathBuf {
    PathBuf::from(format!("{}/wire", env!("CARGO_MANIFEST_DIR")))
}

fn wire_dir() -> PathBuf {
    wire_root().join(format!("v{}", wire::MAJOR))
}

fn read_optional(name: &str) -> Option<String> {
    std::fs::read_to_string(wire_dir().join(name)).ok()
}

fn read_optional_json(name: &str) -> Option<Value> {
    read_optional(name).map(|text| {
        serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("wire fixture {name} is not JSON: {error}"))
    })
}

/// A fixture that must exist. A missing file panics with its path.
fn fixture(name: &str) -> Value {
    let path = wire_dir().join(name);
    read_optional_json(name).unwrap_or_else(|| panic!("missing wire fixture {}", path.display()))
}

fn write_fixture(name: &str, value: &Value) {
    std::fs::create_dir_all(wire_dir()).expect("create the wire fixture directory");
    let mut text = serde_json::to_string_pretty(value).expect("encode a fixture");
    text.push('\n');
    std::fs::write(wire_dir().join(name), text).expect("write a wire fixture");
}

/// `jobs.get.response.<variant>.json` names the status `<variant>`.
fn job_status_variant(name: &str) -> Option<&str> {
    name.strip_prefix("jobs.get.response.")?
        .strip_suffix(".json")
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn join(path: &str, segment: &str) -> String {
    if path.is_empty() {
        segment.to_string()
    } else {
        format!("{path}.{segment}")
    }
}

/// Every key of `fixture` is in `value` with the same JSON type, recursively.
/// Arrays are compared element by element against the fixture's first
/// element. Extra keys in `value` are allowed and values are not compared.
/// An integer and a float are both numbers. Errors name the path.
fn compatible(fixture: &Value, value: &Value, path: &str) -> Result<(), String> {
    if kind(fixture) != kind(value) {
        return Err(format!(
            "{path}: expected {}, found {}",
            kind(fixture),
            kind(value)
        ));
    }
    match (fixture, value) {
        (Value::Object(fields), Value::Object(actual)) => {
            for (key, expected) in fields {
                let child = join(path, key);
                let Some(found) = actual.get(key) else {
                    return Err(format!("{child}: missing key"));
                };
                compatible(expected, found, &child)?;
            }
            Ok(())
        }
        (Value::Array(items), Value::Array(actual)) => {
            // An empty fixture array has no element to compare against.
            let Some(first) = items.first() else {
                return Ok(());
            };
            for (index, element) in actual.iter().enumerate() {
                compatible(first, element, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Compatible in both directions.
fn equal(a: &Value, b: &Value) -> Result<(), String> {
    compatible(a, b, "")?;
    compatible(b, a, "")
}

/// What the current code answers for each response fixture. Built through the
/// real handlers where they answer with JSON, and from a sample `JobStatus`
/// where the status is only ever serialized.
#[allow(clippy::await_holding_lock)]
async fn current_bodies(inner: &Arc<Inner>) -> Vec<(String, Value)> {
    let mut bodies: Vec<(String, Value)> = Vec::new();

    let health = handle_health(inner, &Uri::from_static("/librepaper/local/health"));
    bodies.push(("health.get.response.json".into(), body_json(health).await));

    let capabilities = handle_capabilities(inner, &bearer(inner), Some(ORIGIN), false).await;
    bodies.push((
        "capabilities.get.response.full.json".into(),
        body_json(capabilities).await,
    ));

    {
        // Records are process-wide; the lock covers this block and its await.
        let _lock = records::TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        records::init(&inner.state_home);
        let request = Request::put("/renderers/latex.pdf")
            .header("content-type", "application/json")
            .body(Body::from(
                fixture("renderers.put.request.set.json").to_string(),
            ))
            .expect("request");
        let response =
            handle_renderer_set(inner, &bearer(inner), Some(ORIGIN), "latex.pdf", request).await;
        assert_eq!(response.status(), StatusCode::OK);
        bodies.push((
            "renderers.put.response.json".into(),
            body_json(response).await,
        ));
    }

    let job = fixture("jobs.post.request.full.json");
    let admitted = jobs::handle_jobs_post(
        inner,
        &bearer(inner),
        Some(ORIGIN),
        job_request(job_body(&job, &[("main.typ", b"= Title")])),
    )
    .await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    let admitted = body_json(admitted).await;
    let id = admitted["id"].as_str().expect("an id").to_string();
    bodies.push(("jobs.post.response.json".into(), admitted));

    let cancel = jobs::handle_cancel(inner, &bearer(inner), Some(ORIGIN), &id).await;
    assert_eq!(cancel.status(), StatusCode::OK);
    bodies.push((
        "jobs.cancel.post.response.json".into(),
        body_json(cancel).await,
    ));

    for variant in ["queued", "running", "done", "failed", "canceled"] {
        let value = serde_json::to_value(sample_status(variant)).expect("encode a status");
        bodies.push((format!("jobs.get.response.{variant}.json"), value));
    }
    bodies
}

/// A `JobStatus` in the state `variant` names, with the fields that state sets.
fn sample_status(variant: &str) -> JobStatus {
    let mut status = JobStatus {
        id: "job-id".into(),
        status: JobState::Queued,
        stage: "staging".into(),
        snapshot: "snapshot-1".into(),
        generation: 1,
        provenance: Provenance {
            backend: "local".into(),
            format: "typst".into(),
            output: "pdf".into(),
        },
        ..Default::default()
    };
    match variant {
        "queued" => {}
        "running" => {
            status.status = JobState::Running;
            status.stage = "build".into();
            status.log_tail = "compiling main.typ".into();
        }
        "done" => {
            status.status = JobState::Done;
            status.stage = "finished".into();
            status.log_tail = "compiled main.typ".into();
            status.outputs = BTreeMap::from([
                (
                    "artifact".to_string(),
                    OutputEntry::from_bytes(b"%PDF-1.5\n"),
                ),
                ("synctex".to_string(), OutputEntry::from_bytes(b"SyncTeX")),
                ("log".to_string(), OutputEntry::from_bytes(b"compiled\n")),
            ]);
        }
        "failed" => {
            status.status = JobState::Failed;
            status.stage = "finished".into();
            status.exit = 1;
            status.error = Some("the render command exited with status 1".into());
            status.log_tail = "error: unknown function".into();
        }
        "canceled" => {
            status.status = JobState::Canceled;
            status.stage = "finished".into();
        }
        other => panic!("no sample status for {other}"),
    }
    status
}

/// A service whose runner reports one renderer, so `capabilities` has a body
/// with every field the fixture names.
async fn wire_inner() -> (Arc<Inner>, tempfile::TempDir, tempfile::TempDir) {
    let (mut inner, state_home, cache_home) = test_inner().await;
    let capabilities = Capabilities {
        platform: "linux".into(),
        zotero: Tool {
            available: true,
            version: Some("7.0.4".into()),
            note: "Zotero desktop is running".into(),
        },
        renderers: BTreeMap::from([(
            "latex.pdf".to_string(),
            RendererReport {
                command: vec!["latexmk".into(), "-xelatex".into(), "{input}".into()],
                result: "{stem}.pdf".into(),
                edited: false,
                found: true,
                version: Some("Latexmk 4.87".into()),
            },
        )]),
    };
    Arc::get_mut(&mut inner).expect("sole owner").runner = Arc::new(FakeRunner {
        capabilities,
        ..Default::default()
    });
    (inner, state_home, cache_home)
}

#[test]
fn requests_still_decode_and_keep_their_fields() {
    // Decoding fills in defaults, which the comparison allows. What it must
    // not do is drop a field: with strict decoding gone from the job type
    // (SPEC-wire 7.3), this is what catches a field removed from the type.
    // There is one major, so no `wire::upgrade` exists yet; Phase 3 adds the
    // older majors' request checks here.
    for name in JOB_REQUESTS {
        let fixture = fixture(name);
        let decoded = JobRequest::decode(fixture.clone())
            .unwrap_or_else(|error| panic!("{name} no longer decodes: {error}"));
        let reencoded = serde_json::to_value(&decoded).expect("encode a job");
        compatible(&fixture, &reencoded, name)
            .unwrap_or_else(|error| panic!("{name}: a field was lost: {error}"));
    }
    // `RendererRequest` is strict (deny_unknown_fields), so an unknown field
    // is already refused on decode; this checks the known ones survive.
    for name in RENDERER_REQUESTS {
        let fixture = fixture(name);
        let decoded: RendererRequest = serde_json::from_value(fixture.clone())
            .unwrap_or_else(|error| panic!("{name} no longer decodes: {error}"));
        let reencoded = serde_json::to_value(&decoded).expect("encode a renderer request");
        compatible(&fixture, &reencoded, name)
            .unwrap_or_else(|error| panic!("{name}: a field was lost: {error}"));
    }
}

#[tokio::test]
async fn the_current_major_has_not_broken() {
    let bless = std::env::var("WIRE_BLESS").as_deref() == Ok("1");
    let (inner, _state_home, _cache_home) = wire_inner().await;
    let bodies = current_bodies(&inner).await;

    let mut names: Vec<&str> = bodies.iter().map(|(name, _)| name.as_str()).collect();
    names.sort();
    let mut listed: Vec<&str> = RESPONSES.to_vec();
    listed.sort();
    assert_eq!(names, listed, "current_bodies and RESPONSES disagree");

    let mut breaking: Option<String> = None;
    let mut additive: Option<String> = None;
    // The fixtures that differ from the current bodies, or are missing.
    let mut stale: Vec<(String, Value)> = Vec::new();
    for (name, current) in &bodies {
        let Some(fixture) = read_optional_json(name) else {
            additive.get_or_insert(format!("{name}: a new fixture"));
            stale.push((name.clone(), current.clone()));
            continue;
        };
        if let Err(error) = compatible(&fixture, current, name) {
            breaking.get_or_insert(error);
        } else if let Err(error) = compatible(current, &fixture, name) {
            additive.get_or_insert(error);
        }
        if equal(&fixture, current).is_err() {
            stale.push((name.clone(), current.clone()));
        }
    }

    if !bless {
        if let Some(error) = &breaking {
            panic!(
                "Breaking wire change at {error}: raise wire::MAJOR, add wire/v{}/ and src/local/wire/v{}.rs",
                wire::MAJOR + 1,
                wire::MAJOR
            );
        }
        if let Some(error) = &additive {
            panic!("Additive wire change at {error}: raise wire::MINOR and run with WIRE_BLESS=1");
        }
        for (name, current) in &bodies {
            if let Some(variant) = job_status_variant(name) {
                assert_eq!(current["status"], variant, "{name}: the body's status");
                assert_eq!(
                    fixture(name)["status"],
                    variant,
                    "{name}: the fixture's status"
                );
            }
        }
        return;
    }

    // Blessing. A new major is the only case where the previous major's
    // directory and migration file are looked for.
    let previous = wire::MAJOR.checked_sub(1);
    let previous_major_dir =
        previous.is_some_and(|major| wire_root().join(format!("v{major}")).is_dir());
    let migration_file = previous.is_some_and(|major| {
        std::path::Path::new(&format!(
            "{}/src/local/wire/v{major}.rs",
            env!("CARGO_MANIFEST_DIR")
        ))
        .is_file()
    });
    let on_disk = read_optional("VERSION").map(|text| text.trim().to_string());
    if let Err(error) = bless_allowed(
        on_disk.as_deref(),
        &wire::version(),
        breaking.is_some(),
        additive.is_some(),
        previous_major_dir,
        migration_file,
    ) {
        panic!("WIRE_BLESS refused: {error}");
    }
    for (name, current) in &stale {
        write_fixture(name, current);
    }
    std::fs::create_dir_all(wire_dir()).expect("create the wire fixture directory");
    std::fs::write(wire_dir().join("VERSION"), format!("{}\n", wire::version()))
        .expect("write VERSION");
}

/// Whether `WIRE_BLESS=1` may rewrite the fixtures and `VERSION` (SPEC-wire
/// 9.3). `on_disk` is the `VERSION` file, or `None` for a new major directory.
fn bless_allowed(
    on_disk: Option<&str>,
    current: &str,
    breaking: bool,
    additive: bool,
    previous_major_dir: bool,
    migration_file: bool,
) -> Result<(), String> {
    let now = parse_version(current)?;
    let Some(on_disk) = on_disk else {
        if now.1 != 0 || now.2 != 0 {
            return Err(format!("a new major must be {}.0.0, not {current}", now.0));
        }
        if !previous_major_dir {
            return Err(format!(
                "a new major needs wire/v{}/ to exist",
                now.0.saturating_sub(1)
            ));
        }
        if !migration_file {
            return Err(format!(
                "a new major needs src/local/wire/v{}.rs",
                now.0.saturating_sub(1)
            ));
        }
        return Ok(());
    };
    let before = parse_version(on_disk)?;
    if breaking {
        return Err(format!(
            "a breaking change needs a new major: raise wire::MAJOR, add wire/v{}/ and src/local/wire/v{}.rs",
            now.0 + 1,
            now.0
        ));
    }
    if additive {
        if now.0 != before.0 {
            return Err(format!(
                "an additive change keeps the major, but {on_disk} is {current}"
            ));
        }
        if now.1 <= before.1 {
            return Err(format!(
                "an additive change needs a greater minor than {on_disk}: raise wire::MINOR"
            ));
        }
        return Ok(());
    }
    if current == on_disk {
        return Ok(());
    }
    if now.0 == before.0 && now.1 == before.1 && now.2 > before.2 {
        return Ok(());
    }
    Err(format!(
        "without a shape change, {current} must be a greater patch of {on_disk}"
    ))
}

fn parse_version(text: &str) -> Result<(u32, u32, u32), String> {
    let parts = text
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<u32>, _>>()
        .map_err(|error| format!("{text} is not MAJOR.MINOR.PATCH: {error}"))?;
    match parts.as_slice() {
        [major, minor, patch] => Ok((*major, *minor, *patch)),
        _ => Err(format!("{text} is not MAJOR.MINOR.PATCH")),
    }
}

#[test]
fn every_fixture_is_read_by_a_test() {
    let mut on_disk: Vec<String> = std::fs::read_dir(wire_dir())
        .expect("read the wire fixture directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".json"))
        .collect();
    on_disk.sort();
    let mut checked: Vec<String> = JOB_REQUESTS
        .iter()
        .chain(RENDERER_REQUESTS)
        .chain(RESPONSES)
        .map(|name| name.to_string())
        .collect();
    checked.sort();
    assert_eq!(
        on_disk,
        checked,
        "every fixture in wire/v{} needs a name in wire_tests.rs",
        wire::MAJOR
    );
}

#[test]
fn a_missing_key_is_incompatible_and_the_path_is_named() {
    let fixture = json!({"a": {"b": 1}});
    let error = compatible(&fixture, &json!({"a": {}}), "").unwrap_err();
    assert!(error.contains("a.b"), "{error}");
    assert!(error.contains("missing"), "{error}");
}

#[test]
fn an_extra_key_is_compatible() {
    let fixture = json!({"a": 1});
    compatible(&fixture, &json!({"a": 1, "b": [1, 2]}), "").expect("extra keys are allowed");
}

#[test]
fn a_type_change_is_incompatible_but_an_integer_and_a_float_are_not() {
    let error = compatible(&json!({"a": "x"}), &json!({"a": 1}), "").unwrap_err();
    assert!(error.contains("a"), "{error}");
    assert!(compatible(&json!({"a": null}), &json!({"a": 1}), "").is_err());
    compatible(&json!({"a": 1}), &json!({"a": 1.5}), "").expect("numbers are numbers");
}

#[test]
fn a_nested_array_element_that_lacks_a_key_is_incompatible() {
    let fixture = json!([{"a": 1}]);
    compatible(&fixture, &json!([{"a": 2}]), "").expect("values are not compared");
    let error = compatible(&fixture, &json!([{"a": 1}, {"b": 2}]), "").unwrap_err();
    assert!(error.contains("[1].a"), "{error}");
}

#[test]
fn equal_checks_both_directions() {
    assert!(equal(&json!({"a": 1}), &json!({"a": 1})).is_ok());
    assert!(equal(&json!({"a": 1}), &json!({"a": 1, "b": 2})).is_err());
}

#[test]
fn bless_adds_a_fixture_key_only_with_a_greater_minor() {
    assert!(bless_allowed(Some("3.1.0"), "3.2.0", false, true, true, true).is_ok());
    assert!(bless_allowed(Some("3.1.0"), "3.1.1", false, true, true, true).is_err());
    assert!(bless_allowed(Some("3.1.0"), "3.1.0", false, true, true, true).is_err());
    assert!(bless_allowed(Some("3.1.0"), "4.2.0", false, true, true, true).is_err());
}

#[test]
fn bless_refuses_a_breaking_change_on_an_existing_major() {
    assert!(bless_allowed(Some("3.1.0"), "3.1.0", true, false, true, true).is_err());
    assert!(bless_allowed(Some("3.1.0"), "3.2.0", true, false, true, true).is_err());
    assert!(bless_allowed(Some("3.1.0"), "4.0.0", true, false, true, true).is_err());
}

#[test]
fn bless_needs_a_greater_patch_without_a_shape_change() {
    assert!(bless_allowed(Some("3.1.0"), "3.1.0", false, false, true, true).is_ok());
    assert!(bless_allowed(Some("3.1.0"), "3.1.1", false, false, true, true).is_ok());
    assert!(bless_allowed(Some("3.1.1"), "3.1.0", false, false, true, true).is_err());
    assert!(bless_allowed(Some("3.1.0"), "3.2.0", false, false, true, true).is_err());
    assert!(bless_allowed(Some("3.1.0"), "4.0.0", false, false, true, true).is_err());
}

#[test]
fn bless_starts_a_new_major_only_at_n_zero_zero_with_its_predecessor() {
    assert!(bless_allowed(None, "4.0.0", true, false, true, true).is_ok());
    assert!(bless_allowed(None, "4.1.0", true, false, true, true).is_err());
    assert!(bless_allowed(None, "4.0.0", true, false, false, true).is_err());
    assert!(bless_allowed(None, "4.0.0", true, false, true, false).is_err());
}
