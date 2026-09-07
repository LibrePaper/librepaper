#!/usr/bin/env node
// Native TeX is a developer-side reference only; no application dependency.
import { spawnSync, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { CASES, selectedCases, treeOf, treeDigest } from "./corpus.mjs";
import { RESULTS, inspectPDF, warnings, writeJSON } from "./results.mjs";

for (const example of selectedCases(process.argv.slice(2))) {
  const tree = treeOf(example);
  const work = join(RESULTS, "native", example.id);
  rmSync(work, { recursive: true, force: true });
  mkdirSync(work, { recursive: true });
  for (const [path, contents] of Object.entries({ ...tree.texts, ...tree.assets })) {
    mkdirSync(dirname(join(work, path)), { recursive: true });
    writeFileSync(join(work, path), typeof contents === "string" ? contents : Buffer.from(contents));
  }
  const start = performance.now();
  const engineFlag = { pdflatex: "-pdf", xelatex: "-xelatex", lualatex: "-lualatex" }[example.engine];
  const run = spawnSync("latexmk", ["-norc", engineFlag, "-no-shell-escape", "-interaction=nonstopmode", "-halt-on-error", "-file-line-error", "-synctex=1", example.main], {
    cwd: work, encoding: "utf8", timeout: 180000, maxBuffer: 16 * 1024 * 1024,
    // LuaTeX's font loader opens absolute filenames returned by kpathsea.
    // openin_any=p prevents those legitimate reads; use the restricted read
    // policy while retaining paranoid writes and disabled shell escape.
    env: { ...process.env, openin_any: "r", openout_any: "p", TEXMFVAR: join(work, ".texmf-var"), TEXMFCONFIG: join(work, ".texmf-config") },
  });
  const seconds = (performance.now() - start) / 1000;
  const stem = basename(example.main, ".tex");
  const log = existsSync(join(work, `${stem}.log`)) ? readFileSync(join(work, `${stem}.log`), "utf8") : "";
  writeFileSync(join(work, "commands.log"), (run.stdout || "") + (run.stderr || "") + (run.error ? String(run.error) : ""));
  const pdf = inspectPDF(join(work, `${stem}.pdf`));
  const result = {
    id: example.id, engine: example.engine, treeSha256: treeDigest(tree),
    version: execFileSync(example.engine, ["--version"], { encoding: "utf8" }).split("\n")[0],
    exitCode: run.status, error: run.error?.message || null, seconds,
    ...pdf, warnings: warnings(log), synctex: existsSync(join(work, `${stem}.synctex.gz`)),
    biberRan: /applying rule 'biber/.test(run.stdout || ""),
    status: run.status === 0 && pdf.pdf && !warnings(log).length ? "reference-ready" : "reference-needs-review",
  };
  writeJSON(join(work, "result.json"), result);
  console.log(`${example.id}: ${result.status}, ${pdf.pages} pages, ${seconds.toFixed(1)}s${result.error ? ` (${result.error})` : ""}`);
}
const all = CASES.flatMap(({ id }) => {
  const path = join(RESULTS, "native", id, "result.json");
  if (!existsSync(path)) return [];
  const { text, ...result } = JSON.parse(readFileSync(path, "utf8"));
  return [result];
});
writeJSON(join(RESULTS, "native.json"), { date: new Date().toISOString(), cases: all });
