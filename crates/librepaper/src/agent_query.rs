//! Bounded reads over an immutable source and annotation snapshot.
//!
//! This module deliberately has no knowledge of HTTP, rooms, or handles. A
//! caller captures a `QuerySnapshot` atomically and asks `read` for projections.
//! Source ranges are byte ranges into the UTF-8 strings in the snapshot.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The immutable source and annotation state used by one query group.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct QuerySnapshot {
    #[serde(default)]
    pub source_revision: String,
    #[serde(default)]
    pub annotation_revision: u64,
    #[serde(default)]
    pub main: String,
    /// Canonical tree metadata. It is never returned whole.
    #[serde(default)]
    pub tree: Value,
    #[serde(default)]
    pub texts: BTreeMap<String, String>,
    #[serde(default)]
    pub comments: Vec<Value>,
}

/// Limits applying to one query result.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryBudget {
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
}

const fn default_max_bytes() -> usize {
    12_000
}
const fn default_max_tokens() -> usize {
    3_000
}

impl Default for QueryBudget {
    fn default() -> Self {
        Self {
            max_bytes: default_max_bytes(),
            max_tokens: default_max_tokens(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Cursor {
    version: u8,
    revision: String,
    kind: String,
    query: String,
    offset: usize,
}

#[derive(Clone)]
struct HeadingIndex {
    start: usize,
    end: usize,
    line: usize,
    level: usize,
    title_start: usize,
    title: String,
}

#[derive(Clone)]
struct BibliographyIndex {
    start: usize,
    end: usize,
    key: String,
    entry_type: String,
    author: Value,
    title: Value,
}

#[derive(Clone)]
struct SearchIndex {
    // Search caches hold many hits; share one allocation per scanned path.
    path: Arc<str>,
    start: usize,
    end: usize,
    line: usize,
    column: usize,
}

#[derive(Clone)]
struct TextOffsets {
    checkpoints: Vec<OffsetCheckpoint>,
}

#[derive(Clone)]
struct OffsetCheckpoint {
    byte: usize,
    chars: usize,
    line: usize,
    line_start_chars: usize,
}

struct SnapshotIndexes {
    headings: BTreeMap<String, Vec<HeadingIndex>>,
    bibliography: BTreeMap<String, Vec<BibliographyIndex>>,
    partial_headings: BTreeSet<String>,
    partial_bibliography: BTreeSet<String>,
    identities: BTreeMap<String, (String, String)>,
    cited: BTreeSet<String>,
    offsets: BTreeMap<String, TextOffsets>,
    searches: Mutex<BTreeMap<String, Vec<SearchIndex>>>,
}

type IndexCache = Mutex<VecDeque<(String, Arc<SnapshotIndexes>)>>;

static INDEX_CACHE: OnceLock<IndexCache> = OnceLock::new();
const INDEX_CACHE_ENTRIES: usize = 4;
const SEARCH_CACHE_MATCHES: usize = 8_192;
const MAX_OFFSET_ITEMS: usize = 65_536;
const MAX_STRUCTURAL_ITEMS: usize = 16_384;

fn snapshot_index_key(snapshot: &QuerySnapshot) -> String {
    let mut hash = Sha256::new();
    hash.update(snapshot.source_revision.as_bytes());
    hash.update([0]);
    hash.update(snapshot.annotation_revision.to_le_bytes());
    for (path, source) in &snapshot.texts {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update((source.len() as u64).to_le_bytes());
        // Revisions are authoritative for immutable snapshots. These short
        // sentinels also keep tests and accidental mutable callers from
        // reusing an index after replacing a file without changing its size.
        hash.update(&source.as_bytes()[..source.len().min(16)]);
        hash.update(&source.as_bytes()[source.len().saturating_sub(16)..]);
    }
    hex::encode(hash.finalize())
}

fn snapshot_indexes(snapshot: &QuerySnapshot) -> Arc<SnapshotIndexes> {
    let key = snapshot_index_key(snapshot);
    let cache = INDEX_CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    {
        let mut entries = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(index) = entries.iter().position(|(cached, _)| cached == &key) {
            let entry = entries.remove(index).expect("index cache position exists");
            entries.push_front(entry.clone());
            return entry.1;
        }
    }
    let mut headings = BTreeMap::new();
    let mut bibliography = BTreeMap::new();
    let mut partial_headings = BTreeSet::new();
    let mut partial_bibliography = BTreeSet::new();
    let mut identities = BTreeMap::new();
    let mut offsets = BTreeMap::new();
    let mut structural_items = MAX_STRUCTURAL_ITEMS;
    for (path, source) in &snapshot.texts {
        let (heading_items, heading_partial) = scan_headings(source, structural_items);
        structural_items = structural_items.saturating_sub(heading_items.len());
        if heading_partial {
            partial_headings.insert(path.clone());
        }
        headings.insert(path.clone(), heading_items);
        let (bibliography_items, bibliography_partial) =
            scan_bibliography(source, structural_items);
        structural_items = structural_items.saturating_sub(bibliography_items.len());
        if bibliography_partial {
            partial_bibliography.insert(path.clone());
        }
        bibliography.insert(path.clone(), bibliography_items);
        identities.insert(path.clone(), file_identity(snapshot, path, source));
        offsets.insert(path.clone(), scan_offsets(source));
    }
    let indexes = Arc::new(SnapshotIndexes {
        headings,
        bibliography,
        partial_headings,
        partial_bibliography,
        identities,
        cited: cited_keys(snapshot),
        offsets,
        searches: Mutex::new(BTreeMap::new()),
    });
    let mut entries = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    entries.push_front((key, Arc::clone(&indexes)));
    while entries.len() > INDEX_CACHE_ENTRIES {
        entries.pop_back();
    }
    indexes
}

fn scan_headings(source: &str, item_limit: usize) -> (Vec<HeadingIndex>, bool) {
    if item_limit == 0 {
        return (Vec::new(), true);
    }
    let mut headings = Vec::new();
    let mut fence: Option<&str> = None;
    let mut byte = 0;
    for (line_number, line_with_end) in source.split_inclusive('\n').enumerate() {
        let line = line_with_end.strip_suffix('\n').unwrap_or(line_with_end);
        let trimmed = line.trim_start_matches([' ', '\t']);
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = &trimmed[..3];
            fence = if fence == Some(marker) {
                None
            } else if fence.is_none() {
                Some(marker)
            } else {
                fence
            };
        } else if fence.is_none() {
            if let Some((level, title_start, title)) = is_heading_line(line) {
                headings.push(HeadingIndex {
                    start: byte,
                    end: byte + line.len(),
                    line: line_number + 1,
                    level,
                    title_start: byte + title_start,
                    title: title.to_string(),
                });
                if headings.len() >= item_limit {
                    return (headings, true);
                }
            }
        }
        byte += line_with_end.len();
    }
    (headings, false)
}

fn scan_offsets(source: &str) -> TextOffsets {
    let mut checkpoints = vec![OffsetCheckpoint {
        byte: 0,
        chars: 0,
        line: 1,
        line_start_chars: 0,
    }];
    let mut chars = 0;
    let mut line = 1;
    let mut line_start_chars = 0;
    for (byte, character) in source.char_indices() {
        chars += 1;
        if character == '\n' {
            line += 1;
            line_start_chars = chars;
        }
        if chars % 256 == 0 && checkpoints.len() < MAX_OFFSET_ITEMS {
            checkpoints.push(OffsetCheckpoint {
                byte: byte + character.len_utf8(),
                chars,
                line,
                line_start_chars,
            });
        }
    }
    TextOffsets { checkpoints }
}

fn indexed_position(source: &str, offsets: Option<&TextOffsets>, at: usize) -> (usize, usize) {
    let Some(offsets) = offsets else {
        return source_position(source, at);
    };
    let checkpoint = offsets
        .checkpoints
        .partition_point(|checkpoint| checkpoint.byte <= at)
        .saturating_sub(1);
    let checkpoint = &offsets.checkpoints[checkpoint];
    if checkpoint.byte < at
        && offsets
            .checkpoints
            .last()
            .is_some_and(|last| last.byte < at)
        && offsets.checkpoints.len() >= MAX_OFFSET_ITEMS
    {
        return source_position(source, at);
    }
    let mut line = checkpoint.line;
    let mut line_start_chars = checkpoint.line_start_chars;
    let mut chars = checkpoint.chars;
    for character in source[checkpoint.byte..at].chars() {
        chars += 1;
        if character == '\n' {
            line += 1;
            line_start_chars = chars;
        }
    }
    (line, chars.saturating_sub(line_start_chars) + 1)
}

fn scan_bibliography(source: &str, item_limit: usize) -> (Vec<BibliographyIndex>, bool) {
    if item_limit == 0 {
        return (Vec::new(), true);
    }
    let mut entries = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find('@') {
        let start = cursor + relative;
        let tail = &source[start + 1..];
        let open = match (tail.find('{'), tail.find('(')) {
            (Some(brace), Some(paren)) => brace.min(paren),
            (Some(brace), None) => brace,
            (None, Some(paren)) => paren,
            (None, None) => return (entries, true),
        };
        let open_at = start + 1 + open;
        let open_byte = source.as_bytes()[open_at];
        let Some(end) = balanced_entry_end(source, open_at, open_byte) else {
            // A malformed tail is an honest partial index. Keep entries
            // already found and let callers distinguish it from an empty
            // bibliography.
            return (entries, true);
        };
        let key_start = open_at + 1;
        let key_end = source[key_start..end - 1]
            .find(',')
            .map_or(end - 1, |i| key_start + i);
        let key = source[key_start..key_end].trim();
        if key.is_empty() {
            cursor = end;
            continue;
        }
        let body = &source[key_end..end];
        entries.push(BibliographyIndex {
            start,
            end,
            key: key.to_string(),
            entry_type: source[start + 1..start + 1 + open].trim().to_string(),
            author: field_value(body, "author"),
            title: field_value(body, "title"),
        });
        if entries.len() >= item_limit {
            return (entries, true);
        }
        // Continue after the balanced entry. Looking from key_end could find
        // an @ embedded in a field value and overlap the next page.
        cursor = end;
    }
    (entries, false)
}

fn balanced_entry_end(source: &str, open_at: usize, open_byte: u8) -> Option<usize> {
    let close_byte = match open_byte {
        b'{' => b'}',
        b'(' => b')',
        _ => return None,
    };
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (relative, byte) in source[open_at..].bytes().enumerate() {
        if quoted {
            if byte == b'"' && !escaped {
                quoted = false;
            }
            escaped = byte == b'\\' && !escaped;
            continue;
        }
        match byte {
            b'"' => quoted = true,
            byte if byte == open_byte => depth = depth.saturating_add(1),
            byte if byte == close_byte => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open_at + relative + 1);
                }
            }
            _ => {}
        }
        escaped = false;
    }
    None
}

