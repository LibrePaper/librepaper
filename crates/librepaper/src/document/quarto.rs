//! Native, execution-free Quarto draft diagnostics and text exports.
//!
//! Shared source is never changed. The virtual Markdown keeps line positions
//! so diagnostics still point at the original `.qmd`, including hidden cells.

use std::collections::BTreeMap;

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

pub fn compile(
    main: &str,
    source: &str,
    title: &str,
    texts: &BTreeMap<String, String>,
) -> Compiled {
    let parsed = crate::quarto::parse_qmd(source, main);
    let mut lines: Vec<String> = source.split('\n').map(str::to_owned).collect();
    let metadata = parsed
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
        .unwrap_or_default();
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
        } else {
            // Replace just the info string; code and its closing fence remain
            // opaque to the Markdown compiler, including nested fence examples.
            if let Some(line) = lines.get_mut(start) {
                let indent = line.len() - line.trim_start().len();
                let count = line
                    .trim_start()
                    .chars()
                    .take_while(|c| *c == '`' || *c == '~')
                    .count();
                let prefix = line.get(..indent + count).unwrap_or("```");
                *line = format!("{prefix}{}", cell.language);
            }
            for line in lines.iter_mut().skip(start + 1).take(option_lines.len()) {
                line.clear();
            }
        }
    }
    let virtual_source = lines.join("\n");
    let mut compiled = wasm_bibliography::citations::compile(
        main,
        &virtual_source,
        title,
        texts,
        &wasm_markdown::markdown::no_assets,
    );
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
        let source = "---\nexecute:\n  echo: false\n---\n```{r echo=FALSE}\nhidden()\n```\n```{r echo=true}\nvisible()\n```\n```{r echo=true}\n#| echo: false\nalso_hidden()\n```\n";
        let result = compile("paper.qmd", source, "Paper", &BTreeMap::new());
        let html = result
            .output
            .as_ref()
            .and_then(|output| output.html())
            .unwrap();
        assert!(html.contains("visible()"));
        assert!(!html.contains("hidden()"));
    }
}
