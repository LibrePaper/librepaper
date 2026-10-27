//! The local bridge protocol: what a page in the browser may say to the
//! loopback service, and what the service is allowed to believe of it.
//!
//! This is the one surface where a request turns into files on the author's
//! own machine, so two things are asserted about it, and they are the two
//! things a page would try to break.
//!
//! **A safe path is a path that stays put.** `RelPath` is a
//! string check -- no leading slash, no `..`, no backslash, no empty
//! component -- and what happens afterwards is a `Path::join` under a project
//! root. Those are two different notions of a path, and the target holds them
//! against each other: whatever the string check accepts, joining it under a
//! root must give a path still under that root, with nothing but ordinary
//! components between them.
//!
//! **A job names a tool record and never a command.** `JobRequest::decode`
//! reads a format, an output, an entry file and a binding, and refuses every
//! other key, so a page that smuggles a `command`, `args`, `env` or a nested
//! envelope past the declaration is refused rather than ignored. The target
//! sends exactly that, with any keys and any values, and asserts that an
//! admitted request carries what was declared and no key beyond the known
//! ones.
//!
//! And nothing anywhere may panic: the service answers this on loopback, so a
//! panic here is a page in a browser stopping the author's render service.
#![no_main]

use std::path::{Component, Path};

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use librepaper::protocol::{JobRequest, RelPath};
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

/// A field the mutator would essentially never guess: a format has to be one
/// of four words, an output one of three, an entrypoint a safe path. Left to
/// arbitrary strings the door never opens and the target passes without having
/// tried anything. So each of those is a choice between the words that get
/// through and a string that may not.
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
    /// Narrow, so that version 3 -- the only one with a door -- comes up often.
    protocol: u8,
    project: String,
    origin: String,
    snapshot: String,
    generation: u64,
    format: Choice,
    output: Choice,
    entrypoint: Choice,
    binding_id: Choice,
    /// The options object: only `deadline_seconds` is known to it.
    options: Vec<(String, Json)>,
    /// Keys set on the request object itself, under the declared ones.
    /// `deny_unknown_fields` should refuse an unknown one outright.
    extra: Vec<(String, Json)>,
}

/// The keys a job may carry.
const KNOWN: [&str; 11] = [
    "protocol",
    "project",
    "origin",
    "snapshot",
    "generation",
    "format",
    "output",
    "entrypoint",
    "binding_id",
    "manifest",
    "options",
];

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
    let sent_format = input.format.of(&["latex", "typst", "markdown", "quarto"]);
    let sent_output = input.output.of(&["pdf", "html", "docx"]);
    let sent_entrypoint = input
        .entrypoint
        .of(&["main.tex", "chapters/main.tex", "../x.qmd", "paper.typ"]);
    let sent_binding = input.binding_id.of(&["hosted", ""]);

    raw.insert("protocol".into(), json!(u32::from(input.protocol % 4)));
    raw.insert("project".into(), json!(input.project));
    raw.insert("origin".into(), json!(input.origin));
    raw.insert("snapshot".into(), json!(input.snapshot));
    raw.insert("generation".into(), json!(input.generation));
    raw.insert("format".into(), json!(sent_format));
    raw.insert("output".into(), json!(sent_output));
    raw.insert("entrypoint".into(), json!(sent_entrypoint));
    raw.insert("binding_id".into(), json!(sent_binding));
    raw.insert("options".into(), Value::Object(options));
    raw.entry("manifest").or_insert_with(|| json!([]));
    let unknown_key = raw.keys().any(|key| !KNOWN.contains(&key.as_str()));

    let Ok(job) = JobRequest::decode(Value::Object(raw)) else {
        // A refusal is the safe answer and needs no checking. What matters is
        // that it is a refusal and not a panic, and not an acceptance.
        return;
    };

    assert!(!unknown_key, "a job with a key it does not know was admitted");
    assert_eq!(job.protocol, 3, "a job was admitted off protocol {}", job.protocol);
    // The declaration, as the request made it. These pick the tool record,
    // and they are what nothing else in the request may move.
    assert_eq!(job.format, sent_format);
    assert_eq!(job.output, sent_output);
    assert_eq!(job.entrypoint, sent_entrypoint);
    assert_eq!(job.binding_id, sent_binding);
    assert!(
        matches!(job.format.as_str(), "latex" | "typst" | "markdown" | "quarto"),
        "a job was admitted for format {:?}",
        job.format
    );
    assert!(
        matches!(job.output.as_str(), "pdf" | "html" | "docx"),
        "a job was admitted for output {:?}",
        job.output
    );
    assert!(
        RelPath::is_local(&job.entrypoint),
        "a job was admitted with entrypoint {:?}",
        job.entrypoint
    );
    check_path(&job.entrypoint);
    assert!(
        !job.binding_id.is_empty() && job.binding_id.len() <= 256,
        "a job was admitted with an unusable binding"
    );
    assert!(
        (1..=3600).contains(&job.options.deadline_seconds),
        "a job was admitted with a deadline outside the supported range"
    );
});

/// A path the string check accepts is a path that stays under the root it is
/// joined to. The string check knows about `/` and the filesystem knows about
/// components; this is where the two have to agree.
fn check_path(path: &str) {
    if !RelPath::is_local(path) {
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