/// Execute bounded, discriminated document queries against `snapshot`.
///
/// Source/context queries accept `path` and a byte range (`start`/`end`,
/// `start_byte`/`end_byte`, or a two element array). Search accepts `query`
/// (or `text`) and an optional `path`. An inline selection object is accepted;
/// an opaque selection string is reported as unsupported so the parent service
/// can resolve it first.
pub fn read(
    snapshot: &QuerySnapshot,
    queries: &[Value],
    budget: QueryBudget,
) -> Result<Value, String> {
    if queries.is_empty() {
        return Ok(envelope(snapshot, Vec::new(), true, None, None, budget));
    }
    if queries.len() > 8 {
        return Err("at most 8 queries may be sent in one read".into());
    }
    let mut results = Vec::new();
    let mut complete = true;
    let mut reason = None;
    let mut next_cursor = None;
    for (query_index, query) in queries.iter().enumerate() {
        let object = query
            .as_object()
            .ok_or("each query must be a JSON object")?;
        validate_query_keys(object)?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("query is missing string field kind")?;
        let mut page = object.clone();
        let used = encoded_cost(&envelope(
            snapshot,
            results.clone(),
            complete,
            reason.clone(),
            next_cursor.clone(),
            budget,
        ))
        .0;
        let mut available = QueryBudget {
            max_bytes: budget.max_bytes.saturating_sub(used),
            max_tokens: budget.max_tokens,
        };
        let mut admitted = false;
        for _ in 0..16 {
            let (candidate, query_complete, query_cursor) =
                execute(snapshot, &page, kind, available)?;
            let mut proposed = results.clone();
            proposed.push(candidate.clone());
            let projected = envelope(
                snapshot,
                proposed,
                complete && query_complete,
                reason.clone(),
                query_cursor.clone(),
                budget,
            );
            if within_budget(&projected, budget)
                && !(candidate["blocks"][0]["source"] == "" && candidate["complete"] == false)
            {
                complete &= query_complete;
                if !query_complete && reason.is_none() {
                    reason = Some("query".into());
                }
                results.push(candidate);
                next_cursor = query_cursor;
                admitted = true;
                break;
            }
            available.max_bytes = available.max_bytes.saturating_mul(3) / 4;
            let limit = page.get("limit").and_then(Value::as_u64).unwrap_or(24);
            page.insert("limit".into(), json!((limit / 2).max(1)));
        }
        if !admitted {
            // Never advance past evidence that was not delivered. The caller
            // retries this same query with a narrower scope or larger budget.
            complete = false;
            reason = Some(format!("budget: query {query_index} not returned"));
            next_cursor = None;
            break;
        }
    }
    let result = envelope(snapshot, results, complete, reason, next_cursor, budget);
    if result
        .get("returned_bytes")
        .and_then(Value::as_u64)
        .is_some_and(|bytes| bytes as usize > budget.max_bytes)
    {
        return Err("query budget is too small for the result envelope".into());
    }
    Ok(result)
}

fn validate_query_keys(query: &Map<String, Value>) -> Result<(), String> {
    // Keep this list explicit so misspelled fields cannot silently change a
    // query's meaning. The parent MCP schema may expose a narrower subset.
    const ALLOWED: &[&str] = &[
        "kind",
        "path",
        "file",
        "paths",
        "files",
        "line",
        "end_line",
        "start",
        "end",
        "range",
        "range_bytes",
        "range_id",
        "query",
        "queries",
        "text",
        "id",
        "key",
        "keys",
        "selection",
        "context",
        "include",
        "limit",
        "cursor",
        "revision",
    ];
    if let Some(unknown) = query.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("unsupported query field {unknown:?}"));
    }
    Ok(())
}

