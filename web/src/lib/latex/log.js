// The TeX log, read.
//
// The log is parsed here, in JavaScript, and not in the engine crate, because
// the compiler is not our crate: there is no Rust on the other side of a
// LaTeX compile to hand us a diagnostic list the way typst's does. So this
// file is the one place in Komodoc where a diagnostic is manufactured rather
// than received, and it emits `engine/src/diagnostic.rs`'s shape unchanged so
// that everything downstream -- the gutter mark, the underline, the badge --
// cannot tell a LaTeX error from a typst one.
//
// TeX's log format is not a format. It is what a program printed in 1982 and
// has printed since, and every rule below is a rule about that printing:
//
//   - A line beginning `!` is an error. The line after the last hint is
//     `l.<n> <the text of the line>`, and `<n>` is where the error is, in the
//     file the parenthesis tracking says is open. Nothing gives a column, so
//     the span is the whole line.
//   - `(` followed by a path opens a file and `)` closes it. Parentheses also
//     appear inside messages and inside `[1]` page markers, so the tracking
//     is a scan for path-shaped openings rather than a bracket count.
//   - `LaTeX Warning:`, `Package <p> Warning:`, `Overfull \hbox`,
//     `Underfull \hbox` are warnings, with a line when the log gives one and
//     `line` 0 when it does not.
//   - Everything else is prose. A line this parser does not recognise is not
//     an error -- it is not anything -- and the raw log is one click from the
//     badge for whatever was missed.
//
// It is a pure function of the log string and of the tree's paths, which is
// what lets `latex-log-check.mjs` run it over every log in the corpus with no
// browser and no engine.

/// How many lines after a `!` a `l.<n>` may appear and still belong to it.
/// TeX prints the message, then its advice, then the line; the advice is at
/// most a couple of lines in every log in the corpus. Beyond this the `l.<n>`
/// belongs to a later error -- an emergency stop, usually -- and claiming it
/// would put the first error on the wrong line.
const LINE_WITHIN = 4;

/// The warnings, as a table. Each entry says how to recognise the line, what
/// the message is, and where to look for a line number. Adding a warning is
/// adding a row.
const WARNINGS = [
  {
    // `Overfull \hbox (12.3pt too wide) in paragraph at lines 3--6`
    // `Overfull \vbox (5.0pt too high) detected at line 12`
    match: /^(Overfull|Underfull) \\(hbox|vbox) \(/,
    line: /at lines? (\d+)/,
  },
  {
    // `Package hyperref Warning: Token not allowed ... on input line 42.`
    match: /^Package [^ ]+ Warning:/,
    line: /on input line (\d+)/,
  },
  {
    // `LaTeX Warning: Reference `fig:one' on page 1 undefined on input line 8.`
    match: /^(LaTeX|Class [^ ]+) Warning:/,
    line: /on input line (\d+)/,
  },
];

/// Everything the log has to say about a compile, as diagnostics.
///
/// `main` is the tree's main path, used only to recognise it and report it as
/// the empty file the way `diagnostic.rs` asks. `paths` is the tree's paths,
/// so that a file the log names by a form the tree does not use -- `./x.tex`
/// for `x.tex`, `chapters/01` for `chapters/01.tex` -- is reported as the
/// path the editor can actually open.
export function parse(log, { main = "main.tex", paths = [] } = {}) {
  const lines = unwrap(log);
  const known = new Set(paths);
  const diagnostics = [];
  const open = []; // the parenthesis stack: files TeX has entered

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    track(line, open, known, main);

    if (line.startsWith("!")) {
      diagnostics.push(error(lines, i, open, main, known));
      continue;
    }
    const rule = WARNINGS.find((one) => one.match.test(line));
    if (rule) {
      // A warning's own line may be wrapped onto the next one, and the line
      // number often sits there rather than on the first line.
      const text = [line, lines[i + 1] || ""].join(" ");
      const at = text.match(rule.line);
      diagnostics.push(
        place({
          severity: "warning",
          message: clean(line),
          hints: [],
          file: current(open, main),
          line: at ? Number(at[1]) : 0,
        }),
      );
    }
  }
  return diagnostics;
}

