// Dark mode on the real reader shell: the default, the choice, first paint on
// reload, following the system scheme, and the Settings select.
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

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-theme-"));
const out = join(temp, "build");

const room = join(temp, "room.js");
writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
const text = files.setContainer("main", new LoroText()); text.insert(0, "A paragraph of source.");
paths.set("main", "main.html"); meta.set("main", "main");
server.commit();
const update = encode(server.export({ mode: "update" }));
const vector = encode(server.oplogVersion().encode());
export function openRoom(slug, {onMessage, onConnected}) {
  queueMicrotask(() => onConnected(true));
  return {
    send(message) { if (message.type === "doc-open") queueMicrotask(() => onMessage({type:"doc-state", protocol:"librepaper.room.v3", vector, durableVector: vector, updates:[update]})); return {ok:true}; },
    sendLive() { return {ok:true}; },
    close() {},
  };
}
`);

const entry = join(temp, "reader.js");
writeFileSync(entry, `
  import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
  globalThis.WebSocket = class { constructor() { queueMicrotask(() => this.onopen?.()); } send() {} close() { this.onclose?.(); } };
  globalThis.fetch = async (url) => {
    const path = String(url);
    if (path.endsWith('/me')) return Response.json({ name: 'Tester', providers: [] });
    if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md'], text_extensions: ['.html', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
    if (path === '/api/documents/paper') return Response.json({ title: 'A paper', created_at: 'test', role: 'editor', source_format: 'html', docs_origin: location.origin, can_moderate: true, can_see_sharing: true });
    if (path.endsWith('/frame')) return Response.json({ token: 'frame-token', until: 9999999999 });
    if (path.endsWith('/chat')) return Response.json({ id: 'chat', token: 'private' });
    if (path.includes('/comments')) return { ok: true, json: async () => ({ comments: [] }) };
    return { ok: false, json: async () => ({}) };
  };
  window.theme = await import(${JSON.stringify(join(root, "web/src/lib/theme.js"))});
  const { default: Reader } = await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
  const { mount } = await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
  mount(Reader, { target: document.body });
`);
await build({
  configFile: false, root: join(root, "web"), resolve: { alias: loroAlias },
  plugins: [tailwindcss(), svelte(), { name: "mock-room", enforce: "pre", resolveId(id) { if (/(^|\/)room\.js$/.test(id)) return room; } }],
  build: { outDir: out, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js" } },
  logLevel: "error",
});

const shell = readFileSync(join(root, "web/pages/reader.html"), "utf8")
  .replace("../src/entries/reader.js", "/check.js")
  .replace("__MODULES__", "{}");
const firstPaint = readFileSync(join(root, "web/public/theme.js"), "utf8");
const hits = [];
const server = createServer((request, response) => {
  const path = new URL(request.url, "http://127.0.0.1").pathname;
  hits.push(path);
  if (path === "/docs/paper") { response.setHeader("content-type", "text/html"); response.end(shell); return; }
  if (path === "/theme.js") { response.setHeader("content-type", "text/javascript"); response.end(firstPaint); return; }
  try {
    response.setHeader("content-type", contentType(path));
    response.end(readFileSync(join(out, path.slice(1))));
  } catch { response.statusCode = 404; response.end(); }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const url = `http://127.0.0.1:${server.address().port}/docs/paper`;

const luminance = (rgb) => {
  const [r, g, b] = rgb.match(/[\d.]+/g).slice(0, 3).map((n) => {
    const c = Number(n) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

let tab;
try {
  tab = await browser("chromium", join(temp, "profile"), 27000 + Math.floor(Math.random() * 1000));
  const open = async () => {
    await tab.navigate(url);
    await tab.resize(1280, 900);
    await until("editor ready", () => tab.evaluate("Boolean(document.querySelector('.cm-editor')) && Boolean(window.theme)"), 20000);
  };
  const mode = () => tab.evaluate("document.documentElement.dataset.mode");
  const paint = () => tab.evaluate(`({
    body: getComputedStyle(document.body).backgroundColor,
    text: getComputedStyle(document.querySelector('.cm-editor')).color,
  })`);

  // The page is served from an origin with nothing stored and the default scheme.
  await open();
  assert.equal(await mode(), "light");
  assert.equal(await tab.evaluate("localStorage.getItem('librepaper-theme')"), null);
  const light = await paint();
  console.log("theme-browser: default is light and stores nothing");

  await tab.evaluate("window.theme.setChoice('dark')");
  assert.equal(await mode(), "dark");
  const dark = await paint();
  assert.notEqual(dark.body, light.body);
  assert.ok(luminance(dark.body) < 0.3, `dark body background is dark, got ${dark.body}`);
  assert.ok(luminance(dark.text) > 0.6, `dark editor text is light, got ${dark.text}`);
  assert.equal(await tab.evaluate("localStorage.getItem('librepaper-theme')"), '"dark"');
  console.log("theme-browser: dark repaints the shell and the editor and is stored");

  // The first evaluate after load runs before any later work, so a mode that is
  // already set here came from the first-paint script or the bundle's start.
  hits.length = 0;
  await tab.navigate(url);
  assert.equal(await mode(), "dark");
  assert.ok(hits.includes("/theme.js"), "the page requests /theme.js");
  console.log("theme-browser: a reload keeps dark and loads the first-paint script");

  // Chromium's own emulated scheme stands in for the operating system.
  const emulate = (scheme) => tab.command("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
  await until("editor ready", () => tab.evaluate("Boolean(document.querySelector('.cm-editor')) && Boolean(window.theme)"), 20000);
  await tab.evaluate("window.theme.setChoice('light')");
  assert.equal(await mode(), "light");
  await emulate("dark");
  await tab.evaluate("window.theme.setChoice('system')");
  await until("system dark", async () => (await mode()) === "dark", 5000);
  await emulate("light");
  await until("system flips to light", async () => (await mode()) === "light", 5000);
  await emulate("dark");
  await until("system flips to dark again", async () => (await mode()) === "dark", 5000);
  await emulate("light");
  console.log("theme-browser: System follows the emulated scheme without a reload");

  // Settings opens on Appearance, the first category.
  await tab.evaluate("window.theme.setChoice('light')");
  await tab.evaluate('window.dispatchEvent(new CustomEvent("librepaper:open-settings"))');
  const select = "#appearance-theme select[aria-label=Theme]";
  await until("theme select visible", () => tab.evaluate(`document.querySelector(${JSON.stringify(select)})?.checkVisibility() === true`), 5000);
  await tab.evaluate(`(() => {
    const node = document.querySelector(${JSON.stringify(select)});
    node.value = "dark";
    node.dispatchEvent(new Event("input", { bubbles: true }));
    node.dispatchEvent(new Event("change", { bubbles: true }));
  })()`);
  await until("settings select sets dark", async () => (await mode()) === "dark", 5000);
  console.log("theme-browser: the Settings Theme select switches to dark");
} finally {
  await tab?.close();
  server.close();
  removeTemporary(temp);
}