fn execute(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    kind: &str,
    budget: QueryBudget,
) -> Result<(Value, bool, Option<String>), String> {
    let cursor = decode_cursor(snapshot, query, kind)?;
    let offset = cursor.as_ref().map_or(0, |c| c.offset);
    let fingerprint = fingerprint(query);
    match kind {
        "context"
            if !query.contains_key("path")
                && !query.contains_key("start")
                && !query.contains_key("line")
                && !query.contains_key("selection") =>
        {
            Ok((json!({"kind":"context","main":snapshot.main}), true, None))
        }
        "source" | "context" | "section" => source_query(
            snapshot,
            query,
            kind == "context",
            offset,
            &fingerprint,
            budget,
        ),
        "search" => search_query(snapshot, query, offset, &fingerprint),
        "outline" | "headings" => outline_query(snapshot, query, offset, &fingerprint),
        "manifest" | "files" => manifest_query(snapshot, query, offset, &fingerprint),
        "bibliography" => bibliography_query(snapshot, query, offset, &fingerprint),
        "thread" => thread_query(snapshot, query, offset, &fingerprint),
        "diagnostics" | "changes" | "rendered" => {
            optional_projection(snapshot, query, kind, offset, &fingerprint)
        }
        other => Ok((
            json!({"kind":other,"status":"unsupported","reason":"unknown query kind"}),
            false,
            None,
        )),
    }
}

