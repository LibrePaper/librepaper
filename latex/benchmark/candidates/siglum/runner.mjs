#!/usr/bin/env node
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { browser, until } from "../../../../web/tools/browser-driver.mjs";
import { CASES, selectedCases, treeOf, treeDigest } from "../../corpus.mjs";
import { inspectPDF, referenceFor, textAgreement, warnings } from "../../results.mjs";

const HERE = new URL(".", import.meta.url).pathname;
const RESULTS = join(HERE, "results");
mkdirSync(RESULTS, { recursive: true });
const base = "http://127.0.0.1:8701";
const server = spawn(process.execPath, [join(HERE, "server.mjs"), "--port", "8701"], { stdio: "ignore" });
const selected = selectedCases(process.argv.slice(2));
const report = {
  date: new Date().toISOString(), revision: "00db6721bc7c50dd651761da1c07a1ef3285dbcf",
  browser: "chromium", serverPort: 8701, devtoolsPort: 9701,
  method: "SiglumCompiler v0.1.4 source at pinned commit; local CDN assets; enableCtan=false; fresh profile per case, then edit and reload in same profile.",
  cases: [],
};
const bytes = async () => (await (await fetch(`${base}/__bytes`)).json()).total;
const reset = async () => { await fetch(`${base}/__reset`); };
const evaluate = async (driver, source) => driver.evaluate(`(async () => (${source}))()`);
const inputFor = (tree) => {
  const files = {};
  for (const [name, value] of Object.entries(tree.texts)) if (name !== tree.main) files[name] = value;
  for (const [name, value] of Object.entries(tree.assets)) files[name] = value;
  return { source: tree.texts[tree.main], files };
};
const pdfFrom = (value, path) => {
  if (!value) return { pdf: false, pages: 0, text: "" };
  const data = Buffer.from(value, "base64");
  writeFileSync(path, data);
  return inspectPDF(path);
};
const compile = async (driver, input, engine) => evaluate(driver, `globalThis.siglumCompile(${JSON.stringify({ ...input, engine })})`);
const waitReady = async (driver) => {
  await until("Siglum page", () => evaluate(driver, "Boolean(globalThis.siglumReady)"), 120000);
  const error = await evaluate(driver, "globalThis.siglumInitError || null");
  if (error) throw new Error(error);
};

try {
  await until("Siglum server", async () => (await fetch(`${base}/__bytes`)).ok, 10000);
  for (const example of selected) {
    const tree = treeOf(example);
    const input = inputFor(tree);
    const reference = referenceFor(example.id);
    const record = { id: example.id, requestedEngine: example.engine, treeSha256: treeDigest(tree), referenceReady: reference?.treeSha256 === treeDigest(tree) && reference?.status === "reference-ready", phases: [] };
    const work = join(RESULTS, example.id);
    rmSync(work, { recursive: true, force: true });
    mkdirSync(work, { recursive: true });
    let driver;
    try {
      driver = await browser("chromium", join(work, "profile"), 9701);
      await driver.navigate(`${base}/`);
      await waitReady(driver);
      for (const phase of ["cold", "edit", "reload"]) {
        // Initialization is intentionally complete before this reset: phase
        // bytes/time describe compilation and lazy bundles, not startup.
        await reset();
        const started = performance.now();
        if (phase === "reload") {
          await driver.navigate(`${base}/`);
          await waitReady(driver);
        }
        const phaseInput = structuredClone(input);
        if (phase === "edit") {
          const at = phaseInput.source.lastIndexOf("\\end{document}");
          phaseInput.source = phaseInput.source.slice(0, at) + "\n\\par Benchmark edit.\n" + phaseInput.source.slice(at);
        }
        const result = await compile(driver, phaseInput, example.engine);
        const seconds = (performance.now() - started) / 1000;
        const served = await bytes();
        const pdf = pdfFrom(result.pdf, join(work, `${phase}.pdf`));
        const errors = (result.log || "").split("\n").filter((line) => /! |Error:|Fatal|Emergency stop|undefined control sequence/i.test(line)).slice(0, 20);
        const reviewWarnings = warnings(result.log || "");
        const agreement = phase !== "edit" && pdf.pdf && record.referenceReady ? textAgreement(reference.text, pdf.text) : null;
        const editVisible = phase === "edit" && pdf.pdf ? /Benchmark\s+edit\./.test(pdf.text) : null;
        const status = !result.success || !pdf.pdf ? "failed" : editVisible === false || reviewWarnings.length || (record.referenceReady && (pdf.pages !== reference.pages || agreement < 0.99)) || (example.engine === "lualatex") ? "needs-review" : "pdf-produced";
        const measurement = { phase, seconds, bytes: served, success: result.success, pdf: pdf.pdf, pages: pdf.pages, syncTex: result.syncTexLength > 0, syncTexLength: result.syncTexLength, error: result.error || null, exitCode: result.exitCode, errors, warnings: reviewWarnings, editVisible, textAgreement: agreement, referencePages: record.referenceReady ? reference.pages : null, stats: result.stats, logTail: (result.log || "").split("\n").slice(-20), status };
        record.phases.push(measurement);
        writeFileSync(join(work, `${phase}.log`), result.log || result.error || "");
        console.log(`${example.id}/${phase}: ${status}, ${pdf.pages} pages, ${seconds.toFixed(1)}s, ${(served / 1e6).toFixed(1)} MB`);
      }
    } catch (error) {
      record.error = String(error);
      console.log(`${example.id}: ${record.error}`);
    } finally {
      await driver?.close();
      rmSync(join(work, "profile"), { recursive: true, force: true });
    }
    report.cases.push(record);
    writeFileSync(join(RESULTS, "report.json"), JSON.stringify(report, null, 2) + "\n");
  }
} finally {
  server.kill();
}
