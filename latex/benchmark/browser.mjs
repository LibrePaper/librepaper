#!/usr/bin/env node
// Measure the application adapters without changing their engine selection,
// package mirror, bibliography behavior, or production visibility flags.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { gunzipSync } from "node:zlib";
import { browser, until } from "../../web/tools/browser-driver.mjs";
import { ROOT, selectedCases, treeOf, treeDigest, digest } from "./corpus.mjs";
import { RESULTS, inspectPDF, referenceFor, textAgreement, warnings, writeJSON } from "./results.mjs";

const argv = process.argv.slice(2);
const option = (name, fallback) => argv.includes(name) ? argv[argv.indexOf(name) + 1] : fallback;
const browserName = option("--browser", "chromium");
if (!["chromium", "firefox"].includes(browserName)) throw new Error("--browser must be chromium or firefox");
const manifestPath = join(ROOT, "latex/mirror/manifest.json");
if (!existsSync(manifestPath)) throw new Error("Build latex/mirror before benchmarking");
const manifestBytes = readFileSync(manifestPath);
const manifest = JSON.parse(manifestBytes);
const names = option("--only", "swiftlatex-pdftex,busytex,texlyre-busytex").split(",");
for (const name of names) if (!manifest.distributions[name]) throw new Error(`Unknown distribution: ${name}`);
const port = 8400 + Math.floor(Math.random() * 200);
const base = `http://localhost:${port}`;
const server = spawn(process.execPath, [join(ROOT, "latex/tools/serve.mjs"), "--port", String(port)], { stdio: ["ignore", "pipe", "pipe"] });
let serverError, serverLog = "";
server.on("error", (e) => { serverError = e; });
server.stdout.on("data", (b) => { serverLog += b; });
server.stderr.on("data", (b) => { serverLog += b; });

const report = {
  date: new Date().toISOString(), browser: browserName, manifestSha256: digest(manifestBytes),
  method: "Fresh browser profile per case/distribution; cold includes app boot and choose; warm is a visible body edit; reload returns to original source with persistent caches. Local uncompressed mirror, no network/CPU throttling. Memory not measured. Text agreement is a word-multiset F1, not visual or bibliography-order equivalence.",
  cases: [],
};
const output = join(RESULTS, `${browserName}.json`);
let driver;
const evaluate = async (source) => JSON.parse(await driver.evaluate(`(async () => JSON.stringify(await (async () => { ${source} })()))()`));
const resetBytes = () => fetch(`${base}/__reset`);
const servedBytes = async () => (await (await fetch(`${base}/__bytes`)).json()).total;

async function startPage(name) {
  // Page.navigate can resolve before the old document's execution context is
  // gone. Prevent its readiness flag from satisfying the next page's wait.
  await evaluate("globalThis.latexReady = false; return true;");
  await driver.navigate(`${base}/`);
  await until("LaTeX harness", () => evaluate("return Boolean(globalThis.latexReady);"));
  await evaluate(`
    globalThis.pendingBenchmark = null;
    latex.at(${JSON.stringify(`${base}/mirror/`)});
    latex.choose(${JSON.stringify(name)}).then(
      () => globalThis.pendingBenchmark = { ready: true },
      (e) => globalThis.pendingBenchmark = { error: String(e) }
    ); return true;
  `);
  await until("compiler initialization", () => evaluate("return globalThis.pendingBenchmark !== null;"), 120000);
  const ready = await evaluate("return globalThis.pendingBenchmark;");
  if (ready.error) throw new Error(ready.error);
}

async function compile(tree) {
  await evaluate(`
    globalThis.pendingBenchmark = null;
    latex.compile(${JSON.stringify(tree)}).then((result) => {
      const encode = (bytes) => {
        if (!bytes) return null;
        const data = new Uint8Array(bytes);
        let binary = '';
        for (let i = 0; i < data.length; i += 8192) binary += String.fromCharCode(...data.subarray(i, i + 8192));
        return btoa(binary);
      };
      globalThis.pendingBenchmark = { pdf: encode(result.pdf), synctex: encode(result.synctex), log: result.log || '', diagnostics: result.diagnostics || [] };
    }, (e) => globalThis.pendingBenchmark = { error: String(e), log: '', diagnostics: [] });
    return true;
  `);
  await until("compile result", () => evaluate("return globalThis.pendingBenchmark !== null;"), 120000);
  return evaluate("return globalThis.pendingBenchmark;");
}

