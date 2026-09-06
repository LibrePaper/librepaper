// The log parser, against every log in the corpus.
//
// Shaped like `check-diagnostics.mjs`: a table of cases, a pure function, no
// browser and no engine. It runs always, because it needs neither the mirror
// nor a distribution -- the logs are checked in beside the documents that
// produced them, which is the whole point of collecting them.
//
// Two kinds of case. The first is a table of small hand-written logs, one per
// rule, so that a rule that stops working says which rule. The second is the
// corpus: for every `latex/corpus/<doc>/logs/<engine>.log`, the parser must
// produce what `expected.json` says if there is one, and in every case must
// not invent an error out of a log whose document compiled.

import { readFileSync, existsSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { parse, rerun, needsBibtex } from "../src/lib/latex/log.js";

const HERE = dirname(new URL(import.meta.url).pathname);
const CORPUS = join(dirname(dirname(HERE)), "latex", "corpus");

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`latex-log: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

/* ------------------------------------------------------------- the table */

const CASES = [
  {
    what: "an error is the ! line, at the l. line's number, spanning the whole line",
    log: ["(./main.tex", "! Undefined control sequence.", "l.7 \\nope", "                "].join("\n"),
    expect: [
      {
        severity: "error",
        message: "Undefined control sequence.",
        hints: [],
        file: "",
        line: 7,
        column: 1,
        end_line: 7,
        end_column: 0,
      },
    ],
  },
  {
    what: "the hints are the lines between the ! and the l.",
    log: [
      "(./main.tex",
      "! Package inputenc Error: Unicode character not set up.",
      "See the inputenc package documentation for explanation.",
      "Type  H <return>  for immediate help.",
      "l.12 caf\\'e",
    ].join("\n"),
    expect: [
      {
        severity: "error",
        message: "Package inputenc Error: Unicode character not set up.",
        hints: [
          "See the inputenc package documentation for explanation.",
          "Type  H <return>  for immediate help.",
        ],
        file: "",
        line: 12,
        column: 1,
        end_line: 12,
        end_column: 0,
      },
    ],
  },
  {
    what: "an error with no l. line near it has no place in the source",
    log: ["(./main.tex", "! LaTeX Error: File `nope.sty' not found.", "", "", "", "", "", "l.99 \\x"].join("\n"),
    expect: [
      {
        severity: "error",
        message: "LaTeX Error: File `nope.sty' not found.",
        hints: [],
        file: "",
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
      },
    ],
  },
  {
    what: "the parenthesis tracking names the file an error is in",
    log: [
      "(./main.tex",
      "(./chapters/01.tex",
      "! Undefined control sequence.",
      "l.7 \\nope",
    ].join("\n"),
    paths: ["main.tex", "chapters/01.tex"],
    expect: [
      {
        severity: "error",
        message: "Undefined control sequence.",
        hints: [],
        file: "chapters/01.tex",
        line: 7,
        column: 1,
        end_line: 7,
        end_column: 0,
      },
    ],
  },
  {
    what: "a file that has been closed again no longer names the error",
    log: [
      "(./main.tex",
      "(./chapters/01.tex",
      ")",
      "! Undefined control sequence.",
      "l.9 \\nope",
    ].join("\n"),
    paths: ["main.tex", "chapters/01.tex"],
    expect: [{ file: "", line: 9, severity: "error" }],
    only: ["file", "line", "severity"],
  },
  {
    what: "an overfull hbox is a warning at the line the log gives",
    log: ["(./main.tex", "Overfull \\hbox (18.9pt too wide) in paragraph at lines 3--6", "[]"].join("\n"),
    expect: [
      {
        severity: "warning",
        message: "Overfull \\hbox (18.9pt too wide) in paragraph at lines 3--6",
        hints: [],
        file: "",
        line: 3,
        column: 1,
        end_line: 3,
        end_column: 0,
      },
    ],
  },
  {
    what: "an underfull vbox is a warning too",
    log: ["(./main.tex", "Underfull \\vbox (badness 10000) detected at line 40"].join("\n"),
    expect: [{ severity: "warning", line: 40 }],
    only: ["severity", "line"],
  },
  {
    what: "a package warning is a warning at its input line",
    log: ["(./main.tex", "Package brokenpkg Warning: This package warns on purpose on input line 9."].join("\n"),
    expect: [
      {
        severity: "warning",
        message: "Package brokenpkg Warning: This package warns on purpose on input line 9.",
        hints: [],
        file: "",
        line: 9,
        column: 1,
        end_line: 9,
        end_column: 0,
      },
    ],
  },
  {
    what: "a LaTeX warning with no line number is a warning at line 0",
    log: ["(./main.tex", "LaTeX Warning: There were undefined references."].join("\n"),
    expect: [{ severity: "warning", line: 0, column: 0 }],
    only: ["severity", "line", "column"],
  },
  {
    what: "a warning wrapped by TeX at 79 columns still finds its line number",
    log: [
      "(./main.tex",
      "Package hyperref Warning: Token not allowed in a PDF string (Unicode): remo" + "ving `\\newline' on input line 42.",
    ].join("\n"),
    expect: [{ severity: "warning", line: 42 }],
    only: ["severity", "line"],
  },
  {
    what: "an unrecognised line is not a diagnostic",
    log: [
      "(./main.tex",
      "LaTeX Font Info:    Checking defaults for OML/cmm/m/it on input line 21.",
      "\\openout1 = `main.aux'.",
      "[1{/usr/share/texmf/fonts/map/pdftex.map}]",
      "Here is how much of TeX's memory you used:",
    ].join("\n"),
    expect: [],
  },
  {
    what: "a path outside the tree does not name a diagnostic",
    log: [
      "(./main.tex",
      "(/nix/store/abc-texlive/tex/latex/base/article.cls",
      "! Undefined control sequence.",
      "l.5 \\nope",
    ].join("\n"),
    paths: ["main.tex"],
    expect: [{ file: "", line: 5 }],
    only: ["file", "line"],
  },
];

