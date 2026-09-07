#!/usr/bin/env node
// Generate phase-specific native references. This is a developer-side oracle.
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { EXAMPLE, PHASES, fixture } from "./fixture.mjs";
import { inspectPDF, warnings } from "../../results.mjs";
import { treeDigest } from "../../corpus.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "references");
const selected = process.argv.slice(2).filter(x => !x.startsWith("-"));
const phases = selected.length ? selected : PHASES;
if (!selected.length) rmSync(OUT, { recursive: true, force: true });

for (const phase of phases) {
  const tree = fixture(phase), work = join(OUT, phase);
  rmSync(work, { recursive: true, force: true });
  for (const [path, contents] of Object.entries({ ...tree.texts, ...tree.assets })) {
    mkdirSync(dirname(join(work, path)), { recursive: true });
    writeFileSync(join(work, path), typeof contents === "string" ? contents : Buffer.from(contents));
  }
  const run = spawnSync("latexmk", ["-norc", "-xelatex", "-no-shell-escape", "-interaction=nonstopmode", "-halt-on-error", "-file-line-error", "-synctex=1", EXAMPLE.main], {
    cwd: work, encoding: "utf8", timeout: 180000, maxBuffer: 16 * 1024 * 1024,
    env: { ...process.env, openin_any: "r", openout_any: "p", TEXMFVAR: join(work, ".texmf-var"), TEXMFCONFIG: join(work, ".texmf-config") },
  });
  const stem = basename(EXAMPLE.main, ".tex"), pdfPath = join(work, stem + ".pdf");
  const log = existsSync(join(work, stem + ".log")) ? readFileSync(join(work, stem + ".log"), "utf8") : "";
  writeFileSync(join(work, "commands.log"), (run.stdout || "") + (run.stderr || "") + (run.error ? String(run.error) : ""));
  const pdf = inspectPDF(pdfPath);
  const record = { phase, treeSha256: treeDigest(tree), exitCode: run.status, error: run.error?.message || null, ...pdf,
    warnings: warnings(log),
    tools: { xelatex: execFileSync("xelatex", ["--version"], { encoding: "utf8" }).split("\n")[0], biber: execFileSync("biber", ["--version"], { encoding: "utf8" }).split("\n")[0] },
    bbl: existsSync(join(work, stem + ".bbl")) ? stem + ".bbl" : null,
    biberInvoked: /applying rule ['"]biber|Run number \d+ of rule ['"]biber /.test((run.stdout || "") + log) };
  writeFileSync(join(work, "reference.json"), JSON.stringify(record, null, 2) + "\n");
  if (run.status !== 0 || !pdf.pdf || record.warnings.length || !existsSync(join(work, stem + ".bbl"))) process.exitCode = 1;
  console.log(`${phase}: ${record.pdf ? "pdf" : "failed"}, ${record.pages} pages`);
}
writeFileSync(join(OUT, "manifest.json"), JSON.stringify({ engine: execFileSync("xelatex", ["--version"], { encoding: "utf8" }).split("\n")[0], phases }, null, 2) + "\n");
