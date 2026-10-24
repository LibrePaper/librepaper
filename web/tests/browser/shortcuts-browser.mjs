// The keyboard, driven the way a person drives it. The rules live in
// lib/commands.js and are checked there; what is checked here is that the real
// window is wired to them: that `?` opens the settings from the page and types a
// question mark in the source, that a chord nobody can type still works from
// inside the editor, that the palette runs what it offers, and that a panel
// command moves the focus rather than only opening a column.
//
// The room is the only module replaced. Reader, Nav, the editor and the real
// CSS all mount.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-shortcuts-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");
writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths");
const text = new LoroText(); text.insert(0, "a line of source");
files.setContainer("main", text); paths.set("main", "main.md");
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
  if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md'], text_extensions: ['.html', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/paper') return Response.json({title:'Keys',created_at:'test',role:'editor',source_format:'markdown',docs_origin:location.origin});
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
    configFile: false, define: KATEX_DEFINE, root: join(root, "web"), resolve: { alias: loroAlias },
    plugins: [tailwindcss(), svelte(), { name: "mock-room", enforce: "pre", resolveId(id) { if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room; } }],
    build: { outDir: out, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js" } }, logLevel: "error",
  });
  serverHttp = createServer((req, res) => {
    if (serveKatex(req, res)) return;
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
  b = await browser("chromium", join(temp, "profile"), 21000 + Math.floor(Math.random() * 1000));
  await b.resize(1400, 900);
  await b.navigate(`http://127.0.0.1:${serverHttp.address().port}/docs/paper`);
  await until("the source", () => b.evaluate('Boolean(document.querySelector(".cm-content"))'), 15000);

  const flush = () => b.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  const MODIFIER = { alt: 1, ctrl: 2, meta: 4, shift: 8 };
  // A chord, sent the way the browser sends one. `text` is what makes a key
  // insert a character: without it CodeMirror hears the keydown and types
  // nothing, which is exactly the difference this test is about.
  const key = async (value, { code = "", modifiers = 0, text = undefined, virtual = 0 } = {}) => {
    const held = Array.isArray(modifiers) ? modifiers.reduce((all, name) => all | MODIFIER[name], 0) : modifiers;
    await b.command("Input.dispatchKeyEvent", { type: "keyDown", key: value, code, modifiers: held, text, windowsVirtualKeyCode: virtual });
    await b.command("Input.dispatchKeyEvent", { type: "keyUp", key: value, code, modifiers: held, windowsVirtualKeyCode: virtual });
    await flush();
  };
  const dialog = () => b.evaluate(`(() => {
    const open = document.querySelector('[role="dialog"][data-state="open"]');
    return open ? (open.querySelector("[data-part=title], .modal-page-title")?.textContent || "").trim() : "";
  })()`);
  const focusIn = () => b.evaluate('document.activeElement?.closest("[id^=sidebar-panel-]")?.id || document.activeElement?.tagName || ""');
  const focusBody = () => b.evaluate('document.getElementById("main").focus()');
  const focusSource = () => b.evaluate('document.querySelector(".cm-content").focus()');
  const source = () => b.evaluate('document.querySelector(".cm-content").textContent');

  /* ------------------------------------------------- ? is the settings, and a letter */

  await focusBody();
  await key("?", { code: "Slash", modifiers: ["shift"], virtual: 191 });
  assert.equal(await dialog(), "Settings", "? on the page opens the settings");

  // The table is the registry drawn: keycaps, grouped, with the commands the
  // workspace is currently offering named. It lives on the Editor page of the
  // settings, under #editor-shortcuts.
  const table = await b.evaluate(`(() => {
    const open = document.querySelector('[role="dialog"][data-state="open"]');
    const row = open.querySelector("#editor-shortcuts");
    if (!row) return null;
    return {
      caps: [...row.querySelectorAll("kbd")].map(node => node.textContent),
      names: [...row.querySelectorAll("th[scope=row]")].map(node => node.textContent.trim()),
      groups: [...row.querySelectorAll("h3")].map(node => node.textContent.trim()),
    };
  })()`);
  assert.ok(table, "? opens the settings with the shortcut table on the Editor page");
  assert.ok(table.groups.includes("Navigation"), "the table is grouped: " + JSON.stringify(table.groups));
  assert.ok(table.names.includes("Command palette"), "and names the commands: " + JSON.stringify(table.names));
  assert.ok(table.caps.includes("Ctrl") && table.caps.includes("Alt"), "with this platform's modifier labels: " + JSON.stringify(table.caps));
  assert.ok(!table.caps.includes("⌘"), "and not the other platform's");

  await key("Escape", { code: "Escape", virtual: 27 });
  assert.equal(await dialog(), "", "Escape closes the settings");

  // The same key, in the source, is a question mark. This is the whole reason
  // the dispatcher asks whether a chord could have been typed.
  const before = await source();
  await focusSource();
  await key("?", { code: "Slash", modifiers: ["shift"], text: "?", virtual: 191 });
  assert.equal(await dialog(), "", "? in the source opens nothing");
  assert.equal(await source(), `?${before}`, "it types a question mark");

  /* ------------------------------------------- a chord nobody can type as text */

  const layout = () => b.evaluate('[...document.getElementById("main").classList].filter(name => name.startsWith("no-") || name === "editing").sort().join(" ")');
  await focusSource();
  const first = await layout();
  await key("\\", { code: "Backslash", modifiers: ["ctrl"], virtual: 220 });
  const second = await layout();
  assert.notEqual(second, first, "Ctrl+\\ cycles the layout from inside the editor");
  assert.equal(await source(), `?${before}`, "and types nothing");
  await key("\\", { code: "Backslash", modifiers: ["ctrl"], virtual: 220 });
  await key("\\", { code: "Backslash", modifiers: ["ctrl"], virtual: 220 });
  assert.equal(await layout(), first, "three presses walk the three arrangements and come home");

  /* ------------------------------------------------------- bold and italic */

  // The editor answers these itself, in the markup of the file's format.
  await focusSource();
  await key("a", { code: "KeyA", modifiers: ["ctrl"], virtual: 65 });
  await key("b", { code: "KeyB", modifiers: ["ctrl"], virtual: 66 });
  assert.equal(await source(), `**?${before}**`, "Ctrl+B wraps the selection in Markdown bold");
  await key("b", { code: "KeyB", modifiers: ["ctrl"], virtual: 66 });
  assert.equal(await source(), `?${before}`, "and a second press takes it off again");
  await key("i", { code: "KeyI", modifiers: ["ctrl"], virtual: 73 });
  assert.equal(await source(), `*?${before}*`, "Ctrl+I wraps it in italic");
  await key("i", { code: "KeyI", modifiers: ["ctrl"], virtual: 73 });
  assert.equal(await source(), `?${before}`, "and a second press takes it off again");

  /* --------------------------------------------------------- focusing a panel */

  await focusSource();
  await key("e", { code: "KeyE", modifiers: ["ctrl", "alt"], virtual: 69 });
  assert.equal(await focusIn(), "sidebar-panel-files", "Ctrl+Alt+E puts the keyboard in the Files panel");

  /* -------------------------------------------------------------- the palette */

  await focusSource();
  await key("p", { code: "KeyP", modifiers: ["ctrl", "alt"], virtual: 80 });
  assert.equal(await dialog(), "Run a command", "Ctrl+Alt+P opens the palette");

  for (const letter of ["o", "u", "t"]) await key(letter, { code: `Key${letter.toUpperCase()}`, text: letter, virtual: letter.toUpperCase().charCodeAt(0) });
  const offered = await b.evaluate('[...document.querySelectorAll(\'[role="option"]\')].map(node => node.querySelector(".result-name").textContent.trim())');
  assert.ok(offered.includes("Focus Outline"), "typing filters to what was asked for: " + JSON.stringify(offered));
  assert.ok(!offered.includes("Compile now"), "and leaves out what was not");

  // What the palette runs is what the shortcut runs, which is why this ends
  // with the focus in the panel rather than with a panel merely open.
  await key("Enter", { code: "Enter", virtual: 13 });
  await flush();
  await until("the outline", () => b.evaluate('document.activeElement?.closest("#sidebar-panel-outline") !== null'), 4000);
  assert.equal(await dialog(), "", "and the palette closes behind it");

  /* ----------------------------------------------------- the insert palettes */

  // Each opens over the editor with the keyboard where the palette starts, and
  // hands the keyboard back to the editor however it closes.
  const active = () => b.evaluate('document.activeElement?.matches("input[type=search]") ? "search" : document.activeElement?.closest(".cm-editor") ? "editor" : document.activeElement?.matches("[data-template]") ? "tile" : document.activeElement?.tagName || ""');
  await focusSource();
  await key("s", { code: "KeyS", modifiers: ["ctrl", "alt"], virtual: 83 });
  await until("the Symbols overlay", async () => (await dialog()) === "Insert symbol" && (await active()) === "search", 4000);
  const typed = await source();
  await b.evaluate('document.querySelector(".symbol-palette [data-symbol]").click()');
  await until("the symbol inserted", async () => (await source()) !== typed, 4000);
  // The picker stays open for the next symbol, and the focus stays out of the editor.
  assert.equal(await dialog(), "Insert symbol", "a picked symbol keeps the picker open");
  assert.notEqual(await active(), "editor", "and the focus is not handed to the editor");
  await key("Escape", { code: "Escape", virtual: 27 });
  await until("the Symbols overlay closed", async () => (await dialog()) === "" && (await active()) === "editor", 4000);

  await key("t", { code: "KeyT", modifiers: ["ctrl", "alt"], virtual: 84 });
  await until("the Math overlay", async () => (await dialog()) === "Math" && (await active()) === "tile", 4000);
  await b.evaluate('document.querySelector("[data-template]").click()');
  await until("a template form", () => b.evaluate('Boolean(document.querySelector(".math-form"))'), 4000);
  // The dialog hears Escape first. In a form, the press is the form's.
  await key("Escape", { code: "Escape", virtual: 27 });
  await until("back at the tiles", () => b.evaluate('!document.querySelector(".math-form")'), 4000);
  assert.equal(await dialog(), "Math", "Escape in a template form goes back to the tiles");
  await key("Escape", { code: "Escape", virtual: 27 });
  await until("the editor again", async () => (await dialog()) === "" && (await active()) === "editor", 4000);

  /* ------------------------------------------------------------ the settings */

  // The same table, in the other place it is offered: beside the choice of
  // editor keys, which is the setting that decides who owns half of it. The
  // same table, literally, and this is what says so.
  await focusBody();
  await key(",", { code: "Comma", modifiers: ["ctrl"], virtual: 188 });
  assert.equal(await dialog(), "Settings", "Ctrl+, opens the settings");
  const listed = await b.evaluate(`(() => {
    const row = document.getElementById("editor-shortcuts");
    if (!row) return null;
    return {
      names: [...row.querySelectorAll("th[scope=row]")].map(node => node.textContent.trim()),
      caps: [...row.querySelectorAll("kbd")].map(node => node.textContent),
    };
  })()`);
  assert.ok(listed, "the editor settings carry a shortcuts row");
  // The same commands in the same order with the same keys. Not the same
  // availability: a question mark was typed into the source since ? opened the
  // settings, so there is something to undo now and the table says so. That it
  // moved is the point -- this is the workspace as it stands, not a picture of
  // it taken when the page loaded.
  const plain = (names) => names.map((name) => name.replace(/\(not available here\)$/, ""));
  assert.deepEqual(plain(listed.names), plain(table.names), "it is the table ? opened, not a second copy of it");
  assert.deepEqual(listed.caps, table.caps);
  assert.ok(table.names.includes("Undo(not available here)"), "nothing had been typed when ? opened the settings");
  assert.ok(listed.names.includes("Undo"), "and something had by the time Ctrl+, opened them");
  await key("Escape", { code: "Escape", virtual: 27 });
  assert.equal(await dialog(), "");

  /* ------------------------------------------------- what the browser keeps */

  // Ctrl+T is the browser's. Nothing in the page may answer it, and nothing
  // may cancel it either.
  await focusBody();
  // Registered without capture and after the reader's own handler, so it reads
  // `defaultPrevented` as the browser will: whatever the page did, it did
  // before this runs.
  await b.evaluate(`(() => {
    let prevented = null;
    const listen = event => { prevented = event.defaultPrevented; };
    window.addEventListener("keydown", listen);
    window.__claimed = () => prevented;
  })()`);
  await key("t", { code: "KeyT", modifiers: ["ctrl"], virtual: 84 });
  assert.equal(await dialog(), "", "Ctrl+T opens nothing of ours");
  assert.equal(await b.evaluate("window.__claimed()"), false, "and nothing cancels it");

  // While a chord we do claim is cancelled, so the browser does not also act
  // on it. The two assertions are one rule read from both sides.
  await key("\\", { code: "Backslash", modifiers: ["ctrl"], virtual: 220 });
  assert.equal(await b.evaluate("window.__claimed()"), true, "a chord we answer is cancelled");

  assert.deepEqual(await b.evaluate("window.testErrors"), []);
  console.log("shortcuts-browser: ? opens the shortcut table in the settings from the page, and is a letter in the source, ⌃\\ cycles from the editor, ⌃B and ⌃I toggle emphasis, panels take the focus, the palette runs what it offers, the insert palettes open over the editor and give it back, and the settings list the same table");
} finally {
  await b?.close();
  serverHttp?.close();
  removeTemporary(temp);
}
