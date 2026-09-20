// The bar at the top of the reader is one control, not five buttons. This
// drives it the way a person does -- press a word, slide onto the next, walk
// the row with the arrow keys -- and checks that the row keeps up. The room
// is the only module replaced; Reader, Nav and the real CSS mount.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../../tools/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-menubar-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");
writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
const text = new LoroText(); text.insert(0, "a line of source");
files.setContainer("main", text); paths.set("main", "main.md"); meta.set("main", "main");
server.commit();
const update = encode(server.export({ mode: "update" }));
const vector = encode(server.oplogVersion().encode());
export function openRoom(slug, {onMessage, onConnected}) {
  queueMicrotask(() => onConnected(true));
  return {
    send(message) { if (message.type === "doc-open") queueMicrotask(() => onMessage({type:"doc-state", protocol:"librepaper.room.v3", vector, durableVector: vector, updates:[update]})); return {ok:true}; },
    sendLive() { return {ok:true}; },
    close() {}
  };
}
`);
writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
window.testErrors = [];
window.addEventListener('error', event => window.testErrors.push(event.message));
globalThis.WebSocket = class { constructor(){ queueMicrotask(() => this.onopen?.()); } send(){} close(){} };
globalThis.fetch = async (url) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Tester',providers:[]});
  if (path.endsWith('/config')) return Response.json({});
  if (path === '/api/documents/paper') return Response.json({title:'Menus',created_at:'test',role:'editor',source_format:'markdown',docs_origin:location.origin});
  if (path.endsWith('/frame')) return Response.json({token:'frame-token',until:9999999999});
  if (path.includes('/comments')) return { ok:true, json:async()=>({comments:[]}) };
  return { ok:false, json:async()=>({}) };
};
const { default: Reader } = await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
const { mount } = await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);

let serverHttp, b;
try {
  await build({
    configFile: false, root: join(root, "web"), resolve: { alias: loroAlias },
    plugins: [tailwindcss(), svelte(), { name: "mock-room", enforce: "pre", resolveId(id) { if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room; } }],
    build: { outDir: out, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js" } }, logLevel: "error",
  });
  serverHttp = createServer((req, res) => {
    if (req.url === "/docs/paper") {
      res.setHeader("content-type", "text/html");
      res.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    const file = join(out, req.url.split("?")[0].slice(1));
    try { res.setHeader("content-type", contentType(file)); res.end(readFileSync(file)); }
    catch { res.statusCode = 404; res.end(); }
  });
  await new Promise((resolve, reject) => { serverHttp.once("error", reject); serverHttp.listen(0, "127.0.0.1", resolve); });
  b = await browser("chromium", join(temp, "profile"), 19000 + Math.floor(Math.random() * 1000));
  await b.resize(1400, 900);
  await b.navigate(`http://127.0.0.1:${serverHttp.address().port}/docs/paper`);
  await until("the bar", () => b.evaluate('document.querySelectorAll(".menubar-item").length >= 5'), 10000);

  const flush = () => b.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  const centre = (name) => b.evaluate(`(() => { const r = document.querySelector('[data-menubar=${JSON.stringify(name)}]').getBoundingClientRect(); return {x: r.left + r.width/2, y: r.top + r.height/2}; })()`);
  // Zag reads real pointer events; a synthetic `click()` does not open a menu.
  const press = async (name) => {
    const { x, y } = await centre(name);
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
      await b.command("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, buttons: type === "mousePressed" ? 1 : 0 });
    }
    await flush();
  };
  const point = async (name) => {
    const { x, y } = await centre(name);
    await b.command("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await flush();
  };
  const key = async (value, code, windowsVirtualKeyCode) => {
    for (const type of ["keyDown", "keyUp"]) await b.command("Input.dispatchKeyEvent", { type, key: value, code, windowsVirtualKeyCode });
    await flush();
  };
  const opened = () => b.evaluate('[...document.querySelectorAll("[data-menubar]")].filter(node => node.dataset.state === "open").map(node => node.dataset.menubar)');

  assert.deepEqual(await opened(), [], "the bar is closed until something asks for it");

  // Pointing at a word while the bar is at rest must not open anything: a
  // menu bar is not a hover menu.
  await point("edit");
  assert.deepEqual(await opened(), [], "hover alone opens nothing");

  await press("file");
  assert.deepEqual(await opened(), ["file"], "pressing File opens File");

  // The whole point of a bar: with one menu open, the next one is a slide
  // away, and only one is ever open.
  await point("edit");
  assert.deepEqual(await opened(), ["edit"], "sliding onto Edit moves the menu there");

  await key("ArrowRight", "ArrowRight", 39);
  assert.deepEqual(await opened(), ["insert"], "the right arrow walks the row");
  await key("ArrowLeft", "ArrowLeft", 37);
  assert.deepEqual(await opened(), ["edit"], "and the left arrow walks back");

  await key("Escape", "Escape", 27);
  assert.deepEqual(await opened(), [], "Escape closes the bar");

  // Pressing the open word again closes it rather than reopening it.
  await press("view");
  assert.deepEqual(await opened(), ["view"]);
  await press("view");
  assert.deepEqual(await opened(), [], "pressing the open word closes it");

  // What the keyboard already does, said in the menu. Not in Vim or Emacs
  // mode, where the keys belong to the mode -- that is Reader's own test.
  await press("edit");
  const shortcuts = await b.evaluate('[...document.querySelectorAll(".menuitem-keys")].map(node => node.textContent)');
  assert.ok(shortcuts.some((keys) => /Z$/.test(keys)), "Undo names its shortcut: " + JSON.stringify(shortcuts));

  // Every name in a menu starts at the same place, whether or not its line
  // carries a tick.
  await press("view");
  const starts = await b.evaluate(`(() => {
    const trigger = document.querySelector("[data-menubar=view]");
    const panel = document.getElementById(trigger.getAttribute("aria-controls"));
    return [...panel.querySelectorAll(".menuitem")].map(item => Math.round(item.getBoundingClientRect().left + parseFloat(getComputedStyle(item).paddingLeft)));
  })()`);
  assert.ok(starts.length > 3, "the View menu has lines to compare");
  assert.equal(new Set(starts).size, 1, "every name starts at one place: " + JSON.stringify(starts));

  assert.deepEqual(await b.evaluate("window.testErrors"), []);
  console.log("menubar-browser: one bar, hover to switch, arrows across the row, shortcuts and a single name column passed");
} finally {
  await b?.close();
  serverHttp?.close();
  rmSync(temp, { recursive: true, force: true });
}
