//! Quarto source parsing and freshness classification.
//!
//! Immutable result contracts are defined in crate::results. This module
//! keeps the QMD parser and re-exports the old paths for compatibility.

pub(crate) use crate::results::parameters_sha256;
pub use crate::results::*;

use serde::{Deserialize, Serialize};

/* ------------------------------- source parsing and freshness ---------- */

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QmdCell {
    pub id: String,
    pub label: Option<String>,
    pub language: String,
    pub options: String,
    pub source: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source_sha256: String,
}

/// A source-located inline computation. The expression and location are kept
/// together so a captured value can only replace the exact occurrence it came
/// from; callers must retain the original expression when no matching value
/// exists or when its context is stale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineExpression {
    pub id: String,
    pub expression: String,
    pub source: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QmdDocument {
    pub front_matter: Option<String>,
    pub cells: Vec<QmdCell>,
    /// Inline computations and includes are retained as ordered source
    /// records. Their exact bytes participate in the conservative context
    /// fingerprint rather than being guessed from rendered output.
    pub inline_expressions: Vec<String>,
    pub inline_records: Vec<InlineExpression>,
    pub includes: Vec<String>,
    /// Ordered `path\0sha256` records for shared files outside the main
    /// document. Local execution supplies these from its verified input
    /// manifest; private linked-project files are intentionally excluded.
    pub dependencies: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse fenced executable cells with a line scanner. Fences inside a cell
/// are respected according to their opening fence length, so examples in
/// strings and nested Markdown do not become cells.
pub fn parse_qmd(source: &str, path: &str) -> QmdDocument {
    let mut result = QmdDocument::default();
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut front_end = 0;
    if lines.first().is_some_and(|line| {
        line.trim_end_matches(['\r', '\n'])
            .trim_start_matches('\u{feff}')
            == "---"
    }) {
        for (index, line) in lines.iter().enumerate().skip(1) {
            if line.trim_end_matches(['\r', '\n']) == "---"
                || line.trim_end_matches(['\r', '\n']) == "..."
            {
                front_end = index + 1;
                break;
            }
        }
        if front_end == 0 {
            result.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: "unfinished YAML front matter".into(),
                source_path: Some(path.into()),
                start_line: Some(1),
            });
        } else {
            result.front_matter = Some(lines[..front_end].concat());
        }
    }
    let mut index = front_end;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let Some((fence, info)) = opening_fence(trimmed) else {
            collect_inline_and_include(trimmed, index + 1, path, &mut result);
            index += 1;
            continue;
        };
        let start = index;
        index += 1;
        let body_start = index;
        while index < lines.len() {
            let candidate = lines[index].trim_end_matches(['\r', '\n']);
            if closes_fence(candidate, &fence) {
                break;
            }
            index += 1;
        }
        let closed = index < lines.len();
        let body_end = index.min(lines.len());
        let (language, options, mut label) = parse_info(info);
        let body = lines[body_start..body_end].concat();
        // A `#| label:` option is part of Quarto's executable cell syntax.
        // Keep the option line in the source fingerprint while using the
        // declared label as the durable association when it is unique.
        if label.is_none() {
            label = option_label(&body);
        }
        if !info.starts_with('{') || info.starts_with("{{") {
            // Every fenced block is consumed above, including Markdown code
            // examples. Only a single braced Quarto info string is a cell;
            // this prevents documentation containing ```{{r}} from running.
            index = index.saturating_add(usize::from(closed));
            continue;
        }
        // A braced, identifier-like language that this collector does not
        // execute still changes computation context. Keep it in the source
        // model and fingerprint; the collector can report it unsupported.
        if language.is_empty()
            || !language
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
        {
            index = index.saturating_add(usize::from(closed));
            continue;
        }
        let source_sha256 = cell_fingerprint(&language, &options, &body);
        let id = label.clone().map_or_else(
            || {
                let occurrence = result
                    .cells
                    .iter()
                    .filter(|cell| cell.source_sha256 == source_sha256)
                    .count();
                if occurrence > 0 {
                    result.diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Warning,
                        message: "duplicate unlabelled cell fingerprint; association is ambiguous"
                            .into(),
                        source_path: Some(path.into()),
                        start_line: Some(start + 1),
                    });
                }
                format!("{path}#cell-{}-{}", &source_sha256[..16], occurrence + 1)
            },
            |label| format!("{path}#{label}"),
        );
        if result.cells.iter().any(|cell: &QmdCell| cell.id == id) {
            result.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: format!("duplicate cell label: {id}"),
                source_path: Some(path.into()),
                start_line: Some(start + 1),
            });
        }
        result.cells.push(QmdCell {
            id,
            label,
            language,
            options,
            source_sha256,
            source: body,
            start_line: start + 1,
            end_line: body_end + 1,
        });
        if !closed {
            result.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: "unfinished executable fence".into(),
                source_path: Some(path.into()),
                start_line: Some(start + 1),
            });
            break;
        }
        index += 1;
    }
    result
}