for (const one of CASES) {
  const got = parse(one.log, { main: "main.tex", paths: one.paths || ["main.tex"] });
  const trim = (list) =>
    one.only ? list.map((d) => Object.fromEntries(one.only.map((k) => [k, d[k]]))) : list;
  const a = JSON.stringify(trim(got));
  const b = JSON.stringify(one.expect);
  check(one.what, a === b, a === b ? "" : `got ${a}`);
}

/* --------------------------------------------------------- rerun and bibtex */

check(
  "a log asking to be rerun says so",
  rerun("LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right."),
);
check("a clean log does not ask to be rerun", !rerun("Output written on main.pdf (3 pages)."));
check(
  "an undefined citation asks for BibTeX",
  needsBibtex("LaTeX Warning: Citation `knuth1984' on page 1 undefined on input line 8."),
);
check("a clean log does not ask for BibTeX", !needsBibtex("Output written on main.pdf (3 pages)."));

/* ---------------------------------------------------------------- the corpus */

let logsSeen = 0;
for (const doc of readdirSync(CORPUS, { withFileTypes: true })) {
  if (!doc.isDirectory()) continue;
  const dir = join(CORPUS, doc.name);
  const logs = join(dir, "logs");
  if (!existsSync(logs)) continue;
  const paths = treePaths(dir);
  const expected = existsSync(join(dir, "expected.json"))
    ? JSON.parse(readFileSync(join(dir, "expected.json"), "utf8"))
    : null;

  for (const file of readdirSync(logs)) {
    if (!file.endsWith(".log")) continue;
    logsSeen += 1;
    const engine = file.replace(/\.log$/, "");
    const log = readFileSync(join(logs, file), "latin1");
    const got = parse(log, { main: "main.tex", paths });

    if (expected) {
      const want = expected[engine] || expected.default;
      if (want) {
        const a = JSON.stringify(got, null, 1);
        const b = JSON.stringify(want, null, 1);
        check(`${doc.name}/${file} matches its expected diagnostics`, a === b, a === b ? "" : diff(got, want));
        continue;
      }
    }
    // The log says for itself whether a document came out of it. When one
    // did, the parser must find no error in it: an error over a document
    // that exists is a false alarm, and a false alarm is the one failure
    // mode a log parser must not have. When none did -- an engine that
    // cannot set this document, which is half the point of the corpus --
    // there is nothing to hold it to here beyond naming real files, and
    // `broken/expected.json` is where the exact diagnostics live.
    const produced = /Output written on [^(]*\(\d+ pages?/.test(log) && !/no output PDF file produced/.test(log);
    const errors = got.filter((one) => one.severity === "error");
    if (produced) {
      check(
        `${doc.name}/${file} yields no error from a log that produced a document`,
        errors.length === 0,
        errors.map((one) => one.message).join(" | "),
      );
    } else {
      check(
        `${doc.name}/${file} is a compile that produced nothing, and the parser says why`,
        errors.length > 0,
        "no error at all in a log with no document behind it",
      );
    }
    // And every file it names is a path of the tree, or the main file.
    const strange = got.filter((one) => one.file && !paths.includes(one.file));
    check(
      `${doc.name}/${file} names only paths of the tree`,
      strange.length === 0,
      strange.map((one) => one.file).join(" "),
    );
  }
}
check("there are logs in the corpus to check", logsSeen > 0, `${logsSeen} logs`);

function treePaths(dir, prefix = "") {
  const out = [];
  for (const item of readdirSync(dir, { withFileTypes: true })) {
    if (item.name === "logs") continue;
    const path = prefix ? `${prefix}/${item.name}` : item.name;
    if (item.isDirectory()) out.push(...treePaths(join(dir, item.name), path));
    else out.push(path);
  }
  return out;
}

function diff(got, want) {
  for (let i = 0; i < Math.max(got.length, want.length); i++) {
    const a = JSON.stringify(got[i]);
    const b = JSON.stringify(want[i]);
    if (a !== b) return `at ${i}: got ${a}, wanted ${b}`;
  }
  return "";
}

if (failures) {
  console.error(`latex-log: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`latex-log: the parser agrees with ${CASES.length} rules and ${logsSeen} log(s) from the corpus`);
