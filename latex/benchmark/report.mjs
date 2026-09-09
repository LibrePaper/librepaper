#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { HERE, CASES, SOURCES } from "./corpus.mjs";
import { RESULTS, referenceFor, writeJSON } from "./results.mjs";

const name = process.argv[2] || "chromium";
if (!["chromium", "firefox"].includes(name)) throw new Error("Expected chromium or firefox");
const report = JSON.parse(readFileSync(join(RESULTS, `${name}.json`), "utf8"));
const cell = (value) => String(value ?? "—").replaceAll("|", "\\|").replace(/\s+/g, " ");
const n = (value, scale = 1) => value == null ? "—" : (value / scale).toFixed(2);
const lines = [
  "# Public-project LaTeX baseline", "", `Measured: ${report.date}.`, "", report.method, "",
  `Browser: ${report.userAgent}.`, "", `Mirror manifest SHA-256: \`${report.manifestSha256}\`.`, "",
  "These measurements assess LibrePaper's current adapters and local mirror together. A failure does not establish an engine limitation. The native reference uses the installed TeX Live, whose version may differ from the browser distribution. A PDF and matching text still require visual review; text agreement does not validate citation order or layout.", "",
  "## Sources", "",
  ...Object.entries(SOURCES).map(([id, source]) => `- ${id}: [${source.revision.slice(0, 12)}](${source.repository}/tree/${source.revision}). ${source.license}.`), "",
  "## Native references", "",
  "| Case | Engine | Pages | Status |", "| --- | --- | ---: | --- |",
  ...CASES.map(({ id, engine }) => {
    const ref = referenceFor(id);
    return `| ${id} | ${engine} | ${ref?.pages ?? "—"} | ${ref?.status ?? "not run"} |`;
  }), "",
  ...[...new Set(CASES.map(({ id }) => referenceFor(id)?.version).filter(Boolean))].map((version) => `- ${version}`), "",
  "## Browser results", "",
  "Cold includes initialization and first compilation; edit inserts visible text before the document ends. Reload recompiles the original input after page navigation, with the same browser profile. MB means uncompressed response-body bytes served by the local mirror, not compressed network transfer. A fast failure is not a fast preview.", "",
  "| Distribution | Case | Cold status | Pages | Cold MB | Cold s | Edit s | Reload MB | SyncTeX | Text F1 |",
  "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | ---: |",
];
for (const run of report.cases) {
  const cold = run.phases.find((p) => p.phase === "cold");
  const edit = run.phases.find((p) => p.phase === "edit");
  const reload = run.phases.find((p) => p.phase === "reload");
  lines.push(`| ${run.distribution} | ${run.id} | ${cell(cold?.status || run.error)} | ${cold?.pages ?? "—"} | ${n(cold?.bytes, 1e6)} | ${n(cold?.seconds)} | ${n(edit?.seconds)} | ${n(reload?.bytes, 1e6)} | ${cold?.synctex ? "yes" : "no"} | ${n(cold?.textAgreement)} |`);
}
lines.push("", "## Failures and review notes", "");
for (const run of report.cases) {
  const cold = run.phases.find((p) => p.phase === "cold");
  const logPath = join(RESULTS, name, run.distribution, run.id, "cold.log");
  let fallback;
  if (cold?.status === "failed") {
    const log = readFileSync(logPath, "utf8");
    fallback = log.split("\n").find((line) => /^Error:|fatal:/i.test(line)) || "No PDF produced; inspect the saved compiler log.";
  }
  const reason = run.error || cold?.error || cold?.errors?.[0] || cold?.warnings?.[0] || fallback;
  if (reason) lines.push(`- ${run.distribution}/${run.id}: ${cell(reason)}`);
  if (run.selectedEngine && run.selectedEngine !== run.requestedEngine) lines.push(`- ${run.distribution}/${run.id}: requested ${run.requestedEngine}, selected ${run.selectedEngine}.`);
}
lines.push("", "## Remaining evaluation", "",
  "- Integrate and pin the Siglum and WasmTex candidates before comparing them with this baseline; neither has been benchmarked here.",
  ...(!referenceFor("thesis") || referenceFor("thesis").status !== "reference-ready" ? ["- Complete the thesis native reference before interpreting its browser output as a compatibility result."] : []),
  "- Measure repeated trials, realistic network conditions, worker/WASM memory, and Safari on actual hardware. This local run supplies none of those numbers.",
  "- Inspect PDF layout, bibliography content and ordering, and source/PDF navigation. SyncTeX here means a valid gzip with a SyncTeX header, not verified navigation accuracy.", "");
const path = join(RESULTS, `${name}.md`);
writeFileSync(path, lines.join("\n"));
console.log(path);
if (process.argv.includes("--snapshot")) {
  writeFileSync(join(HERE, "BASELINE.md"), lines.join("\n"));
  const native = CASES.flatMap(({ id }) => {
    const ref = referenceFor(id);
    if (!ref) return [];
    const { text, ...summary } = ref;
    return [summary];
  });
  writeJSON(join(HERE, "baseline.json"), { ...report, sources: SOURCES, native });
  console.log("Snapshot: latex/benchmark/BASELINE.md and baseline.json");
}
