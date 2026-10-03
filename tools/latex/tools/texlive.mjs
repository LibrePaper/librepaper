#!/usr/bin/env node
// Compile the LaTeX corpus with a local TeX Live as an independent oracle.
// Successful examples must produce a positive page count; `broken` is the one
// intentional negative fixture and must fail with the diagnostics it contains.
// All results are collected before any checked-in log/page fixture is replaced.
//
//     node tools/latex/tools/texlive.mjs [--pdf]

import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const DEFAULT_CORPUS = join(dirname(HERE), "corpus");
const CORPUS = DEFAULT_CORPUS;
const KEEP_PDF = process.argv.includes("--pdf");

/// The `xetex` sample tests rejection by pdfTeX; `unicode-fonts` is the
/// independent positive XeTeX glyph-coverage sample using named Libertinus
/// text/math fonts. `broken` is the only example expected to exit unsuccessfully.
const EXAMPLES = [
  { name: "article", engine: "pdflatex", bibtex: true },
  { name: "paper", engine: "pdflatex", bibtex: true },
  { name: "broken", engine: "pdflatex", negative: true },
  { name: "xetex", engine: "xelatex" },
  { name: "unicode-fonts", engine: "xelatex", unicode: true },
  // This package-set probe is outside the browser comparison set.
  { name: "packages", engine: "pdflatex", bibtex: true },
];

const GENERATED = /\.(?:aux|bbl|bcf|blg|fdb_latexmk|fls|log|out|pdf|run\.xml|synctex\.gz|toc|xdv)$/;

export function pagesFromLog(log) {
  const written = log.match(/Output written on [^(]*\((\d+) pages?/);
  return written ? Number(written[1]) : 0;
}

function run(command, args, cwd, env) {
  try {
    execFileSync(command, args, { cwd, env, stdio: "ignore" });
    return 0;
  } catch (error) {
    if (error.code === "ENOENT") throw new Error(`required TeX Live command not found: ${command}`);
    if (Number.isInteger(error.status)) return error.status;
    throw new Error(`${command} could not complete (${error.signal || error.code || "unknown error"})`);
  }
}

function compile(example, work, keepPdf, env) {
  const args = ["-interaction=nonstopmode", "-synctex=1", "main.tex"];
  const status = run(example.engine, args, work, env);
  if (example.negative) {
    const log = join(work, "main.log");
    if (status === 0 || !existsSync(log)) throw new Error("broken fixture unexpectedly compiled or produced no log");
    const text = readFileSync(log, "latin1");
    for (const diagnostic of ["Undefined control sequence.", "chapters/missing.tex", "Fatal error occurred"]) {
      if (!text.includes(diagnostic)) throw new Error(`broken fixture log is missing expected diagnostic: ${diagnostic}`);
    }
    if (pagesFromLog(text) !== 0) throw new Error("broken fixture unexpectedly reports a successful page count");
    return { text, pages: 0 };
  }
  if (status !== 0) throw new Error(`${example.name}: ${example.engine} exited with status ${status}`);
  if (example.bibtex) {
    const bibliographyStatus = run("bibtex", ["main"], work, env);
    if (bibliographyStatus !== 0) throw new Error(`${example.name}: bibtex exited with status ${bibliographyStatus}`);
    for (let pass = 0; pass < 2; pass += 1) {
      const passStatus = run(example.engine, args, work, env);
      if (passStatus !== 0) throw new Error(`${example.name}: ${example.engine} exited with status ${passStatus}`);
    }
  }
  const log = join(work, "main.log");
  if (!existsSync(log)) throw new Error(`${example.name}: ${example.engine} produced no main.log`);
  const text = readFileSync(log, "latin1");
  const pages = pagesFromLog(text);
  if (pages < 1) throw new Error(`${example.name}: ${example.engine} log has no positive page count`);
  if (!existsSync(join(work, "main.synctex.gz"))) throw new Error(`${example.name}: ${example.engine} produced no SyncTeX file`);
  if (example.unicode && /Missing character:/i.test(text)) {
    throw new Error(`${example.name}: XeTeX reported a missing glyph`);
  }
  if (keepPdf) {
    const pdf = join(work, "main.pdf");
    if (!existsSync(pdf) || readFileSync(pdf).length === 0) throw new Error(`${example.name}: ${example.engine} produced no non-empty PDF`);
  }
  return { text, pages };
}

function replaceAtomically(target, bytes) {
  const temporary = `${target}.${process.pid}.tmp`;
  try {
    writeFileSync(temporary, bytes);
    renameSync(temporary, target);
  } finally {
    rmSync(temporary, { force: true });
  }
}

// Compilation and outcome validation finish before the first replacement.
// Each corpus file is renamed atomically; the set of renames is not a
// cross-file filesystem transaction if the disk fails during publication.
export function compileCorpus({ corpus = CORPUS, keepPdf = KEEP_PDF, env = process.env } = {}) {
  const stage = mkdtempSync(join(tmpdir(), "librepaper-texlive-"));
  const record = {};
  const outputs = [];
  try {
    for (const example of EXAMPLES) {
      const source = join(corpus, example.name);
      const work = join(stage, example.name);
      mkdirSync(work, { recursive: true });
      cpSync(source, work, {
        recursive: true,
        filter: (from) => {
          const rel = relative(source, from);
          if (!rel) return true;
          const parts = rel.split(sep);
          if (parts.includes("logs") || GENERATED.test(basename(from))) return false;
          if (example.name === "article" && basename(from) === "article.bib") return false;
          return true;
        },
      });
      const result = compile(example, work, keepPdf, env);
      const log = join(work, "main.log");
      outputs.push({ target: join(source, "logs", "texlive.log"), bytes: readFileSync(log) });
      if (keepPdf && !example.negative) outputs.push({ target: join(source, "main.pdf"), bytes: readFileSync(join(work, "main.pdf")) });
      record[example.name] = {
        engine: example.engine,
        pages: result.pages,
        synctex: existsSync(join(work, "main.synctex.gz")),
      };
      console.log(`texlive: ${example.name.padEnd(13)} ${result.pages} page(s) with ${example.engine}`);
    }

    outputs.push({ target: join(corpus, "pages.json"), bytes: Buffer.from(`${JSON.stringify(record, null, 2)}\n`) });
    for (const output of outputs) {
      mkdirSync(dirname(output.target), { recursive: true });
      replaceAtomically(output.target, output.bytes);
    }
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
  console.log(`texlive: page counts written to ${join(corpus, "pages.json")}`);
  return record;
}

const invoked = process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1]);
if (invoked) {
  try {
    compileCorpus();
  } catch (error) {
    console.error(`texlive: ${error.message}`);
    process.exitCode = 1;
  }
}
