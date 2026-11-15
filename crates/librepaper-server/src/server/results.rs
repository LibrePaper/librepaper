//! Shared results: `/api/documents/{slug}/results`.
//!
//! One record per project, uploaded by an editor after a local render and
//! shown by every collaborator's draft. A Quarto record carries per-cell and
//! inline-value HTML; a Calepin record carries the files of its `.calepin/`
//! folder. It is not a
//! project file: it is never listed, synced or versioned. Its bytes count
//! toward the owner's storage.

use std::collections::HashSet;

use serde::Serialize;

use super::*;

const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
const MAX_CELLS: usize = 2000;
const MAX_INLINES: usize = 10000;
const MAX_INLINE_HTML_BYTES: usize = 64 * 1024;
const MAX_FILES: usize = 2000;
const MAX_PATH_BYTES: usize = 1024;
const MAX_LABEL_BYTES: usize = 256;
const QUOTA_MESSAGE: &str = "Shared results would exceed your storage quota.";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultsRecord {
    version: u32,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    engine: Option<String>,
    #[serde(default)]
    cells: Vec<ResultsCell>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<ResultsFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    inlines: Vec<ResultsInline>,
}

/// A cell from a file pulled in with `{{< include >}}` names that file in `path`.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultsCell {
    fingerprint: String,
    label: Option<String>,
    html: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

/// The value of one executable inline code span in a Quarto document, in
/// document order. `path` names the included file it sits in, as for a cell.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultsInline {
    fingerprint: String,
    html: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultsFile {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
}

/// Checks the limits serde cannot express. The message is the 400 body.
fn validate(record: &ResultsRecord) -> Result<(), &'static str> {
    if record.version != 1 {
        return Err("unsupported results version");
    }
    if record.path.is_empty() || record.path.len() > MAX_PATH_BYTES {
        return Err("results path must be 1 to 1024 bytes");
    }
    match record.engine.as_deref() {
        None => {
            if !record.files.is_empty() {
                return Err("a Quarto results record holds no files");
            }
            validate_cells(&record.cells)?;
            validate_inlines(&record.inlines)
        }
        Some("calepin") => {
            if !record.cells.is_empty() {
                return Err("a Calepin results record holds no cells");
            }
            if !record.inlines.is_empty() {
                return Err("a Calepin results record holds no inline values");
            }
            validate_files(&record.files)
        }
        Some(_) => Err("unsupported results engine"),
    }
}

fn validate_cells(cells: &[ResultsCell]) -> Result<(), &'static str> {
    if cells.len() > MAX_CELLS {
        return Err("too many result cells");
    }
    for cell in cells {
        if !is_fingerprint(&cell.fingerprint) {
            return Err("a cell fingerprint must be 64 lowercase hex characters");
        }
        if cell
            .label
            .as_ref()
            .is_some_and(|l| l.len() > MAX_LABEL_BYTES)
        {
            return Err("a cell label is too long");
        }
        if cell.path.as_deref().is_some_and(|path| !is_safe_path(path)) {
            return Err("a cell path must be a safe relative path");
        }
    }
    Ok(())
}

fn validate_inlines(inlines: &[ResultsInline]) -> Result<(), &'static str> {
    if inlines.len() > MAX_INLINES {
        return Err("too many inline values");
    }
    for inline in inlines {
        if !is_fingerprint(&inline.fingerprint) {
            return Err("an inline fingerprint must be 64 lowercase hex characters");
        }
        if inline.html.len() > MAX_INLINE_HTML_BYTES {
            return Err("an inline value is too large");
        }
        if inline
            .path
            .as_deref()
            .is_some_and(|path| !is_safe_path(path))
        {
            return Err("an inline path must be a safe relative path");
        }
    }
    Ok(())
}

fn is_fingerprint(fingerprint: &str) -> bool {
    fingerprint.len() == 64
        && fingerprint
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A cell or inline path: non-empty, bounded, and relative to the project.
fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && librepaper_base::paths::RelPath::is_local(path)
}