fn opening_fence(line: &str) -> Option<(String, &str)> {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let first = trimmed.as_bytes().first().copied()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|byte| *byte == first).count();
    if run < 3 {
        return None;
    }
    let run = trimmed[..run].to_owned();
    let info = trimmed[run.len()..].trim();
    Some((run, info))
}

fn closes_fence(line: &str, opener: &str) -> bool {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return false;
    }
    let candidate = &line[indent..];
    let Some(first) = opener.as_bytes().first().copied() else {
        return false;
    };
    let run = candidate.bytes().take_while(|byte| *byte == first).count();
    run >= opener.len() && candidate[run..].chars().all(char::is_whitespace)
}

fn option_label(body: &str) -> Option<String> {
    let mut in_options = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() && !in_options {
            continue;
        }
        let Some(option) = trimmed.strip_prefix("#|") else {
            break;
        };
        in_options = true;
        let Some(value) = option.trim().strip_prefix("label:") else {
            continue;
        };
        let yaml = format!("label: {}", value.trim());
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&yaml) {
            if let Some(label) = value.get("label").and_then(serde_yaml::Value::as_str) {
                if !label.is_empty() {
                    return Some(label.to_owned());
                }
            }
        }
    }
    None
}

fn collect_inline_and_include(
    line: &str,
    line_number: usize,
    path: &str,
    result: &mut QmdDocument,
) {
    if line.contains("{{") || has_executable_inline(line) {
        result.inline_expressions.push(line.trim().to_owned());
    }
    let mut occurrence = 0;
    let mut in_tick = false;
    let mut start = 0;
    for (index, byte) in line.bytes().enumerate() {
        if byte != b'`' {
            continue;
        }
        if in_tick {
            let expression = line[start..index].trim();
            if executable_inline_segment(expression) {
                result.inline_records.push(InlineExpression {
                    id: format!("{path}#inline-{line_number}-{occurrence}"),
                    expression: expression.to_owned(),
                    source: line.to_owned(),
                    line: line_number,
                    column: start + 1,
                });
                occurrence += 1;
            }
            in_tick = false;
        } else {
            start = index + 1;
            in_tick = true;
        }
    }
    let mut offset = 0;
    while let Some(relative) = line[offset..].find("{{<") {
        let start = offset + relative;
        let Some(end) = line[start..].find(">}}") else {
            break;
        };
        let end = start + end + 3;
        result.includes.push(line[start..end].to_owned());
        offset = end;
    }
}

/// Quarto/knitr inline execution lives in backticks and is distinct from a
/// literal Markdown inline code span. Keep the recognised engine set small and
/// conservative; an unknown inline form remains source text but the known
/// computational forms invalidate cached context when their expression edits.
fn has_executable_inline(line: &str) -> bool {
    let mut in_tick = false;
    let mut start = 0;
    for (index, byte) in line.bytes().enumerate() {
        if byte != b'`' {
            continue;
        }
        if in_tick {
            let segment = line[start..index].trim();
            if executable_inline_segment(segment) {
                return true;
            }
        } else {
            start = index + 1;
        }
        in_tick = !in_tick;
    }
    false
}