/// One `!` line and whatever belongs to it.
function error(lines, at, open, main, known) {
  const message = clean(lines[at].replace(/^!\s*/, ""));
  const hints = [];
  let line = 0;
  for (let i = at + 1; i < lines.length && i <= at + LINE_WITHIN; i++) {
    const next = lines[i];
    // A second `!` ends the first error, whatever else was coming.
    if (next.startsWith("!")) break;
    const marker = next.match(/^l\.(\d+)/);
    if (marker) {
      line = Number(marker[1]);
      break;
    }
    // The spec's rule: the hints are the lines between the `!` and the
    // `l.<n>`. Blank lines and TeX's own trailing spaces are not hints.
    const hint = clean(next);
    if (hint) hints.push(hint);
  }
  // A `!` with no `l.<n>` within reach has no place in any source; it is
  // reported spanless rather than guessed at.
  if (!line) hints.length = Math.min(hints.length, 3);
  return place({
    severity: "error",
    message,
    hints: line ? hints : hints,
    file: current(open, main),
    line,
  });
}

/// A diagnostic in `diagnostic.rs`'s shape. TeX reports no column, so the
/// span is the whole line: column 1, and an end that is the same line with
/// column 0, which every surface already reads as "to the end".
function place({ severity, message, hints, file, line }) {
  return {
    severity,
    message,
    hints,
    file,
    line,
    column: line ? 1 : 0,
    end_line: line,
    end_column: 0,
  };
}

/// The file the log is inside, as a path of the tree, or `""` for the main
/// file -- which is what `diagnostic.rs` means by an empty `file`.
function current(open, main) {
  for (let i = open.length - 1; i >= 0; i--) {
    if (open[i] && open[i] !== main) return open[i];
    if (open[i] === main) return "";
  }
  return "";
}

// TeX opens a file with `(` and a path and closes it with `)`, and does both
// several times on one line, mixed in with page markers and font names. This
// walks the line character by character, which is the only way that does not
// mistake a `)` in a message for a file closing -- and even then it is
// approximate, which is why an unmatched `)` pops nothing rather than
// throwing.
function track(line, open, known, main) {
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === "(") {
      let j = i + 1;
      while (j < line.length && !" ()[]{}".includes(line[j])) j++;
      const raw = line.slice(i + 1, j);
      open.push(raw ? normalise(raw, known, main) : null);
      i = j - 1;
    } else if (c === ")") {
      open.pop();
    }
  }
}

/// The path the tree knows, from the path TeX printed. TeX writes `./x.tex`
/// for a file beside the document and drops the extension in some places, and
/// an absolute path for anything out of the distribution -- which is not a
/// file of the tree at all and is reported as `null`, so it neither names a
/// diagnostic nor hides the file underneath it.
function normalise(raw, known, main) {
  const path = raw.replace(/^\.\//, "");
  if (path === main) return main;
  if (known.has(path)) return path;
  if (known.has(path + ".tex")) return path + ".tex";
  return null;
}

/// TeX wraps its log at 79 columns with no continuation marker, which turns
/// one message into two lines and hides a line number in the middle of a
/// word. Joining a line to the next when the first is exactly at the wrap
/// width is the standard reading of that, and it is what makes
/// `on input line 9.` findable when the wrap fell inside it.
function unwrap(log) {
  const raw = log.replace(/\r\n?/g, "\n").split("\n");
  const out = [];
  // Whether the previous *raw* line was full, not whether the line built so
  // far is long. Testing the accumulated line instead would join the whole
  // rest of the log onto the first wide path TeX printed, and every file
  // opened after it would be invisible to the parenthesis tracking.
  let continued = false;
  for (const line of raw) {
    if (continued && out.length && line && !line.startsWith("!")) {
      out[out.length - 1] += line;
    } else {
      out.push(line);
    }
    continued = line.length === WRAP;
  }
  return out;
}

/// TeX's `max_print_line`, and so the width at which it breaks a log line
/// with no continuation marker of any kind.
const WRAP = 79;

const clean = (text) => text.replace(/\s+$/, "").trim();

/// Whether the log asks to be run again -- an undefined reference, a moved
/// label, a table of contents that changed. This is not a diagnostic; it is
/// what makes `compile` run a second and third pass instead of always running
/// three, which on a document of any size is seconds saved on every keystroke
/// that did not move a label.
export function rerun(log) {
  return (
    /Rerun to get|Rerun LaTeX|Please rerun|Label\(s\) may have changed/.test(log) ||
    /LaTeX Warning: (Reference|Citation) .* undefined/.test(log)
  );
}

/// Whether the log says BibTeX has work to do: a citation the document made
/// and no bibliography to resolve it from yet.
export function needsBibtex(log) {
  return /LaTeX Warning: Citation .* undefined/.test(log) || /No file .*\.bbl\./.test(log);
}
