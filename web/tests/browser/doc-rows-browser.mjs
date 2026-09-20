// A join where the client's vector does not cover everything (§6.2 step 4)
// answers with `doc-state` for the base and a separate `doc-rows` frame for
// whatever came after it. That is the common join path, not an edge case --
// most returning tabs already hold the base locally and are only ever
// missing rows.
//
// Reader.svelte's `doc-rows` handler used to call `.then()` on
// `session.rows(event)`, which is synchronous and returns undefined. That
// threw a TypeError on every `doc-rows` frame, before the preview repaint or
// the error handler ever ran. This test drives a real `doc-rows` frame at
// the mounted Reader and checks both that the text it carried actually
// lands, and that nothing was thrown along the way.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-doc-rows-"));
const entry = join(temp, "entry.js"), room = join(temp, "room.js"), out = join(temp, "build");
const BASE_TEXT = "<p>base draft</p>";
const ROWS_ADDITION = "<p>row that only doc-rows carries</p>";

// The server builds its document in two commits: the first is what `doc-state`
// sends as `base`, the second is what only `doc-rows` ever carries. A client
// whose join vector covers the first commit but not the second is exactly the
// case SPEC-server-is-a-log §6.2 describes.
writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = bytes => btoa(String.fromCharCode(...bytes));
const decode = value => Uint8Array.from(atob(value), char => char.charCodeAt(0));
const server = new LoroDoc();
const text = server.getMap('files').setContainer('main', new LoroText());
text.insert(0, ${JSON.stringify(BASE_TEXT)});
server.getMap('paths').set('main', 'main.html');
server.getMap('meta').set('main', 'main');
server.commit();
const base = server.export({ mode: "update" });
const afterBase = server.oplogVersion();
text.insert(text.length, ${JSON.stringify(ROWS_ADDITION)});
server.commit();
export function openRoom(_slug, {onMessage, onConnected}) {
  queueMicrotask(() => onConnected(true));
  return { send(message) {
    if (message.type === 'doc-open') {
      // doc-state carries only the base commit's vector -- the second commit
      // is what the following doc-rows frame is for, which is the split
      // this test exists to exercise.
      onMessage({type:'doc-state', protocol:'librepaper.room.v2', vector:encode(afterBase.encode()), base:encode(base), updates:[]});
      setTimeout(() => onMessage({type:'doc-rows', vector:encode(server.oplogVersion().encode()), updates:[encode(server.export({ mode: "update", from: afterBase }))]}), 30);
    }
    return {ok:true};
  }, sendLive() { return {ok:true}; }, close() {} };
}`);
writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
window.testErrors=[];
addEventListener('error', event => window.testErrors.push(event.error?.stack || event.message));
addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
const {default: Reader}=await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
const {mount}=await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
mount(Reader,{target:document.body});
`);

let http, tab;
const json = (response, value, status = 200) => {
  response.statusCode = status; response.setHeader("content-type", "application/json"); response.end(JSON.stringify(value));
};
try {
  await build({ configFile: false, root: join(root, "web"), resolve: { alias: loroAlias }, plugins: [tailwindcss(), svelte(), {
    name: "doc-rows-test-room", enforce: "pre", resolveId(id) {
      if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
    },
  }], build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" } }, logLevel: "error" });
  http = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/api/me") return json(response, { name: "Editor", providers: [] });
    if (url.pathname === "/api/documents/paper") return json(response, {
      title: "doc-rows test", created_at: "doc-rows-test", role: "owner", source_format: "html",
      docs_origin: `http://${request.headers.host}`, can_see_sharing: true, can_moderate: true,
    });
    if (url.pathname === "/api/documents/paper/share") return json(response, { links: {} });
    if (url.pathname === "/api/documents/paper/comments") return json(response, { comments: [] });
    if (url.pathname.startsWith("/raw/")) { response.setHeader("content-type", "text/html"); return response.end("<!doctype html><body></body>"); }
    if (url.pathname === "/docs/paper") {
      response.setHeader("content-type", "text/html");
      return response.end('<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/check.css"></head><body><script type="module" src="/check.js"></script></body></html>');
    }
    try {
      const file = join(out, url.pathname.slice(1));
      response.setHeader("content-type", contentType(file)); response.end(readFileSync(file));
    } catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve, reject) => { http.once("error", reject); http.listen(0, "127.0.0.1", resolve); });
  const origin = `http://127.0.0.1:${http.address().port}`;
  tab = await browser("chromium", join(temp, "profile"), 36000 + Math.floor(Math.random() * 1000));
  await tab.navigate(`${origin}/docs/paper`);
  // The base lands from doc-state first; this only proves the join started.
  await until("base source visible", () => tab.evaluate(`document.querySelector(".cm-content")?.textContent.includes("base draft")`), 20000);
  // The addition only exists in the doc-rows frame's update. If `rows()`
  // threw before importing it, this text never arrives and the wait times
  // out -- which is the failure this test is meant to catch.
  await until("doc-rows addition visible", () => tab.evaluate(`document.querySelector('.cm-content')?.textContent.includes('row that only doc-rows carries')`), 10000);
  assert.deepEqual(await tab.evaluate("window.testErrors"), [], "doc-rows must not throw while it is imported");
  console.log("doc-rows: a join answered with doc-state then doc-rows imports both without throwing");
} catch (error) {
  if (tab) console.error(await tab.evaluate(`JSON.stringify({errors:window.testErrors,source:[...document.querySelectorAll('.cm-content')].map(node=>node.textContent)})`));
  throw error;
} finally {
  await tab?.close(); http?.close(); rmSync(temp, { recursive: true, force: true });
}
