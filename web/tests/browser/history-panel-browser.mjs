// Browser regression check for the real history panel timeline interactions.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-history-panel-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");
const points = Array.from({ length: 5 }, (_, index) => ({
  sha: String(index + 1).padStart(64, "0"),
  at: index === 0 ? "2026-09-09T09:00:00Z" : `2026-09-10T09:0${index}:00Z`,
  by: "Vincent",
  why: "quiet",
  label: index === 4 ? "Submitted draft" : index === 0 ? "Earlier draft" : "",
  parent: index ? String(index).padStart(64, "0") : undefined,
}));

const source = `
import History from ${JSON.stringify(join(root, "web/src/components/reader/History.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
const points = ${JSON.stringify(points)};
const events = [];
const component = createClassComponent({ component: History, target: document.body, props: {
  checkpoints: points, canEdit: true, changes: [{ position: 0, old: "old", insert: "new", currentBefore: "", currentAfter: "text", path: "main.md" }], currentLabel: "",
  onview: (sha) => events.push(["view", sha]), onstep: () => events.push(["step"]),
  onname: (sha, label) => events.push(["name", sha, label]),
} });
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 30)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };
window.historyPanelCheck = async () => {
  await flush();
  check(document.querySelector('.timeline-folded'), 'routine checkpoints are grouped');
  const session = document.querySelector('.timeline-folded');
  check(session.textContent.includes('3 versions'), 'group displays a readable version count');
  session.click(); await flush();
  check(events.some((event) => event[0] === 'view' && event[1] === points[3].sha), 'session label previews newest version');
  document.querySelector('[aria-label="Expand editing session"]').click(); await flush();
  check(document.querySelectorAll('.timeline-point').length >= 6, 'expansion reveals every version');
  component.$set({ viewing: points[1].sha }); await flush();
  check(document.querySelector('[aria-label="Collapse editing session"]'), 'selecting a hidden version reveals its session');
  document.querySelector('[aria-label="Collapse editing session"]').click(); await flush();
  check(document.querySelector('[aria-label="Expand editing session"]') && !document.querySelector('[aria-label="Collapse editing session"]'), 'session can collapse again without being reopened');
  document.querySelector('.history-switch input').click(); await flush();
  const filtered = [...document.querySelectorAll('[data-sha]')].map((node) => node.dataset.sha);
  check(filtered.length === 3 && filtered[0] === points[4].sha && filtered[1] === points[1].sha && filtered[2] === points[0].sha, 'named filter preserves chronological order across days');
  document.querySelector('.history-switch input').click(); await flush();
  const currentName = document.querySelector('[aria-label="Name the current version"]');
  check(currentName, 'current version has a naming action'); currentName.click(); await flush();
  const input = document.querySelector('[aria-label="Name the current version"]');
  check(input && input.tagName === 'INPUT', 'current naming uses an accessible input');
  input.value = 'Working draft'; input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await flush();
  check(events.some((event) => event[0] === 'name' && event[1] === 'current' && event[2] === 'Working draft'), 'current naming reports the current pseudo-version');
  window.dispatchEvent(new KeyboardEvent('keydown', { key: ']' })); await flush();
  check(events.some((event) => event[0] === 'step'), 'keyboard change navigation remains active');
  component.$destroy();
  return true;
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "history-panel-check.js" } } });
  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); return; }
    response.setHeader("Content-Type", "text/html"); response.end('<body><script type="module" src="/history-panel-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 22000 + Math.floor(Math.random() * 1000));
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("history panel component", () => tab.evaluate("Boolean(window.historyPanelCheck)"));
  assert.equal(await tab.evaluate("window.historyPanelCheck()"), true);
  console.log("history panel: grouping, expansion, filtering, naming, and keyboard controls passed");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
