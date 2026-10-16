// The Changes panel, in a real browser.
//
// What is pinned here is what a reviewer can actually do: see the words a
// change takes out beside the ones it puts in, filter by author, file and run,
// walk the queue from the keyboard, answer the change they are looking at, and
// answer for several at once. A reader without the right to review sees the
// rows and no verbs.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-changes-"));
const entry = join(temporary, "entry.js");
writeFileSync(entry, `
import ${JSON.stringify(join(root, "src/styles/app.css"))};
import { mount, unmount, tick } from ${JSON.stringify(join(root, "node_modules/svelte/src/index-client.js"))};
import Changes from ${JSON.stringify(join(root, "src/components/reader/Changes.svelte"))};

window.accepted = [];
window.rejected = [];
window.revealed = [];
window.tracked = [];

// Three changes over two files. The first two are one agent run.
const rows = [
  { id: 'c1', file_id: 'f1', path: 'paper.md', author: 'Ada', who: 'agent:ada', run: 'run-aaaa1111',
    at: 3, position: 4, before: 'cat', after: 'tabby' },
  { id: 'c2', file_id: 'f2', path: 'notes.md', author: 'Ada', who: 'agent:ada', run: 'run-aaaa1111',
    at: 2, position: 0, before: '', after: 'A new line.\\n' },
  { id: 'c3', file_id: 'f1', path: 'paper.md', author: 'Grace', who: 'acct-grace', run: '',
    at: 1, position: 40, before: 'old', after: '' },
];
const files = [{ id: 'f1', path: 'paper.md' }, { id: 'f2', path: 'notes.md' }];

const handlers = {
  onaccept: (row) => { window.accepted.push(row.id); return Promise.resolve(); },
  onreject: (row) => { window.rejected.push(row.id); return Promise.resolve(); },
  onreveal: (row) => window.revealed.push(row.id),
  ontracking: (on) => window.tracked.push(on),
};

let component = null;
window.show = async (props = {}) => {
  if (component) await unmount(component);
  window.accepted = []; window.rejected = []; window.revealed = []; window.tracked = [];
  component = mount(Changes, { target: document.body, props: {
    changes: rows, files, canReview: true, canTrack: true, identity: 'Grace', ...handlers, ...props,
  } });
  await tick();
};
window.rowsData = rows;

window.rows = () => [...document.querySelectorAll('.change-row')].map((row) => ({
  id: row.querySelector('.row-main')?.id?.replace('change-', '') || '',
  removed: row.querySelector('.deletion')?.textContent?.trim() || '',
  added: row.querySelector('.insertion')?.textContent?.trim() || '',
  meta: row.querySelector('.row-meta')?.textContent?.trim() || '',
  active: row.classList.contains('active'),
}));
window.count = () => document.querySelector('.changes-meta')?.textContent || '';
window.feedback = () => document.querySelector('.panel-status')?.textContent || '';
const press = (node) => {
  for (const type of ['pointerdown', 'pointerup', 'click']) {
    node.dispatchEvent(new PointerEvent(type, { bubbles: true, composed: true, button: 0, isPrimary: true, pointerType: 'mouse' }));
  }
};
const settle = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 60)); await tick(); };
const mainOf = (id) => {
  const node = [...document.querySelectorAll('.row-main')].find((one) => one.id === 'change-' + id);
  if (!node) throw new Error('no row ' + id);
  return node;
};
// Only the change being looked at offers an answer, so reaching one means
// selecting its row first, which is what a reviewer does with the click that
// opens it.
window.look = async (id) => { mainOf(id).click(); await settle(); };
window.act = async (id, action) => {
  await window.look(id);
  mainOf(id).closest('.change-row').querySelector('.row-action.' + action).click();
};
window.actionCount = () => document.querySelectorAll('.row-action').length;
window.key = (key) => {
  document.querySelector('.changes-panel')
    .dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
};
// Only the menu actually on show counts: the filter beside the menu keeps its
// own items in the page whether or not anybody has opened it.
const openItems = () => [...document.querySelectorAll('[data-part="content"][data-state="open"] [data-part="item"]')];
window.menuItems = async () => {
  const trigger = document.querySelector('.changes-menu');
  if (!trigger) return [];
  press(trigger);
  await settle();
  return openItems().map((node) => node.textContent.trim());
};
window.menu = async (label) => {
  const trigger = document.querySelector('.changes-menu');
  if (!trigger) throw new Error('change menu is unavailable');
  press(trigger);
  await settle();
  const item = openItems().find((node) => node.textContent.trim() === label);
  if (!item) throw new Error('no menu item ' + label);
  press(item);
  await settle();
};
window.filterItems = async () => {
  const trigger = document.querySelector('.filter-trigger');
  if (!trigger) return null;
  press(trigger);
  await settle();
  const labels = openItems().map((node) => node.textContent.trim().replace('\\u2713', ''));
  press(trigger);
  await settle();
  return labels;
};
window.filter = async (label) => {
  const trigger = document.querySelector('.filter-trigger');
  if (!trigger) throw new Error('change filters are unavailable');
  press(trigger);
  await settle();
  const item = openItems().find((node) => node.textContent.trim().endsWith(label));
  if (!item) throw new Error('no filter ' + label);
  press(item);
  await settle();
};
window.pick = (id) => {
  mainOf(id).closest('.row-head').querySelector('.row-pick').click();
};
window.barButton = (word) => [...document.querySelectorAll('.select-bar .bar-action')]
  .find((node) => node.textContent.trim().startsWith(word)) || null;
window.selectedCount = () => document.querySelector('.select-bar .select-count')?.textContent?.trim() || '';
window.picks = () => document.querySelectorAll('.row-pick').length;
window.empty = () => document.querySelector('.changes-empty')?.textContent?.trim() || '';
window.ready = true;
await window.show();
`);

