//! Semantic display-output capture shared by managed R and Python renders.
//! Unknown mappings are omitted; duplicate code is never treated as identity.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use crate::quarto::{
    self, CellCoverage, CellRecord, Diagnostic, DiagnosticSeverity, OutputKind, OutputRecord,
};

pub fn filter() -> &'static str {
    include_str!("../../assets/quarto-capture.lua")
}

/// Return inline records whose authored line is a complete, single-line
/// paragraph. The Pandoc filter can then associate a rendered value without
/// guessing across soft-wrapped prose or duplicate occurrences.
pub fn inline_capture_records(
    source: &str,
    source_path: &str,
) -> Vec<crate::quarto::InlineExpression> {
    let parsed = quarto::parse_qmd(source, source_path);
    let lines: Vec<_> = source.lines().collect();
    parsed
        .inline_records
        .iter()
        .filter(|record| {
            let Some(line) = record.line.checked_sub(1).and_then(|line| lines.get(line)) else {
                return false;
            };
            if line.trim().is_empty()
                || (record.line > 1 && !lines[record.line - 2].trim().is_empty())
                || (record.line < lines.len() && !lines[record.line].trim().is_empty())
            {
                return false;
            }
            parsed
                .inline_records
                .iter()
                .filter(|other| other.line == record.line)
                .count()
                == 1
        })
        .cloned()
        .collect()
}

#[derive(Deserialize)]
struct CaptureFile {
    schema: u32,
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    inline: Vec<CapturedInline>,
}

#[derive(Deserialize)]
struct CapturedInline {
    #[serde(default)]
    id: String,
    #[serde(default)]
    expression: String,
    #[serde(default)]
    line: usize,
    #[serde(default)]
    column: usize,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Deserialize)]
struct Candidate {
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    code: Vec<String>,
    #[serde(default)]
    outputs: Vec<CapturedOutput>,
}

#[derive(Deserialize)]
struct CapturedOutput {
    kind: String,
    asset: Option<String>,
    text: Option<String>,
    caption: Option<String>,
}

