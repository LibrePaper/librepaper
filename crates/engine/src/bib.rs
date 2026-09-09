//! Shared, recoverable BibTeX/BibLaTeX parsing for completion and citations.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use biblatex::{Bibliography, ChunksExt, Person};
use serde_json::{json, Value};

use crate::diagnostic::{Diagnostic, Severity};

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 4096;
const MAX_MACRO_BYTES: usize = 1024 * 1024;
const MAX_SELECTED_BIB_BYTES: usize = 64 * 1024 * 1024;
const MAX_CROSSREF_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub entry_type: String,
    pub authors: Vec<String>,
    pub year: Option<String>,
    pub title: Option<String>,
    pub container: Option<String>,
    pub doi: Option<String>,
    pub url: Option<String>,
    pub raw: BTreeMap<String, String>,
    /// Field values serialized by biblatex, retaining protected inner braces.
    pub raw_biblatex: BTreeMap<String, String>,
    pub path: String,
    pub line: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Library {
    pub entries: Vec<Entry>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compute the stable cache key for a parsed bibliography library.
///
/// Prose changes that do not alter bibliography declarations leave this key
/// unchanged. Resource declarations include their source line, while the
/// selected `.bib` paths and contents are hashed in deterministic order.
pub fn cache_key(main: &str, format: &str, source: &str, texts: &BTreeMap<String, String>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let main = clean_path(main);
    main.hash(&mut hasher);
    format.hash(&mut hasher);
    bibliography_names(format, source).hash(&mut hasher);
    let mut files: Vec<(String, &str)> = texts
        .iter()
        .filter_map(|(path, text)| {
            let path = clean_path(path);
            path.to_ascii_lowercase()
                .ends_with(".bib")
                .then_some((path, text.as_str()))
        })
        .collect();
    if main.to_ascii_lowercase().ends_with(".bib") {
        if let Some(file) = files.iter_mut().find(|(path, _)| *path == main) {
            file.1 = source;
        } else {
            files.push((main.clone(), source));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files.hash(&mut hasher);
    hasher.finish()
}

/// Parse the selected bibliography files in a document tree. `texts` is
/// keyed by relative path and `source` is the authoritative main-file text.
pub fn library(
    main: &str,
    format: &str,
    source: &str,
    texts: &BTreeMap<String, String>,
) -> Library {
    let main = clean_path(main);
    let mut tree = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (path, text) in texts {
        let normalized_path = clean_path(path);
        if normalized_path != main && !normalized_path.to_ascii_lowercase().ends_with(".bib") {
            continue;
        }
        if tree.len() >= MAX_FILES {
            diagnostics.push(file_diagnostic(
                path,
                "too many files in bibliography request",
            ));
        } else if text.len() > MAX_FILE_BYTES {
            diagnostics.push(file_diagnostic(
                path,
                "bibliography file exceeds the 16 MiB limit",
            ));
        } else {
            tree.insert(normalized_path, text.clone());
        }
    }
    if source.len() <= MAX_FILE_BYTES {
        tree.insert(main.clone(), source.to_string());
    } else if main.to_ascii_lowercase().ends_with(".bib") {
        diagnostics.push(file_diagnostic(
            &main,
            "main bibliography exceeds the 16 MiB limit",
        ));
    }
    let candidates: Vec<String> = tree
        .keys()
        .filter(|p| p.to_ascii_lowercase().ends_with(".bib"))
        .cloned()
        .collect();
    let named = bibliography_names(format, source);
    let mut selected = Vec::new();
    if named.is_empty() {
        selected = candidates;
    } else {
        for (name, line) in named {
            let found = resolve_path(&main, &name)
                .and_then(|wanted| candidates.iter().find(|p| **p == wanted).cloned());
            match found {
                Some(path) if !selected.contains(&path) => selected.push(path),
                Some(_) => {}
                None => {
                    let mut d = Diagnostic::spanless(
                        Severity::Error,
                        format!("bibliography file `{name}` is not in the document tree"),
                    );
                    d.file = main.clone();
                    d.line = line;
                    d.column = 1;
                    d.end_line = line;
                    d.end_column = name.chars().count() + 1;
                    diagnostics.push(d);
                }
            }
        }
        selected.sort();
    }

    let mut entries = BTreeMap::new();
    let mut duplicate = BTreeSet::new();
    let mut selected_bytes = 0usize;
    for path in selected {
        let Some(text) = tree.get(&path) else {
            continue;
        };
        if selected_bytes.saturating_add(text.len()) > MAX_SELECTED_BIB_BYTES {
            diagnostics.push(file_diagnostic(
                &path,
                "selected bibliographies exceed the 64 MiB limit",
            ));
            continue;
        }
        selected_bytes = selected_bytes.saturating_add(text.len());
        let parsed = parse_file(&path, text);
        diagnostics.extend(parsed.diagnostics);
        for entry in parsed.entries {
            if entries.contains_key(&entry.key) {
                if duplicate.insert(entry.key.clone()) {
                    diagnostics.push(entry_diagnostic(
                        &entry,
                        format!(
                            "duplicate bibliography key `{}`; earlier entry kept",
                            entry.key
                        ),
                    ));
                }
            } else {
                entries.insert(entry.key.clone(), entry);
            }
        }
    }
    let (entries, crossref_diagnostics) = resolve_crossrefs(entries);
    diagnostics.extend(crossref_diagnostics);
    Library {
        entries,
        diagnostics,
    }
}

/// Find missing citation keys separately from library parsing, so prose edits
/// do not invalidate a bibliography cache.
pub fn missing_citations(
    main: &str,
    format: &str,
    source: &str,
    library: &Library,
) -> Vec<Diagnostic> {
    let known: BTreeSet<&str> = library.entries.iter().map(|e| e.key.as_str()).collect();
    let mut seen = BTreeSet::new();
    citation_keys(format, source)
        .into_iter()
        .filter_map(|(key, line)| {
            if known.contains(key.as_str()) || !seen.insert(key.clone()) {
                return None;
            }
            let mut d = Diagnostic::spanless(
                Severity::Error,
                format!("citation key `{key}` has no bibliography entry"),
            );
            d.file = clean_path(main);
            d.line = line;
            d.column = 1;
            d.end_line = line;
            d.end_column = key.chars().count() + 2;
            Some(d)
        })
        .collect()
}

/// JSON request/response wrapper used by the browser ABI.
pub fn analyze_json(input: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(input).map_err(|e| format!("invalid request JSON: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "bibliography request must be a JSON object".to_string())?;
    let main = required(object, "main")?;
    let format = required(object, "format")?;
    let source = required(object, "source")?;
    let mut texts = BTreeMap::new();
    if let Some(map) = object.get("texts") {
        for (path, text) in map
            .as_object()
            .ok_or_else(|| "bibliography request `texts` must be an object".to_string())?
        {
            texts.insert(
                path.clone(),
                text.as_str()
                    .ok_or_else(|| format!("bibliography text `{path}` must be a string"))?
                    .to_string(),
            );
        }
    }
    let result = library(main, format, source, &texts);
    let entries: Vec<Value> = result
        .entries
        .iter()
        .map(|e| {
            json!({
                "key": e.key, "type": e.entry_type, "authors": e.authors, "year": e.year,
                "title": e.title, "container": e.container, "doi": e.doi, "url": e.url,
                "raw": e.raw, "raw_biblatex": e.raw_biblatex, "path": e.path, "line": e.line
            })
        })
        .collect();
    let diagnostics: Vec<Value> = result
        .diagnostics
        .iter()
        .filter_map(|d| serde_json::from_str(&d.to_json()).ok())
        .collect();
    serde_json::to_string(&json!({"entries": entries, "diagnostics": diagnostics}))
        .map_err(|e| e.to_string())
}

fn required<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("bibliography request requires string `{key}`"))
}

struct ParsedFile {
    entries: Vec<Entry>,
    diagnostics: Vec<Diagnostic>,
}
struct Block {
    start: usize,
    text: String,
    complete: bool,
    kind: String,
}

fn parse_file(path: &str, source: &str) -> ParsedFile {
    let blocks = scan_blocks(source);
    let mut diagnostics = Vec::new();
    let mut macros = String::new();
    let mut macro_limit = false;
    for block in blocks
        .iter()
        .filter(|b| b.kind.eq_ignore_ascii_case("string"))
    {
        let candidate = format!("{macros}{}", block.text);
        if !block.complete || Bibliography::parse(&candidate).is_err() {
            diagnostics.push(block_diagnostic(
                path,
                source,
                block,
                "malformed @string definition",
            ));
        } else if macros.len().saturating_add(block.text.len()) <= MAX_MACRO_BYTES {
            macros.push_str(&block.text);
        } else if !macro_limit {
            diagnostics.push(block_diagnostic(
                path,
                source,
                block,
                "bibliography macro definitions exceed the 1 MiB limit; remaining macros ignored",
            ));
            macro_limit = true;
        }
    }
    let mut entries = Vec::new();
    for block in blocks {
        if ["string", "comment", "preamble"]
            .iter()
            .any(|kind| block.kind.eq_ignore_ascii_case(kind))
        {
            continue;
        }
        if !block.complete {
            diagnostics.push(block_diagnostic(
                path,
                source,
                &block,
                "malformed bibliography entry",
            ));
            continue;
        }
        let parsed = match Bibliography::parse(&format!("{macros}{}", block.text)) {
            Ok(b) => b,
            Err(error) => {
                diagnostics.push(block_diagnostic(
                    path,
                    source,
                    &block,
                    &format!("malformed bibliography entry: {error}"),
                ));
                continue;
            }
        };
        let Some(entry) = parsed.into_iter().next() else {
            continue;
        };
        let mut raw = BTreeMap::new();
        let mut raw_biblatex = BTreeMap::new();
        for (field, chunks) in &entry.fields {
            raw.insert(field.clone(), chunks.format_verbatim().trim().to_string());
            raw_biblatex.insert(field.clone(), chunks.to_biblatex_string(false));
        }
        let authors = entry
            .author()
            .ok()
            .unwrap_or_default()
            .into_iter()
            .map(person_display)
            .collect();
        let item = Entry {
            key: entry.key,
            entry_type: entry.entry_type.to_string().to_ascii_lowercase(),
            authors,
            year: field(&raw, &["year"]).or_else(|| field(&raw, &["date"]).and_then(first_year)),
            title: field(&raw, &["title"]),
            container: field(
                &raw,
                &[
                    "journaltitle",
                    "journal",
                    "booktitle",
                    "eventtitle",
                    "series",
                ],
            ),
            doi: field(&raw, &["doi"]).map(|v| v.trim_start_matches("doi:").trim().to_string()),
            url: field(&raw, &["url"]),
            raw,
            raw_biblatex,
            path: path.to_string(),
            line: source[..block.start]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                + 1,
        };
        entries.push(item);
    }
    ParsedFile {
        entries,
        diagnostics,
    }
}

fn person_display(person: Person) -> String {
    format!("{} {}", person.given_name, person.name)
        .trim()
        .to_string()
}
fn field(raw: &BTreeMap<String, String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| raw.get(*n).filter(|v| !v.is_empty()).cloned())
}
fn first_year(value: String) -> Option<String> {
    value
        .as_bytes()
        .windows(4)
        .position(|w| w.iter().all(u8::is_ascii_digit))
        .map(|i| value[i..i + 4].to_string())
}

fn resolve_crossrefs(mut all: BTreeMap<String, Entry>) -> (Vec<Entry>, Vec<Diagnostic>) {
    let keys: Vec<String> = all.keys().cloned().collect();
    for key in &keys {
        let inherited = inherited(key, &all, &mut BTreeSet::new(), 0);
        let inherited_serialized = inherited_serialized(key, &all, &mut BTreeSet::new(), 0);
        let parent_container = parent_container(key, &all);
        if let Some(entry) = all.get_mut(key) {
            for (field, value) in inherited {
                entry.raw.entry(field).or_insert(value);
            }
            for (field, value) in inherited_serialized {
                entry.raw_biblatex.entry(field).or_insert(value);
            }
            // BibLaTeX's crossref convention uses a proceedings/book title as
            // the child's `booktitle`. A child title must remain its own title;
            // only fill the container field when the child did not provide it.
            if !entry.raw.contains_key("booktitle") {
                if let Some((value, serialized)) = &parent_container {
                    entry.raw.insert("booktitle".to_string(), value.clone());
                    entry
                        .raw_biblatex
                        .insert("booktitle".to_string(), serialized.clone());
                }
            }
            if entry.authors.is_empty() {
                entry.authors = entry
                    .raw
                    .get("author")
                    .map(|v| parse_authors(v))
                    .unwrap_or_default();
            }
            entry.title = field(&entry.raw, &["title"]);
            entry.year = field(&entry.raw, &["year"])
                .or_else(|| field(&entry.raw, &["date"]).and_then(first_year));
            entry.container = field(
                &entry.raw,
                &[
                    "journaltitle",
                    "journal",
                    "booktitle",
                    "eventtitle",
                    "series",
                ],
            );
            entry.doi = field(&entry.raw, &["doi"])
                .map(|v| v.trim_start_matches("doi:").trim().to_string());
            entry.url = field(&entry.raw, &["url"]);
        }
    }
    let mut diagnostics = Vec::new();
    let mut seen = BTreeSet::new();
    for key in &keys {
        inspect_crossref(key, &all, &mut Vec::new(), &mut seen, &mut diagnostics);
    }
    (all.into_values().collect(), diagnostics)
}
fn parent_container(key: &str, all: &BTreeMap<String, Entry>) -> Option<(String, String)> {
    let entry = all.get(key)?;
    if !matches!(
        entry.entry_type.as_str(),
        "inproceedings" | "incollection" | "inbook" | "inreference" | "inset"
    ) {
        return None;
    }
    let parent_key = entry.raw.get("crossref")?.trim();
    let parent = all.get(parent_key)?;
    if !matches!(
        parent.entry_type.as_str(),
        "proceedings" | "book" | "collection" | "anthology"
    ) {
        return None;
    }
    let value = parent
        .raw
        .get("booktitle")
        .or_else(|| parent.raw.get("title"))?;
    let serialized = parent
        .raw_biblatex
        .get("booktitle")
        .or_else(|| parent.raw_biblatex.get("title"))?;
    Some((value.clone(), serialized.clone()))
}
fn parse_authors(value: &str) -> Vec<String> {
    Bibliography::parse(&format!("@article{{x,author={{{value}}}}}"))
        .ok()
        .and_then(|b| b.into_iter().next())
        .and_then(|e| e.author().ok())
        .map(|v| v.into_iter().map(person_display).collect())
        .unwrap_or_else(|| {
            value
                .split(" and ")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect()
        })
}
fn inherited(
    key: &str,
    all: &BTreeMap<String, Entry>,
    seen: &mut BTreeSet<String>,
    depth: usize,
) -> BTreeMap<String, String> {
    if depth >= MAX_CROSSREF_DEPTH || !seen.insert(key.to_string()) {
        return BTreeMap::new();
    }
    let Some(e) = all.get(key) else {
        return BTreeMap::new();
    };
    let Some(parent) = e.raw.get("crossref").map(|v| v.trim()) else {
        return BTreeMap::new();
    };
    let mut out = inherited(parent, all, seen, depth + 1);
    if let Some(p) = all.get(parent) {
        for (k, v) in &p.raw {
            if !k.starts_with("__") {
                out.entry(k.clone()).or_insert(v.clone());
            }
        }
    }
    out
}
fn inherited_serialized(
    key: &str,
    all: &BTreeMap<String, Entry>,
    seen: &mut BTreeSet<String>,
    depth: usize,
) -> BTreeMap<String, String> {
    if depth >= MAX_CROSSREF_DEPTH || !seen.insert(key.to_string()) {
        return BTreeMap::new();
    }
    let Some(e) = all.get(key) else {
        return BTreeMap::new();
    };
    let Some(parent) = e.raw.get("crossref").map(|v| v.trim()) else {
        return BTreeMap::new();
    };
    let mut out = inherited_serialized(parent, all, seen, depth + 1);
    if let Some(p) = all.get(parent) {
        for (k, v) in &p.raw_biblatex {
            if !k.starts_with("__") {
                out.entry(k.clone()).or_insert(v.clone());
            }
        }
    }
    out
}
fn inspect_crossref(
    key: &str,
    all: &BTreeMap<String, Entry>,
    stack: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    if stack.len() >= MAX_CROSSREF_DEPTH {
        if seen.insert(format!("depth:{key}")) {
            if let Some(entry) = all.get(key) {
                out.push(entry_diagnostic(
                    entry,
                    format!("crossref chain exceeds {MAX_CROSSREF_DEPTH} entries"),
                ));
            }
        }
        return;
    }
    if let Some(i) = stack.iter().position(|v| v == key) {
        let cycle = stack[i..].join(" -> ") + " -> " + key;
        if seen.insert(format!("cycle:{cycle}")) {
            if let Some(e) = all.get(key) {
                out.push(entry_diagnostic(
                    e,
                    format!("crossref cycle detected: {cycle}"),
                ));
            }
        }
        return;
    }
    let Some(e) = all.get(key) else { return };
    let Some(parent) = e.raw.get("crossref").map(|v| v.trim()) else {
        return;
    };
    if !all.contains_key(parent) {
        if seen.insert(format!("missing:{key}:{parent}")) {
            out.push(entry_diagnostic(
                e,
                format!("crossref parent `{parent}` is not in the bibliography"),
            ));
        }
        return;
    }
    stack.push(key.to_string());
    inspect_crossref(parent, all, stack, seen, out);
    stack.pop();
}

fn scan_blocks(source: &str) -> Vec<Block> {
    let bytes = source.as_bytes();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(rel) = source[cursor..].find('@') else {
            break;
        };
        let start = cursor + rel;
        if let Some(p) = source[cursor..start].find('%') {
            cursor = source[cursor + p..]
                .find('\n')
                .map(|n| cursor + p + n + 1)
                .unwrap_or(bytes.len());
            continue;
        }
        let mut k = start + 1;
        while k < bytes.len() && bytes[k].is_ascii_alphabetic() {
            k += 1;
        }
        if k == start + 1 {
            cursor = start + 1;
            continue;
        }
        let mut open = k;
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if open >= bytes.len() || (bytes[open] != b'{' && bytes[open] != b'(') {
            cursor = k;
            continue;
        }
        let close = if bytes[open] == b'{' { b'}' } else { b')' };
        let mut depth = 1;
        let mut braces = 0;
        let mut quote = false;
        let mut escaped = false;
        let mut comment = false;
        let mut i = open + 1;
        let mut end = None;
        while i < bytes.len() {
            let b = bytes[i];
            if comment {
                if b == b'\n' {
                    comment = false
                };
                i += 1;
                continue;
            }
            if !quote && braces == 0 && depth == 1 && b == b'@' {
                break;
            }
            if !quote && braces == 0 && b == b'%' {
                comment = true;
                i += 1;
                continue;
            }
            if b == b'"' && braces == 0 && !escaped {
                quote = !quote;
            }
            if !quote {
                if b == b'{' && !escaped {
                    braces += 1
                } else if b == b'}' && braces > 0 && !escaped {
                    braces -= 1
                } else if braces == 0 && b == bytes[open] && !escaped {
                    depth += 1
                } else if braces == 0 && b == close && !escaped {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
            }
            escaped = b == b'\\' && !escaped;
            if b != b'\\' {
                escaped = false;
            }
            i += 1;
        }
        let (end, complete) = end.map(|e| (e, true)).unwrap_or_else(|| {
            (
                source[start + 1..]
                    .find('@')
                    .map(|n| start + 1 + n)
                    .unwrap_or(bytes.len()),
                false,
            )
        });
        blocks.push(Block {
            start,
            text: source[start..end].to_string(),
            complete,
            kind: source[start + 1..k].to_string(),
        });
        cursor = end.max(start + 1);
    }
    blocks
}

fn clean_path(path: &str) -> String {
    let mut out = Vec::new();
    let normalized = path.replace('\\', "/");
    for p in normalized.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => {
                out.push(p);
            }
        }
    }
    out.join("/")
}
fn resolve_path(main: &str, name: &str) -> Option<String> {
    let name = name.trim().trim_matches(['"', '\'']);
    if name.starts_with('/') {
        return None;
    }
    let name = if name.to_ascii_lowercase().ends_with(".bib") {
        name.to_string()
    } else {
        format!("{name}.bib")
    };
    let parent = main.rsplit_once('/').map(|v| v.0).unwrap_or("");
    let mut parts: Vec<String> = parent
        .split('/')
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect();
    let normalized = name.replace('\\', "/");
    for p in normalized.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p.to_string()),
        }
    }
    Some(parts.join("/"))
}
fn bibliography_names(format: &str, source: &str) -> Vec<(String, usize)> {
    let f = format.to_ascii_lowercase();
    if f.contains("markdown") || f == "md" || f == "quarto" || f == "qmd" {
        markdown_names(source)
    } else if f.contains("typst") || f == "typ" {
        typst_names(source)
    } else if f.contains("latex") || f == "tex" {
        latex_names(source)
    } else {
        Vec::new()
    }
}
fn markdown_names(source: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut list_indent = None;
    let mut front = false;
    for (i, line) in source.lines().enumerate() {
        let t = line.trim();
        if i == 0 && t == "---" {
            front = true;
            continue;
        }
        if front && (t == "---" || t == "...") {
            break;
        }
        if !front {
            continue;
        }
        if let Some(v) = t.strip_prefix("bibliography:") {
            if v.trim().is_empty() {
                let base = line.len() - line.trim_start().len();
                list_indent = Some(if base == 0 { 0 } else { base + 1 })
            } else {
                list_indent = None;
                out.extend(names(&without_yaml_comment(v), i + 1))
            }
        } else if let Some(indent) = list_indent {
            let n = line.len() - line.trim_start().len();
            if !t.is_empty() && n < indent {
                list_indent = None
            } else if n >= indent && t.starts_with('-') {
                out.extend(names(
                    &without_yaml_comment(t.trim_start_matches('-')),
                    i + 1,
                ))
            } else if !t.is_empty() && !t.starts_with('-') && n <= indent {
                list_indent = None
            }
        }
    }
    out
}
fn typst_names(source: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let cleaned = source
        .lines()
        .map(|line| without_comment(line, "//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut at = 0;
    while let Some(offset) = cleaned[at..].find("#bibliography") {
        let start = at + offset;
        let rest = cleaned[start + 13..].trim_start();
        if !rest.starts_with('(') {
            at = start + 1;
            continue;
        }
        let rest = rest[1..].trim_start();
        if !rest.starts_with('"') {
            at = start + 1;
            continue;
        }
        if let Some(end) = rest[1..].find('"') {
            let line = cleaned[..start].bytes().filter(|b| *b == b'\n').count() + 1;
            out.push((rest[1..1 + end].to_string(), line));
            at = start + 13 + 1 + end + 2;
        } else {
            at = start + 1;
        }
    }
    out
}
fn latex_names(source: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let cleaned = source
        .lines()
        .map(|line| without_comment(line, "%"))
        .collect::<Vec<_>>()
        .join("\n");
    for command in ["addbibresource", "addglobalbib", "bibliography"] {
        let needle = format!("\\{command}");
        let mut at = 0;
        while let Some(offset) = cleaned[at..].find(&needle) {
            let start = at + offset;
            let rest = cleaned[start + needle.len()..].trim_start();
            if !rest.starts_with('{') {
                at = start + 1;
                continue;
            }
            if let Some(end) = rest[1..].find('}') {
                let line = cleaned[..start].bytes().filter(|b| *b == b'\n').count() + 1;
                for name in rest[1..1 + end].split(',') {
                    if !name.trim().is_empty() {
                        out.push((name.trim().to_string(), line));
                    }
                }
                at = start + needle.len() + 1 + end + 1;
            } else {
                at = start + 1;
            }
        }
    }
    out
}
fn names(value: &str, line: usize) -> Vec<(String, usize)> {
    value
        .trim()
        .trim_matches(['[', ']'])
        .split(',')
        .filter_map(|v| {
            let v = v.trim().trim_matches(['"', '\'']);
            (!v.is_empty()).then(|| (v.to_string(), line))
        })
        .collect()
}
fn without_yaml_comment(value: &str) -> String {
    let mut quote = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if matches!(character, '"' | '\'') && !escaped {
            quote = !quote;
        }
        if character == '#'
            && !quote
            && (index == 0 || value[..index].ends_with(char::is_whitespace))
        {
            return value[..index].trim_end().to_string();
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    value.to_string()
}
fn without_comment(line: &str, marker: &str) -> String {
    let mut quote = false;
    let mut escaped = false;
    let mut i = 0;
    while i < line.len() {
        let c = line[i..].chars().next().expect("UTF-8 boundary");
        if c == '"' && !escaped {
            quote = !quote
        }
        if !quote && line[i..].starts_with(marker) {
            return line[..i].to_string();
        }
        escaped = c == '\\' && !escaped;
        if c != '\\' {
            escaped = false
        }
        i += c.len_utf8()
    }
    line.to_string()
}
fn file_diagnostic(path: &str, message: &str) -> Diagnostic {
    let mut d = Diagnostic::spanless(Severity::Error, message);
    d.file = clean_path(path);
    d.line = 1;
    d.column = 1;
    d.end_line = 1;
    d.end_column = 1;
    d
}
fn entry_diagnostic(entry: &Entry, message: String) -> Diagnostic {
    let mut d = file_diagnostic(&entry.path, &message);
    d.line = entry.line;
    d.end_line = entry.line;
    d
}
fn block_diagnostic(path: &str, source: &str, block: &Block, message: &str) -> Diagnostic {
    let line = source[..block.start]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1;
    let mut d = file_diagnostic(
        path,
        &format!(
            "{message}: {}",
            block.text.chars().take(240).collect::<String>()
        ),
    );
    d.line = line;
    d.end_line = line;
    d.end_column = source[block.start..]
        .lines()
        .next()
        .map(|v| v.chars().count() + 1)
        .unwrap_or(1);
    d
}

fn citation_keys(format: &str, source: &str) -> Vec<(String, usize)> {
    if format.to_ascii_lowercase().contains("latex") {
        Vec::new()
    } else {
        let mut out = Vec::new();
        for (i, line) in source.lines().enumerate() {
            for part in line.split('@').skip(1) {
                let key: String = part
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, ':' | '_' | '-' | '.' | '+'))
                    .collect();
                if !key.is_empty() {
                    out.push((key, i + 1));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tree(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items
            .iter()
            .map(|(p, v)| (p.to_string(), v.to_string()))
            .collect()
    }
    #[test]
    fn parses_fields_macros_and_protected_values() {
        let b="@string{venue={Journal}}\n@article{k,author={Smith, Jane},title={A {NASA} Study},journal=venue,year=2020}";
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", b)]));
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].authors, vec!["Jane Smith"]);
        assert!(r.entries[0].raw_biblatex["title"].contains("{NASA}"));
    }
    #[test]
    fn chained_string_macros_resolve_in_order() {
        let bib = "@string{base={Journal}}\n@string{venue=base # { of Testing}}\n@article{k,journal=venue,title={T}}";
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", bib)]));
        assert_eq!(
            r.entries[0].container.as_deref(),
            Some("Journal of Testing")
        );
    }
    #[test]
    fn scanner_accepts_quotes_and_escaped_braces_inside_values() {
        let bib = r#"@article{k,title={A "quoted" \{brace\} value},year={2024}}"#;
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", bib)]));
        assert_eq!(r.entries.len(), 1);
        assert!(r.diagnostics.is_empty());
    }
    #[test]
    fn malformed_entry_recovers() {
        let b = "@article{bad,title={oops\n@article{good,title={Good}}";
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", b)]));
        assert!(r.entries.iter().any(|e| e.key == "good"));
        assert!(!r.diagnostics.is_empty());
    }
    #[test]
    fn selection_is_relative_and_comments_ignored() {
        let r = library(
            "chapters/main.typ",
            "typst",
            "// #bibliography(\"refs.bib\")\n#bibliography(\"refs.bib\")",
            &tree(&[
                ("refs.bib", "@book{root,title={Root}}"),
                ("chapters/refs.bib", "@book{local,title={Local}}"),
            ]),
        );
        assert_eq!(r.entries[0].key, "local");
    }
    #[test]
    fn bibliography_yaml_list_does_not_consume_next_key() {
        let source = "---\nbibliography:\n  - refs.bib\nauthor:\n  - someone\n---\n# Paper";
        let r = library(
            "main.md",
            "markdown",
            source,
            &tree(&[("refs.bib", "@book{ok,title={OK}}")]),
        );
        assert_eq!(r.entries.len(), 1);
        assert!(r.diagnostics.is_empty());
    }
    #[test]
    fn bibliography_yaml_allows_unindented_list_and_inline_comments() {
        let source = "---\nbibliography: refs.bib # local refs\n---\n# Paper";
        let r = library(
            "main.md",
            "markdown",
            source,
            &tree(&[("refs.bib", "@book{ok,title={OK}}")]),
        );
        assert_eq!(r.entries.len(), 1);
        let source = "---\nbibliography:\n- refs.bib\nauthor:\n- someone\n---\n# Paper";
        let r = library(
            "main.md",
            "markdown",
            source,
            &tree(&[("refs.bib", "@book{ok,title={OK}}")]),
        );
        assert_eq!(r.entries.len(), 1);
        assert!(r.diagnostics.is_empty());
    }
    #[test]
    fn duplicate_and_crossref_diagnostics() {
        let b = "@book{a,crossref={missing}}\n@book{one,crossref={two}}\n@book{two,crossref={one}}";
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", b)]));
        assert!(r.diagnostics.iter().any(|d| d.message.contains("parent")));
        assert!(r.diagnostics.iter().any(|d| d.message.contains("cycle")));
    }
    #[test]
    fn duplicate_keys_keep_earlier_path() {
        let r = library(
            "main.md",
            "markdown",
            "# P",
            &tree(&[
                ("a.bib", "@book{k,title={First}}"),
                ("b.bib", "@book{k,title={Second}}"),
            ]),
        );
        assert_eq!(r.entries[0].title.as_deref(), Some("First"));
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.message.contains("duplicate")));
    }
    #[test]
    fn crossref_inherits_parent_summary_fields() {
        let bib = "@proceedings{p,booktitle={Rust Proceedings},year={2023}}\n@inproceedings{c,title={Child},crossref={p}}";
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", bib)]));
        let child = r.entries.iter().find(|e| e.key == "c").expect("child");
        assert_eq!(child.year.as_deref(), Some("2023"));
        assert_eq!(child.container.as_deref(), Some("Rust Proceedings"));
    }
    #[test]
    fn crossref_maps_parent_title_to_child_container() {
        let bib = "@proceedings{p,title={Conference},year={2020}}\n@inproceedings{c,title={Paper},crossref={p}}";
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", bib)]));
        let child = r.entries.iter().find(|e| e.key == "c").expect("child");
        assert_eq!(child.title.as_deref(), Some("Paper"));
        assert_eq!(child.container.as_deref(), Some("Conference"));
        assert!(child.raw_biblatex["booktitle"].contains("Conference"));
    }
    #[test]
    fn json_validation_rejects_wrong_request_shape() {
        assert!(analyze_json("[]").is_err());
        assert!(analyze_json(r#"{"main":"main.md"}"#).is_err());
    }
    #[test]
    fn long_crossref_chain_is_bounded() {
        let mut bib = String::new();
        for index in 0..=MAX_CROSSREF_DEPTH {
            bib.push_str(&format!("@book{{k{index},crossref={{k{}}}}}\n", index + 1));
        }
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", &bib)]));
        assert!(r.diagnostics.iter().any(|d| d.message.contains("exceeds")));
    }
    #[test]
    fn unicode_malformed_excerpt_is_safe() {
        let body = "é".repeat(400);
        let bib = format!("@article{{bad,title={{{body}}}");
        let r = library("main.md", "markdown", "# P", &tree(&[("refs.bib", &bib)]));
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.message.contains("malformed")));
    }
    #[test]
    fn multiline_format_directives_are_selected() {
        let typst = "#bibliography(\n  \"refs.bib\"\n)";
        let latex = "\\addbibresource\n{refs.bib}";
        let refs = tree(&[("refs.bib", "@book{ok,title={OK}}")]);
        assert_eq!(library("main.typ", "typst", typst, &refs).entries.len(), 1);
        assert_eq!(library("main.tex", "latex", latex, &refs).entries.len(), 1);
    }
    #[test]
    fn cache_key_ignores_unrelated_prose_but_tracks_bib_content() {
        let texts = tree(&[("refs.bib", "@book{k,title={One}}")]);
        let first = cache_key("main.md", "markdown", "# P", &texts);
        assert_eq!(
            first,
            cache_key("main.md", "markdown", "# P\nMore prose", &texts)
        );
        let changed = tree(&[("refs.bib", "@book{k,title={Two}}")]);
        assert_ne!(first, cache_key("main.md", "markdown", "# P", &changed));
    }
    #[test]
    fn cache_key_tracks_comment_toggle_in_declaration() {
        let texts = tree(&[("refs.bib", "@book{k,title={One}}")]);
        let commented = cache_key(
            "main.typ",
            "typst",
            "// #bibliography(\"refs.bib\")",
            &texts,
        );
        let active = cache_key("main.typ", "typst", "#bibliography(\"refs.bib\")", &texts);
        assert_ne!(commented, active);
    }
}
