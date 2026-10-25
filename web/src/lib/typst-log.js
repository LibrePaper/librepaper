// The Typst command line's log, read.
//
// A command run on this computer returns only what it printed. A failed
// Typst compile prints `error: <message>` and, under it, a line such as
// `┌─ chapters/main.typ:4:7`. The log is reduced to one error diagnostic in
// the shape the browser engines produce, so the gutter mark and the badge
// treat it like any other.

const ANSI = /\x1b\[[0-?]*[ -/]*[@-~]/g;
const LOCATION = /┌─\s*([^:]+):(\d+):(\d+)/g;

/// The log without terminal colour codes, which a page would show as junk.
export function stripAnsi(log) {
  return String(log || "").replace(ANSI, "");
}

/// A one-line summary of a failed run: the first line that starts with
/// "error" in any case, without an `error:` label. A log with no such line
/// has no summary.
export function failureReason(log) {
  for (const raw of stripAnsi(log).split("\n")) {
    const line = raw.trim();
    const lower = line.toLowerCase();
    if (!lower.startsWith("error")) continue;
    const text = lower.startsWith("error:") ? line.slice(6).trim() : line;
    if (text) return text;
  }
  return null;
}

/// The diagnostics of a failed run. `paths` are the project's files: a
/// location is kept only when it names one of them, so a tool that quotes a
/// path on this computer cannot put it in the page.
export function parse(log, { paths = [] } = {}) {
  const known = new Set(paths);
  let file = "";
  let line = 0;
  let column = 0;
  for (const match of String(log || "").matchAll(LOCATION)) {
    const candidate = match[1].trim();
    if (!known.has(candidate)) continue;
    file = candidate;
    line = Number(match[2]);
    column = Number(match[3]);
    break;
  }
  return [{
    severity: "error",
    message: failureReason(log) ?? "The Typst command failed.",
    hints: [],
    file,
    line,
    column,
    end_line: line,
    end_column: column,
  }];
}
