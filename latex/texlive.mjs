#!/usr/bin/env node
// The corpus, compiled on a TeX Live on this machine.
//
// Every example has to compile somewhere a person can check by hand, or the
// page counts the browser distributions are measured against are only the
// browser distributions agreeing with each other. So this runs the same
// documents through the pdflatex, xelatex and bibtex on the PATH, writes the
// log to `examples/latex/<doc>/logs/texlive.log`, and records the page count
// in `examples/latex/pages.json`, which the headless check then holds each
// distribution to.
//
// It writes nothing into the examples but the logs and the page counts: the
// compile happens in a temporary directory, so no `.aux` litter reaches the
// corpus and a re-run cannot be made green by a stale `.aux`.
//
//     node latex/texlive.mjs

import { execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";

const HERE = dirname(new URL(import.meta.url).pathname);
const CORPUS = join(dirname(HERE), "examples", "latex");

/// Which engine each example wants, and what it is for. The `xetex` example
/// is the one pdfTeX must refuse, so it is compiled with xelatex here and its
/// refusal is recorded rather than its page count.
// `--pdf` keeps the PDF as well as the log. The viewer of step 4 has to draw
// a real document -- pages, a hyphenated line end, a footnote, a ligature --
// and the corpus is where those live. It is a build output rather than a
// fixture: git ignores it, and a machine with no TeX Live skips the checks
// that need it.
const KEEP_PDF = process.argv.includes("--pdf");

const EXAMPLES = [
  { name: "article", engine: "pdflatex", bibtex: true },
  { name: "paper", engine: "pdflatex", bibtex: true },
  { name: "broken", engine: "pdflatex", bibtex: false },
  { name: "xetex", engine: "xelatex", bibtex: false },
];

// The page count comes from the log, not from the PDF. Every TeX engine ends
// a successful run with `Output written on main.pdf (3 pages, ...)`, and the
// PDF itself hides its page tree inside a compressed object stream that
// counting would have to inflate. The log line is the same on pdfTeX, XeTeX
// and LuaTeX, and it is the number the browser distributions are held to.
export function pagesFromLog(log) {
  const written = log.match(/Output written on [^(]*\((\d+) pages?/);
  return written ? Number(written[1]) : 0;
}

const record = {};
for (const example of EXAMPLES) {
  const source = join(CORPUS, example.name);
  const work = mkdtempSync(join(tmpdir(), `komodoc-latex-${example.name}-`));
  cpSync(source, work, { recursive: true, filter: (from) => !from.includes("/logs") });
  rmSync(join(work, "logs"), { recursive: true, force: true });

  const run = (command, args) => {
    try {
      execFileSync(command, args, { cwd: work, stdio: "ignore" });
    } catch {
      /* a failing compile is a result, not a crash: the log is the point */
    }
  };
  run(example.engine, ["-interaction=nonstopmode", "-synctex=1", "main.tex"]);
  if (example.bibtex) {
    run("bibtex", ["main"]);
    run(example.engine, ["-interaction=nonstopmode", "-synctex=1", "main.tex"]);
  }
  run(example.engine, ["-interaction=nonstopmode", "-synctex=1", "main.tex"]);

  const log = join(work, "main.log");
  const logs = join(source, "logs");
  mkdirSync(logs, { recursive: true });
  const text = existsSync(log) ? readFileSync(log, "latin1") : "";
  if (text) writeFileSync(join(logs, "texlive.log"), text, "latin1");
  if (KEEP_PDF) {
    const pdf = join(work, "main.pdf");
    if (existsSync(pdf)) cpSync(pdf, join(source, "main.pdf"));
  }
  const count = pagesFromLog(text);
  record[example.name] = { engine: example.engine, pages: count, synctex: existsSync(join(work, "main.synctex.gz")) };
  console.log(`texlive: ${example.name.padEnd(8)} ${count} page(s) with ${example.engine}`);
  rmSync(work, { recursive: true, force: true });
}

writeFileSync(join(CORPUS, "pages.json"), JSON.stringify(record, null, 2) + "\n");
console.log(`texlive: page counts written to ${join(CORPUS, "pages.json")}`);
