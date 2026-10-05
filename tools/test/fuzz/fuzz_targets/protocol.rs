//! The local bridge protocol: what a page in the browser may say to the
//! loopback service, and what the service is allowed to believe of it.
//!
//! This is the one surface where a request turns into files on the author's
//! own machine, so two things are asserted about it, and they are the two
//! things a page would try to break.
//!
//! **A safe path is a path that stays put.** `safe_relative_path` is a
//! string check -- no leading slash, no `..`, no backslash, no empty
//! component -- and what happens afterwards is a `Path::join` under a project
//! root. Those are two different notions of a path, and the target holds them
//! against each other: whatever the string check accepts, joining it under a
//! root must give a path still under that root, with nothing but ordinary
//! components between them.
//!
//! **What was declared wins.** `decode_preview`'s contract is that "old
//! nested tool envelopes cannot override the declared workspace, builder, or
//! entrypoint". The envelope is `options`, which is merged into the adapter's
//! own option map -- so a page that puts `main` in `options` is trying to
//! render a file other than the one the request names. The target sends
//! exactly that, with any keys and any values, and asserts the request's own
//! entrypoint, format and binding are what came back.
//!
//! And nothing anywhere may panic: the service answers this on loopback, so a
//! panic here is a page in a browser stopping the author's build service.
#![no_main]

use std::path::{Component, Path};

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use librepaper::protocol::{decode_preview, safe_relative_path, PreviewInputs, WorkspaceRequest};
use serde_json::{json, Value};

/// JSON the mutator can shape, so that `options` is a real tree rather than
/// noise that never deserialises.
#[derive(Arbitrary, Debug)]
enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn value(&self) -> Value {
        match self {
            Json::Null => Value::Null,
            Json::Bool(it) => json!(it),
            Json::Int(it) => json!(it),
            // A non-finite float is not JSON; `json!` would make it null, and
            // making it null here says so plainly.
            Json::Float(it) => {
                if it.is_finite() {
                    json!(it)
                } else {
                    Value::Null
                }
            }
            Json::Text(it) => json!(it),
            Json::List(items) => Value::Array(items.iter().take(8).map(Json::value).collect()),
            Json::Object(fields) => Value::Object(
                fields
                    .iter()
                    .take(8)
                    .map(|(key, value)| (key.clone(), value.value()))
                    .collect(),
            ),
        }
    }
}

/// A field the mutator would essentially never guess: an entrypoint has to
/// end in `.qmd` or `.md`, a format has to be one of four words, a builder one
/// of two. Left to arbitrary strings the door never opens and the target
/// passes without having tried anything. So each of those is a choice between
/// the words that get through and a string that may not.
#[derive(Arbitrary, Debug)]
enum Choice {
    Word(u8),
    Any(String),
}

impl Choice {
    fn of(&self, words: &[&str]) -> String {
        match self {
            Choice::Word(nth) => words[usize::from(*nth) % words.len()].to_string(),
            Choice::Any(text) => text.clone(),
        }
    }
}

#[derive(Arbitrary, Debug)]
struct Input {
    /// Any path at all, for the check against the filesystem's own notion.
    path: String,
    /// The request. Every field is the page's to choose.
    /// Narrow, so that version 2 -- the only one with a door -- comes up often.
    protocol: u8,
    kind: Choice,
    project: String,
    origin: String,
    snapshot: String,
    generation: u64,
    builder: Choice,
    entrypoint: Choice,
    output: Choice,
    bound: bool,
    binding_id: Choice,
    /// The envelope: whatever a page wants to smuggle past the declaration.
    options: Vec<(String, Json)>,
    preset: Option<String>,
    /// Keys set on the request object itself, over the top of the declared
    /// ones. `deny_unknown_fields` should refuse an unknown one outright.
    extra: Vec<(String, Json)>,
}

