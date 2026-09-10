//! Native, execution-free Quarto draft diagnostics and text exports.
//!
//! Shared source is never changed. The virtual Markdown keeps line positions
//! so diagnostics still point at the original `.qmd`, including hidden cells.

use std::collections::{BTreeMap, BTreeSet};

use wasm_helpers::diagnostic::{Compiled, Diagnostic, Severity};

fn header_visibility(options: &str, key: &str) -> Option<bool> {
    static OPTIONS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?:^|[,\s])(echo|include)\s*=\s*(?i:true|false|t|f)\b")
            .expect("cell visibility pattern")
    });
    OPTIONS
        .captures_iter(options)
        .filter(|capture| &capture[1] == key)
        .last()
        .map(|capture| {
            let value = capture[0].rsplit('=').next().unwrap_or_default().trim();
            value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("t")
        })
}

const MAX_INCLUDE_DEPTH: usize = 8;

fn include_directive(line: &str, start: usize) -> Option<(usize, usize, String)> {
    let begin = line[start..].find("{{<")? + start;
    let end = line[begin..].find(">}}")? + begin + 3;
    let body = line[begin + 3..end - 3].trim();
    let name = body.strip_prefix("include")?.trim();
    let name = name.split_whitespace().next()?.to_owned();
    Some((begin, end, name))
}

fn resolve_include_name(
    current: &str,
    requested: &str,
    texts: &BTreeMap<String, String>,
) -> Option<String> {
    if requested.is_empty()
        || requested.starts_with('/')
        || requested.contains(':')
        || requested.split('/').any(|part| part.is_empty())
    {
        return None;
    }
    let parent = std::path::Path::new(current).parent();
    let candidate = parent
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(
            || requested.to_owned(),
            |path| path.join(requested).to_string_lossy().replace('\\', "/"),
        );
    let mut parts = Vec::new();
    for part in candidate.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value),
        }
    }
    let normalized = parts.join("/");
    if texts.contains_key(&normalized) {
        Some(normalized)
    } else if texts.contains_key(requested) {
        Some(requested.to_owned())
    } else {
        None
    }
}

#[derive(Clone, Debug)]
struct SourceLocation {
    file: String,
    line: usize,
}

#[derive(Clone, Debug)]
struct ExpandedSource {
    text: String,
    locations: Vec<SourceLocation>,
}

fn locations_for(text: &str, file: &str, line: usize) -> Vec<SourceLocation> {
    text.split('\n')
        .map(|_| SourceLocation {
            file: file.to_owned(),
            line,
        })
        .collect()
}

fn front_matter_metadata(
    parsed: &crate::quarto::QmdDocument,
    fallback: &serde_yaml::Value,
) -> serde_yaml::Value {
    parsed
        .front_matter
        .as_deref()
        .and_then(|front| {
            let mut lines: Vec<&str> = front.lines().collect();
            if lines.len() < 2 {
                return None;
            }
            lines.remove(0);
            lines.pop();
            serde_yaml::from_str::<serde_yaml::Value>(&lines.join("\n")).ok()
        })
        .unwrap_or_else(|| fallback.clone())
}

/// Apply the execution visibility rules to a source before it is expanded.
/// Includes are never executed here; this only removes hidden cells and
/// normalizes visible cell fences for the native Markdown compiler.
fn apply_visibility(
    source: &str,
    parsed: &crate::quarto::QmdDocument,
    inherited_metadata: &serde_yaml::Value,
) -> String {
    let mut lines: Vec<String> = source.split('\n').map(str::to_owned).collect();
    let metadata = front_matter_metadata(parsed, inherited_metadata);
    for cell in &parsed.cells {
        let start = cell.start_line.saturating_sub(1);
        let end = cell.end_line.min(lines.len());
        let option_lines: Vec<String> = cell
            .source
            .lines()
            .take_while(|line| line.trim_start().starts_with("#|"))
            .map(|line| {
                line.trim_start()
                    .trim_start_matches("#|")
                    .trim_start()
                    .to_string()
            })
            .collect();
        let options =
            serde_yaml::from_str::<serde_yaml::Value>(&option_lines.join("\n")).unwrap_or_default();
        let enabled = |key: &str| {
            options
                .get(key)
                .and_then(serde_yaml::Value::as_bool)
                .or_else(|| header_visibility(&cell.options, key))
                .or_else(|| {
                    metadata
                        .get("execute")
                        .and_then(|execute| execute.get(key))
                        .and_then(serde_yaml::Value::as_bool)
                })
                != Some(false)
        };
        if !enabled("include") || !enabled("echo") {
            for line in lines.iter_mut().take(end).skip(start) {
                line.clear();
            }
        } else if let Some(line) = lines.get_mut(start) {
            // Replace just the info string; code and its closing fence remain
            // opaque to the Markdown compiler, including nested fence examples.
            let indent = line.len() - line.trim_start().len();
            let count = line
                .trim_start()
                .chars()
                .take_while(|c| *c == '`' || *c == '~')
                .count();
            let prefix = line.get(..indent + count).unwrap_or("```");
            *line = format!("{prefix}{}", cell.language);
            for line in lines.iter_mut().skip(start + 1).take(option_lines.len()) {
                line.clear();
            }
        }
    }
    lines.join("\n")
}

