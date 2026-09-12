// Editor publication workflow: source synchronization precedes metadata,
// publication is explicit, and an edit during upload remains unpublished.
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

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-publication-editor-"));
const entry = join(temp, "entry.js"), room = join(temp, "room.js"), out = join(temp, "build");
const yjs = join(root, "web/node_modules/yjs/dist/yjs.mjs");

writeFileSync(room, `
import * as Y from ${JSON.stringify(yjs)};
const encode = bytes => btoa(String.fromCharCode(...bytes));
const decode = value => Uint8Array.from(atob(value), char => char.charCodeAt(0));
const server = new Y.Doc(), files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
const saved = localStorage.getItem("publication-editor-state");
if (saved) Y.applyUpdate(server, decode(saved));
let text = files.get("main");
if (!(text instanceof Y.Text)) {
  text = new Y.Text(); text.insert(0, "<h1>Published editor</h1><p>first version</p>");
  files.set("main", text); paths.set("main", "main.html"); meta.set("main", "main");
}
const persist = () => localStorage.setItem("publication-editor-state", encode(Y.encodeStateAsUpdate(server)));
persist();
let callback = null, initial = true;
server.on("update", update => { persist(); if (!initial && callback) callback({type:"y-update", update:encode(update)}); });
export function openRoom(_slug, {onMessage, onConnected}) {
  callback = onMessage; window.roomSent = []; window.roomControl = {
    append(value) { text.insert(text.length, value); }, source() { return text.toString(); }
  };
  queueMicrotask(() => onConnected(true));
  return {
    send(message) {
      window.roomSent.push(message);
      if (message.type === "y-open") queueMicrotask(() => { initial = false; onMessage({type:"y-state",update:encode(Y.encodeStateAsUpdate(server)),count:1}); });
      if (message.type === "y-update") Y.applyUpdate(server, decode(message.update), "browser");
      return {ok:true};
    },
    sendLive(message) { window.roomSent.push(message); return {ok:true}; }, close() { callback = null; }
  };
}`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
window.testErrors=[];
addEventListener("error", event => window.testErrors.push(event.error?.stack || event.message));
addEventListener("unhandledrejection", event => window.testErrors.push(String(event.reason)));
const nativeFetch=fetch.bind(globalThis); window.fetchRequests=[];
globalThis.fetch=(url,init={})=>{ window.fetchRequests.push({url:String(url),method:init.method||"GET"}); return nativeFetch(url,init); };
const {default: Reader}=await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
const {mount}=await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
mount(Reader,{target:document.body});`);

let http, tab, metadataRelease, uploadRelease;
let metadataReady = false, publication = null, prepared = null, activateBody = null, activations = 0;
const waitForMetadata = () => new Promise(resolve => { metadataRelease = resolve; });
const waitForUpload = () => new Promise(resolve => { uploadRelease = resolve; });
async function body(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return Buffer.concat(chunks).toString();
}
function json(response, value, status = 200) {
  response.statusCode = status; response.setHeader("content-type", "application/json"); response.end(JSON.stringify(value));
}

try {
  await build({ configFile:false, root:join(root, "web"), plugins:[tailwindcss(), svelte(), {
    name:"publication-editor-room", enforce:"pre", resolveId(id) {
      if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
    },
  }], build:{outDir:out,emptyOutDir:true,minify:false,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });

  http = createServer(async (request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/api/me") return json(response, {name:"Editor",providers:[]});
    if (url.pathname === "/api/documents/paper") return json(response, {
      title:"Editor publication", created_at:"editor-publication", role:"owner", source_format:"html",
      docs_origin:`http://${request.headers.host}`, can_see_sharing:true, can_moderate:true,
    });
    if (url.pathname === "/api/documents/paper/share") return json(response, {links:{}});
    if (url.pathname === "/api/documents/paper/publication" && request.method === "GET") {
      if (!metadataReady) await waitForMetadata();
      return json(response, {publication});
    }
    if (url.pathname === "/api/documents/paper/publication/prepare") {
      prepared = JSON.parse(await body(request)).manifest;
      return json(response, {missing:[prepared.html.sha256]});
    }
    if (/^\/api\/documents\/paper\/publication\/objects\//.test(url.pathname)) {
      await body(request); return json(response, {}, 201);
    }
    if (url.pathname === "/api/documents/paper/publication/activate") {
      activateBody = JSON.parse(await body(request));
      activations += 1;
      if (activations === 2) await waitForUpload();
      publication = {id:`pub-${activations}`, published_at:"2026-01-01T00:00:00Z", publisher:"Editor",
        source_sha256:activateBody.manifest.source_sha256,
        render_config_sha256:activateBody.manifest.render_config_sha256};
      return json(response, {publication});
    }
    if (url.pathname.startsWith("/raw/")) { response.setHeader("content-type", "text/html"); return response.end("<!doctype html><body></body>"); }
    if (url.pathname === "/docs/paper") {
      response.setHeader("content-type", "text/html");
      return response.end('<!doctype html><html data-theme="librepaper"><body><script type="module" src="/check.js"></script></body></html>');
    }
    const file = join(out, url.pathname.slice(1));
    try { response.setHeader("content-type", file.endsWith(".css") ? "text/css" : "text/javascript"); response.end(readFileSync(file)); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve, reject) => { http.once("error", reject); http.listen(0, "127.0.0.1", resolve); });
  const origin = `http://127.0.0.1:${http.address().port}`;
  tab = await browser("chromium", join(temp, "profile"), 35000 + Math.floor(Math.random() * 1000));
  await tab.resize(1280, 900); await tab.navigate(`${origin}/docs/paper`);
  const flush = () => tab.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
  await until("source state", () => tab.evaluate("window.roomSent?.some(message=>message.type==='y-open')"), 10000);
  await flush();
  await tab.evaluate("document.querySelector('.sidebar-activity [aria-label=Share]').click()"); await flush();
  await until("metadata request", () => Promise.resolve(typeof metadataRelease === "function"), 10000);
  assert.equal(await tab.evaluate("document.body.innerText.includes('Checking published version')"), true, "Share shows that publication metadata is still being checked");
  assert.equal(await tab.evaluate("[...document.querySelectorAll('button')].some(button=>button.textContent.trim()==='Publish')"), false, "metadata has to establish the expected publication before publishing can start");
  assert.equal(await tab.evaluate("window.fetchRequests.some(request=>request.url.includes('/publication/prepare'))"), false, "metadata and editor startup do not publish");

  metadataReady = true; metadataRelease();
  await until("publication controls", () => tab.evaluate("document.body.innerText.includes('Not published yet.')"), 10000);
  assert.match(await tab.evaluate("document.body.innerText"), /Not published yet/);
  await tab.evaluate("[...document.querySelectorAll('button')].find(button=>button.textContent.trim()==='Publish').click()");
  await until("initial activation", () => Promise.resolve(activations === 1), 10000);
  await until("published without edits", () => tab.evaluate("document.body.innerText.includes('Published') && !document.body.innerText.includes('Unpublished changes')"), 10000);
  assert.equal(activateBody.expected_publication_id, null, "first explicit publish has no expected predecessor");
  await tab.navigate(`${origin}/docs/paper`);
  await until("reloaded source state", () => tab.evaluate("window.roomSent?.some(message=>message.type==='y-open')"), 10000);
  await tab.evaluate("document.querySelector('.sidebar-activity [aria-label=Share]').click()"); await flush();
  try {
    await until("reloaded unchanged publication", () => tab.evaluate("[...document.querySelectorAll('button')].some(button=>button.textContent.trim()==='Published' || button.textContent.trim()==='Publish update')"), 10000);
  } catch (error) {
    const browserState = await tab.evaluate(`JSON.stringify({
      text: document.body.innerText.slice(0, 2000), errors: window.testErrors,
      requests: window.fetchRequests, source: window.roomControl?.source()
    })`);
    throw new Error(`${error.message}; publication=${JSON.stringify(publication)} metadataReady=${metadataReady} activations=${activations}; browser=${browserState}`);
  }
  assert.equal(await tab.evaluate("document.body.innerText.includes('Unpublished changes')"), false, "an unchanged source stays published after reload");

  const uploadsBeforeEdit = await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/publication/prepare')).length");
  const sourceAfterFirstEdit = await tab.evaluate("window.roomControl.append('<p>first update</p>'); window.roomControl.source()");
  try {
    await until("source edit status", () => tab.evaluate("document.body.innerText.includes('Unpublished changes')"), 10000);
  } catch (error) {
    const browserState = await tab.evaluate(`JSON.stringify({
      text: document.body.innerText.slice(0, 2000), errors: window.testErrors,
      requests: window.fetchRequests, source: window.roomControl?.source()
    })`);
    throw new Error(`${error.message}; publication=${JSON.stringify(publication)} metadataReady=${metadataReady} activations=${activations}; appended=${sourceAfterFirstEdit}; browser=${browserState}`);
  }
  assert.equal(await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/publication/prepare')).length"), uploadsBeforeEdit, "editing and local preview do not publish automatically");
  await tab.evaluate("[...document.querySelectorAll('button')].find(button=>button.textContent.trim()==='Publish update').click()");
  await until("update activation", () => Promise.resolve(activations === 2), 10000);
  await tab.evaluate("window.roomControl.append('<p>edited during upload</p>')"); await flush();
  uploadRelease();
  await until("unpublished indicator", () => tab.evaluate("document.body.innerText.includes('Unpublished changes')"), 10000);
  assert.equal(await tab.evaluate("window.fetchRequests.some(request=>request.url.includes('/publication/prepare'))"), true, "the Share action starts publication");
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
  console.log("publication-editor-browser: explicit editor publication and captured-source status passed");
} finally {
  await tab?.close();
  await new Promise(resolve => http ? http.close(resolve) : resolve());
  rmSync(temp, {recursive:true,force:true});
}
