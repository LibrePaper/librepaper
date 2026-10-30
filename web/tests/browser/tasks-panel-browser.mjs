// Browser acceptance for the project Tasks panel.
//
// The Reader is real. Only its room and HTTP shell are local test doubles: the
// room is backed by Loro, so a checkbox that changes a file takes the same path
// as a collaborator's update. The project has two Markdown files, and the panel
// reads both of them.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-tasks-panel-browser-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");

// The first file by name is the project's main file. Its id in the room is
// "main", as in the outline check.
const plan = "# Plan\n\n- [ ] Write intro priority:1 due:2020-01-01 #paper assignee:alice\n- [x] Old done item\n- [/] Draft methods #paper/methods\n";
const notes = "- [ ] Check references id:refs\n- [?] Final pass after:refs priority:2\n- [ ] Ghost after:missing\n";
const initial = { "a-main.md": plan, "notes.md": notes };

writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = bytes => btoa(String.fromCharCode(...bytes));
const decode = text => Uint8Array.from(atob(text), c => c.charCodeAt(0));
const rooms = new Map();
const initial = ${JSON.stringify(initial)};

function makeRoom() {
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  for (const [path, value] of Object.entries(initial)) {
    const id = path === "a-main.md" ? "main" : path;
    const text = new LoroText();
    text.insert(0, value);
    files.setContainer(id, text);
    paths.set(id, path);
  }
  doc.commit();
  return { doc, files, paths };
}

function bytes(update) { return new Uint8Array(update); }
function installRoom(slug, callbacks) {
  let current = rooms.get(slug);
  if (!current) {
    current = makeRoom();
    rooms.set(slug, current);
  }
  current.callback = callbacks.onMessage;
  return current;
}

export function openRoom(slug, {onMessage, onConnected}) {
  const room = installRoom(slug, {onMessage});
  window.roomControl = {
    snapshot(path) {
      const id = [...room.paths.entries()].find(([, name]) => name === path)?.[0];
      return id ? room.files.get(id)?.toString() || "" : "";
    },
  };
  const updateSub = room.doc.subscribeLocalUpdates((update) => {
    room.callback?.({type: "doc-update", update: encode(update)});
  });
  queueMicrotask(() => onConnected(true));
  return {
    send(message) {
      if (message.type === "doc-open") {
        queueMicrotask(() => onMessage({type: "doc-state", protocol: "librepaper.room.v3",
          vector: encode(room.doc.oplogVersion().encode()), durableVector: encode(room.doc.oplogVersion().encode()), updates: [encode(room.doc.export({ mode: "update" }))]}));
      } else if (message.type === "doc-update") {
        room.doc.import(bytes(decode(message.update)));
        // Sec 6.1: doc-ack carries upTo, not seq; coverage is gone.
        queueMicrotask(() => onMessage({type: "doc-ack", upTo: message.seq}));
      }
      return {ok: true};
    },
    sendLive() { return {ok: true}; },
    close() { updateSub?.(); },
  };
}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};
window.editorState = () => {
  const view = EditorView.findFromDOM(document.querySelector('.cm-editor'));
  return { text: view.state.doc.toString(), caret: view.state.selection.main.head };
};
window.testErrors = [];
window.resizeNotifications = 0;
window.addEventListener('error', event => {
  // Pane changes can defer a ResizeObserver delivery to the next paint.
  if (event.message === 'ResizeObserver loop completed with undelivered notifications.') window.resizeNotifications++;
  else window.testErrors.push(event.message);
});
window.addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
globalThis.WebSocket = class {
  constructor(url) {
    if (!String(url).includes('/chat/')) throw new Error('Unexpected socket: ' + url);
    queueMicrotask(() => this.onopen?.());
  }
  send(raw) {
    if (JSON.parse(raw).type === 'join') queueMicrotask(() => this.onmessage?.({data:JSON.stringify({type:'ready',agent:true})}));
  }
  close() { this.onclose?.(); }
};
globalThis.fetch = async (url) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Tasks tester',providers:[]});
  if (path.endsWith('/config')) return Response.json({text_extensions:['.md','.qmd','.bib'],asset_extensions:[],max_path:200});
  const documentMatch = path.match(/\\/api\\/documents\\/([^/]+)$/);
  if (documentMatch) return Response.json({title:'Tasks browser',created_at:'tasks-test',role:'editor',source_format:'markdown',docs_origin:location.origin,can_moderate:true,can_see_sharing:true,renderers:['markdown']});
  if (path.endsWith('/frame')) return Response.json({token:'tasks-frame',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'tasks-chat',token:'private'});
  if (path.includes('/comments')) return Response.json({comments:[]});
  return new Response('', {status:404});
};
const { default: Reader } = await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
const { mount } = await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);

