//! Normalization of native builder output into the shared diagnostic shape.

use regex::Regex;

use crate::local::protocol::{safe_relative_path, Diagnostic};

pub fn normalize(builder: &str, log: &str, entrypoint: &str, paths: &[String]) -> Vec<Diagnostic> {
    if matches!(builder, "tex" | "latexmk" | "tectonic") {
        return crate::local::texlog::parse(log, entrypoint, paths);
    }
    let location = Regex::new(r"┌─\s*([^:]+):(\d+):(\d+)").expect("static regex");
    let mut file = String::new();
    let mut line = 0;
    let mut column = 0;
    for capture in location.captures_iter(log) {
        let candidate = capture.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if safe_relative_path(candidate) && paths.iter().any(|path| path == candidate) {
            file = candidate.to_owned();
            line = capture
                .get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            column = capture
                .get(3)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            break;
        }
    }
    let message = log
        .lines()
        .find_map(|line| line.strip_prefix("error:").map(str::trim))
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| {
            log.lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("native build failed")
                .trim()
        });
    vec![Diagnostic {
        severity: "error".into(),
        message: message.into(),
        file,
        line,
        column,
        end_line: line,
        end_column: column,
        hints: Vec::new(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typst_location_is_kept_only_for_manifest_files() {
        let log = "error: unknown function\n┌─ main.typ:4:7";
        let diagnostics = normalize("typst", log, "main.typ", &["main.typ".into()]);
        assert_eq!(diagnostics[0].file, "main.typ");
        assert_eq!(diagnostics[0].line, 4);
        assert_eq!(diagnostics[0].column, 7);
    }

    #[test]
    fn outside_paths_are_not_exposed() {
        let diagnostics = normalize(
            "calepin",
            "error: failed\n┌─ /tmp/private.typ:1:1",
            "main.typ",
            &["main.typ".into()],
        );
        assert!(diagnostics[0].file.is_empty());
    }
}