fn executable_inline_segment(segment: &str) -> bool {
    let segment = segment.trim();
    let engines = ["r", "python", "julia", "ojs", "bash", "embed"];
    engines.iter().any(|engine| {
        segment
            .strip_prefix(engine)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace) && !rest.trim().is_empty())
            || segment
                .strip_prefix(&format!("{{{engine}}}"))
                .is_some_and(|rest| !rest.trim().is_empty())
    })
}

fn parse_info(info: &str) -> (String, String, Option<String>) {
    let (language, attrs) = if let Some(rest) = info.strip_prefix('{') {
        let end = rest.find('}').unwrap_or(rest.len());
        let inside = &rest[..end];
        let inside = inside.trim_start();
        let split = inside
            .find(|character: char| character.is_whitespace() || character == ',')
            .unwrap_or(inside.len());
        let attrs = &inside[split..];
        let attrs = if let Some(trimmed) = attrs.trim_start().strip_prefix(',') {
            trimmed.trim_start()
        } else {
            attrs
        };
        (inside[..split].to_ascii_lowercase(), attrs.to_owned())
    } else {
        let mut pieces = info.splitn(2, '{');
        (
            pieces
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase(),
            pieces
                .next()
                .unwrap_or_default()
                .trim_end_matches('}')
                .trim()
                .to_owned(),
        )
    };
    let label = attrs
        .split_whitespace()
        .find_map(|item| item.strip_prefix('#').map(str::to_owned));
    (language, attrs, label)
}