fn source_query(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    context: bool,
    cursor_offset: usize,
    fingerprint: &str,
    budget: QueryBudget,
) -> Result<(Value, bool, Option<String>), String> {
    let output_kind = if context {
        "context"
    } else if query.get("kind").and_then(Value::as_str) == Some("section") {
        "section"
    } else {
        "source"
    };
    let mut path = query
        .get("path")
        .or_else(|| query.get("file"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if query.get("range_id").is_some()
        && query.get("range").is_none()
        && query.get("selection").is_none()
    {
        return Ok((
            json!({
                "kind": output_kind,
                "status":"unsupported",
                "reason":"range handles must be resolved by the parent service"
            }),
            false,
            None,
        ));
    }
    let mut range = query.get("range");
    if let Some(selection) = query.get("selection") {
        match selection {
            Value::Object(selection) => {
                if path.is_empty() {
                    path = selection
                        .get("path")
                        .or_else(|| selection.get("file"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                if range.is_none() {
                    range = selection.get("range");
                }
                if range.is_none() {
                    return Ok((
                        json!({
                            "kind": output_kind,
                            "status":"unsupported",
                            "reason":"source selections must be resolved by the parent service"
                        }),
                        false,
                        None,
                    ));
                }
            }
            Value::String(_) => {
                return Ok((
                    json!({
                        "kind": output_kind,
                        "status":"unsupported",
                        "reason":"selection handles must be resolved by the parent service"
                    }),
                    false,
                    None,
                ))
            }
            _ => return Err("selection must be an object or opaque string handle".into()),
        }
    }
    if path.is_empty() {
        path.clone_from(&snapshot.main);
    }
    let source = snapshot
        .texts
        .get(&path)
        .ok_or_else(|| format!("no source file {path:?}"))?;
    let direct_range =
        if range.is_none() && (query.contains_key("start") || query.contains_key("end")) {
            match (query.get("start"), query.get("end")) {
                (Some(start), Some(end)) => {
                    Some((as_usize(start, "range start")?, as_usize(end, "range end")?))
                }
                _ => return Err("range requires both start and end byte offsets".into()),
            }
        } else {
            None
        };
    let line_range = line_number_range(source, query).transpose()?;
    let (mut start, mut end) = match direct_range {
        Some(range) => range,
        None => parse_range(range.or_else(|| query.get("range_bytes")))?
            .or(line_range)
            .unwrap_or((0, source.len())),
    };
    if output_kind == "section" {
        let wanted = query
            .get("query")
            .and_then(Value::as_str)
            .ok_or("section requires an exact heading title in query")?;
        (start, end) = section_range(snapshot, &path, source, wanted)?;
    }
    if context {
        let context_name = query
            .get("context")
            .and_then(Value::as_str)
            .unwrap_or("paragraph");
        (start, end) = context_range(source, start, context_name)?;
    }
    if cursor_offset > start {
        start = cursor_offset.min(end);
    }
    validate_range(source, start, end)?;
    let requested_end = end;
    let available = budget.max_bytes.saturating_sub(1024);
    let room = available.min(end.saturating_sub(start));
    let mut emitted_end = start + room;
    while emitted_end > start && !source.is_char_boundary(emitted_end) {
        emitted_end -= 1;
    }
    let partial = emitted_end < requested_end;
    let page_cursor = if partial {
        Some(encode_cursor(
            snapshot,
            output_kind,
            fingerprint,
            emitted_end,
        ))
    } else {
        None
    };
    let block = range_value(snapshot, &path, source, start, emitted_end, partial);
    Ok((
        json!({
            "kind": output_kind,
            "blocks":[block], "complete":!partial, "next_cursor":page_cursor
        }),
        !partial,
        page_cursor,
    ))
}

fn section_range(
    snapshot: &QuerySnapshot,
    path: &str,
    source: &str,
    wanted: &str,
) -> Result<(usize, usize), String> {
    let indexes = snapshot_indexes(snapshot);
    if indexes.partial_headings.contains(path) {
        return Err("section index is partial; request an explicit source range".into());
    }
    let headings = indexes.headings.get(path).cloned().unwrap_or_default();
    let matching = headings
        .iter()
        .enumerate()
        .filter(|(_, heading)| heading.title == wanted)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(
            "section title must identify one heading; use a source range for duplicate headings"
                .into(),
        );
    }
    let (index, heading) = matching[0];
    let start = heading.start;
    let level = heading.level;
    let end = headings[index + 1..]
        .iter()
        .find(|next| next.level <= level)
        .map(|next| next.start)
        .unwrap_or(source.len());
    Ok((start, end))
}

fn line_number_range(
    source: &str,
    query: &Map<String, Value>,
) -> Option<Result<(usize, usize), String>> {
    let line = query.get("line")?.as_u64()?;
    let end_line = query
        .get("end_line")
        .and_then(Value::as_u64)
        .unwrap_or(line);
    if line == 0 || end_line < line {
        return Some(Err(
            "line and end_line must be positive, increasing numbers".into(),
        ));
    }
    let mut start = None;
    let mut end = source.len();
    let mut byte = 0;
    for (current, line_with_end) in (1u64..).zip(source.split_inclusive('\n')) {
        if current == line {
            start = Some(byte);
        }
        if current == end_line {
            end = byte + line_with_end.len();
            break;
        }
        byte += line_with_end.len();
    }
    let Some(start) = start else {
        if line == 1 && source.is_empty() {
            return Some(Ok((0, 0)));
        }
        return Some(Err("starting line is past the end of the file".into()));
    };
    Some(Ok((start, end)))
}

fn parse_range(value: Option<&Value>) -> Result<Option<(usize, usize)>, String> {
    let Some(value) = value else { return Ok(None) };
    if let Some(numbers) = value.as_array() {
        if numbers.len() != 2 {
            return Err("range array must contain [start, end]".into());
        }
        return Ok(Some((
            as_usize(&numbers[0], "range start")?,
            as_usize(&numbers[1], "range end")?,
        )));
    }
    let object = value
        .as_object()
        .ok_or("range must be an object or [start, end]")?;
    let start = object
        .get("start_byte")
        .or_else(|| object.get("start"))
        .or_else(|| object.get("from"));
    let end = object
        .get("end_byte")
        .or_else(|| object.get("end"))
        .or_else(|| object.get("to"));
    match (start, end) {
        (Some(start), Some(end)) => Ok(Some((
            as_usize(start, "range start")?,
            as_usize(end, "range end")?,
        ))),
        (None, None) => Ok(None),
        _ => Err("range requires both start and end byte offsets".into()),
    }
}

fn as_usize(value: &Value, label: &str) -> Result<usize, String> {
    value
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| format!("{label} must be a non-negative integer"))
}

fn validate_range(source: &str, start: usize, end: usize) -> Result<(), String> {
    if start > end || end > source.len() {
        return Err(format!("source range {start}..{end} is outside the file"));
    }
    if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err("source range must fall on UTF-8 boundaries".into());
    }
    Ok(())
}

fn context_range(source: &str, anchor: usize, context: &str) -> Result<(usize, usize), String> {
    if anchor > source.len() || !source.is_char_boundary(anchor) {
        return Err("context anchor must be a UTF-8 boundary".into());
    }
    let (mut start, mut end) = line_range(source, anchor);
    match context {
        "line" => {}
        "paragraph" => {
            start = source[..anchor].rfind("\n\n").map_or(0, |i| i + 2);
            end = source[anchor..]
                .find("\n\n")
                .map_or(source.len(), |i| anchor + i);
        }
        "heading" => {
            let mut cursor = start;
            while cursor > 0 {
                let previous_end = cursor - 1;
                let previous_start = source[..previous_end].rfind('\n').map_or(0, |i| i + 1);
                if is_heading_line(&source[previous_start..previous_end]).is_some() {
                    start = previous_start;
                    break;
                }
                cursor = previous_start;
            }
            end = source[end..].find("\n\n").map_or(source.len(), |i| end + i);
        }
        other => return Err(format!("unsupported context {other:?}")),
    }
    Ok((start, end))
}

fn line_range(source: &str, anchor: usize) -> (usize, usize) {
    let start = source[..anchor].rfind('\n').map_or(0, |i| i + 1);
    let end = source[anchor..]
        .find('\n')
        .map_or(source.len(), |i| anchor + i + 1);
    (start, end)
}

fn range_value(
    snapshot: &QuerySnapshot,
    path: &str,
    source: &str,
    start: usize,
    end: usize,
    partial: bool,
) -> Value {
    let (file_id, file_hash) = file_identity(snapshot, path, source);
    json!({
        "path":path, "file_id":file_id, "file_hash":file_hash,
        "source_revision":snapshot.source_revision,
        "start":start, "end":end,
        "range":{"start":start,"end":end,"start_byte":start,"end_byte":end},
        "source":&source[start..end], "partial":partial
    })
}

fn file_identity(snapshot: &QuerySnapshot, path: &str, source: &str) -> (String, String) {
    let entry = snapshot
        .tree
        .get("files")
        .and_then(Value::as_object)
        .and_then(|files| files.get(path))
        .or_else(|| snapshot.tree.get(path));
    let id = entry
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let hash = entry
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("sha").or_else(|| entry.get("hash")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| digest(source.as_bytes()));
    (id, hash)
}

fn search_query(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    offset: usize,
    fingerprint: &str,
) -> Result<(Value, bool, Option<String>), String> {
    let terms: Vec<String> = match query.get("queries") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or("search queries must be strings".to_string())
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("search queries must be an array".into()),
        None => vec![query
            .get("query")
            .or_else(|| query.get("text"))
            .and_then(Value::as_str)
            .ok_or("search requires string query")?
            .to_string()],
    };
    if terms.is_empty() || terms.iter().any(String::is_empty) {
        return Err("search text cannot be empty".into());
    }
    let paths = selected_paths(snapshot, query)?;
    let limit = query
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(24)
        .clamp(1, 24) as usize;
    let indexes = snapshot_indexes(snapshot);
    let search_key = serde_json::to_string(&(&paths, &terms)).unwrap_or_default();
    let (hits, base_offset, scan_complete, page_scan) = {
        let cached = indexes
            .searches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&search_key)
            .cloned();
        if let Some(hits) = cached {
            (hits, 0, true, false)
        } else {
            let (scanned, complete) = scan_search_all(snapshot, &indexes, &paths, &terms);
            if complete {
                indexes
                    .searches
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(search_key, scanned.clone());
                let mut searches = indexes
                    .searches
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while searches.len() > 16 {
                    let Some(oldest) = searches.keys().next().cloned() else {
                        break;
                    };
                    searches.remove(&oldest);
                }
                (scanned, 0, true, false)
            } else {
                let (page, complete) =
                    scan_search_page(snapshot, &indexes, &paths, &terms, offset, limit);
                (page, offset, complete, true)
            }
        }
    };
    let mut matches = Vec::new();
    for hit in hits
        .iter()
        .skip(offset.saturating_sub(base_offset))
        .take(limit)
    {
        let source = snapshot
            .texts
            .get(hit.path.as_ref())
            .ok_or("selected path disappeared")?;
        let prefix_start = source[..hit.start]
            .char_indices()
            .rev()
            .nth(32)
            .map_or(0, |(i, _)| i);
        let suffix_end = source[hit.end..]
            .char_indices()
            .nth(32)
            .map_or(source.len(), |(i, _)| hit.end + i);
        let file_id = indexes
            .identities
            .get(hit.path.as_ref())
            .map(|identity| identity.0.clone())
            .unwrap_or_default();
        matches.push(json!({
            "path":hit.path.as_ref(), "file_id":file_id,
            "start":hit.start, "end":hit.end,
            "range":{"start":hit.start,"end":hit.end,"start_byte":hit.start,"end_byte":hit.end},
            "source_revision":snapshot.source_revision, "exact":&source[hit.start..hit.end],
            "line":hit.line, "column":hit.column,
            "prefix":&source[prefix_start..hit.start], "suffix":&source[hit.end..suffix_end]
        }));
    }
    let end = offset.saturating_add(matches.len());
    let has_more = if page_scan {
        !scan_complete
    } else {
        end < hits.len()
    };
    if has_more {
        let cursor = encode_cursor(snapshot, "search", fingerprint, end);
        return Ok((
            json!({"kind":"search","matches":matches,"complete":false,"next_cursor":cursor}),
            false,
            Some(cursor),
        ));
    }
    Ok((
        json!({"kind":"search","matches":matches,"complete":true}),
        true,
        None,
    ))
}

