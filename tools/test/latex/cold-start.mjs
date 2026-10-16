// Where does a cold LaTeX compile spend its time? A measuring tool, not a
// test: it is in no suite and never fails on slowness. It starts the mirror
// server (serve.mjs), then for each session opens the harness page in a real
// browser on one shared profile and compiles docs/examples/tutorial-latex
// through the real controller (web/src/lib/latex.js). Each session compiles
// cold, then warm after a one-word edit. Session 0 is a first visit; later
// sessions relaunch on the same profile, so they see the working set the
// earlier ones left behind. The bundleStats counters are cumulative per engine.
//
//     node tools/test/latex/cold-start.mjs [--engine pdflatex|xelatex] [--sessions 3] [--browser chromium|firefox] [--mirror DIR]
//
// Exits non-zero only on errors: a missing mirror, a compile that fails, or a
// browser that will not start.

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, pause, removeTemporary, until } from "../../../web/tests/helpers/browser-driver.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = dirname(dirname(dirname(HERE)));
const PROJECT = join(ROOT, "docs", "examples", "tutorial-latex");
const MAIN = "librepaper.tex";
const PORT = 8814;
const DEBUG_PORT = 9814;
const BASE = `http://127.0.0.1:${PORT}`;

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const at = argv.indexOf(name);
  return at >= 0 && argv[at + 1] !== undefined ? argv[at + 1] : fallback;
};
const ENGINE = flag("--engine", "pdflatex");
const BROWSER = flag("--browser", "chromium");
const SESSIONS = Number(flag("--sessions", "3"));
const MIRROR = resolve(flag("--mirror", join(ROOT, "..", "wasm-latex", "mirror")));

const TEXT_EXT = /\.(tex|bib|sty|cls|bst)$/;
const ASSET_EXT = /\.(png|pdf|jpe?g|otf|ttf)$/;

function filesIn(directory, prefix = "") {
  return readdirSync(directory, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => (entry.isDirectory()
      ? filesIn(join(directory, entry.name), `${prefix}${entry.name}/`)
      : [`${prefix}${entry.name}`]));
}

function readTree(directory) {
  const texts = {};
  const assets = {};
  for (const path of filesIn(directory)) {
    if (TEXT_EXT.test(path)) texts[path] = readFileSync(join(directory, path), "utf8");
    else if (ASSET_EXT.test(path)) assets[path] = [...readFileSync(join(directory, path))];
  }
  return { main: MAIN, texts, assets };
}

// The warm compile: one word added to the main file, so the engines are
// reused and only the document changes.
function warmEdit(tree) {
  const before = tree.texts[MAIN];
  const after = before.replace("\\begin{document}", "\\begin{document}\nWarm");
  if (after === before) throw new Error(`${MAIN} has no \\begin{document} to edit`);
  return { ...tree, texts: { ...tree.texts, [MAIN]: after } };
}

function releaseId() {
  if (!existsSync(MIRROR)) throw new Error(`no LaTeX mirror at ${MIRROR}; build wasm-latex next to this checkout or pass --mirror DIR`);
  const ids = readdirSync(MIRROR, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && /^[a-f0-9]{64}$/.test(entry.name))
    .map((entry) => entry.name);
  if (ids.length !== 1) throw new Error(`${MIRROR} must hold exactly one release directory; found ${ids.length}`);
  return ids[0];
}

// Runs in the page. Returns a summary, never the PDF bytes.
const COMPILE = `async (releaseUrl, engine, tree) => {
  const latex = await import("/src/lib/latex.js");
  latex.at(releaseUrl);
  latex.configure({ project: "cold-start", settings: { engine } });
  const assets = {};
  for (const path of Object.keys(tree.assets)) assets[path] = new Uint8Array(tree.assets[path]);
  const started = performance.now();
  const result = await latex.compile({ ...tree, assets }, { manual: true });
  const ms = performance.now() - started;
  return { ok: result.ok, status: result.status, ms, timings: result.timings ?? null, log: String(result.log || "").slice(-2000) };
}`;

// CDP evaluate has a 60 s cap, so the job runs in the page and is polled.
let jobSerial = 0;
async function runJob(driver, expression, timeoutMs = 300000) {
  const slot = `__coldStartJob${++jobSerial}`;
  await driver.evaluate(
    `globalThis.${slot} = { done: false }; (${expression}).then(` +
      `(r) => { globalThis.${slot} = { done: true, result: r }; }, ` +
      `(e) => { globalThis.${slot} = { done: true, error: String(e && e.message ? e.message : e) }; });true`,
  );
  await until(slot, () => driver.evaluate(`globalThis.${slot}.done`), timeoutMs);
  const job = await driver.evaluate(`globalThis.${slot}`);
  if (job.error) throw new Error(job.error);
  return job.result;
}