fn validate_files(files: &[ResultsFile]) -> Result<(), &'static str> {
    if files.is_empty() {
        return Err("a Calepin results record needs its files");
    }
    if files.len() > MAX_FILES {
        return Err("too many result files");
    }
    let mut seen = HashSet::new();
    for file in files {
        if file.path.len() > MAX_PATH_BYTES
            || !file.path.starts_with(".calepin/")
            || !librepaper_base::paths::RelPath::is_local(&file.path)
        {
            return Err("a result file path must be a safe path under .calepin/");
        }
        if file.text.is_some() == file.base64.is_some() {
            return Err("a result file holds either text or base64");
        }
        if !seen.insert(file.path.as_str()) {
            return Err("a result file path is repeated");
        }
    }
    Ok(())
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl Server {
    pub(super) async fn handle_results(
        &self,
        request: Request<Body>,
        context: &RequestContext,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let headers = request.headers().clone();
        if cross_site_refused(&headers, &context.arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let query = request.uri().query().map(str::to_string);
        let mut response = match request.method().as_str() {
            "GET" => {
                let auth = match self
                    .authorize::<access::rung::Read>(context, &headers, slug, query.as_deref())
                    .await
                {
                    Ok(auth) => auth,
                    Err(refusal) => return access::deny(&refusal),
                };
                self.read_results(&auth.entry.storage_id).await
            }
            "PUT" => {
                let auth = match self
                    .authorize::<access::rung::Edit>(context, &headers, slug, query.as_deref())
                    .await
                {
                    Ok(auth) => auth,
                    Err(refusal) => return access::deny(&refusal),
                };
                let bytes = match to_bytes(request.into_body(), MAX_BODY_BYTES).await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return write_json(413, &json!({"error": "shared results are too large"}))
                    }
                };
                let record: ResultsRecord = match serde_json::from_slice(&bytes) {
                    Ok(record) => record,
                    Err(_) => return write_json(400, &json!({"error": "invalid results record"})),
                };
                if let Err(message) = validate(&record) {
                    return write_json(400, &json!({"error": message}));
                }
                let Ok(body) = serde_json::to_string(&record) else {
                    return write_json(400, &json!({"error": "invalid results record"}));
                };
                let written = self
                    .mutate(&auth, |viewer, entry| async move {
                        let Ok(document_id) = uuid::Uuid::parse_str(&entry.storage_id) else {
                            return write_json(404, &json!({"error": "not found"}));
                        };
                        let name = viewer.authorship("A collaborator");
                        match self
                            .store
                            .catalog
                            .put_results(document_id, &body, viewer.account_id(), name.display())
                            .await
                        {
                            Ok(done) => write_json(
                                200,
                                &json!({
                                    "bytes": done.byte_length,
                                    "updatedAt": rfc3339(done.updated_at),
                                }),
                            ),
                            Err(error) => results_error(error),
                        }
                    })
                    .await;
                match written {
                    Ok(reply) => {
                        if reply.status() == 200 {
                            self.announce_results(slug).await;
                        }
                        reply
                    }
                    Err(refusal) => return access::deny(&refusal),
                }
            }
            "DELETE" => {
                let auth = match self
                    .authorize::<access::rung::Edit>(context, &headers, slug, query.as_deref())
                    .await
                {
                    Ok(auth) => auth,
                    Err(refusal) => return access::deny(&refusal),
                };
                let deleted = self
                    .mutate(&auth, |_, entry| async move {
                        let Ok(document_id) = uuid::Uuid::parse_str(&entry.storage_id) else {
                            return write_json(404, &json!({"error": "not found"}));
                        };
                        match self.store.catalog.delete_results(document_id).await {
                            Ok(freed) => write_json(200, &json!({"bytes": freed})),
                            Err(error) => results_error(error),
                        }
                    })
                    .await;
                match deleted {
                    Ok(reply) => {
                        if reply.status() == 200 {
                            self.announce_results(slug).await;
                        }
                        reply
                    }
                    Err(refusal) => return access::deny(&refusal),
                }
            }
            _ => return write_json(405, &json!({"error": "unsupported results operation"})),
        };
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    async fn read_results(&self, storage_id: &str) -> Reply {
        let Ok(document_id) = uuid::Uuid::parse_str(storage_id) else {
            return write_json(404, &json!({"error": "no shared results"}));
        };
        let stored = match self.store.catalog.get_results(document_id).await {
            Ok(Some(stored)) => stored,
            Ok(None) => return write_json(404, &json!({"error": "no shared results"})),
            Err(error) => return results_error(error),
        };
        let Ok(Value::Object(mut record)) = serde_json::from_str::<Value>(&stored.body) else {
            return write_json(500, &json!({"error": "stored results are unreadable"}));
        };
        record.insert("uploaderName".into(), json!(stored.uploader_name));
        record.insert("updatedAt".into(), json!(rfc3339(stored.updated_at)));
        record.insert("bytes".into(), json!(stored.byte_length));
        write_json(200, &Value::Object(record))
    }

    /// Tells everyone in the room that the record changed. A room that cannot
    /// open is not an error: nobody is there to tell.
    async fn announce_results(&self, slug: &str) {
        if let Ok(room) = self.rooms.get(slug).await {
            room.broadcast(&json!({"type": "results-changed"})).await;
        }
    }
}