fuzz_target!(|input: Input| {
    check_path(&input.path);

    let mut options = serde_json::Map::new();
    for (key, value) in input.options.iter().take(16) {
        options.insert(key.clone(), value.value());
    }

    let mut raw = serde_json::Map::new();
    // Smuggled keys go in first and the declaration goes on top of them, so
    // that `sent_` below really is what the request said. What is left of
    // them is whatever the shape does not know about, which `deny_unknown_
    // fields` has to refuse rather than ignore.
    for (key, value) in input.extra.iter().take(8) {
        raw.insert(key.clone(), value.value());
    }
    let sent_kind = input.kind.of(&["preview", "build"]);
    let sent_builder = input.builder.of(&["quarto", "calepin", "typst", "tex"]);
    let sent_entrypoint = input
        .entrypoint
        .of(&["index.qmd", "a/b.md", "../x.qmd", "x.tex"]);
    let sent_output = input.output.of(&["html", "pdf", "docx", "revealjs"]);
    let sent_binding = input.binding_id.of(&["b1", ""]);

    raw.insert("protocol".into(), json!(u32::from(input.protocol % 4)));
    raw.insert("kind".into(), json!(sent_kind));
    raw.insert("project".into(), json!(input.project));
    raw.insert("origin".into(), json!(input.origin));
    raw.insert("snapshot".into(), json!(input.snapshot));
    raw.insert("generation".into(), json!(input.generation));
    raw.insert("builder".into(), json!(sent_builder));
    raw.insert("entrypoint".into(), json!(sent_entrypoint));
    raw.insert("output".into(), json!(sent_output));
    raw.insert("options".into(), Value::Object(options));
    raw.insert(
        "workspace".into(),
        if input.bound {
            json!({"mode": "bound", "binding_id": sent_binding})
        } else {
            json!({"mode": "snapshot", "binding_id": sent_binding})
        },
    );
    if let Some(preset) = &input.preset {
        raw.insert("preset".into(), json!(preset));
    }

    let Ok(preview) = decode_preview(Value::Object(raw)) else {
        // A refusal is the safe answer and needs no checking. What matters is
        // that it is a refusal and not a panic, and not an acceptance.
        return;
    };

    // Whatever came back was accepted, so every field of it is a field the
    // service will act on.
    assert_eq!(preview.protocol, 2, "a preview was admitted off protocol 2");
    // Every field of an admitted preview is required, so there is nothing to
    // unwrap: `decode_preview` either fills all of them in from one validated
    // shape or refuses. That it is bound is still worth asserting -- a
    // snapshot preview is refused, and this is where that would show.
    let WorkspaceRequest::Bound { binding_id } = &preview.workspace else {
        panic!("a managed preview was admitted without a bound workspace");
    };
    assert!(
        !binding_id.is_empty() && binding_id.len() <= 256,
        "a managed preview was admitted with an unusable binding"
    );
    let builder = preview.builder.as_str();
    assert!(
        matches!(builder, "quarto" | "calepin"),
        "a managed preview was admitted for builder {builder:?}"
    );

    // The declaration, as the request made it. These are what the adapter is
    // told to render, and they are what the envelope must not have moved.
    assert_eq!(
        preview.entrypoint, sent_entrypoint,
        "the request came back naming a different file than it sent"
    );
    assert_eq!(preview.output, sent_output);
    assert_eq!(preview.builder, sent_builder);
    assert_eq!(*binding_id, sent_binding);
    let entrypoint = preview.entrypoint.as_str();
    let output = preview.output.as_str();
    assert!(
        safe_relative_path(entrypoint),
        "a managed preview was admitted with entrypoint {entrypoint:?}"
    );
    check_path(entrypoint);

    // One adapter, never neither and never both, chosen by the builder the
    // request named. That used to be two optional fields and an assertion
    // that exactly one of them was set; it is now the shape of the value,
    // and what is left to check is that the shape agrees with the builder.
    match &preview.inputs {
        PreviewInputs::Quarto(quarto) => {
            assert_eq!(builder, "quarto", "a Quarto adapter for builder {builder:?}");
            assert_eq!(
                quarto.main, entrypoint,
                "the options envelope moved the file that will be rendered"
            );
            assert_eq!(
                quarto.format, output,
                "the options envelope moved the format that will be produced"
            );
            assert_eq!(
                quarto.binding_id, *binding_id,
                "the options envelope moved the workspace that will be read"
            );
            // Every extra file the adapter is told to copy is a path the
            // check above accepts, so none of them reaches outside the
            // project.
            for path in &quarto.data_inputs {
                assert!(
                    safe_relative_path(path),
                    "a declared data input {path:?} was admitted"
                );
                check_path(path);
            }
        }
        PreviewInputs::Calepin(calepin) => {
            assert_eq!(
                builder, "calepin",
                "a Calepin adapter for builder {builder:?}"
            );
            assert_eq!(calepin.main, entrypoint);
            assert_eq!(calepin.format, output);
            assert_eq!(calepin.binding_id, *binding_id);
        }
    }
});

/// A path the string check accepts is a path that stays under the root it is
/// joined to. The string check knows about `/` and the filesystem knows about
/// components; this is where the two have to agree.
fn check_path(path: &str) {
    if !safe_relative_path(path) {
        return;
    }
    let root = Path::new("/srv/project");
    let joined = root.join(path);
    assert!(
        joined.starts_with(root),
        "a safe path {path:?} joined to {root:?} and left it"
    );
    let mut depth = 0_i64;
    for part in joined
        .strip_prefix(root)
        .expect("it starts with the root")
        .components()
    {
        match part {
            Component::Normal(_) => depth += 1,
            // `.` is dropped by the string check, `..` would take the join
            // back out, and a root or prefix component would replace it
            // outright.
            other => panic!("a safe path {path:?} carried the component {other:?}"),
        }
    }
    assert!(
        depth > 0,
        "a safe path {path:?} named the root itself rather than something in it"
    );
}