pub struct Capture {
    pub cells: Vec<CellRecord>,
    pub inline_results: Vec<crate::results::InlineResult>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The source must be the one recorded before the invocation. `output_root`
/// contains its generated dependencies, not the author's entire project.
pub fn collect(
    manifest_path: &Path,
    source_path: &str,
    source: &str,
    output_root: &Path,
) -> Result<Capture, String> {
    let parsed = quarto::parse_qmd(source, source_path);
    let raw = read_bounded(manifest_path, quarto::MAX_MANIFEST_BYTES)?;
    let capture: CaptureFile =
        serde_json::from_slice(&raw).map_err(|error| format!("invalid Quarto capture: {error}"))?;
    if capture.schema != 1 {
        return Err("unsupported Quarto capture schema".into());
    }
    let root = output_root
        .canonicalize()
        .map_err(|error| format!("resolve output root: {error}"))?;
    let mut label_counts = BTreeMap::<String, usize>::new();
    for cell in &parsed.cells {
        if let Some(label) = &cell.label {
            *label_counts.entry(label.clone()).or_default() += 1;
        }
    }
    let mut diagnostics = parsed.diagnostics;
    let mut cells = Vec::new();
    for cell in &parsed.cells {
        let options = cell_options(&cell.source);
        let hidden = option_is_false(&options, &cell.options, "include")
            || option_is_false(&options, &cell.options, "output")
            || option_is_false(&options, &cell.options, "eval");
        let ambiguous = cell
            .label
            .as_ref()
            .is_some_and(|label| label_counts.get(label).copied().unwrap_or(0) > 1);
        let code = without_options(&cell.source);
        let duplicate_code = parsed
            .cells
            .iter()
            .filter(|other| without_options(&other.source) == code)
            .count()
            > 1;
        let candidates: Vec<_> = capture
            .candidates
            .iter()
            .filter(|candidate| {
                if let Some(label) = &cell.label {
                    candidate.labels.contains(label)
                } else {
                    !duplicate_code && candidate.code.len() == 1 && candidate.code[0] == code
                }
            })
            .collect();
        let mut outputs = Vec::new();
        if !hidden && !ambiguous {
            // Nested Quarto floats repeat their parent cell's output. Select
            // the enclosing candidate, preserving repeated outputs *within*
            // that cell (printing the same value twice is meaningful).
            if let Some(candidate) = candidates
                .iter()
                .max_by_key(|candidate| (candidate.outputs.len(), candidate.code.len()))
            {
                for output in &candidate.outputs {
                    let Some(record) = normalize(output, &root)? else {
                        continue;
                    };
                    outputs.push(record);
                }
            }
        }
        let image_count = outputs
            .iter()
            .filter(|output| output.kind == OutputKind::Image)
            .count();
        let table_count = outputs
            .iter()
            .filter(|output| output.kind == OutputKind::Table)
            .count();
        for (ordinal, output) in outputs.iter_mut().enumerate() {
            output.ordinal = ordinal as u32;
            // Quarto may lift the caption out of the Image/Table node into
            // its float wrapper. A single display can still use its explicit
            // source caption without guessing which of several plots it names.
            if output.caption.is_none() {
                let key = match output.kind {
                    OutputKind::Image if image_count == 1 => Some("fig-cap"),
                    OutputKind::Table if table_count == 1 => Some("tbl-cap"),
                    _ => None,
                };
                output.caption = key
                    .and_then(|key| options.get(key))
                    .and_then(serde_yaml::Value::as_str)
                    .map(str::to_string);
            }
        }
        let coverage = if hidden {
            CellCoverage::IntentionallyHidden
        } else if ambiguous || (cell.label.is_none() && duplicate_code) {
            CellCoverage::Ambiguous
        } else if candidates.is_empty() {
            CellCoverage::Unavailable
        } else {
            CellCoverage::Captured
        };
        if matches!(
            coverage,
            CellCoverage::Unavailable | CellCoverage::Ambiguous
        ) {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Saved output could not be associated uniquely with {}",
                    cell.id
                ),
                source_path: Some(source_path.into()),
                start_line: Some(cell.start_line),
            });
        }
        cells.push(CellRecord {
            id: cell.id.clone(),
            source_path: source_path.into(),
            label: cell.label.clone().unwrap_or_default(),
            source_sha256: cell.source_sha256.clone(),
            context_sha256: None,
            coverage,
            outputs,
        });
    }
    let mut inline_results = Vec::new();
    for record in &parsed.inline_records {
        let matches: Vec<_> = capture
            .inline
            .iter()
            .filter(|candidate| {
                let location_matches = (candidate.line == 0 || candidate.line == record.line)
                    && (candidate.column == 0 || candidate.column == record.column);
                let expression_matches = candidate.expression == record.expression;
                let identity_matches = (!candidate.id.is_empty() && candidate.id == record.id)
                    || (candidate.id.is_empty()
                        && candidate.line == record.line
                        && candidate.column == record.column);
                location_matches && expression_matches && identity_matches
            })
            .collect();
        if matches.len() == 1 {
            let candidate = matches[0];
            if candidate.value.is_none() {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    message: format!("Captured inline expression has no value: {}", record.id),
                    source_path: Some(source_path.into()),
                    start_line: Some(record.line),
                });
            } else {
                inline_results.push(crate::results::InlineResult {
                    id: record.id.clone(),
                    expression: record.expression.clone(),
                    source_path: source_path.into(),
                    line: record.line,
                    column: record.column,
                    value: candidate.value.clone().unwrap_or_default(),
                    context_sha256: None,
                });
            }
        } else if !matches.is_empty() {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: format!("Captured inline expression is ambiguous: {}", record.id),
                source_path: Some(source_path.into()),
                start_line: Some(record.line),
            });
        } else {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "Inline value could not be captured; source expression retained: {}",
                    record.id
                ),
                source_path: Some(source_path.into()),
                start_line: Some(record.line),
            });
        }
    }
    Ok(Capture {
        cells,
        inline_results,
        diagnostics,
    })
}