fn results_error(error: librepaper_engine::storage::postgres::Error) -> Reply {
    use librepaper_engine::storage::postgres::{Error, QuotaScope};
    match error {
        Error::Quota(QuotaScope::Owner) => write_json(413, &json!({"error": QUOTA_MESSAGE})),
        Error::Quota(QuotaScope::Deployment) => write_json(
            413,
            &json!({"error": "This server has no room left for shared results."}),
        ),
        Error::NotFound => write_json(404, &json!({"error": "not found"})),
        Error::Busy => write_json(503, &json!({"error": "storage is busy; try again shortly"})),
        _ => write_json(503, &json!({"error": "could not store shared results"})),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn fingerprint(seed: char) -> String {
        seed.to_string().repeat(64)
    }

    fn quarto() -> Value {
        json!({
            "version": 1,
            "path": "main.qmd",
            "cells": [{"fingerprint": fingerprint('a'), "label": null, "html": "<p>a</p>"}],
        })
    }

    fn parse(value: &Value) -> ResultsRecord {
        serde_json::from_value(value.clone()).expect("record parses")
    }

    fn with_inlines(inlines: Value) -> Value {
        let mut record = quarto();
        record["inlines"] = inlines;
        record
    }

    #[test]
    fn a_quarto_record_with_inlines_is_accepted_and_round_trips() {
        let record = parse(&with_inlines(json!([
            {"fingerprint": fingerprint('b'), "html": "42"},
            {"fingerprint": fingerprint('c'), "html": "7", "path": "sections/part.qmd"},
        ])));
        assert_eq!(validate(&record), Ok(()));
        assert_eq!(record.inlines.len(), 2);
        assert!(record.inlines[0].path.is_none());

        let stored = serde_json::to_value(&record).unwrap();
        assert_eq!(stored["inlines"][1]["path"], "sections/part.qmd");
        assert!(stored["inlines"][0].get("path").is_none());
    }

    #[test]
    fn an_empty_or_absent_inlines_list_is_dropped_on_serialize() {
        let absent = parse(&quarto());
        assert_eq!(validate(&absent), Ok(()));
        let stored = serde_json::to_value(&absent).unwrap();
        assert!(stored.get("inlines").is_none(), "{stored}");

        let empty = parse(&with_inlines(json!([])));
        assert_eq!(validate(&empty), Ok(()));
        let stored = serde_json::to_value(&empty).unwrap();
        assert!(stored.get("inlines").is_none(), "{stored}");
    }

    #[test]
    fn bad_inlines_are_refused() {
        let cases = [
            (
                "short fingerprint",
                json!([{"fingerprint": "abc", "html": "1"}]),
            ),
            (
                "uppercase fingerprint",
                json!([{"fingerprint": "A".repeat(64), "html": "1"}]),
            ),
            (
                "unsafe path",
                json!([{"fingerprint": fingerprint('b'), "html": "1", "path": "../x.qmd"}]),
            ),
            (
                "empty path",
                json!([{"fingerprint": fingerprint('b'), "html": "1", "path": ""}]),
            ),
            (
                "oversized html",
                json!([{"fingerprint": fingerprint('b'), "html": "x".repeat(MAX_INLINE_HTML_BYTES + 1)}]),
            ),
        ];
        for (name, inlines) in cases {
            let record = parse(&with_inlines(inlines));
            assert!(validate(&record).is_err(), "{name}");
        }
    }

    #[test]
    fn an_inline_with_an_unknown_field_does_not_deserialize() {
        let unknown = with_inlines(json!([
            {"fingerprint": fingerprint('b'), "html": "1", "extra": true}
        ]));
        assert!(serde_json::from_value::<ResultsRecord>(unknown).is_err());
    }

    #[test]
    fn too_many_inlines_are_refused() {
        let entries: Vec<Value> = (0..=MAX_INLINES)
            .map(|_| json!({"fingerprint": fingerprint('b'), "html": ""}))
            .collect();
        let record = parse(&with_inlines(Value::Array(entries)));
        assert_eq!(validate(&record), Err("too many inline values"));
    }

    #[test]
    fn a_calepin_record_with_inlines_is_refused() {
        let record = parse(&json!({
            "version": 1,
            "path": "paper.typ",
            "engine": "calepin",
            "files": [{"path": ".calepin/calepin.typ", "text": "#let x = 1"}],
            "inlines": [{"fingerprint": fingerprint('b'), "html": "1"}],
        }));
        assert_eq!(
            validate(&record),
            Err("a Calepin results record holds no inline values")
        );
    }
}