fn expand_includes(
    source: &str,
    source_name: &str,
    texts: &BTreeMap<String, String>,
) -> (ExpandedSource, Vec<Diagnostic>) {
    fn expand_text(
        text: &str,
        source_name: &str,
        texts: &BTreeMap<String, String>,
        seen: &mut BTreeSet<String>,
        depth: usize,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> ExpandedSource {
        let mut output = Vec::new();
        let mut fence: Option<(u8, usize)> = None;
        for (line_index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if let Some((character, length)) = fence {
                let run = trimmed
                    .bytes()
                    .take_while(|byte| *byte == character)
                    .count();
                if run >= length && trimmed[run..].trim().is_empty() {
                    fence = None;
                }
                output.push(ExpandedSource {
                    text: line.to_owned(),
                    locations: locations_for(line, source_name, line_index + 1),
                });
                continue;
            }
            let opener = trimmed
                .as_bytes()
                .first()
                .copied()
                .filter(|byte| *byte == b'`' || *byte == b'~');
            if let Some(character) = opener {
                let length = trimmed
                    .bytes()
                    .take_while(|byte| *byte == character)
                    .count();
                if length >= 3 {
                    fence = Some((character, length));
                    output.push(ExpandedSource {
                        text: line.to_owned(),
                        locations: locations_for(line, source_name, line_index + 1),
                    });
                    continue;
                }
            }

            // A whole-line include can preserve the included file and line
            // information exactly. Inline includes remain attached to the
            // authored line below because their generated text has no single
            // source line of its own.
            if let Some((begin, end, name)) = include_directive(line, 0) {
                if line[..begin].trim().is_empty() && line[end..].trim().is_empty() {
                    let Some(resolved) = resolve_include_name(source_name, &name, texts) else {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            message: format!("include is unavailable or unauthorized: {name}"),
                            file: source_name.to_owned(),
                            line: line_index + 1,
                            ..Default::default()
                        });
                        output.push(ExpandedSource {
                            text: line.to_owned(),
                            locations: locations_for(line, source_name, line_index + 1),
                        });
                        continue;
                    };
                    if depth >= MAX_INCLUDE_DEPTH || seen.contains(&resolved) {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            message: format!("include cycle or depth limit: {resolved}"),
                            file: source_name.to_owned(),
                            line: line_index + 1,
                            ..Default::default()
                        });
                    } else {
                        seen.insert(resolved.clone());
                        output.push(expand_text(
                            &texts[&resolved],
                            &resolved,
                            texts,
                            seen,
                            depth + 1,
                            diagnostics,
                        ));
                        seen.remove(&resolved);
                        continue;
                    }
                    output.push(ExpandedSource {
                        text: line.to_owned(),
                        locations: locations_for(line, source_name, line_index + 1),
                    });
                    continue;
                }
            }

            let mut current = 0;
            let mut expanded = String::new();
            while let Some((begin, end, name)) = include_directive(line, current) {
                expanded.push_str(&line[current..begin]);
                let Some(resolved) = resolve_include_name(source_name, &name, texts) else {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!("include is unavailable or unauthorized: {name}"),
                        file: source_name.to_owned(),
                        line: line_index + 1,
                        ..Default::default()
                    });
                    expanded.push_str(&line[begin..end]);
                    current = end;
                    continue;
                };
                if depth >= MAX_INCLUDE_DEPTH || seen.contains(&resolved) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!("include cycle or depth limit: {resolved}"),
                        file: source_name.to_owned(),
                        line: line_index + 1,
                        ..Default::default()
                    });
                    expanded.push_str(&line[begin..end]);
                } else {
                    seen.insert(resolved.clone());
                    expanded.push_str(
                        &expand_text(
                            &texts[&resolved],
                            &resolved,
                            texts,
                            seen,
                            depth + 1,
                            diagnostics,
                        )
                        .text,
                    );
                    seen.remove(&resolved);
                }
                current = end;
            }
            expanded.push_str(&line[current..]);
            output.push(ExpandedSource {
                locations: locations_for(&expanded, source_name, line_index + 1),
                text: expanded,
            });
        }
        let mut expanded = String::new();
        let mut locations = Vec::new();
        for (index, line) in output.into_iter().enumerate() {
            if index > 0 {
                expanded.push('\n');
            }
            expanded.push_str(&line.text);
            locations.extend(line.locations);
        }
        if locations.is_empty() {
            // `str::lines` has no item for an empty file, while Markdown still
            // treats the expanded text as one empty physical line.
            locations.push(SourceLocation {
                file: source_name.to_owned(),
                line: 1,
            });
        }
        ExpandedSource {
            text: expanded,
            locations,
        }
    }

    let mut diagnostics = Vec::new();
    let mut seen = BTreeSet::new();
    (
        expand_text(source, source_name, texts, &mut seen, 0, &mut diagnostics),
        diagnostics,
    )
}

