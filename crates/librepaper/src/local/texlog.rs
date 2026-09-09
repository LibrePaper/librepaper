//! The TeX log, read, in Rust.
//!
//! This is a direct port of `web/src/lib/latex/log.js`: the browser parses
//! the browser engine's log in JavaScript because the engine lives in the browser;
//! the native runner parses a log a real `pdflatex`/`xelatex`/`lualatex`
//! wrote to disk, and it must say the same thing about it, so that a reader
//! cannot tell a native diagnostic from a browser one. See that file's
//! module comment for the rules this follows; nothing here should diverge
//! from it except where Rust's string handling forces a different shape
//! (owned `String`s instead of slices, once a wrapped line is rejoined).
//!
//! `rerun` and `needs_bibtex` are the two heuristics `native.rs` uses to
//! decide whether another pass, or a bibliography tool, is needed; they are
//! ported here rather than duplicated because they read the same log text
//! `parse` does.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use super::protocol::Diagnostic;

/// How many lines after a `!` a `l.<n>` may appear and still belong to it.
const LINE_WITHIN: usize = 4;
/// TeX's `max_print_line`: the width at which a log line is broken with no
/// continuation marker of any kind.
const WRAP: usize = 79;

struct WarningRule {
    matches: Regex,
    line: Regex,
}

fn warning_rules() -> &'static Vec<WarningRule> {
    static RULES: OnceLock<Vec<WarningRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            WarningRule {
                matches: Regex::new(r"^(Overfull|Underfull) \\(hbox|vbox) \(").unwrap(),
                line: Regex::new(r"at lines? (\d+)").unwrap(),
            },
            WarningRule {
                matches: Regex::new(r"^Package [^ ]+ Warning:").unwrap(),
                line: Regex::new(r"on input line (\d+)").unwrap(),
            },
            WarningRule {
                matches: Regex::new(r"^(LaTeX|Class [^ ]+) Warning:").unwrap(),
                line: Regex::new(r"on input line (\d+)").unwrap(),
            },
        ]
    })
}

fn line_marker() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^l\.(\d+)").unwrap())
}

/// Everything the log has to say about a compile, as diagnostics. `main` is
/// the project-relative main file (used only to report it as the empty
/// `file`, as `diagnostic.rs` asks); `paths` is every project-relative path
/// the workspace holds, so a file the log names by a form the tree does not
/// use -- `./x.tex` for `x.tex`, an extensionless include -- is reported as
/// the path the editor can actually open.
pub fn parse(log: &str, main: &str, paths: &[String]) -> Vec<Diagnostic> {
    let lines = unwrap_log(log);
    let known: HashSet<&str> = paths.iter().map(|s| s.as_str()).collect();
    let mut diagnostics = Vec::new();
    let mut open: Vec<Option<String>> = Vec::new();
    let rules = warning_rules();

    for i in 0..lines.len() {
        let line = lines[i].as_str();
        track(line, &mut open, &known, main);

        if let Some(rest) = line.strip_prefix('!') {
            let _ = rest;
            diagnostics.push(error_diagnostic(&lines, i, &open, main));
            continue;
        }
        if let Some(rule) = rules.iter().find(|r| r.matches.is_match(line)) {
            let next = lines.get(i + 1).map(String::as_str).unwrap_or("");
            let text = format!("{line} {next}");
            let at = rule
                .line
                .captures(&text)
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().parse::<u32>().ok());
            diagnostics.push(Diagnostic {
                severity: "warning".to_string(),
                message: clean(line),
                hints: Vec::new(),
                file: current(&open, main),
                line: at.unwrap_or(0),
                column: if at.is_some() { 1 } else { 0 },
                end_line: at.unwrap_or(0),
                end_column: 0,
            });
        }
    }
    diagnostics
}

