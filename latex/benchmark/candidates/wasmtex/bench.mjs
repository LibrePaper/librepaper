import { spawn } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { browser, until } from '../../../../web/tools/browser-driver.mjs';
import { CASES, treeOf } from '../../corpus.mjs';
import { inspectPDF } from '../../results.mjs';

const root = new URL('./', import.meta.url).pathname;
const sourceRoot = join(root, 'source');
const base = 'http://127.0.0.1:8702';
const profileRoot = new URL('./profiles/', import.meta.url).pathname;
const resultsRoot = new URL('./results/', import.meta.url).pathname;
rmSync(profileRoot, { recursive: true, force: true });
mkdirSync(profileRoot, { recursive: true });
mkdirSync(resultsRoot, { recursive: true });
const config = new URL('./harness/vite-bench.config.mjs', import.meta.url).pathname;
const server = spawn(join(sourceRoot, 'node_modules/.bin/vite'), ['--config', config, '--host', '127.0.0.1', '--port', '8702'], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
let serverLog = '';
server.stdout.on('data', (b) => { serverLog += b; });
server.stderr.on('data', (b) => { serverLog += b; });
let driver;
const evaluate = async (source) => driver.evaluate(`(async () => (${source}))()`);
const run = async (payload) => {
  const encoded = JSON.stringify(payload);
  return evaluate(`globalThis.wasmtexRun(${encoded})`);
};
const save = (caseId, phase, result) => {
  const dir = join(resultsRoot, caseId);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, `${phase}.json`), JSON.stringify({ ...result, pdf: result.pdf ? `<${Buffer.from(result.pdf, 'base64').length} bytes>` : null }, null, 2));
  if (result.pdf) writeFileSync(join(dir, `${phase}.pdf`), Buffer.from(result.pdf, 'base64'));
  if (result.synctex) writeFileSync(join(dir, `${phase}.synctex`), Buffer.from(result.synctex, 'base64'));
  const pdf = result.pdf ? inspectPDF(join(dir, `${phase}.pdf`)) : { pdf: false, pages: 0 };
  return { phase, success: result.success, pdf: pdf.pdf, pages: pdf.pages, pdfBytes: result.pdf ? Buffer.from(result.pdf, 'base64').length : 0,
    elapsedMs: Math.round(result.elapsedMs || 0), errors: result.errors?.map((e) => e.message || String(e)) || [],
    logTail: (result.log || '').slice(-1200), synctexBytes: result.synctex ? Buffer.from(result.synctex, 'base64').length : 0,
    phaseTimings: result.phaseTimings || null, telemetry: result.telemetry || null, error: result.error || null };
};

try {
  await until('Vite', async () => (await fetch(`${base}/harness/index.html`)).ok, 20000);
  driver = await browser('chromium', join(profileRoot, 'chromium'), 9702);
  await driver.navigate(`${base}/harness/index.html`);
  await until('WasmTex harness', () => driver.evaluate('Boolean(globalThis.wasmtexReady)'), 20000);
  const wanted = process.argv.slice(2).length ? process.argv.slice(2) : ['multifile', 'packages', 'biber-related', 'unicode-fonts'];
  const assetBaseUrl = 'http://127.0.0.1:8702/';
  const texliveUrl = 'http://127.0.0.1:8702/2025/';
  const report = { revision: '44c5861fcdf729838205b00b96ac9509bc7fb677', assetBaseUrl, texliveUrl, cases: [] };
  for (const id of wanted) {
    const example = CASES.find((c) => c.id === id);
    if (!example) throw new Error(`unknown case ${id}`);
    const tree = treeOf(example);
    const files = { ...tree.texts, ...tree.assets };
    const options = { assetBaseUrl, texliveUrl, texliveVersion: '2025', mainFile: tree.main, files, persistentCache: true };
    const cold = save(id, 'cold', await run({ options, files }));
    // A fresh compiler on the same page exercises persistent cache plus a visible edit.
    const edited = { ...files, [tree.main]: files[tree.main].replace('\\end{document}', '\\par WasmTex benchmark edit.\\end{document}') };
    const edit = save(id, 'edit', await run({ options: { ...options, files: edited }, files: edited }));
    // Navigation recreates the compiler and keeps this profile's IndexedDB cache.
    await driver.navigate(`${base}/harness/index.html`);
    await until('WasmTex harness reload', () => driver.evaluate('Boolean(globalThis.wasmtexReady)'), 20000);
    const reload = save(id, 'reload', await run({ options, files }));
    report.cases.push({ id, cold, edit, reload });
    console.log(JSON.stringify({ id, cold: { success: cold.success, pages: cold.pages, ms: cold.elapsedMs, pdfBytes: cold.pdfBytes }, edit: { success: edit.success, pages: edit.pages, ms: edit.elapsedMs }, reload: { success: reload.success, pages: reload.pages, ms: reload.elapsedMs, synctexBytes: reload.synctexBytes } }));
  }
  writeFileSync(join(resultsRoot, 'report.json'), JSON.stringify(report, null, 2));
} finally {
  await driver?.close();
  server.kill();
  if (server.exitCode !== null && server.exitCode !== 0) console.error(serverLog);
}
