// Exercise real Editor and Changes components together; server decision
// durability and permissions are covered by the Rust revision tests.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-live-tracking-"));
const output = join(temporary, "dist");
const entry = join(temporary, "entry.js");
const modulePath = (path) => JSON.stringify(join(root, path));
writeFileSync(entry, `
import * as Y from ${modulePath("node_modules/yjs/dist/yjs.mjs")};
import { tick } from ${modulePath("node_modules/svelte/src/index-client.js")};
import { createClassComponent } from ${modulePath("node_modules/svelte/src/legacy/legacy-client.js")};
import { EditorView } from ${modulePath("node_modules/@codemirror/view/dist/index.js")};
import { Transaction } from ${modulePath("node_modules/@codemirror/state/dist/index.js")};
import Editor from ${modulePath("src/components/Editor.svelte")};
import Changes from ${modulePath("src/components/reader/Changes.svelte")};
import { join as joinSession } from ${modulePath("src/lib/collab.js")};
import { createRevisionController } from ${modulePath("src/lib/track-changes.js")};

const session = joinSession({ send() {}, mayEdit: true });
const file = session.addText("paper.md", "The brown fox.");
session.setMain(file);
const tracking = createRevisionController({
  doc: session.doc, author: "Reviewer", documentId: "browser-test",
  fileOf: (id) => session.paths.get(id), textOf: (id) => session.textOf(id),
  send(request) {
    // Deliberately defer acknowledgment, so the UI must remain busy.
    window.pendingRequest = request;
    (window.pendingRequests ||= []).push(request);
  },
});
const editor = createClassComponent({ component: Editor, target: document.querySelector("#editor"),
  props: { session, file, tracking, format: "markdown" } });
const pane = createClassComponent({ component: Changes, target: document.querySelector("#changes"),
  props: { canTrack: true, canModerate: true,
    ontracking: (value) => tracking.setEnabled(value),
    onmarkup: (value) => tracking.setShowMarkup(value),
    onrevisiondecide: (id, action) => tracking.decide(id, action),
    onrevisionundo: (id) => tracking.undo(id),
    onrevisionreveal: (record) => editor.goToIn(file, tracking.locate(record).offset),
  } });
const refresh = () => { const s = tracking.snapshot(); pane.$set({ revisions: s.revisions, tracking: s.enabled, showMarkup: s.showMarkup }); };
tracking.onChange(refresh); refresh();
window.live = {
  state: () => ({ ...tracking.snapshot(), text: session.textOf(file).toString() }),
  edit: async (from, to, insert, userEvent = "input.type") => {
    const view = EditorView.findFromDOM(document.querySelector(".cm-editor"));
    view.dispatch({ changes: { from, to, insert }, selection: { anchor: from + insert.length }, annotations: Transaction.userEvent.of(userEvent) });
    await tick();
  },
  remote: async () => {
    session.doc.transact(() => session.textOf(file).insert(0, "Remote "), "remote");
    await tick();
  },
  undo: async () => { await editor.editCommand("undo"); await tick(); },
  redo: async () => { await editor.editCommand("redo"); await tick(); },
  acknowledge: async () => {
    const request = window.pendingRequests.shift();
    const record = JSON.parse(tracking.revisions.get(request.revision_id));
    const status = request.action === "undo" ? "pending" : "accepted";
    session.doc.transact(() => tracking.revisions.set(record.id, JSON.stringify({ ...record, status })), "remote");
    tracking.receive({ type: "revision-decision", request_id: request.request_id, revision_id: record.id, status });
    await tick();
  },
  acknowledgeAll: async () => {
    while (window.pendingRequests.length) await window.live.acknowledge();
  },
};
`);