fn scan_search_all(
    snapshot: &QuerySnapshot,
    indexes: &SnapshotIndexes,
    paths: &[String],
    terms: &[String],
) -> (Vec<SearchIndex>, bool) {
    let mut hits = Vec::new();
    for path in paths {
        let Some(source) = snapshot.texts.get(path) else {
            continue;
        };
        let path: Arc<str> = Arc::from(path.as_str());
        for term in terms {
            for (start, matched) in source.match_indices(term) {
                let (line, column) =
                    indexed_position(source, indexes.offsets.get(path.as_ref()), start);
                hits.push(SearchIndex {
                    path: Arc::clone(&path),
                    start,
                    end: start + matched.len(),
                    line,
                    column,
                });
                if hits.len() > SEARCH_CACHE_MATCHES {
                    return (hits, false);
                }
            }
        }
    }
    (hits, true)
}

fn scan_search_page(
    snapshot: &QuerySnapshot,
    indexes: &SnapshotIndexes,
    paths: &[String],
    terms: &[String],
    offset: usize,
    limit: usize,
) -> (Vec<SearchIndex>, bool) {
    let mut page = Vec::with_capacity(limit);
    let mut seen = 0;
    for path in paths {
        let Some(source) = snapshot.texts.get(path) else {
            continue;
        };
        let path: Arc<str> = Arc::from(path.as_str());
        for term in terms {
            for (start, matched) in source.match_indices(term) {
                if seen < offset {
                    seen += 1;
                    continue;
                }
                if page.len() >= limit {
                    return (page, false);
                }
                let (line, column) =
                    indexed_position(source, indexes.offsets.get(path.as_ref()), start);
                page.push(SearchIndex {
                    path: Arc::clone(&path),
                    start,
                    end: start + matched.len(),
                    line,
                    column,
                });
                seen += 1;
            }
        }
    }
    (page, true)
}

fn selected_paths(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
) -> Result<Vec<String>, String> {
    if let Some(path) = query.get("path").and_then(Value::as_str) {
        if !snapshot.texts.contains_key(path) {
            return Err(format!("no source file {path:?}"));
        }
        return Ok(vec![path.to_string()]);
    }
    if let Some(paths) = query.get("paths") {
        let paths = paths.as_array().ok_or("query paths must be an array")?;
        let mut selected = Vec::with_capacity(paths.len());
        for path in paths {
            let path = path.as_str().ok_or("query paths must be strings")?;
            if !snapshot.texts.contains_key(path) {
                return Err(format!("no source file {path:?}"));
            }
            selected.push(path.to_string());
        }
        return Ok(selected);
    }
    Ok(snapshot.texts.keys().cloned().collect())
}

fn source_position(source: &str, offset: usize) -> (usize, usize) {
    let line = source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    (line, source[line_start..offset].chars().count() + 1)
}

fn outline_query(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    offset: usize,
    fingerprint: &str,
) -> Result<(Value, bool, Option<String>), String> {
    let indexes = snapshot_indexes(snapshot);
    let mut headings = Vec::new();
    let mut partial = false;
    for path in selected_paths(snapshot, query)? {
        if !snapshot.texts.contains_key(&path) {
            return Err("selected path disappeared".into());
        }
        partial |= indexes.partial_headings.contains(&path);
        for heading in indexes.headings.get(&path).into_iter().flatten() {
            let file_id = indexes
                .identities
                .get(&path)
                .map(|identity| identity.0.as_str())
                .unwrap_or("");
            headings.push(json!({
                "path":path, "file_id":file_id,
                "start":heading.start, "end":heading.end,
                "line":heading.line, "level":heading.level, "text":heading.title,
                "range":{"start":heading.start,"end":heading.end,"start_byte":heading.start,"end_byte":heading.end},
                "title_range":{"start":heading.title_start,"end":heading.end,"start_byte":heading.title_start,"end_byte":heading.end},
                "source_revision":snapshot.source_revision
            }));
        }
    }
    let output_kind = query
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("outline");
    let page = paginate_array(
        output_kind,
        snapshot,
        fingerprint,
        headings,
        offset,
        query.get("limit"),
    );
    let complete = page.1 && !partial;
    Ok((
        // `partial` describes index coverage; it must not discard a cursor
        // for the page that was actually emitted. A caller can still consume
        // already indexed items while the index is marked incomplete.
        json!({"kind":output_kind,"headings":page.0["items"],"parser":"conservative-v1","partial":partial,"complete":complete,"next_cursor":page.0["next_cursor"]}),
        complete,
        page.2,
    ))
}

fn is_heading_line(line: &str) -> Option<(usize, usize, &str)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let trimmed = &line[indent..];
    let marker = trimmed.as_bytes().first().copied()?;
    let level = if marker == b'#' || marker == b'=' {
        trimmed.bytes().take_while(|byte| *byte == marker).count()
    } else {
        0
    };
    if (1..=6).contains(&level)
        && trimmed
            .as_bytes()
            .get(level)
            .is_some_and(u8::is_ascii_whitespace)
    {
        let title = trimmed[level..].trim();
        let title_start = indent
            + level
            + trimmed[level..]
                .find(|character: char| !character.is_whitespace())
                .unwrap_or(trimmed[level..].len());
        return Some((level, title_start, title));
    }
    for (command, level) in [
        ("\\chapter", 1),
        ("\\section", 2),
        ("\\subsection", 3),
        ("\\subsubsection", 4),
    ] {
        if let Some(rest) = trimmed.strip_prefix(command) {
            let rest = rest.trim_start_matches('*').trim_start();
            if let Some(title) = rest
                .strip_prefix('{')
                .and_then(|tail| tail.split_once('}').map(|(title, _)| title))
            {
                let title_start = line.len() - rest.len() + 1;
                return Some((level, title_start, title));
            }
        }
    }
    None
}

fn manifest_query(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    offset: usize,
    fingerprint: &str,
) -> Result<(Value, bool, Option<String>), String> {
    let mut paths = BTreeSet::new();
    paths.extend(snapshot.texts.keys().cloned());
    if let Some(files) = snapshot.tree.get("files").and_then(Value::as_object) {
        paths.extend(files.keys().cloned());
    }
    let mut files = Vec::new();
    for path in paths {
        let source = snapshot.texts.get(&path);
        let metadata = snapshot
            .tree
            .get("files")
            .and_then(Value::as_object)
            .and_then(|files| files.get(&path))
            .or_else(|| snapshot.tree.get(&path));
        let kind = metadata
            .and_then(|v| v.get("kind"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if source.is_some() {
                    "text".into()
                } else {
                    "asset".into()
                }
            });
        let mut item = json!({"path":path,"kind":kind,"main":path == snapshot.main});
        if let Some(source) = source {
            item["bytes"] = json!(source.len());
            let identity = file_identity(snapshot, &path, source);
            item["file_id"] = json!(identity.0);
            item["file_hash"] = json!(identity.1);
        } else if let Some(metadata) = metadata {
            if let Some(size) = metadata.get("size") {
                item["bytes"] = size.clone();
            }
            if let Some(id) = metadata.get("id") {
                item["file_id"] = id.clone();
            }
            if let Some(hash) = metadata.get("sha").or_else(|| metadata.get("hash")) {
                item["file_hash"] = hash.clone();
            }
        }
        files.push(item);
    }
    let page = paginate_array(
        "manifest",
        snapshot,
        fingerprint,
        files,
        offset,
        query.get("limit"),
    );
    let kind = if query.get("kind").and_then(Value::as_str) == Some("files") {
        "files"
    } else {
        "manifest"
    };
    Ok((
        json!({"kind":kind,"main":snapshot.main,"files":page.0["items"],"complete":page.0["complete"],"next_cursor":page.0["next_cursor"]}),
        page.1,
        page.2,
    ))
}