fn cell_options(source: &str) -> serde_yaml::Value {
    let options = source
        .lines()
        .take_while(|line| line.trim_start().starts_with("#|"))
        .map(|line| line.trim_start().trim_start_matches("#|").trim_start())
        .collect::<Vec<_>>()
        .join("\n");
    serde_yaml::from_str(&options).unwrap_or_default()
}

fn header_option_is_false(options: &str, key: &str) -> bool {
    options
        .split(|character: char| character.is_whitespace() || character == ',')
        .filter_map(|item| item.split_once('='))
        .any(|(name, value)| name == key && value.eq_ignore_ascii_case("false"))
}

fn option_is_false(body: &serde_yaml::Value, header: &str, key: &str) -> bool {
    body.get(key)
        .and_then(serde_yaml::Value::as_bool)
        .map_or_else(|| header_option_is_false(header, key), |value| !value)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file =
        std::fs::File::open(path).map_err(|error| format!("read captured output: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("captured output is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("captured output exceeds its size limit".into());
    }
    Ok(bytes)
}

fn without_options(source: &str) -> String {
    source
        .lines()
        .skip_while(|line| line.trim_start().starts_with("#|"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize(output: &CapturedOutput, root: &Path) -> Result<Option<OutputRecord>, String> {
    let kind = match output.kind.as_str() {
        "image" => OutputKind::Image,
        "table" => OutputKind::Table,
        "text" => OutputKind::Text,
        _ => return Ok(None),
    };
    let (asset, text, hash) = if kind == OutputKind::Image {
        let Some(path) = output.asset.as_deref() else {
            return Ok(None);
        };
        if !super::protocol::safe_relative_path(path) || path.contains(':') {
            return Ok(None);
        }
        let resolved = root
            .join(path)
            .canonicalize()
            .map_err(|error| format!("captured image {path}: {error}"))?;
        if !resolved.starts_with(root) || !resolved.is_file() {
            return Err("captured image escapes output directory".into());
        }
        let bytes = read_bounded(&resolved, quarto::MAX_BLOB_BYTES)?;
        (Some(path.to_string()), None, Some(quarto::sha256(&bytes)))
    } else {
        let value = output.text.clone().unwrap_or_default();
        let hash = quarto::sha256(value.as_bytes());
        (None, Some(value), Some(hash))
    };
    Ok(Some(OutputRecord {
        ordinal: 0,
        kind,
        asset,
        text,
        content_sha256: hash,
        caption: output.caption.clone().filter(|caption| !caption.is_empty()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn real_capture(source: &str) -> Capture {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("paper.qmd"), source).unwrap();
        std::fs::write(dir.path().join("capture.lua"), filter()).unwrap();
        let captured = dir.path().join("capture.json");
        let records = inline_capture_records(source, "paper.qmd");
        let records_path = dir.path().join("inline-records.json");
        std::fs::write(&records_path, serde_json::to_vec(&records).unwrap()).unwrap();
        let result = std::process::Command::new("quarto")
            .args([
                "render",
                "paper.qmd",
                "--to",
                "html",
                "--lua-filter",
                "capture.lua",
                "--no-cache",
                "--no-execute-daemon",
            ])
            .current_dir(dir.path())
            .env("LIBREPAPER_QUARTO_CELL_MANIFEST", &captured)
            .env("LIBREPAPER_QUARTO_INLINE_RECORDS", &records_path)
            .output()
            .expect("Quarto installed for explicit runtime test");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(dir.path().join("paper.html").exists());
        collect(&captured, "paper.qmd", source, dir.path()).unwrap()
    }

    #[test]
    #[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
    fn real_r_render_captures_distinct_figures_hidden_cells_and_table() {
        let capture = real_capture(include_str!("../../../../examples/quarto-r.qmd"));
        let first = capture
            .cells
            .iter()
            .find(|cell| cell.label == "fig-first")
            .unwrap();
        let second = capture
            .cells
            .iter()
            .find(|cell| cell.label == "fig-second")
            .unwrap();
        assert_eq!(first.coverage, CellCoverage::Captured);
        assert_eq!(second.coverage, CellCoverage::Captured);
        assert_ne!(
            first.outputs[0].content_sha256,
            second.outputs[0].content_sha256
        );
        let table = capture
            .cells
            .iter()
            .find(|cell| cell.label == "tbl-summary")
            .unwrap();
        assert!(table.outputs.iter().any(|out| out.kind == OutputKind::Table
            && out.text.as_deref().is_some_and(|text| text.contains("64"))));
        for label in ["setup", "change-input", "documented-only"] {
            let cell = capture
                .cells
                .iter()
                .find(|cell| cell.label == label)
                .unwrap();
            assert_eq!(cell.coverage, CellCoverage::IntentionallyHidden);
            assert!(cell.outputs.is_empty());
        }
    }

    #[test]
    #[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
    fn real_r_render_recovers_decimal_inline_values_without_suffix_guessing() {
        let source = "```{r}\npi_value <- 3.141593\n```\n\nThe mean is `r pi_value`.\n\nThe mean is `r pi_value`.\nNext sentence.\n";
        let capture = real_capture(source);
        let values: Vec<_> = capture
            .inline_results
            .iter()
            .map(|result| result.value.as_str())
            .collect();
        assert_eq!(values, vec!["3.141593"]);
    }

    #[test]
    #[ignore = "requires installed Quarto and Python Jupyter kernel"]
    fn real_python_render_captures_figure_table_and_text() {
        let capture = real_capture(include_str!("../../../../examples/quarto-python.qmd"));
        for (label, kind) in [
            ("fig-values", OutputKind::Image),
            ("tbl-summary", OutputKind::Table),
            ("text-summary", OutputKind::Text),
        ] {
            let cell = capture
                .cells
                .iter()
                .find(|cell| cell.label == label)
                .unwrap();
            assert_eq!(cell.coverage, CellCoverage::Captured, "{label}");
            assert!(
                cell.outputs.iter().any(|output| output.kind == kind),
                "{label}"
            );
        }
    }

    #[test]
    fn identical_labelled_code_keeps_distinct_images() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("first.png"), b"first image").unwrap();
        std::fs::write(dir.path().join("second.png"), b"second image").unwrap();
        let file = dir.path().join("capture.json");
        std::fs::write(&file, serde_json::to_vec(&json!({"schema":1,"candidates":[
            {"labels":["fig-first"],"code":["plot(x)"],"outputs":[{"kind":"image","asset":"first.png"}]},
            {"labels":["fig-second"],"code":["plot(x)"],"outputs":[{"kind":"image","asset":"second.png"}]}
        ]})).unwrap()).unwrap();
        let source = "```{r}\n#| label: fig-first\nplot(x)\n```\n```{r}\n#| label: fig-second\nplot(x)\n```\n";
        let capture = collect(&file, "paper.qmd", source, dir.path()).unwrap();
        assert_eq!(
            capture.cells[0].outputs[0].asset.as_deref(),
            Some("first.png")
        );
        assert_eq!(
            capture.cells[1].outputs[0].asset.as_deref(),
            Some("second.png")
        );
    }

    #[test]
    fn repeated_outputs_are_preserved_without_duplicating_nested_floats() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("capture.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&json!({"schema":1,"candidates":[
                {"labels":["results"],"code":["print_twice()"],"outputs":[
                    {"kind":"text","text":"same"},{"kind":"text","text":"same"}
                ]},
                {"labels":["results"],"outputs":[{"kind":"text","text":"same"}]}
            ]}))
            .unwrap(),
        )
        .unwrap();
        let captured = collect(
            &file,
            "paper.qmd",
            "```{r}\n#| label: results\nprint_twice()\n```\n",
            dir.path(),
        )
        .unwrap();
        assert_eq!(captured.cells[0].outputs.len(), 2);
        assert_eq!(captured.cells[0].outputs[1].ordinal, 1);
    }

    #[test]
    fn inline_capture_records_require_single_line_paragraphs() {
        let source = "The mean is `r pi`.\n\nThe mean is `r pi`.\nNext sentence.\n";
        let records = inline_capture_records(source, "paper.qmd");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].line, 1);
        assert!(inline_capture_records("A `r pi`. B `r pi`.\n", "paper.qmd").is_empty());
    }

    #[test]
    fn knitr_comma_include_option_stays_hidden_in_capture() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("capture.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&json!({
                "schema": 1,
                "candidates": [{"labels":["hidden"],"outputs":[{"kind":"text","text":"secret"}]}]
            }))
            .unwrap(),
        )
        .unwrap();
        let source = "```{r,include=false #hidden}\nsecret()\n```\n";
        let captured = collect(&file, "paper.qmd", source, dir.path()).unwrap();
        assert_eq!(
            captured.cells[0].coverage,
            CellCoverage::IntentionallyHidden
        );
        assert!(captured.cells[0].outputs.is_empty());
    }

    #[test]
    fn cell_options_override_header_visibility_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("capture.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&json!({
                "schema": 1,
                "candidates": [
                    {"labels":["body-wins"],"outputs":[{"kind":"text","text":"shown"}]},
                    {"labels":["body-hides"],"outputs":[{"kind":"text","text":"hidden"}]}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let source = "```{r,include=false #body-wins}\n#| include: true\nshown()\n```\n```{r,include=true #body-hides}\n#| include: false\nhidden()\n```\n";
        let captured = collect(&file, "paper.qmd", source, dir.path()).unwrap();
        assert_eq!(captured.cells[0].coverage, CellCoverage::Captured);
        assert_eq!(
            captured.cells[1].coverage,
            CellCoverage::IntentionallyHidden
        );
    }

    #[test]
    fn inline_capture_requires_the_source_occurrence_identity() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("capture.json");
        let source = "Value: `r 1 + 1`.\n";
        let record = quarto::parse_qmd(source, "paper.qmd").inline_records[0].clone();
        std::fs::write(
            &file,
            serde_json::to_vec(&json!({
                "schema": 1,
                "candidates": [],
                "inline": [{
                    "id": record.id,
                    "expression": record.expression,
                    "line": record.line,
                    "column": record.column,
                    "value": "2"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let captured = collect(&file, "paper.qmd", source, dir.path()).unwrap();
        assert_eq!(captured.inline_results.len(), 1);
        assert_eq!(captured.inline_results[0].value, "2");
        assert!(captured.inline_results[0].context_sha256.is_none());
    }

    #[test]
    fn inline_capture_preserves_empty_values_and_rejects_identity_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("capture.json");
        let source = "Value: `r 1 + 1`.\n";
        let record = quarto::parse_qmd(source, "paper.qmd").inline_records[0].clone();
        std::fs::write(
            &file,
            serde_json::to_vec(&json!({
                "schema": 1,
                "candidates": [],
                "inline": [{"id": record.id.clone(), "expression": "r 2 + 2", "line": record.line, "column": record.column, "value": "wrong"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let captured = collect(&file, "paper.qmd", source, dir.path()).unwrap();
        assert!(captured.inline_results.is_empty());
        std::fs::write(
            &file,
            serde_json::to_vec(&json!({
                "schema": 1,
                "candidates": [],
                "inline": [{"id": record.id, "expression": record.expression, "line": record.line, "column": record.column, "value": ""}]
            }))
            .unwrap(),
        )
        .unwrap();
        let captured = collect(&file, "paper.qmd", source, dir.path()).unwrap();
        assert_eq!(captured.inline_results.len(), 1);
        assert_eq!(captured.inline_results[0].value, "");
    }
}
