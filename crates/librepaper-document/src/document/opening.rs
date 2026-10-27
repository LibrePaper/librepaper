//! Which file of a project renders on its own, and which one it opens on.
//!
//! A project has no main file. These rules read the text of every file and
//! nothing else, so the browser and the server reach the same answer from the
//! same files. `web/src/lib/opening.js` is the twin of this module, and both
//! pass `web/tests/fixtures/opening.json`.

use std::collections::{BTreeMap, BTreeSet};

use librepaper_base::lines::lines;

use super::render::document_format;

/// The files that include each file, sorted. Only paths present in `texts`
/// appear, and a file never includes itself.
pub fn includers(texts: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, body) in texts {
        let Some(format) = document_format(path) else {
            continue;
        };
        for name in included_names(format, body) {
            if let Some(target) = resolve(format, path, &name, texts) {
                if &target != path {
                    found.entry(target).or_default().insert(path.clone());
                }
            }
        }
    }
    found
        .into_iter()
        .map(|(target, from)| (target, from.into_iter().collect()))
        .collect()
}

/// Whether `path` renders on its own: it is a document and nothing makes it
/// a part of another one. A file with no document format never does.
pub fn renders_alone(
    path: &str,
    texts: &BTreeMap<String, String>,
    includers: &BTreeMap<String, Vec<String>>,
) -> bool {
    let included = includers.get(path).is_some_and(|from| !from.is_empty());
    match document_format(path) {
        Some("latex") => texts.get(path).is_some_and(|body| has_document_class(body)),
        Some("typst") => !included,
        Some("quarto") | Some("markdown") => !base_name(path).starts_with('_') && !included,
        Some("html") => true,
        _ => false,
    }
}

/// The file a project opens on, or `None` for a project with no text.
///
/// Paths sort case-insensitively, ties broken by byte order. The first match
/// of these wins: a top-level file that renders on its own, is not included
/// and is not a README or LICENSE; the same over the whole tree; any file
/// that renders on its own, top level first; any text file, top level first.
pub fn opening_file(texts: &BTreeMap<String, String>) -> Option<String> {
    let includers = includers(texts);
    let mut paths: Vec<&str> = texts.keys().map(String::as_str).collect();
    paths.sort_by_key(|path| (path.to_lowercase(), *path));
    let top: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|path| !path.contains('/'))
        .collect();
    let free = |path: &&str| {
        renders_alone(path, texts, &includers)
            && !includers.contains_key(*path)
            && !is_readme_or_license(path)
    };
    let alone = |path: &&str| renders_alone(path, texts, &includers);

    top.iter()
        .copied()
        .find(free)
        .or_else(|| paths.iter().copied().find(free))
        .or_else(|| top.iter().copied().find(alone))
        .or_else(|| paths.iter().copied().find(alone))
        .or_else(|| top.first().copied())
        .or_else(|| paths.first().copied())
        .map(str::to_string)
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_readme_or_license(path: &str) -> bool {
    let stem = base_name(path).split('.').next().unwrap_or("");
    stem.eq_ignore_ascii_case("README") || stem.eq_ignore_ascii_case("LICENSE")
}

fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |at| &path[..at])
}

/// Joins a folder and a relative name, folding `.` and `..`. A name that
/// climbs out of the project root gives `None`.
fn join(folder: &str, name: &str) -> Option<String> {
    let mut parts: Vec<&str> = if folder.is_empty() {
        Vec::new()
    } else {
        folder.split('/').collect()
    };
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// The file a name inside `from` points at, when it is a text of the project.
fn resolve(
    format: &str,
    from: &str,
    name: &str,
    texts: &BTreeMap<String, String>,
) -> Option<String> {
    let folder = folder_of(from);
    let found = |candidate: Option<String>| candidate.filter(|path| texts.contains_key(path));
    match format {
        "latex" => {
            let name = if base_name(name).contains('.') {
                name.to_string()
            } else {
                format!("{name}.tex")
            };
            found(join(folder, &name)).or_else(|| found(join("", &name)))
        }
        "typst" => match name.strip_prefix('/') {
            Some(rooted) => found(join("", rooted)),
            None => found(join(folder, name)),
        },
        _ => found(join(folder, name)),
    }
}

/// The names a file's text includes, in the syntax of its format.
fn included_names(format: &str, body: &str) -> Vec<String> {
    match format {
        "latex" => latex_names(&strip_latex_comments(body)),
        "typst" => typst_names(body),
        "quarto" | "markdown" => shortcode_names(body),
        _ => Vec::new(),
    }
}

/// The text with every LaTeX comment removed. A comment starts at a `%` that
/// an even number of backslashes precedes.
fn strip_latex_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for line in lines(body).map(|line| line.body) {
        let mut slashes = 0;
        let mut end = line.len();
        for (at, ch) in line.char_indices() {
            if ch == '%' && slashes % 2 == 0 {
                end = at;
                break;
            }
            slashes = if ch == '\\' { slashes + 1 } else { 0 };
        }
        out.push_str(&line[..end]);
        out.push('\n');
    }
    out
}

