// Editor bundle workflow: source synchronization precedes metadata,
// bundle is explicit, and an edit during upload remains unpublished.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-bundle-editor-"));
const entry = join(temp, "entry.js"), room = join(temp, "room.js"), out = join(temp, "build");

writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = bytes => btoa(String.fromCharCode(...bytes));
const decode = value => Uint8Array.from(atob(value), char => char.charCodeAt(0));
const server = new LoroDoc(), files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
const saved = localStorage.getItem("bundle-editor-state");
if (saved) server.import(decode(saved));
let text = files.get("main");
if (text?.kind?.() !== "Text") {
  text = new LoroText(); text.insert(0, "<h1>Published editor</h1><p>first version</p>");
  files.setContainer("main", text); paths.set("main", "main.html"); meta.set("main", "main");
  server.commit();
}
const persist = () => localStorage.setItem("bundle-editor-state", encode(server.export({ mode: "update" })));
persist();
let callback = null, initial = true;
let updateSub = server.subscribeLocalUpdates(update => { persist(); if (!initial && callback) callback({type:"doc-update", update:encode(update)}); });
export function openRoom(_slug, {onMessage, onConnected}) {
  callback = onMessage; window.roomSent = []; window.roomControl = {
    append(value) { text.insert(text.length, value); server.commit(); }, source() { return text.toString(); }
  };
  queueMicrotask(() => onConnected(true));
  return {
    send(message) {
      window.roomSent.push(message);
      if (message.type === "doc-open") queueMicrotask(() => { initial = false; onMessage({type:"doc-state",update:encode(server.export({ mode: "update" })),count:1}); });
      if (message.type === "doc-update") server.import(decode(message.update));
      return {ok:true};
    },
    sendLive(message) { window.roomSent.push(message); return {ok:true}; }, close() { callback = null; updateSub?.(); }
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
let metadataReady = false, bundle = null, prepared = null, activateBody = null, activations = 0;
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
  await build({ configFile:false, root:join(root, "web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {
    name:"bundle-editor-room", enforce:"pre", resolveId(id) {
      if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
    },
  }], build:{outDir:out,emptyOutDir:true,minify:false,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });

  http = createServer(async (request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/api/me") return json(response, {name:"Editor",providers:[]});
    if (url.pathname === "/api/documents/paper") return json(response, {
      title:"Editor bundle", created_at:"editor-bundle", role:"owner", source_format:"html",
      docs_origin:`http://${request.headers.host}`, can_see_sharing:true, can_moderate:true,
    });
    if (url.pathname === "/api/documents/paper/share") return json(response, {links:{}});
    if (url.pathname === "/api/documents/paper/bundle" && request.method === "GET") {
      if (!metadataReady) await waitForMetadata();
      return json(response, {bundle});
    }
    if (url.pathname === "/api/documents/paper/bundle/prepare") {
      prepared = JSON.parse(await body(request)).manifest;
      return json(response, {missing:[prepared.html.sha256]});
    }
    if (/^\/api\/documents\/paper\/bundle\/objects\//.test(url.pathname)) {
      await body(request); return json(response, {}, 201);
    }
    if (url.pathname === "/api/documents/paper/bundle/activate") {
      activateBody = JSON.parse(await body(request));
      activations += 1;
      if (activations === 2) await waitForUpload();
      bundle = {id:`pub-${activations}`, published_at:"2026-01-01T00:00:00Z", publisher:"Editor",
        source_sha256:activateBody.manifest.source_sha256,
        render_config_sha256:activateBody.manifest.render_config_sha256};
      return json(response, {bundle});
    }
    if (url.pathname.startsWith("/raw/")) { response.setHeader("content-type", "text/html"); return response.end("<!doctype html><body></body>"); }
    if (url.pathname === "/docs/paper") {
      response.setHeader("content-type", "text/html");
      return response.end('<!doctype html><html data-theme="librepaper"><body><script type="module" src="/check.js"></script></body></html>');
    }
    const file = join(out, url.pathname.slice(1));
    try { response.setHeader("content-type", contentType(file)); response.end(readFileSync(file)); }
    catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve, reject) => { http.once("error", reject); http.listen(0, "127.0.0.1", resolve); });
  const origin = `http://127.0.0.1:${http.address().port}`;
  tab = await browser("chromium", join(temp, "profile"), 35000 + Math.floor(Math.random() * 1000));
  await tab.resize(1280, 900); await tab.navigate(`${origin}/docs/paper`);
  const flush = () => tab.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
  await until("source state", () => tab.evaluate("window.roomSent?.some(message=>message.type==='doc-open')"), 10000);
  await flush();
  await tab.evaluate("document.querySelector('.sidebar-activity [aria-label=Share]').click()"); await flush();
  await until("metadata request", () => Promise.resolve(typeof metadataRelease === "function"), 10000);
  assert.equal(await tab.evaluate("document.body.innerText.includes('Checking the shared view')"), true, "Share says the shared view is still being checked");
  assert.equal(await tab.evaluate("[...document.querySelectorAll('button')].some(button=>button.textContent.trim()==='Publish')"), false, "metadata has to establish the expected bundle before publishing can start");
  assert.equal(await tab.evaluate("window.fetchRequests.some(request=>request.url.includes('/bundle/prepare'))"), false, "metadata and editor startup do not publish");

  metadataReady = true; metadataRelease();
  await until("bundle controls", () => tab.evaluate("document.body.innerText.includes('Not published yet.')"), 10000);
  assert.match(await tab.evaluate("document.body.innerText"), /Not published yet/);
  await tab.evaluate("[...document.querySelectorAll('button')].find(button=>button.textContent.trim()==='Publish').click()");
  await until("initial activation", () => Promise.resolve(activations === 1), 10000);
  await until("published without edits", () => tab.evaluate("document.body.innerText.includes('Published') && !document.body.innerText.includes('Unpublished changes')"), 10000);
  assert.equal(activateBody.expected_bundle_id, null, "first explicit publish has no expected predecessor");
  await tab.navigate(`${origin}/docs/paper`);
  await until("reloaded source state", () => tab.evaluate("window.roomSent?.some(message=>message.type==='doc-open')"), 10000);
  await tab.evaluate("document.querySelector('.sidebar-activity [aria-label=Share]').click()"); await flush();
  try {
    await until("reloaded unchanged bundle", () => tab.evaluate("[...document.querySelectorAll('button')].some(button=>button.textContent.trim()==='Published' || button.textContent.trim()==='Publish update')"), 10000);
  } catch (error) {
    const browserState = await tab.evaluate(`JSON.stringify({
      text: document.body.innerText.slice(0, 2000), errors: window.testErrors,
      requests: window.fetchRequests, source: window.roomControl?.source()
    })`);
    throw new Error(`${error.message}; bundle=${JSON.stringify(bundle)} metadataReady=${metadataReady} activations=${activations}; browser=${browserState}`);
  }
  assert.equal(await tab.evaluate("document.body.innerText.includes('Unpublished changes')"), false, "an unchanged source stays published after reload");

  const uploadsBeforeEdit = await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/bundle/prepare')).length");
  const sourceAfterFirstEdit = await tab.evaluate("window.roomControl.append('<p>first update</p>'); window.roomControl.source()");
  try {
    await until("source edit status", () => tab.evaluate("document.body.innerText.includes('Unpublished changes')"), 10000);
  } catch (error) {
    const browserState = await tab.evaluate(`JSON.stringify({
      text: document.body.innerText.slice(0, 2000), errors: window.testErrors,
      requests: window.fetchRequests, source: window.roomControl?.source()
    })`);
    throw new Error(`${error.message}; bundle=${JSON.stringify(bundle)} metadataReady=${metadataReady} activations=${activations}; appended=${sourceAfterFirstEdit}; browser=${browserState}`);
  }
  assert.equal(await tab.evaluate("window.fetchRequests.filter(request=>request.url.includes('/bundle/prepare')).length"), uploadsBeforeEdit, "editing and local preview do not publish automatically");
  await tab.evaluate("[...document.querySelectorAll('button')].find(button=>button.textContent.trim()==='Publish update').click()");
  await until("update activation", () => Promise.resolve(activations === 2), 10000);
  await tab.evaluate("window.roomControl.append('<p>edited during upload</p>')"); await flush();
  uploadRelease();
  await until("unpublished indicator", () => tab.evaluate("document.body.innerText.includes('Unpublished changes')"), 10000);
  assert.equal(await tab.evaluate("window.fetchRequests.some(request=>request.url.includes('/bundle/prepare'))"), true, "the Share action starts bundle");
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
  console.log("bundle-editor-browser: explicit editor bundle and captured-source status passed");
} finally {
  await tab?.close();
  await new Promise(resolve => http ? http.close(resolve) : resolve());
  rmSync(temp, {recursive:true,force:true});
}