let server;
let page;
try {
  await build({ configFile: false, root, plugins: [svelte()], resolve: { dedupe: ["svelte", "yjs", "@codemirror/state", "@codemirror/view"] },
    build: { outDir: output, emptyOutDir: true, minify: false, rollupOptions: { input: entry, output: { entryFileNames: "entry.js" } } } });
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://localhost").pathname;
    if (path === "/") {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><div id="changes" style="width:420px;height:700px"></div><div id="editor"></div><script type="module" src="/entry.js"></script>');
      return;
    }
    const file = join(output, path);
    if (!existsSync(file)) { response.writeHead(404); response.end(); return; }
    response.setHeader("content-type", path.endsWith(".css") ? "text/css" : "text/javascript");
    response.end(readFileSync(file));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  page = await browser("chromium", join(temporary, "profile"), 23000 + Math.floor(Math.random() * 5000));
  await page.navigate(`http://127.0.0.1:${server.address().port}/`);
  await until("editor mounted", () => page.evaluate("Boolean(window.live && document.querySelector('.cm-editor'))"), 15000);
  const clickText = (text) => page.evaluate(`[...document.querySelectorAll('button')].find(b=>b.textContent.trim()===${JSON.stringify(text)})?.click()`);
  await clickText("Track changes: Off");
  assert.equal(await page.evaluate("live.state().enabled"), true);
  assert.equal(await page.evaluate("document.querySelector('[aria-label=\"Track changes: On\"]')?.getAttribute('aria-pressed')"), "true");
  await page.evaluate('document.querySelector(".markup-toggle input").click()');
  assert.equal(await page.evaluate("live.state().showMarkup"), false);
  await page.evaluate('document.querySelector(".markup-toggle input").click()');
  await page.evaluate("live.edit(4, 9, 'red')");
  let state = await page.evaluate("live.state()");
  assert.equal(state.text, "The red fox.");
  assert.equal(state.revisions.length, 1);
  assert.equal(state.revisions[0].before, "brown");
  assert.equal(state.revisions[0].after, "red");
  await page.evaluate("live.undo()");
  assert.equal((await page.evaluate("live.state()")).text, "The brown fox.");
  await page.evaluate("live.redo()");
  assert.equal((await page.evaluate("live.state()")).text, "The red fox.");
  await page.evaluate("live.remote()");
  state = await page.evaluate("live.state()");
  assert.equal(state.revisions.filter(r => r.status === "pending").length, 1, "remote changes are not captured");
  await page.evaluate("live.edit(19, 19, 'A')");
  await page.evaluate("live.edit(20, 20, 'B')");
  state = await page.evaluate("live.state()");
  assert.equal(state.revisions.filter(r => r.status === "pending").length, 2, "adjacent typing groups");
  assert.equal(state.revisions.find(r => r.after === "AB")?.before, "");
  await page.evaluate("live.edit(19, 21, '')");
  state = await page.evaluate("live.state()");
  assert.equal(state.revisions.filter(r => r.status === "pending").length, 1, "deleting own insertion cancels it");
  await page.evaluate("live.edit(19, 19, '!', 'input.paste')");
  await page.evaluate("live.edit(20, 20, '?', 'input.paste')");
  assert.equal((await page.evaluate("live.state()")).revisions.filter(r => r.status === "pending").length, 3, "separate pastes stay separate review units");
  await page.evaluate("document.querySelector('[aria-label=\"Accept change in paper.md\"]')?.click()");
  await until("decision request", () => page.evaluate("Boolean(window.pendingRequest)"), 2000);
  assert.equal(await page.evaluate("live.state().revisions[0].status"), "pending", "no optimistic acceptance");
  await page.evaluate("live.acknowledge()");
  await until("review advanced", () => page.evaluate("live.state().revisions.filter(r=>r.status==='pending').length===2"), 2000);
  await page.evaluate(`document.querySelectorAll('.change-row input[type=checkbox]').forEach(input=>input.click())`);
  await clickText("Accept 2 selected");
  await until("captured bulk requests", () => page.evaluate("window.pendingRequests.length===2"), 2000);
  await page.evaluate("live.acknowledgeAll()");
  await until("bulk review completed", () => page.evaluate("document.querySelector('#changes').textContent.includes('No pending changes')"), 2000);
  assert.equal(await page.evaluate("live.state().text"), "Remote The red fox.!?");
  await clickText("Undo acceptance (2)");
  await until("captured batch undo", () => page.evaluate("window.pendingRequests.length===2"), 2000);
  await page.evaluate("live.acknowledgeAll()");
  await until("batch undo completed", () => page.evaluate("live.state().revisions.filter(r=>r.status==='pending').length===2"), 2000);
  console.log("live-tracking-browser: toggle, markup, replacement, undo/redo, grouping, remote edits, cancellation, individual and batch review passed");
} finally {
  await page?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