/// One `!` line and whatever belongs to it: the message, the hints between
/// it and the `l.<n>`, and the line the marker gives.
fn error_diagnostic(
    lines: &[String],
    at: usize,
    open: &[Option<String>],
    main: &str,
) -> Diagnostic {
    let message = clean(lines[at].trim_start_matches('!').trim_start());
    let mut hints: Vec<String> = Vec::new();
    let mut line = 0u32;
    let mut j = at + 1;
    while j < lines.len() && j <= at + LINE_WITHIN {
        let next = lines[j].as_str();
        // A second `!` ends the first error, whatever else was coming.
        if next.starts_with('!') {
            break;
        }
        if let Some(cap) = line_marker().captures(next) {
            line = cap[1].parse().unwrap_or(0);
            break;
        }
        let hint = clean(next);
        if !hint.is_empty() {
            hints.push(hint);
        }
        j += 1;
    }
    // A `!` with no `l.<n>` within reach has no place in any source; keep
    // at most three hints for it, the same cap `log.js` uses.
    if line == 0 && hints.len() > 3 {
        hints.truncate(3);
    }
    Diagnostic {
        severity: "error".to_string(),
        message,
        hints,
        file: current(open, main),
        line,
        column: if line > 0 { 1 } else { 0 },
        end_line: line,
        end_column: 0,
    }
}

/// The file the log is inside, as a path of the tree, or `""` for the main
/// file.
fn current(open: &[Option<String>], main: &str) -> String {
    match open.iter().rev().flatten().next() {
        Some(path) if path == main => String::new(),
        Some(path) => path.clone(),
        None => String::new(),
    }
}

