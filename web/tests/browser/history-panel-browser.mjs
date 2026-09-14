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
  changed: ["main.md"],
  parent: index ? String(index).padStart(64, "0") : undefined,
}));
const otherFilePoint = {
  sha: "9".padStart(64, "0"), at: "2026-09-11T09:00:00Z", by: "Vincent",
  why: "quiet", label: "", changed: ["references.bib"], parent: points[4].sha,
};

const source = `
import History from ${JSON.stringify(join(root, "web/src/components/reader/History.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
const points = ${JSON.stringify(points)};
const otherFilePoint = ${JSON.stringify(otherFilePoint)};
const events = [];
const component = createClassComponent({ component: History, target: document.body, props: {
  checkpoints: [...points, otherFilePoint], canEdit: true, currentLabel: "",
  onview: (sha) => events.push(["view", sha]),
  onname: (sha, label) => events.push(["name", sha, label]),
} });
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 30)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };
window.historyPanelCheck = async () => {
  await flush();
  // The timeline is the project's, not the open file's: a version that only
  // moved references.bib is still a version of this project.
  check(document.querySelector('li[data-sha="' + otherFilePoint.sha + '"]'),
    'the timeline is project-wide');
  check(document.querySelectorAll('li[data-sha]').length === points.length + 1,
    'every version is a row of its own');
  check(!document.querySelector('.timeline-folded'),
    'no row stands for several versions');
  const namedRow = document.querySelector('li[data-sha="' + points[4].sha + '"]');
  check(namedRow?.textContent.includes('Submitted draft') && !namedRow?.textContent.includes('Vincent') && !namedRow?.textContent.includes('Autosaved') && !namedRow?.textContent.includes('Named version'),
    'a named revision is presented only with its explicit name');
  check(!document.querySelector('[aria-label="Compare selected checkpoint"]'),
    'there is one comparison, so there is no mode to choose');
  check(!document.querySelector('[aria-label="Name this version"]'),
    'unselected revisions do not expose row actions');
  const row = document.querySelector('li[data-sha="' + points[1].sha + '"] .timeline-point');
  row.click(); await flush();
  check(events.some((event) => event[0] === 'view' && event[1] === points[1].sha),
    'selecting a version asks for its comparison');
  component.$set({ viewing: points[1].sha }); await flush();
  check(document.querySelector('[aria-label="Name this version"]') && document.querySelector('[aria-label="Copy the link to this version"]'),
    'the selected revision exposes naming and link actions');
  check(!document.querySelector('[aria-label="Restore this version"]'),
    'restoring belongs to the comparison, where what it replaces is on the screen');
  const currentRow = document.querySelector('.timeline-now .timeline-point');
  const currentBox = currentRow.getBoundingClientRect();
  check(document.elementFromPoint(currentBox.left + 10, currentBox.top + currentBox.height / 2)?.closest('.timeline-point') === currentRow,
    'selected-version details do not intercept timeline controls');
  const filter = document.querySelector('[aria-label="Filter version history"]');
  check([...filter.options].map((option) => option.value).join() === 'all,named',
    'the only filter left is the named one');
  filter.value = 'named'; filter.dispatchEvent(new Event('change', { bubbles: true })); await flush();
  const filtered = [...document.querySelectorAll('[data-sha]')].map((node) => node.dataset.sha);
  check(filtered.length === 3 && filtered[0] === points[4].sha && filtered[1] === points[1].sha && filtered[2] === points[0].sha, 'named filter preserves chronological order across days, and keeps the selected version');
  filter.value = 'all'; filter.dispatchEvent(new Event('change', { bubbles: true })); await flush();
  component.$set({ viewing: '' }); await flush();
  const currentName = document.querySelector('[aria-label="Name the current version"]');
  check(currentName, 'current version has a naming action'); currentName.click(); await flush();
  const input = document.querySelector('[aria-label="Name the current version"]');
  check(input && input.tagName === 'INPUT', 'current naming uses an accessible input');
  input.value = 'Working draft'; input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await flush();
  check(events.some((event) => event[0] === 'name' && event[1] === 'current' && event[2] === 'Working draft'), 'current naming reports the current pseudo-version');
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
  console.log("history panel: a project-wide dated list, one row per version, selection, filtering and naming passed");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