fn has_document_class(body: &str) -> bool {
    let text = strip_latex_comments(body);
    let mut rest = text.as_str();
    while let Some(at) = rest.find("\\documentclass") {
        rest = &rest[at + "\\documentclass".len()..];
        if !rest.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
            return true;
        }
    }
    false
}

/// The names `\input`, `\include` and `\subfile` point at. Whitespace of any
/// kind, newlines included, may sit between the command and its name. Only
/// `\input` takes TeX's unbraced form, a run of characters up to whitespace,
/// `%`, `\`, `{` or `}`. A longer command such as `\includegraphics` is not
/// one of these.
fn latex_names(text: &str) -> Vec<String> {
    const SPACE: [char; 6] = [' ', '\t', '\r', '\n', '\x0c', '\x0b'];
    let mut names = Vec::new();
    for command in ["\\input", "\\include", "\\subfile"] {
        let mut rest = text;
        while let Some(at) = rest.find(command) {
            rest = &rest[at + command.len()..];
            if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            let open = rest.trim_start_matches(SPACE);
            if let Some(braced) = open.strip_prefix('{') {
                if let Some(close) = braced.find('}') {
                    let name = braced[..close].trim();
                    if !name.is_empty() {
                        names.push(name.to_string());
                    }
                }
            } else if command == "\\input" {
                let end = open
                    .find(|c: char| SPACE.contains(&c) || matches!(c, '%' | '\\' | '{' | '}'))
                    .unwrap_or(open.len());
                if end > 0 {
                    names.push(open[..end].to_string());
                }
            }
        }
    }
    names
}

fn typst_names(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    for command in ["#include", "#import"] {
        let mut rest = body;
        while let Some(at) = rest.find(command) {
            rest = &rest[at + command.len()..];
            let Some(quoted) = rest.trim_start().strip_prefix('"') else {
                continue;
            };
            if let Some(close) = quoted.find('"') {
                let name = &quoted[..close];
                if !name.is_empty() && !name.starts_with('@') {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

fn shortcode_names(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find("{{<") {
        rest = &rest[at + 3..];
        let Some(call) = rest.trim_start().strip_prefix("include") else {
            continue;
        };
        if !call.starts_with(char::is_whitespace) {
            continue;
        }
        let call = call.split(">}}").next().unwrap_or("");
        if let Some(name) = call.split_whitespace().next() {
            let name = name.trim_matches(|ch| ch == '"' || ch == '\'');
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/tests/fixtures/opening.json");
        let raw = std::fs::read_to_string(path).expect("the opening corpus is readable");
        serde_json::from_str(&raw).expect("the opening corpus is JSON")
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .expect("a list")
            .iter()
            .map(|item| item.as_str().expect("a string").to_string())
            .collect()
    }

    #[test]
    fn the_shared_corpus_opens_the_same_way_on_both_sides() {
        let corpus = fixture();
        for case in corpus["cases"].as_array().expect("cases is a list") {
            let name = case["name"].as_str().unwrap_or("<unnamed>");
            let texts: BTreeMap<String, String> = case["files"]
                .as_object()
                .expect("files is a map")
                .iter()
                .map(|(path, body)| (path.clone(), body.as_str().unwrap_or("").to_string()))
                .collect();

            let expected_opening = case["opening"].as_str().map(str::to_string);
            assert_eq!(opening_file(&texts), expected_opening, "{name}: opening");

            let found = includers(&texts);
            let expected_includers: BTreeMap<String, Vec<String>> = case["includers"]
                .as_object()
                .expect("includers is a map")
                .iter()
                .map(|(path, from)| (path.clone(), strings(from)))
                .collect();
            assert_eq!(found, expected_includers, "{name}: includers");

            let alone = strings(&case["alone"]);
            for path in texts.keys() {
                assert_eq!(
                    renders_alone(path, &texts, &found),
                    alone.contains(path),
                    "{name}: {path} renders alone"
                );
            }
        }
    }
}