let server, page;
try {
  await build({
    configFile: false, root, plugins: [svelte(), tailwindcss()], logLevel: "error",
    resolve: { alias: loroAlias },
    build: { outDir: join(temporary, "build"), lib: { entry, formats: ["es"], fileName: () => "panel.js" } },
  });
  server = createServer((request, response) => {
    const file = request.url === "/panel.js" ? "panel.js" : request.url === "/style.css" ? "librepaper-web.css" : null;
    response.setHeader("content-type", file ? contentType(file) : "text/html");
    response.end(file
      ? readFileSync(join(temporary, "build", file))
      : '<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/style.css"><style>body{display:flex;height:700px;width:380px;overflow:hidden}</style></head><body><script type="module" src="/panel.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  page = await browser("chromium", join(temporary, "profile"), 26000 + Math.floor(Math.random() * 10000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("panel", () => page.evaluate("window.ready"), 15000);

  const ev = (code) => page.evaluate(code);
  const waitFor = (label, code) => until(label, async () => Boolean(await ev(code)), 4000);

  // One row per change, in the order the paper is read: file order, then
  // offset within the file. The words on both sides of a change stay readable.
  const rows = await ev("window.rows()");
  assert.deepEqual(rows.map((row) => row.id), ["c1", "c3", "c2"], "rows follow the reading order of the paper");
  assert.equal(rows[0].removed, "− cat", "the words a change takes out stay readable");
  assert.equal(rows[0].added, "+ tabby", "beside the ones it puts in");
  assert.equal(rows.find((row) => row.id === "c2").removed, "", "a pure insertion shows nothing struck through");
  assert.equal(rows.find((row) => row.id === "c3").added, "", "a pure deletion shows nothing inserted");
  assert.match(await ev("window.count()"), /3 pending/);
  assert.match(rows[0].meta, /Ada/, "the author is named under the change");
  assert.match(rows[0].meta, /Run run-aaaa/, "a change that belongs to a run says so");
  assert.match(rows.find((row) => row.id === "c3").meta, /Grace \(you\)/, "the reader's own change is marked");

  // Clicking a row reveals it, and the verbs belong to the change being looked
  // at and to no other.
  assert.equal(await ev("window.actionCount()"), 2, "only the selected change carries a pair of buttons");
  await ev('window.look("c3")');
  assert.deepEqual(await ev("window.revealed.slice(-1)"), ["c3"], "clicking a row reveals it");

  // The three filters. Run is offered because some row has one.
  assert.deepEqual(await ev("window.filterItems()"),
    ["Anyone", "Ada", "Grace (you)", "Every file", "paper.md", "notes.md", "Every run", "Run run-aaaa"],
    "author, file and run are filterable");
  await ev('window.filter("notes.md")');
  await waitFor("file filtered", "window.rows().length === 1");
  assert.deepEqual((await ev("window.rows()")).map((row) => row.id), ["c2"]);
  assert.match(await ev("window.count()"), /3 pending · 1 shown/);
  await ev('window.filter("Every file")');
  await ev('window.filter("Grace (you)")');
  await waitFor("author filtered", "window.rows().length === 1");
  assert.deepEqual((await ev("window.rows()")).map((row) => row.id), ["c3"]);
  await ev('window.filter("Anyone")');
  await ev('window.filter("Run run-aaaa")');
  await waitFor("run filtered", "window.rows().length === 2");
  assert.deepEqual((await ev("window.rows()")).map((row) => row.id), ["c1", "c2"], "the run filter keeps the pass together");
  await ev('window.filter("Every run")');
  await waitFor("filters cleared", "window.rows().length === 3");

  // Without any run there is no run filter.
  await ev("window.show({ changes: window.rowsData.map((row) => ({ ...row, run: '' })) })");
  assert.deepEqual(await ev("window.filterItems()"), ["Anyone", "Ada", "Grace (you)", "Every file", "paper.md", "notes.md"],
    "no run, no run filter");
  // A run handed in by the reader opens the panel already narrowed to it.
  await ev("window.show({ run: 'run-aaaa1111' })");
  await waitFor("opened on a run", "window.rows().length === 2");
  await ev("window.show()");

  // Answering a change calls back with the row, and the queue moves on to the
  // next change in the paper, whether or not the document has dropped the row.
  await ev('window.act("c1", "accept")');
  await waitFor("accepted", "window.accepted.length > 0");
  assert.deepEqual(await ev("window.accepted"), ["c1"]);
  await waitFor("advanced", 'window.rows().some((row) => row.id === "c3" && row.active)');

  // The keyboard walks the same queue.
  await ev("window.show()");
  await ev('window.key("j")');
  await waitFor("moved down", 'window.rows().some((row) => row.id === "c3" && row.active)');
  await ev('window.key("a")');
  await waitFor("accepted by key", "window.accepted.length === 1");
  assert.deepEqual(await ev("window.accepted"), ["c3"]);
  // Answering moves on to the next change in the paper.
  await waitFor("advanced after the key", 'window.rows().some((row) => row.id === "c2" && row.active)');
  await ev('window.key("k")');
  await waitFor("moved up", 'window.rows().some((row) => row.id === "c3" && row.active)');
  await ev('window.key("r")');
  await waitFor("rejected by key", "window.rejected.length === 1");
  assert.deepEqual(await ev("window.rejected"), ["c3"]);
  await waitFor("advanced after reject", 'window.rows().some((row) => row.id === "c2" && row.active)');
  await ev('window.key("ArrowUp")');
  await waitFor("arrow up", 'window.rows().some((row) => row.id === "c3" && row.active)');
  await ev('window.key("ArrowDown")');
  await waitFor("arrow down", 'window.rows().some((row) => row.id === "c2" && row.active)');

  // The tracking switch reports what was asked of it.
  await ev("document.querySelector('.tracking-toggle').click()");
  await waitFor("tracking asked", "window.tracked.length > 0");

  // Ticking rows, and the verbs that go with it, are a mode entered from the
  // menu, so an ordinary review is a queue of changes rather than a queue of
  // changes wearing checkboxes.
  await ev("window.show()");
  assert.equal(await ev("window.picks()"), 0, "no checkboxes until selection is asked for");
  assert.equal(await ev("!!window.barButton('Accept')"), false, "and no bulk verbs standing above the queue");
  await ev("window.menu('Select multiple')");
  await waitFor("selecting", "window.picks() === 3");
  assert.equal(await ev("window.selectedCount()"), "0 selected");
  assert.equal(await ev("window.barButton('Accept').disabled"), true, "with nothing ticked there is nothing to answer for");
  await ev('window.pick("c1")');
  await ev('window.pick("c2")');
  await waitFor("two ticked", 'window.selectedCount() === "2 selected"');
  await ev("window.barButton('Reject').click()");
  await waitFor("bulk rejected", "window.rejected.length === 2");
  assert.deepEqual((await ev("window.rejected")).sort(), ["c1", "c2"]);
  assert.match(await ev("window.feedback()"), /2 changes rejected/);
  assert.equal(await ev("window.picks()"), 0, "the ticks are spent and the mode closes behind them");

  // Hiding a ticked change clears its tick, so the count and the eventual
  // bulk verb continue to name the same rows.
  await ev("window.show()");
  await ev("window.menu('Select multiple')");
  await ev('window.pick("c1")');
  await waitFor("selected before filtering", 'window.selectedCount() === "1 selected"');
  await ev('window.filter("notes.md")');
  await waitFor("hidden tick cleared", 'window.selectedCount() === "0 selected"');

  // The whole queue at once, from the menu, and only after being asked twice.
  await ev("window.show()");
  await ev("window.menu('Accept all pending changes')");
  await waitFor("asked to confirm", "/Accept all 3\\?/.test(document.querySelector('.confirm-bar .select-count')?.textContent || '')");
  assert.deepEqual(await ev("window.accepted"), [], "nothing is answered by opening the menu item");
  await ev("[...document.querySelectorAll('.confirm-bar .bar-action')].find((node) => node.textContent.trim() === 'Accept all').click()");
  await waitFor("all accepted", "window.accepted.length === 3");
  assert.match(await ev("window.feedback()"), /3 changes accepted/);
  await ev("window.show()");
  await ev("window.menu('Reject all pending changes')");
  await ev("[...document.querySelectorAll('.confirm-bar .bar-action')].find((node) => node.textContent.trim() === 'Reject all').click()");
  await waitFor("all rejected", "window.rejected.length === 3");
  // Escape closes the confirmation without answering anything.
  await ev("window.show()");
  await ev("window.menu('Reject all pending changes')");
  await ev('window.key("Escape")');
  await waitFor("confirmation closed", "!document.querySelector('.confirm-bar')");
  assert.deepEqual(await ev("window.rejected"), []);

  // A refused decision is reported and leaves the row where it was.
  await ev("window.show({ onreject: () => Promise.reject(new Error('The document is read-only.')) })");
  await ev('window.act("c1", "reject")');
  await waitFor("refusal shown", "/read-only/.test(window.feedback())");
  assert.equal(await ev("window.rows().length"), 3);

  // Without the right to review the queue is readable and inert: no verbs, no
  // mode, no menu.
  await ev("window.show({ canReview: false })");
  assert.equal((await ev("window.rows()")).length, 3, "a reader still sees every change");
  await ev('window.look("c1")');
  assert.equal(await ev("window.actionCount()"), 0, "and is offered no button to answer one");
  await ev('window.key("a")');
  await ev('window.key("r")');
  assert.deepEqual(await ev("[window.accepted, window.rejected]"), [[], []], "the keys answer nothing for a reader");
  assert.equal(await ev("window.picks()"), 0);
  assert.deepEqual(await ev("window.menuItems()"), [], "a reader is offered no verb and no mode they cannot use");

  // Nothing left to answer: a sentence, and no verb standing over nothing.
  await ev("window.show({ changes: [] })");
  assert.match(await ev("window.empty()"), /No pending changes/);
  assert.equal(await ev("window.actionCount()"), 0);
  assert.equal((await ev("window.rows()")).length, 0, "and no rows remain");

  console.log("changes-browser: rows, filters by author, file and run, keyboard, bulk confirmation and read-only readers passed");
} finally {
  await page?.close();
  server?.close();
  removeTemporary(temporary);
}
