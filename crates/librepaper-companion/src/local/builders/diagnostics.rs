//! Normalization of native builder output into the shared diagnostic shape.

use regex::Regex;

use crate::local::protocol::{safe_relative_path, Diagnostic};

/// `paths` is the job's manifest: a location is kept only when it names a
/// file the caller actually sent, so a builder that quotes an absolute path
/// on this machine cannot leak it back to the browser.
pub fn normalize(log: &str, paths: &[String]) -> Vec<Diagnostic> {
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
    let message = failure_reason(log).unwrap_or_else(|| "native build failed".into());
    vec![Diagnostic {
        severity: "error".into(),
        message,
        file,
        line,
        column,
        end_line: line,
        end_column: column,
        hints: Vec::new(),
    }]
}

/// The log without terminal colour codes, which a page would show as junk.
pub fn strip_ansi(log: &str) -> String {
    let ansi = Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("static regex");
    ansi.replace_all(log, "").into_owned()
}

/// A one-line summary of a failed build: the first line that starts with
/// "error" (any case, ANSI colours ignored), without an `error:` label. The
/// full log always goes with it, so this does not try to say more; a log with
/// no such line has no summary.
pub fn failure_reason(log: &str) -> Option<String> {
    strip_ansi(log).lines().map(str::trim).find_map(|line| {
        let lower = line.to_ascii_lowercase();
        if !lower.starts_with("error") {
            return None;
        }
        let text = if lower.starts_with("error:") {
            line[6..].trim()
        } else {
            line
        };
        (!text.is_empty()).then(|| text.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typst_location_is_kept_only_for_manifest_files() {
        let log = "error: unknown function\n┌─ main.typ:4:7";
        let diagnostics = normalize(log, &["main.typ".into()]);
        assert_eq!(diagnostics[0].file, "main.typ");
        assert_eq!(diagnostics[0].line, 4);
        assert_eq!(diagnostics[0].column, 7);
    }

    #[test]
    fn outside_paths_are_not_exposed() {
        let diagnostics = normalize(
            "error: failed\n┌─ /tmp/private.typ:1:1",
            &["main.typ".into()],
        );
        assert!(diagnostics[0].file.is_empty());
    }

    const QUARTO_NO_R: &str = "processing file: librepaper.qmd\n\x1b[91mERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found\x1b[39m\n\nUnable to locate an installed version of R.\n\x1b[1mInstall R from https://cloud.r-project.org/\x1b[22m\n\n\x1b[33mWARN: Error encountered when rendering files\x1b[39m";

    #[test]
    fn typst_message_is_the_error_line_alone() {
        let diagnostics = normalize("error: unknown function\n┌─ main.typ:4:7", &[]);
        assert_eq!(diagnostics[0].message, "unknown function");
    }

    #[test]
    fn quarto_summary_is_its_error_line_without_colours() {
        let reason = "Error executing 'Rscript': Failed to spawn 'Rscript': entity not found";
        assert_eq!(failure_reason(QUARTO_NO_R).as_deref(), Some(reason));
        assert_eq!(normalize(QUARTO_NO_R, &[])[0].message, reason);
    }

    #[test]
    fn r_error_line_is_kept_whole() {
        let log = "\x1b[31mError in loadNamespace(x) : there is no package called ‘rmarkdown’\nCalls: loadNamespace\nExecution halted";
        assert_eq!(
            failure_reason(log).as_deref(),
            Some("Error in loadNamespace(x) : there is no package called ‘rmarkdown’")
        );
    }

    #[test]
    fn colour_codes_are_stripped_and_text_kept() {
        assert_eq!(
            strip_ansi("\x1b[33mWARN: done\x1b[39m\nInstall with \x1b[1mx\x1b[22m"),
            "WARN: done\nInstall with x"
        );
    }

    #[test]
    fn a_log_without_an_error_line_has_no_summary() {
        assert_eq!(failure_reason(""), None);
        assert_eq!(
            failure_reason("Rendering...\nWARN: Error encountered when rendering files"),
            None
        );
        assert_eq!(
            normalize("WARN: something", &[])[0].message,
            "native build failed"
        );
    }
}