try {
  await until("mirror server", async () => {
    if (serverError) throw serverError;
    if (server.exitCode !== null) throw new Error(serverLog);
    return (await fetch(`${base}/__bytes`)).ok;
  }, 10000);
  for (const name of names) {
    for (const example of selectedCases(argv)) {
      const work = join(RESULTS, browserName, name, example.id);
      rmSync(work, { recursive: true, force: true });
      mkdirSync(work, { recursive: true });
      const tree = treeOf(example);
      const record = { id: example.id, distribution: name, release: manifest.distributions[name].release, requestedEngine: example.engine, treeSha256: treeDigest(tree), phases: [] };
      const reference = referenceFor(example.id);
      const matchedReference = reference?.treeSha256 === record.treeSha256 && reference?.status === "reference-ready";
      record.referenceReady = matchedReference;
      try {
        driver = await browser(browserName, join(work, "profile"), 9400 + Math.floor(Math.random() * 200));
        report.userAgent ||= await evaluate("return navigator.userAgent;");
        for (const phase of ["cold", "edit", "reload"]) {
          await resetBytes();
          const start = performance.now();
          if (phase !== "edit") await startPage(name);
          if (phase === "cold") {
            record.selectedEngine = name === "swiftlatex-pdftex" ? "pdflatex" : name === "swiftlatex-xetex" ? "xelatex" : await evaluate(`
              const { engineFor } = await import('/src/lib/latex/busytex.js');
              return { pdftex: 'pdflatex', xetex: 'xelatex', luatex: 'lualatex' }[engineFor(${JSON.stringify(tree)})];
            `);
          }
          const input = structuredClone(tree);
          if (phase === "edit") {
            const main = input.texts[input.main];
            const at = main.lastIndexOf("\\end{document}");
            if (at < 0) throw new Error("No document end for body-edit benchmark");
            input.texts[input.main] = main.slice(0, at) + "\n\\par Benchmark edit.\n" + main.slice(at);
          }
          const result = await compile(input);
          const seconds = (performance.now() - start) / 1000;
          const bytes = await servedBytes();
          writeFileSync(join(work, `${phase}.log`), result.log || result.error || "");
          const pdfPath = join(work, `${phase}.pdf`);
          if (result.pdf) writeFileSync(pdfPath, Buffer.from(result.pdf, "base64"));
          const pdf = inspectPDF(pdfPath);
          let synctex = false;
          if (result.synctex) {
            const data = Buffer.from(result.synctex, "base64");
            writeFileSync(join(work, `${phase}.synctex.gz`), data);
            try { synctex = gunzipSync(data).toString().startsWith("SyncTeX Version:"); } catch { /* invalid output is recorded as absent */ }
          }
          const errors = result.diagnostics.filter((d) => d.severity === "error").map((d) => d.message);
          const reviewWarnings = warnings(result.log || "");
          const editVisible = phase === "edit" && pdf.pdf ? /Benchmark\s+edit\./.test(pdf.text) : null;
          const agreement = phase !== "edit" && pdf.pdf && matchedReference ? textAgreement(reference.text, pdf.text) : null;
          const differs = matchedReference && phase !== "edit" && (pdf.pages !== reference.pages || agreement < 0.99);
          const measurement = {
            phase, seconds, bytes, pdf: pdf.pdf, pages: pdf.pages, synctex,
            error: result.error || pdf.pdfError || null, errors, warnings: reviewWarnings,
            editVisible,
            textAgreement: agreement,
            referencePages: matchedReference ? reference.pages : null,
            status: !pdf.pdf || result.error || errors.length ? "failed" : editVisible === false || reviewWarnings.length || differs || !matchedReference || record.selectedEngine !== example.engine ? "needs-review" : "pdf-produced",
          };
          record.phases.push(measurement);
          console.log(`${browserName}/${name}/${example.id}/${phase}: ${measurement.status}, ${pdf.pages} pages, ${seconds.toFixed(1)}s, ${(bytes / 1e6).toFixed(1)} MB`);
          // A failure still gets an edit and reload attempt: this detects
          // package state lost between passes or hidden by a previous run.
        }
      } catch (error) {
        record.error = String(error);
        console.log(`${browserName}/${name}/${example.id}: ${record.error}`);
      } finally {
        await driver?.close();
        driver = null;
        rmSync(join(work, "profile"), { recursive: true, force: true });
        report.cases.push(record);
        writeJSON(output, report);
      }
    }
  }
} finally {
  await driver?.close();
  server.kill();
}
console.log(`Report: ${output}`);
// Compiler incompatibilities are measurements. Harness failures fail the run.
if (report.cases.some((c) => c.error)) process.exitCode = 1;