pub fn compile(
    main: &str,
    source: &str,
    title: &str,
    texts: &BTreeMap<String, String>,
) -> Compiled {
    let parsed = crate::quarto::parse_qmd(source, main);
    // Apply visibility and code-fence transformations to authored lines first.
    // Expanding an include before this pass shifts every later source offset
    // and can expose code from a hidden cell.
    let inherited_metadata = front_matter_metadata(&parsed, &serde_yaml::Value::default());
    let transformed_source = apply_visibility(source, &parsed, &inherited_metadata);
    let transformed_texts = texts
        .iter()
        .map(|(path, text)| {
            let included = crate::quarto::parse_qmd(text, path);
            (
                path.clone(),
                apply_visibility(text, &included, &inherited_metadata),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let (expanded_source, include_diagnostics) =
        expand_includes(&transformed_source, main, &transformed_texts);
    let mut compiled = wasm_bibliography::citations::compile(
        main,
        &expanded_source.text,
        title,
        texts,
        &wasm_markdown::markdown::no_assets,
    );
    for diagnostic in &mut compiled.diagnostics {
        // Only the document passed as the virtual source needs remapping;
        // resource diagnostics (for example a bibliography file) already
        // carry their own path and line.
        if diagnostic.line == 0 || (!diagnostic.file.is_empty() && diagnostic.file != main) {
            continue;
        }
        if let Some(location) = expanded_source.locations.get(diagnostic.line - 1) {
            diagnostic.file.clone_from(&location.file);
            diagnostic.line = location.line;
        }
    }
    compiled.diagnostics.extend(include_diagnostics);
    compiled
        .diagnostics
        .extend(parsed.diagnostics.into_iter().map(|diagnostic| Diagnostic {
            severity: Severity::Warning,
            message: diagnostic.message,
            file: diagnostic.source_path.unwrap_or_else(|| main.into()),
            line: diagnostic.start_line.unwrap_or(0),
            column: 1,
            ..Default::default()
        }));
    compiled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_code_never_enters_exported_prose() {
        let source =
            "---\nexecute:\n  echo: false\n---\nA sentence.\n\n```{r}\nsecret_computation()\n```\n";
        let result = compile("paper.qmd", source, "Paper", &BTreeMap::new());
        let html = result
            .output
            .as_ref()
            .and_then(|output| output.html())
            .unwrap();
        assert!(html.contains("A sentence."));
        assert!(!html.contains("secret_computation"));
    }

    #[test]
    fn cell_visibility_overrides_header_and_document_defaults() {
        let source = "---\nexecute:\n  echo: false\n---\n```{r,echo=FALSE}\nhidden()\n```\n```{r echo=true}\nvisible()\n```\n```{r echo=true}\n#| echo: false\nalso_hidden()\n```\n";
        let result = compile("paper.qmd", source, "Paper", &BTreeMap::new());
        let html = result
            .output
            .as_ref()
            .and_then(|output| output.html())
            .unwrap();
        assert!(html.contains("visible()"));
        assert!(!html.contains("hidden()"));
    }

    #[test]
    fn includes_expand_only_from_authorized_project_texts() {
        let source = "Before {{< include safe.qmd >}}\n\n```text\n{{< include safe.qmd >}}\n```\n\n{{< include ../secret.qmd >}}\n";
        let texts = BTreeMap::from([("safe.qmd".into(), "safe included text".into())]);
        let result = compile("paper.qmd", source, "Paper", &texts);
        let html = result
            .output
            .as_ref()
            .and_then(|output| output.html())
            .unwrap();
        assert!(html.contains("safe included text"));
        assert!(html.contains("../secret.qmd"));
        assert!(html.contains("include safe.qmd"));
        assert!(result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("unauthorized")
            && diagnostic.file == "paper.qmd"
            && diagnostic.line == 7));
    }

    #[test]
    fn include_before_hidden_cell_does_not_shift_visibility_offsets() {
        let source =
            "{{< include safe.qmd >}}\n\n```{r include=false}\nsecret()\n```\n\nVisible text.\n";
        let texts = BTreeMap::from([(String::from("safe.qmd"), String::from("Included text."))]);
        let result = compile("paper.qmd", source, "Paper", &texts);
        let html = result
            .output
            .as_ref()
            .and_then(|output| output.html())
            .unwrap();
        assert!(html.contains("Included text."));
        assert!(html.contains("Visible text."));
        assert!(!html.contains("secret()"));
    }

    #[test]
    fn nested_include_resolves_relative_to_including_file() {
        let source = "{{< include chapters/one.qmd >}}\n";
        let texts = BTreeMap::from([
            (
                String::from("chapters/one.qmd"),
                String::from("{{< include two.qmd >}}"),
            ),
            (
                String::from("chapters/two.qmd"),
                String::from("Nested text."),
            ),
        ]);
        let result = compile("paper.qmd", source, "Paper", &texts);
        let html = result
            .output
            .as_ref()
            .and_then(|output| output.html())
            .unwrap();
        assert!(html.contains("Nested text."));
    }

    #[test]
    fn included_hidden_cells_are_removed_before_expansion() {
        let source = "{{< include chapter.qmd >}}\n\nFollowing prose.\n";
        let texts = BTreeMap::from([(
            String::from("chapter.qmd"),
            String::from("```{r echo=false}\nsecret()\n```\nIncluded prose.\n"),
        )]);
        let result = compile("paper.qmd", source, "Paper", &texts);
        let html = result
            .output
            .as_ref()
            .and_then(|output| output.html())
            .unwrap();
        assert!(html.contains("Included prose."));
        assert!(html.contains("Following prose."));
        assert!(!html.contains("secret()"));
    }

    #[test]
    fn expanded_source_map_keeps_included_and_following_lines_distinct() {
        let source = "{{< include chapter.qmd >}}\nFollowing prose.";
        let texts = BTreeMap::from([(
            String::from("chapter.qmd"),
            String::from("Included line one.\nIncluded line two."),
        )]);
        let (expanded, diagnostics) = expand_includes(source, "paper.qmd", &texts);
        assert!(diagnostics.is_empty());
        assert_eq!(
            expanded.text,
            "Included line one.\nIncluded line two.\nFollowing prose."
        );
        assert_eq!(expanded.locations.len(), 3);
        assert_eq!(expanded.locations[0].file, "chapter.qmd");
        assert_eq!(expanded.locations[0].line, 1);
        assert_eq!(expanded.locations[1].file, "chapter.qmd");
        assert_eq!(expanded.locations[1].line, 2);
        assert_eq!(expanded.locations[2].file, "paper.qmd");
        assert_eq!(expanded.locations[2].line, 2);
    }
}