/// TeX opens a file with `(` and a path and closes it with `)`, several
/// times on one line, mixed with page markers and font names. This walks
/// the line character by character rather than counting brackets, so a `)`
/// inside a message is not mistaken for a file closing; an unmatched `)`
/// pops nothing rather than panicking.
fn track(line: &str, open: &mut Vec<Option<String>>, known: &HashSet<&str>, main: &str) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '(' => {
                let mut j = i + 1;
                while j < chars.len() && !" ()[]{}".contains(chars[j]) {
                    j += 1;
                }
                let raw: String = chars[i + 1..j].iter().collect();
                open.push(if raw.is_empty() {
                    None
                } else {
                    normalise(&raw, known, main)
                });
                i = j;
            }
            ')' => {
                open.pop();
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// The path the tree knows, from the path TeX printed. An absolute path
/// outside the project (a TeX Live file) is not a file of the tree at all,
/// and is reported as `None` so it neither names a diagnostic nor hides the
/// file underneath it.
fn normalise(raw: &str, known: &HashSet<&str>, main: &str) -> Option<String> {
    let path = raw.strip_prefix("./").unwrap_or(raw);
    if path == main {
        return Some(main.to_string());
    }
    if known.contains(path) {
        return Some(path.to_string());
    }
    let with_tex = format!("{path}.tex");
    if known.contains(with_tex.as_str()) {
        return Some(with_tex);
    }
    None
}

/// TeX wraps its log at 79 columns with no continuation marker, which turns
/// one message into two lines. Joining a line to the next when the
/// *previous raw* line was exactly at the wrap width is the standard
/// reading of that.
fn unwrap_log(log: &str) -> Vec<String> {
    let normalized = log.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<String> = Vec::new();
    let mut continued = false;
    for line in normalized.split('\n') {
        if continued && !out.is_empty() && !line.is_empty() && !line.starts_with('!') {
            if let Some(last) = out.last_mut() {
                last.push_str(line);
            }
        } else {
            out.push(line.to_string());
        }
        continued = line.chars().count() == WRAP;
    }
    out
}

fn clean(text: &str) -> String {
    text.trim().to_string()
}

/// Whether the log asks to be run again: an undefined reference, a moved
/// label, a table of contents that changed.
pub fn rerun(log: &str) -> bool {
    static AGAIN: OnceLock<Regex> = OnceLock::new();
    static UNDEFINED: OnceLock<Regex> = OnceLock::new();
    let again = AGAIN.get_or_init(|| {
        Regex::new(r"Rerun to get|Rerun LaTeX|Please rerun|Label\(s\) may have changed").unwrap()
    });
    let undefined = UNDEFINED
        .get_or_init(|| Regex::new(r"LaTeX Warning: (Reference|Citation) .* undefined").unwrap());
    again.is_match(log) || undefined.is_match(log)
}

/// Whether the log says BibTeX has work to do: a citation the document made
/// and no bibliography to resolve it from yet.
pub fn needs_bibtex(log: &str) -> bool {
    static CITED: OnceLock<Regex> = OnceLock::new();
    static NO_BBL: OnceLock<Regex> = OnceLock::new();
    let cited = CITED.get_or_init(|| Regex::new(r"LaTeX Warning: Citation .* undefined").unwrap());
    let no_bbl = NO_BBL.get_or_init(|| Regex::new(r"No file .*\.bbl\.").unwrap());
    cited.is_match(log) || no_bbl.is_match(log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn corpus_paths(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some("logs") {
                continue;
            }
            if path.is_dir() {
                corpus_paths(&path, base, out);
            } else if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    fn broken_paths() -> Vec<String> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../latex/corpus/broken");
        let mut out = Vec::new();
        corpus_paths(&base, &base, &mut out);
        out
    }

    #[test]
    fn undefined_control_sequence_lands_in_the_chapter_at_line_seven() {
        let log_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../latex/corpus/broken/logs/texlive.log");
        let log = fs::read_to_string(&log_path).expect("broken/logs/texlive.log");
        let diagnostics = parse(&log, "main.tex", &broken_paths());

        let undefined = diagnostics
            .iter()
            .find(|d| d.severity == "error" && d.message.contains("Undefined control sequence"))
            .expect("an undefined control sequence diagnostic");
        assert_eq!(undefined.file, "chapters/01.tex");
        assert_eq!(undefined.line, 7);
    }

    #[test]
    fn the_package_warning_is_a_warning_not_an_error() {
        let log_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../latex/corpus/broken/logs/texlive.log");
        let log = fs::read_to_string(&log_path).expect("broken/logs/texlive.log");
        let diagnostics = parse(&log, "main.tex", &broken_paths());

        let package_warning = diagnostics
            .iter()
            .find(|d| d.message.contains("brokenpkg Warning"))
            .expect("a brokenpkg warning diagnostic");
        assert_eq!(package_warning.severity, "warning");
        assert_eq!(package_warning.line, 9);
    }

    #[test]
    fn an_overfull_hbox_is_a_warning_with_its_line() {
        let log_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../latex/corpus/broken/logs/texlive.log");
        let log = fs::read_to_string(&log_path).expect("broken/logs/texlive.log");
        let diagnostics = parse(&log, "main.tex", &broken_paths());

        let overfull = diagnostics
            .iter()
            .find(|d| d.message.starts_with("Overfull"))
            .expect("an overfull hbox diagnostic");
        assert_eq!(overfull.severity, "warning");
    }

    #[test]
    fn rerun_recognises_the_standard_phrases() {
        assert!(rerun(
            "LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right."
        ));
        assert!(rerun(
            "Package hyperref Warning: Rerun to get \\  \\ references right."
        ));
        assert!(rerun(
            "LaTeX Warning: Citation `x' on page 1 undefined on input line 3."
        ));
        assert!(!rerun("This document compiled cleanly."));
    }

    #[test]
    fn needs_bibtex_recognises_undefined_citations_and_a_missing_bbl() {
        assert!(needs_bibtex(
            "LaTeX Warning: Citation `knuth1984' on page 1 undefined on input line 5."
        ));
        assert!(needs_bibtex("No file main.bbl."));
        assert!(!needs_bibtex("Everything is fine."));
    }

    #[test]
    fn a_line_wrapped_at_the_print_width_is_rejoined() {
        // TeX wraps at column 79 with no continuation marker: a warning
        // whose message happens to fall on the boundary is split across two
        // raw lines, and `unwrap_log` must rejoin them before the `on input
        // line` regex can find the number.
        let prefix = "Package foo Warning: something rather long goes here up to";
        let first = format!("{prefix}{}", " ".repeat(WRAP - prefix.len()));
        assert_eq!(first.chars().count(), WRAP);
        let log = format!("{first} on input line 12.\n");
        let diagnostics = parse(&log, "main.tex", &[]);
        let warning = diagnostics
            .iter()
            .find(|d| d.severity == "warning")
            .expect("the rejoined warning");
        assert_eq!(warning.line, 12);
    }
}