fn bibliography_query(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    offset: usize,
    fingerprint: &str,
) -> Result<(Value, bool, Option<String>), String> {
    let indexes = snapshot_indexes(snapshot);
    let paths = if let Some(path) = query.get("path").and_then(Value::as_str) {
        vec![path.to_string()]
    } else {
        snapshot
            .texts
            .keys()
            .filter(|path| path.ends_with(".bib") || path.ends_with(".bibtex"))
            .cloned()
            .collect()
    };
    let requested: Option<BTreeSet<String>> = if let Some(key) = query.get("key") {
        Some(
            std::iter::once(
                key.as_str()
                    .map(str::to_string)
                    .ok_or("bibliography key must be a string")?,
            )
            .collect(),
        )
    } else {
        query
            .get("keys")
            .map(|keys| {
                keys.as_array()
                    .ok_or("bibliography keys must be an array")?
                    .iter()
                    .map(|key| {
                        key.as_str()
                            .map(str::to_string)
                            .ok_or("bibliography keys must be strings")
                    })
                    .collect()
            })
            .transpose()?
    };
    let mut entries = Vec::new();
    let mut all_keys = BTreeSet::new();
    let mut partial = false;
    for path in paths {
        if !snapshot.texts.contains_key(&path) {
            return Err(format!("no source file {path:?}"));
        }
        partial |= indexes.partial_bibliography.contains(&path);
        for entry in indexes.bibliography.get(&path).into_iter().flatten() {
            all_keys.insert(entry.key.clone());
            if requested
                .as_ref()
                .is_none_or(|keys| keys.contains(&entry.key))
            {
                entries.push(json!({
                    "key":entry.key,"path":path,
                    "type":entry.entry_type,"author":entry.author,"title":entry.title,
                    "start":entry.start,"end":entry.end,
                    "range":{"start":entry.start,"end":entry.end,"start_byte":entry.start,"end_byte":entry.end},
                    "source_revision":snapshot.source_revision
                }));
            }
        }
    }
    let cited = indexes.cited.clone();
    let missing: Vec<_> = cited
        .difference(&all_keys)
        .cloned()
        .map(Value::String)
        .collect();
    let cited: Vec<_> = cited.into_iter().map(Value::String).collect();
    let duplicates = duplicate_keys(&entries);
    let page = paginate_array(
        "bibliography",
        snapshot,
        fingerprint,
        entries,
        offset,
        query.get("limit"),
    );
    let complete = page.1 && !partial;
    Ok((
        json!({"kind":"bibliography","entries":page.0["items"],"cited":cited,"missing":missing,
               "duplicates":duplicates,"partial":partial,"complete":complete,"next_cursor":page.0["next_cursor"]}),
        complete,
        page.2,
    ))
}

fn field_value(body: &str, field: &str) -> Value {
    let needle = format!("{field}=");
    body.split(',')
        .find_map(|part| {
            let part = part.trim();
            let value = part
                .strip_prefix(&needle)?
                .trim()
                .trim_matches(['{', '}', '"']);
            Some(Value::String(value.to_string()))
        })
        .unwrap_or(Value::Null)
}

fn cited_keys(snapshot: &QuerySnapshot) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for source in snapshot.texts.values() {
        for marker in ["\\cite{", "\\citep{", "\\citet{", "[@"] {
            let mut cursor = 0;
            while cursor < source.len() {
                let Some(found) = source[cursor..].find(marker) else {
                    break;
                };
                let start = cursor + found + marker.len();
                let end = if marker == "[@" {
                    source[start..]
                        .find(']')
                        .map_or(source.len(), |i| start + i)
                } else {
                    source[start..]
                        .find('}')
                        .map_or(source.len(), |i| start + i)
                };
                keys.extend(
                    source[start..end]
                        .split(',')
                        .map(str::trim)
                        .filter(|key| !key.is_empty())
                        .map(str::to_string),
                );
                cursor = end.saturating_add(1);
            }
        }
    }
    keys
}

fn duplicate_keys(entries: &[Value]) -> Vec<Value> {
    let mut counts = BTreeMap::new();
    for key in entries
        .iter()
        .filter_map(|entry| entry.get("key").and_then(Value::as_str))
    {
        *counts.entry(key.to_string()).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(key, count)| json!({"key":key,"count":count}))
        .collect()
}

fn thread_query(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    offset: usize,
    fingerprint: &str,
) -> Result<(Value, bool, Option<String>), String> {
    if query.get("id").and_then(Value::as_str).is_none() {
        let comments = snapshot
            .comments
            .iter()
            .filter(|c| {
                query.get("include").and_then(Value::as_str) != Some("unresolved")
                    || c["resolved"] != true
            })
            .map(|c| {
                let mut summary = c.clone();
                if let Some(object) = summary.as_object_mut() {
                    object.remove("replies");
                    object.remove("source");
                    object.remove("exact");
                    object.remove("prefix");
                    object.remove("suffix");
                    object.remove("proposed");
                }
                summary
            })
            .collect();
        let page = paginate_array(
            "thread",
            snapshot,
            fingerprint,
            comments,
            offset,
            query.get("limit"),
        );
        return Ok((
            json!({"kind":"thread","threads":page.0["items"],"complete":page.0["complete"],"next_cursor":page.0["next_cursor"]}),
            page.1,
            page.2,
        ));
    }
    let id = query
        .get("id")
        .and_then(Value::as_str)
        .ok_or("thread requires string id")?;
    let thread = snapshot
        .comments
        .iter()
        .find(|comment| comment.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| format!("unknown comment {id:?}"))?;
    let include = query
        .get("include")
        .and_then(Value::as_str)
        .unwrap_or("all");
    if include == "unresolved" && thread.get("resolved").and_then(Value::as_bool) == Some(true) {
        return Ok((
            json!({"kind":"thread","id":id,"thread":Value::Null,"complete":true}),
            true,
            None,
        ));
    }
    let replies = thread
        .get("replies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let page = paginate_array(
        "thread",
        snapshot,
        fingerprint,
        replies,
        offset,
        query.get("limit"),
    );
    let mut value = thread.clone();
    if let Value::Object(object) = &mut value {
        object.insert("replies".into(), page.0["items"].clone());
    }
    Ok((
        json!({"kind":"thread","id":id,"thread":value,"complete":page.0["complete"],"next_cursor":page.0["next_cursor"]}),
        page.1,
        page.2,
    ))
}