fn cell_fingerprint(language: &str, options: &str, source: &str) -> String {
    let mut bytes = Vec::with_capacity(language.len() + options.len() + source.len() + 2);
    bytes.extend_from_slice(language.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(options.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(source.as_bytes());
    sha256(&bytes)
}

pub fn computation_fingerprint(
    document: &QmdDocument,
    entrypoint: &str,
    profiles: &[String],
    parameters_sha256: Option<&str>,
) -> String {
    computation_fingerprint_for_format(document, entrypoint, "", profiles, parameters_sha256)
}

pub fn computation_fingerprint_for_format(
    document: &QmdDocument,
    entrypoint: &str,
    format: &str,
    profiles: &[String],
    parameters_sha256: Option<&str>,
) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"librepaper-quarto-context-v1\0");
    bytes.extend_from_slice(entrypoint.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(format.as_bytes());
    bytes.push(0);
    for profile in profiles {
        bytes.extend_from_slice(profile.as_bytes());
        bytes.push(0);
    }
    if let Some(parameters) = parameters_sha256 {
        bytes.extend_from_slice(parameters.as_bytes());
    }
    if let Some(front_matter) = &document.front_matter {
        bytes.extend_from_slice("\u{00fe}".as_bytes());
        bytes.extend_from_slice(front_matter.as_bytes());
    }
    for include in &document.includes {
        bytes.extend_from_slice("\u{00fd}".as_bytes());
        bytes.extend_from_slice(include.as_bytes());
    }
    for expression in &document.inline_expressions {
        bytes.extend_from_slice("\u{00fc}".as_bytes());
        bytes.extend_from_slice(expression.as_bytes());
    }
    for dependency in &document.dependencies {
        bytes.extend_from_slice("\u{00fb}".as_bytes());
        bytes.extend_from_slice(dependency.as_bytes());
    }
    for cell in &document.cells {
        bytes.extend_from_slice(cell.id.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(cell.language.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(cell.options.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(cell.source.as_bytes());
        bytes.extend_from_slice("\u{00ff}".as_bytes());
    }
    sha256(&bytes)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AssociationState {
    Mapped,
    Ambiguous,
    Unmapped,
    Hidden,
    Deleted,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Freshness {
    MatchesRecordedInputs,
    SourceCompatible,
    PotentiallyStale,
    Unknown,
    Missing,
}

pub fn classify_freshness(
    old: &BundleManifest,
    current: &QmdDocument,
    entrypoint: &str,
    profiles: &[String],
    parameters_sha256: Option<&str>,
) -> Freshness {
    if old.provenance.kind == ProvenanceKind::Imported || old.source.tree_sha256.is_none() {
        return Freshness::Unknown;
    }
    let format = match old.context.format {
        OutputFormat::Html => "html",
        OutputFormat::Revealjs => "revealjs",
        OutputFormat::Pdf => "pdf",
        OutputFormat::Docx => "docx",
        OutputFormat::Other => "other",
    };
    let current_hash = computation_fingerprint_for_format(
        current,
        entrypoint,
        format,
        profiles,
        parameters_sha256,
    );
    if current_hash == old.context.computation_sha256 {
        Freshness::MatchesRecordedInputs
    } else {
        Freshness::PotentiallyStale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn typed_parameter_digest_matches_browser_vector() {
        let parameters = BTreeMap::from([
            ("A".into(), serde_json::json!(1)),
            ("a".into(), serde_json::json!(1e-3)),
            ("_".into(), serde_json::json!(true)),
            ("Z".into(), serde_json::json!("é😀")),
            ("number".into(), serde_json::json!(2.5e4)),
        ]);
        assert_eq!(
            parameters_sha256(&parameters),
            "aeea5bcdd2f603f88bb8d6387ce656ebe5b6a3560947363a369b736071bb8b3d"
        );
    }

    #[test]
    fn parser_preserves_nested_fence_and_labels() {
        let doc = parse_qmd(
            "---\ntitle: hi\n---\n\n```{r #plot echo=false}\ncat(\"```\")\n```\n",
            "paper.qmd",
        );
        assert_eq!(doc.cells.len(), 1);
        assert_eq!(doc.cells[0].label.as_deref(), Some("plot"));
        assert!(doc.diagnostics.is_empty());
    }

    #[test]
    fn parser_accepts_knitr_comma_options() {
        let doc = parse_qmd(
            "```{r,echo=FALSE}\nhidden_one()\n```\n```{r, echo=FALSE}\nhidden_two()\n```\n```{r , echo=FALSE}\nhidden_three()\n```\n",
            "paper.qmd",
        );
        assert_eq!(doc.cells.len(), 3);
        assert_eq!(doc.cells[0].language, "r");
        assert_eq!(doc.cells[0].options, "echo=FALSE");
        assert_eq!(doc.cells[1].options, "echo=FALSE");
        assert_eq!(doc.cells[2].options, "echo=FALSE");
    }
    #[test]
    fn context_changes_when_code_or_options_change() {
        let a = parse_qmd("```{r #a}\nx <- 1\n```\n", "p.qmd");
        let b = parse_qmd("```{r #a echo=false}\nx <- 1\n```\n", "p.qmd");
        assert_ne!(
            computation_fingerprint(&a, "p.qmd", &[], None),
            computation_fingerprint(&b, "p.qmd", &[], None)
        );
    }
    #[test]
    fn plain_code_fence_is_not_an_executable_cell() {
        let doc = parse_qmd(
            "```text\n```{r}\nlooks like a nested example\n```\n```\n",
            "p.qmd",
        );
        assert!(doc.cells.is_empty());
    }
    #[test]
    fn option_labels_use_yaml_and_fences_consume_long_closers() {
        let doc = parse_qmd(
            "```{r}\n#| label: 'quoted-label'\nplot(1)\n`````\n",
            "p.qmd",
        );
        assert_eq!(doc.cells[0].label.as_deref(), Some("quoted-label"));
        let unknown = parse_qmd("```{custom-engine}\nrun()\n```\n", "p.qmd");
        assert_eq!(unknown.cells.len(), 1);
        assert_ne!(
            computation_fingerprint(&unknown, "p.qmd", &[], None),
            computation_fingerprint(&QmdDocument::default(), "p.qmd", &[], None)
        );
    }
    #[test]
    fn paths_reject_traversal() {
        assert!(!crate::results::safe_path("../secret.png"));
        assert!(!crate::results::safe_path("/etc/passwd"));
    }
    #[test]
    fn inline_execution_and_all_includes_invalidate_context() {
        let doc = parse_qmd(
            "`r mean(x)`\n`{python} x + 1`\n`ordinary words`\n{{< include one.qmd >}}{{< include two.qmd >}}\n",
            "p.qmd",
        );
        assert_eq!(doc.inline_expressions.len(), 3);
        assert_eq!(doc.includes.len(), 2);
        let mut changed = doc.clone();
        changed.inline_expressions[0].push('!');
        assert_ne!(
            computation_fingerprint(&doc, "p.qmd", &[], None),
            computation_fingerprint(&changed, "p.qmd", &[], None)
        );
    }

    #[test]
    fn inline_records_keep_occurrence_identity_and_location() {
        let doc = parse_qmd("Value `r 1 + 1` and `r 2 + 2`.\n", "paper.qmd");
        assert_eq!(doc.inline_records.len(), 2);
        assert_eq!(doc.inline_records[0].id, "paper.qmd#inline-1-0");
        assert_eq!(doc.inline_records[1].id, "paper.qmd#inline-1-1");
        assert_eq!(doc.inline_records[0].expression, "r 1 + 1");
        assert_eq!(doc.inline_records[1].column, 22);
    }

    #[test]
    fn legacy_bundle_defaults_engine_and_asset_role_without_wire_changes() {
        let old = serde_json::json!({
            "schema": BUNDLE_SCHEMA,
            "render_id": "render-one",
            "document_id": "doc-one",
            "source": {
                "revision": "",
                "tree_sha256": null,
                "main": "main.qmd",
                "verification": "imported"
            },
            "context": {
                "id": "html",
                "fingerprint_version": FINGERPRINT_VERSION,
                "computation_sha256": sha256(b"unknown"),
                "format": "html"
            },
            "provenance": {
                "kind": "imported",
                "computation": "no-execution",
                "external_inputs": "unknown"
            },
            "artifact": null,
            "cells": [],
            "assets": [],
            "coverage": {"full_artifact": false, "cell_outputs": "none"}
        });
        let old_bytes = serde_json::to_vec(&old).expect("legacy JSON encodes");
        let manifest: BundleManifest =
            serde_json::from_slice(&old_bytes).expect("legacy manifest decodes");
        assert_eq!(manifest.engine, ExecutionEngine::Quarto);
        let current_bytes = manifest.encoded().expect("legacy manifest re-encodes");
        let reparsed: BundleManifest =
            serde_json::from_slice(&current_bytes).expect("re-encoded manifest decodes");
        assert_eq!(current_bytes, reparsed.encoded().expect("retry is stable"));
        assert!(!String::from_utf8_lossy(&current_bytes).contains("\"engine\""));
    }

    #[test]
    fn unsupported_bundle_engine_is_rejected() {
        let value = serde_json::json!({"engine": "calepin"});
        assert!(serde_json::from_value::<ExecutionEngine>(value["engine"].clone()).is_err());
    }

    #[test]
    fn document_metadata_normalizes_quarto_to_markdown() {
        let metadata = DocumentMetadata::from_source_format("quarto");
        assert_eq!(metadata.execution_engine, ExecutionEngine::Quarto);
        assert_eq!(metadata.draft_format, DraftFormat::Markdown);
        assert!(metadata
            .validate_bundle_engine(ExecutionEngine::Quarto)
            .is_ok());
        assert!(metadata
            .validate_bundle_engine(ExecutionEngine::None)
            .is_err());
    }
}