let httpServer;
let tab;
try {
  await build({
    configFile: false, define: KATEX_DEFINE,
    root: join(root, "web"),
    resolve: { alias: loroAlias },
    plugins: [
      tailwindcss(),
      svelte(),
      { name: "mock-room", enforce: "pre", resolveId(id) {
        if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
      } },
    ],
    build: {outDir: out, emptyOutDir: true, lib: {entry, formats: ["es"], fileName: () => "check.js"}},
    logLevel: "error",
  });
  httpServer = createServer((request, response) => {
    if (serveKatex(request, response)) return;
    if (request.url?.startsWith("/docs/")) {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    if (request.url?.startsWith("/raw/")) {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><html><body><h1>Rendered document</h1><p>Tasks browser frame.</p></body></html>');
      return;
    }
    const file = join(out, request.url?.split("?")[0].slice(1) || "");
    try {
      response.setHeader("content-type", contentType(file));
      response.end(readFileSync(file));
    } catch {
      response.statusCode = 404;
      response.end();
    }
  });
  await new Promise((resolve, reject) => {
    httpServer.once("error", reject);
    httpServer.listen(0, "127.0.0.1", resolve);
  });
  const port = httpServer.address().port;
  tab = await browser("chromium", join(temp, "profile"), 20000 + Math.floor(Math.random() * 1000));
  const url = `http://127.0.0.1:${port}/docs/tasks`;
  await tab.resize(1280, 900);
  await tab.navigate(url);

  const flush = () => tab.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  const visible = (selector) => tab.evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); return Boolean(node?.getClientRects().length && getComputedStyle(node).display !== 'none' && getComputedStyle(node).visibility !== 'hidden'); })()`);
  const click = async (selector) => { await tab.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); node?.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse'})); node?.click(); })()`); await flush(); };

  const panel = 'section[aria-label="Project tasks"]';
  // A JavaScript expression that finds one task row by its visible text.
  const row = (title) => `[...document.querySelectorAll('${panel} .tasks-item')].find(item => item.querySelector('.tasks-text')?.textContent.trim() === ${JSON.stringify(title)})`;
  const rowTexts = async () => tab.evaluate(`[...document.querySelectorAll('${panel} .tasks-text')].map(node => node.textContent.trim())`);
  // Sets one filter by its label. A select reports a change the same way a person does.
  const setFilter = (label, value) => tab.evaluate(`(() => {
    const field = [...document.querySelectorAll('${panel} .tasks-field')].find(node => node.querySelector('.tasks-label')?.textContent.trim() === ${JSON.stringify(label)});
    const select = field?.querySelector('select');
    if (!select) throw new Error("no " + ${JSON.stringify(label)} + " filter");
    select.value = ${JSON.stringify(value)};
    select.dispatchEvent(new Event("change", {bubbles: true}));
    return true;
  })()`);
  const openTasks = async () => {
    if (await visible(panel)) return;
    await click('.sidebar-activity [aria-label="Tasks"]');
    await until("project tasks", () => tab.evaluate(`Boolean(document.querySelector('${panel} .tasks-item'))`), 10000);
  };

  await until("Reader source", () => tab.evaluate("Boolean(document.querySelector('.cm-editor'))"), 10000);
  await openTasks();

  // The default filter is Open, so done tasks are left out.
  assert.deepEqual(await rowTexts(), ["Write intro", "Draft methods", "Check references", "Final pass", "Ghost"]);

  // An incomplete task past its due date is flagged. Its row names the state in words too.
  assert.equal(await tab.evaluate(`(() => { const item = (${row("Write intro")}); return Boolean(item?.querySelector('.chip.overdue')) && /Overdue/.test(item.textContent); })()`), true, "Write intro is overdue");

  // A dependency on an id nobody holds is a warning on the row.
  assert.equal(await tab.evaluate(`((${row("Ghost")})?.querySelector('.warning .sr-only')?.textContent.includes("No task has id missing.")) === true`), true, "Ghost warns about its missing id");

  // The tag filter keeps the paper tag and its descendants.
  await setFilter("Tag", "paper");
  await until("tag filter keeps paper tasks", async () => JSON.stringify(await rowTexts()) === JSON.stringify(["Write intro", "Draft methods"]), 5000);

  // Clearing the tag and choosing All brings back the done task.
  await setFilter("Tag", "");
  await setFilter("Status", "all");
  await until("All shows the done task", async () => (await rowTexts()).includes("Old done item"), 5000);

  // Ticking a box writes the one change to the source and nothing else.
  await tab.evaluate(`((${row("Check references")})?.querySelector('input.tasks-box')).click()`);
  const ticked = notes.replace("- [ ] Check references", "- [x] Check references");
  await until("notes.md ticked", async () => (await tab.evaluate("window.roomControl.snapshot('notes.md')")) === ticked, 5000);

  // A task's text opens its file at the task's line.
  await tab.evaluate(`((${row("Final pass")})?.querySelector('.tasks-text')).click()`);
  await until("notes.md opens in the editor", () => tab.evaluate(`window.editorState().text.includes(${JSON.stringify("Check references id:refs")})`), 5000);
  await until("the caret is on Final pass", () => tab.evaluate(`document.querySelector('.cm-activeLine')?.textContent.includes("Final pass") === true`), 5000);

  await flush();
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
  console.log("tasks-panel-browser: open list, overdue and missing-id warnings, tag filter, All status, checkbox toggle and jump to source passed");
} finally {
  await tab?.close();
  httpServer?.close();
  removeTemporary(temp);
}