fn optional_projection(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    kind: &str,
    offset: usize,
    fingerprint: &str,
) -> Result<(Value, bool, Option<String>), String> {
    let projection = format!(
        "{kind}:{}",
        query
            .get("id")
            .or_else(|| query.get("revision"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    let Some(items) = snapshot
        .tree
        .get(&projection)
        .or_else(|| snapshot.tree.get(kind))
        .and_then(Value::as_array)
    else {
        return Ok((
            json!({"kind":kind,"status":"unsupported","reason":format!("snapshot has no {kind} projection")}),
            false,
            None,
        ));
    };
    let page = paginate_array(
        kind,
        snapshot,
        fingerprint,
        items.clone(),
        offset,
        query.get("limit"),
    );
    Ok((
        json!({"kind":kind,"items":page.0["items"],"complete":page.0["complete"],"next_cursor":page.0["next_cursor"]}),
        page.1,
        page.2,
    ))
}

fn paginate_array(
    kind: &str,
    snapshot: &QuerySnapshot,
    fingerprint: &str,
    items: Vec<Value>,
    offset: usize,
    limit: Option<&Value>,
) -> (Value, bool, Option<String>) {
    let limit = limit.and_then(Value::as_u64).unwrap_or(24).clamp(1, 24) as usize;
    let total = items.len();
    let page: Vec<_> = items.into_iter().skip(offset).take(limit).collect();
    let end = offset.saturating_add(page.len());
    if end >= total {
        return (
            json!({"items":page,"complete":true,"next_cursor":Value::Null}),
            true,
            None,
        );
    }
    let cursor = encode_cursor(snapshot, kind, fingerprint, end);
    (
        json!({"items":page,"complete":false,"next_cursor":cursor}),
        false,
        Some(cursor),
    )
}

fn encode_cursor(snapshot: &QuerySnapshot, kind: &str, query: &str, offset: usize) -> String {
    let cursor = Cursor {
        version: 1,
        revision: format!(
            "{}.{}",
            snapshot.source_revision, snapshot.annotation_revision
        ),
        kind: kind.to_string(),
        query: query.to_string(),
        offset,
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).unwrap_or_default())
}

fn decode_cursor(
    snapshot: &QuerySnapshot,
    query: &Map<String, Value>,
    kind: &str,
) -> Result<Option<Cursor>, String> {
    let Some(raw) = query.get("cursor").and_then(Value::as_str) else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| "invalid pagination cursor".to_string())?;
    let cursor: Cursor =
        serde_json::from_slice(&bytes).map_err(|_| "invalid pagination cursor".to_string())?;
    if cursor.version != 1
        || cursor.revision
            != format!(
                "{}.{}",
                snapshot.source_revision, snapshot.annotation_revision
            )
        || cursor.kind != kind
        || cursor.query != fingerprint(query)
    {
        return Err("pagination cursor does not match this query or source revision".into());
    }
    Ok(Some(cursor))
}

fn fingerprint(query: &Map<String, Value>) -> String {
    let mut copy = query.clone();
    copy.remove("cursor");
    copy.remove("limit");
    digest(serde_json::to_string(&copy).unwrap_or_default().as_bytes())
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn envelope(
    snapshot: &QuerySnapshot,
    results: Vec<Value>,
    complete: bool,
    reason: Option<String>,
    next_cursor: Option<String>,
    budget: QueryBudget,
) -> Value {
    let mut value = json!({
        "source_revision":snapshot.source_revision,
        "annotation_revision":snapshot.annotation_revision,
        "results":results, "complete":complete,
        "truncation_reason":reason, "next_cursor":next_cursor,
        "returned_bytes":0, "returned_tokens":0
    });
    for _ in 0..3 {
        let (bytes, tokens) = encoded_cost(&value);
        value["returned_bytes"] = json!(bytes);
        value["returned_tokens"] = json!(tokens);
    }
    let (bytes, tokens) = encoded_cost(&value);
    if bytes > budget.max_bytes || tokens > budget.max_tokens {
        value["complete"] = Value::Bool(false);
        if value["truncation_reason"].is_null() {
            value["truncation_reason"] = Value::String("budget".into());
        }
    }
    value
}

fn encoded_cost(value: &Value) -> (usize, usize) {
    let bytes = serde_json::to_vec(value).map_or(usize::MAX, |encoded| encoded.len());
    (bytes, estimate_tokens(value))
}

fn estimate_tokens(value: &Value) -> usize {
    match value {
        Value::String(text) => text.chars().count().div_ceil(4).max(1),
        Value::Array(values) => values.iter().map(estimate_tokens).sum(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| key.len().div_ceil(4) + estimate_tokens(value))
            .sum(),
        _ => 1,
    }
}

fn within_budget(value: &Value, budget: QueryBudget) -> bool {
    let (bytes, tokens) = encoded_cost(value);
    let handles = range_count(value);
    bytes.saturating_add(handles * 110) <= budget.max_bytes
        && tokens.saturating_add(handles * 28) <= budget.max_tokens
}

fn range_count(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(
                map.contains_key("path") && map.contains_key("start") && map.contains_key("end"),
            ) + map.values().map(range_count).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(range_count).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> QuerySnapshot {
        QuerySnapshot {
            source_revision: "rev-a".into(),
            annotation_revision: 7,
            main: "main.md".into(),
            tree: json!({"files":{"main.md":{"id":"file-1","sha":"sha-1"}}}),
            texts: [
                (
                    "main.md".into(),
                    "# Intro\n😀 repeated repeated\nlast\n".into(),
                ),
                (
                    "refs.bib".into(),
                    "@book{key,\nauthor={Ada},\ntitle={A title}\n}\n".into(),
                ),
            ]
            .into_iter()
            .collect(),
            comments: vec![json!({"id":"thread","body":"why","replies":[{"id":"r1"},{"id":"r2"}]})],
        }
    }

    #[test]
    fn source_preserves_final_newline_and_byte_range() {
        let result = read(
            &snapshot(),
            &[json!({"kind":"source","path":"main.md",
            "range":{"start":0,"end":12}})],
            QueryBudget::default(),
        )
        .expect("source query");
        assert_eq!(result["results"][0]["blocks"][0]["source"], "# Intro\n😀");
        assert_eq!(result["results"][0]["blocks"][0]["range"]["end_byte"], 12);
    }

    #[test]
    fn search_reports_duplicate_unicode_matches() {
        let result = read(
            &snapshot(),
            &[json!({"kind":"search","query":"repeated"})],
            QueryBudget::default(),
        )
        .expect("search query");
        let matches = result["results"][0]["matches"].as_array().expect("matches");
        assert_eq!(matches.len(), 2);
        assert_ne!(matches[0]["range"]["start"], matches[1]["range"]["start"]);
    }

    #[test]
    fn pagination_cursor_is_bound_to_revision_and_query() {
        let budget = QueryBudget {
            max_bytes: 20_000,
            max_tokens: 3_000,
        };
        let first = read(
            &snapshot(),
            &[json!({"kind":"search","query":"repeated","limit":1})],
            budget,
        )
        .expect("first page");
        let cursor = first["next_cursor"]
            .as_str()
            .expect("next cursor")
            .to_string();
        let second = read(
            &snapshot(),
            &[json!({"kind":"search","query":"repeated","limit":1,"cursor":cursor})],
            budget,
        )
        .expect("second page");
        assert_eq!(
            second["results"][0]["matches"]
                .as_array()
                .expect("matches")
                .len(),
            1
        );
        let bad = read(
            &snapshot(),
            &[
                json!({"kind":"search","query":"other","limit":1,"cursor":first["next_cursor"].clone()}),
            ],
            budget,
        );
        assert!(bad.is_err());
    }

    #[test]
    fn budget_marks_truncation_without_returning_a_whole_file() {
        let mut small = snapshot();
        small.texts.insert("large.md".into(), "x".repeat(50_000));
        let result = read(
            &small,
            &[json!({"kind":"source","path":"large.md"})],
            QueryBudget {
                max_bytes: 500,
                max_tokens: 200,
            },
        )
        .expect("bounded read");
        assert_eq!(result["complete"], false);
    }

    #[test]
    fn section_ignores_fenced_headings_and_rejects_duplicate_titles() {
        let mut captured = snapshot();
        captured.texts.insert(
            "main.md".into(),
            "# Intro\nbody\n```\n# Fake\n```\n## Child\nchild\n# Next\nnext\n".into(),
        );
        let result = read(
            &captured,
            &[json!({"kind":"section","query":"Intro"})],
            QueryBudget::default(),
        )
        .unwrap();
        assert_eq!(
            result["results"][0]["blocks"][0]["source"],
            "# Intro\nbody\n```\n# Fake\n```\n## Child\nchild\n"
        );
        captured
            .texts
            .get_mut("main.md")
            .unwrap()
            .push_str("# Intro\nagain\n");
        assert!(read(
            &captured,
            &[json!({"kind":"section","query":"Intro"})],
            QueryBudget::default()
        )
        .is_err());
    }

    #[test]
    fn paragraph_pages_advance_without_reexpanding_to_the_start() {
        let mut captured = snapshot();
        let paragraph = "😀\\\"text ".repeat(800);
        captured
            .texts
            .insert("main.md".into(), format!("before\n\n{paragraph}\n\nafter"));
        let mut query = json!({"kind":"context","path":"main.md","start":8,"end":12});
        let mut collected = String::new();
        for _ in 0..100 {
            let result = read(
                &captured,
                &[query.clone()],
                QueryBudget {
                    max_bytes: 2400,
                    max_tokens: 800,
                },
            )
            .unwrap();
            let block = &result["results"][0]["blocks"][0];
            let text = block["source"].as_str().expect("a page must make progress");
            assert!(!text.is_empty());
            collected.push_str(text);
            if result["complete"] == true {
                break;
            }
            query["cursor"] = result["next_cursor"].clone();
        }
        assert_eq!(collected.trim_end(), paragraph.trim_end());
    }

    #[test]
    fn malformed_bibliography_tail_does_not_panic() {
        let mut captured = snapshot();
        captured
            .texts
            .insert("refs.bib".into(), "@book{key, title={unfinished".into());
        let _ = read(
            &captured,
            &[json!({"kind":"bibliography","path":"refs.bib"})],
            QueryBudget::default(),
        );
    }

    #[test]
    fn bibliography_index_stops_at_each_balanced_entry() {
        let mut captured = snapshot();
        captured.texts.insert(
            "refs.bib".into(),
            "@book{first,author={Ada},title={One},}\n@article{second,author={Bob},title={Two}}\n"
                .into(),
        );
        let result = read(
            &captured,
            &[json!({"kind":"bibliography","path":"refs.bib"})],
            QueryBudget {
                max_bytes: 20_000,
                max_tokens: 3_000,
            },
        )
        .unwrap();
        let entries = result["results"][0]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["key"], "first");
        assert_eq!(entries[1]["key"], "second");
        assert!(entries[0]["end"].as_u64().unwrap() < entries[1]["start"].as_u64().unwrap());
    }

    #[test]
    fn structural_pages_keep_cursor_when_page_is_incomplete() {
        let headings = (0..30)
            .map(|index| format!("# Heading {index}\nbody\n"))
            .collect::<String>();
        let bibliography = (0..30)
            .map(|index| format!("@book{{key{index},title={{Title {index}}}}}\n"))
            .collect::<String>();
        let captured = QuerySnapshot {
            source_revision: "structural-pagination".into(),
            annotation_revision: 1,
            main: "main.md".into(),
            tree: Value::Null,
            texts: [
                ("main.md".into(), headings),
                ("refs.bib".into(), bibliography),
            ]
            .into_iter()
            .collect(),
            comments: Vec::new(),
        };
        let budget = QueryBudget {
            max_bytes: 100_000,
            max_tokens: 30_000,
        };

        let first = read(
            &captured,
            &[json!({"kind":"outline","path":"main.md","limit":4})],
            budget,
        )
        .unwrap();
        let cursor = first["next_cursor"].as_str().expect("outline cursor");
        assert_eq!(first["results"][0]["headings"].as_array().unwrap().len(), 4);
        let second = read(
            &captured,
            &[json!({"kind":"outline","path":"main.md","limit":4,"cursor":cursor})],
            budget,
        )
        .unwrap();
        assert_eq!(second["results"][0]["headings"][0]["text"], "Heading 4");

        let first = read(
            &captured,
            &[json!({"kind":"bibliography","path":"refs.bib","limit":4})],
            budget,
        )
        .unwrap();
        let cursor = first["next_cursor"].as_str().expect("bibliography cursor");
        assert_eq!(first["results"][0]["entries"].as_array().unwrap().len(), 4);
        let second = read(
            &captured,
            &[json!({"kind":"bibliography","path":"refs.bib","limit":4,"cursor":cursor})],
            budget,
        )
        .unwrap();
        assert_eq!(second["results"][0]["entries"][0]["key"], "key4");
    }

    #[test]
    fn repeated_queries_reuse_bounded_indexes_for_large_immutable_source() {
        let filler = "x".repeat(4 * 1024 * 1024);
        let source = format!("# Intro\nneedle\n{filler}\nneedle\n");
        let captured = QuerySnapshot {
            source_revision: "index-benchmark-revision".into(),
            annotation_revision: 1,
            main: "main.md".into(),
            tree: Value::Null,
            texts: [("main.md".into(), source)].into_iter().collect(),
            comments: Vec::new(),
        };
        let query = [json!({"kind":"search","path":"main.md","query":"needle"})];
        let started = std::time::Instant::now();
        let first = read(&captured, &query, QueryBudget::default()).unwrap();
        let first_micros = started.elapsed().as_micros();
        let started = std::time::Instant::now();
        for _ in 0..8 {
            assert_eq!(
                read(&captured, &query, QueryBudget::default()).unwrap(),
                first
            );
        }
        let repeated_micros = started.elapsed().as_micros();
        eprintln!(
            "agent_query 4MiB search: first={}us repeated_8={}us",
            first_micros, repeated_micros
        );
        assert_eq!(first["results"][0]["matches"].as_array().unwrap().len(), 2);
    }
}
