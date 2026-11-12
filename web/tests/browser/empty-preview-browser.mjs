// Browser acceptance for the empty-document cover.
//
// The Reader is real. Only its room and HTTP shell are local test doubles: the
// room is backed by Loro, so remote edits take the same path as a collaborator's
// update. An empty or whitespace-only file shows "This document is empty." in
// the preview instead of the waiting display, and the cover clears once the
// file has text.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-empty-preview-browser-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");

writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = bytes => btoa(String.fromCharCode(...bytes));
const decode = text => Uint8Array.from(atob(text), c => c.charCodeAt(0));
const rooms = new Map();

const initial = {
  "a-empty.md": "",
  "b-blank.html": "  \\n\\n",
};

function makeRoom(slug) {
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  for (const [path, value] of Object.entries(initial)) {
    const id = path;
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
    current = makeRoom(slug);
    rooms.set(slug, current);
  }
  current.callback = callbacks.onMessage;
  return current;
}

export function openRoom(slug, {onMessage, onConnected}) {
  const room = installRoom(slug, {onMessage});
  const textFor = (path) => {
    const id = [...room.paths.entries()].find(([, name]) => name === path)?.[0];
    const text = id && room.files.get(id);
    if (text?.kind?.() !== "Text") throw new Error("missing room file " + path);
    return text;
  };
  window.roomControl = {
    remoteSet(path, value) {
      const text = textFor(path);
      if (text.length > 0) text.delete(0, text.length);
      text.insert(0, value);
      room.doc.commit();
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
    sendLive(message) { return {ok: true}; },
    close() { updateSub?.(); },
  };
}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
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
  if (path.endsWith('/me')) return Response.json({name:'Empty preview tester',providers:[]});
  if (path.endsWith('/config')) return Response.json({text_extensions:['.html','.md'],asset_extensions:[],max_path:200});
  const documentMatch = path.match(/\\/api\\/documents\\/([^/]+)$/);
  if (documentMatch) return Response.json({title:'Empty preview browser',created_at:'empty-preview-test',role:'editor',source_format:'markdown',docs_origin:location.origin,can_moderate:true,can_see_sharing:true,renderers:['markdown']});
  if (path.endsWith('/frame')) return Response.json({token:'empty-preview-frame',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'empty-preview-chat',token:'private'});
  if (path.includes('/comments')) return Response.json({comments:[]});
  return new Response('', {status:404});
};
// Advertised so Markdown counts as compilable here. An empty file never loads
// the engine for a page, so the module need not exist.
globalThis.LIBREPAPER_MODULES = {markdown: '/no-engine.js'};
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
      response.end('<!doctype html><html><body><h1>Rendered document</h1><p>Empty preview browser frame.</p></body></html>');
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
  const url = `http://127.0.0.1:${port}/docs/empty-preview`;
  await tab.resize(1280, 900);
  await tab.navigate(url);
  const flush = () => tab.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  const click = async (selector) => { await tab.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); node?.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse'})); node?.click(); })()`); await flush(); };
  const cover = () => tab.evaluate(`document.querySelector('.cover')?.textContent.trim() || ""`);
  const openFile = async (path) => {
    await click('.sidebar-activity [aria-label="Files"]');
    await until(`file ${path}`, () => tab.evaluate(`Boolean(document.querySelector('.explorer-row[data-tip=${JSON.stringify(path)}]'))`), 5000);
    await click(`.explorer-row[data-tip=${JSON.stringify(path)}]`);
  };

  await until("Reader source", () => tab.evaluate("Boolean(document.querySelector('.cm-editor'))"), 10000);
  await until("empty cover", async () => (await cover()) === "This document is empty.", 10000);

  // The old loop showed the spinner and "Preparing the preview" forever.
  await new Promise(resolve => setTimeout(resolve, 3000));
  assert.equal(await cover(), "This document is empty.", "empty document keeps the cover after waiting");
  assert.equal(await tab.evaluate("document.querySelector('.cover .spinner') === null"), true, "empty document shows no spinner");

  // Whitespace counts as empty.
  await openFile("b-blank.html");
  await until("whitespace cover", async () => (await cover()) === "This document is empty.", 5000);

  // Text arriving clears the cover, and emptying the file brings it back.
  await tab.evaluate(`window.roomControl.remoteSet("b-blank.html", "<!doctype html><p>Now it has text</p>")`);
  await until("cover clears once text arrives", async () => (await cover()) === "", 10000);
  await tab.evaluate(`window.roomControl.remoteSet("b-blank.html", "")`);
  await until("cover returns when the file is emptied", async () => (await cover()) === "This document is empty.", 10000);

  console.log("empty-preview-browser: empty and whitespace-only documents show the cover, it clears on text and returns when emptied passed");
} finally {
  await tab?.close();
  httpServer?.close();
  removeTemporary(temp);
}