const ms = (value) => (typeof value === "number" ? `${Math.round(value)}` : "-");

function printStartup(startup) {
  for (const [kind, s] of Object.entries(startup || {})) {
    const v = s ?? {};
    console.log(`    startup ${kind.padEnd(8)} assets ${ms(v.assetsMs)} start ${ms(v.startMs)} format ${ms(v.formatMs)} index ${ms(v.indexMs)} workingSet ${ms(v.workingSetMs)} (${v.workingSetFiles ?? 0} files)`);
  }
}

function printPasses(passes) {
  for (const pass of passes || []) {
    const b = pass.bundleStats ?? {};
    console.log(`    pass    ${String(pass.kind).padEnd(8)} texRun ${ms(pass.phaseTimings?.texRun)} indexParsed ${b.indexParsed} indexParse ${ms(b.indexParseMs)} memo ${b.memoHits ?? "-"}/${b.memoMisses ?? "-"} fetches ${b.bundleFetches ?? "-"} in ${ms(b.bundleFetchMs)} hash ${ms(b.hashMs)}`);
  }
}

async function runSession(index, releaseUrl, profile, cold, warm) {
  const driver = await browser(BROWSER, profile, DEBUG_PORT);
  try {
    await driver.navigate(`${BASE}/`);
    await until("harness page", () => driver.evaluate("document.readyState === 'complete'"), 20000);
    const compile = (tree) => runJob(driver, `(${COMPILE})(${JSON.stringify(releaseUrl)}, ${JSON.stringify(ENGINE)}, ${JSON.stringify(tree)})`);
    const first = await compile(cold);
    if (!first.ok) throw new Error(`session ${index}: cold compile failed (${first.status})\n${first.log}`);
    const second = await compile(warm);
    if (!second.ok) throw new Error(`session ${index}: warm compile failed (${second.status})\n${second.log}`);
    const label = index === 0 ? "first visit" : "reload";
    console.log(`session ${index} (${label}): cold ${ms(first.ms)} ms, warm ${ms(second.ms)} ms, ratio ${(first.ms / second.ms).toFixed(1)}x`);
    console.log("  cold:");
    printStartup(first.timings?.startup);
    printPasses(first.timings?.passes);
    console.log("  warm:");
    printPasses(second.timings?.passes);
  } finally {
    await driver.close();
  }
}

async function main() {
  if (!["pdflatex", "xelatex"].includes(ENGINE)) throw new Error(`--engine must be pdflatex or xelatex; got ${ENGINE}`);
  if (!["chromium", "firefox"].includes(BROWSER)) throw new Error(`--browser must be chromium or firefox; got ${BROWSER}`);
  if (!Number.isInteger(SESSIONS) || SESSIONS < 1) throw new Error(`--sessions must be a positive integer; got ${SESSIONS}`);
  const releaseUrl = `${BASE}/mirror/${releaseId()}/`;
  const cold = readTree(PROJECT);
  const warm = warmEdit(cold);
  const scratch = mkdtempSync(join(tmpdir(), "librepaper-cold-start-"));
  const server = spawn(process.execPath, [join(HERE, "serve.mjs"), "--port", String(PORT), "--mirror", MIRROR], {
    stdio: ["ignore", "pipe", "pipe"],
    cwd: ROOT,
  });
  let serverLog = "";
  server.stdout.on("data", (b) => { serverLog += b; });
  server.stderr.on("data", (b) => { serverLog += b; });
  try {
    await until("mirror server", async () => (await fetch(`${releaseUrl}release.json`)).ok, 20000);
    console.log(`cold-start: ${BROWSER}, ${ENGINE}, ${SESSIONS} session(s), ${Object.keys(cold.texts).length} text file(s), ${Object.keys(cold.assets).length} asset(s)`);
    for (let index = 0; index < SESSIONS; index++) {
      await runSession(index, releaseUrl, join(scratch, BROWSER), cold, warm);
    }
  } catch (error) {
    console.error(serverLog);
    throw error;
  } finally {
    server.kill();
    await pause(200);
    removeTemporary(scratch);
  }
}

main().catch((error) => {
  console.error(`cold-start: ${error.message}`);
  process.exitCode = 1;
});
