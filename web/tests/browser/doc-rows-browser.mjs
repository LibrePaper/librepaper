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
const collab = join(temp, "collab.js");
const BASE_TEXT = "<p>base draft</p>";
const ROWS_ADDITION = "<p>row that only doc-rows carries</p>";

// The server builds its document in two commits: the first is what `doc-state`
// sends as `base`, the second is what only `doc-rows` ever carries. A client
// whose join vector covers the first commit but not the second is exactly the
// case SPEC-server-is-a-log §6.2 describes.
writeFileSync(room, `
import { LoroDoc, LoroText, decodeImportBlobMeta } from "loro-crdt";
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
const durableVector = encode(afterBase.encode());
text.insert(text.length, ${JSON.stringify(ROWS_ADDITION)});
server.commit();
let opens = 0;
let localVector = "";
let relayedRemote = false;
let catchupSeq = 0;
let ackedSeq = 0;
let catchupWasEmpty = false;
let reconnecting = false;
const serverText = () => server.getMap("files").get("main")?.toString() || "";
export function openRoom(_slug, {onMessage, onConnected}) {
  window.roomControl = {
    drop() { reconnecting = true; onConnected(false); },
    resume() { onConnected(true); },
    confirmLocal() { onMessage({ type: "doc-durable", vector: localVector }); },
    state() { return { opens, localVector, headVector: encode(server.oplogVersion().encode()), serverText: serverText(), relayedRemote, catchupSeq, ackedSeq, catchupWasEmpty }; },
  };
  queueMicrotask(() => onConnected(true));
  return { send(message) {
    if (message.type === 'doc-open') {
      opens += 1;
      // doc-state carries only the base commit's vector -- the second commit
      // is what the following doc-rows frame is for, which is the split
      // this test exists to exercise.
      if (!reconnecting) {
        onMessage({type:'doc-state', protocol:'librepaper.room.v3', vector:encode(afterBase.encode()), durableVector, base:encode(base), updates:[]});
        setTimeout(() => onMessage({type:'doc-rows', protocol:'librepaper.room.v3', vector:encode(server.oplogVersion().encode()), durableVector, updates:[encode(server.export({ mode: "update", from: afterBase }))]}), 30);
      } else {
        // The server already has the buffered local edit, so this catch-up is
        // empty. Its head includes that edit while durableVector still names
        // only the persisted baseline.
        onMessage({type:'doc-state', protocol:'librepaper.room.v3', vector:encode(server.oplogVersion().encode()), durableVector, updates:[]});
      }
    } else if (message.type === "doc-update") {
      const update = decode(message.update);
      server.import(update);
      // Leave the first local target frozen. The second update is the empty
      // catch-up sent after reconnect; acknowledge precisely that sequence so
      // the old ack-based client clears its queue, while durableVector still
      // proves the local target was not persisted.
      if (!reconnecting && !localVector && serverText().includes("local buffered")) localVector = encode(server.oplogVersion().encode());
      if (reconnecting) {
        catchupSeq = message.seq;
        catchupWasEmpty = decodeImportBlobMeta(update, true).changeNum === 0;
        queueMicrotask(() => {
          ackedSeq = message.seq;
          onMessage({type:'doc-ack', upTo:message.seq});
          if (!relayedRemote) {
            // Relay only after the empty catch-up, so that export cannot
            // accidentally include this peer's newer work.
            const peer = new LoroDoc();
            peer.import(server.export({ mode: "update" }));
            const peerText = peer.getMap("files").get("main");
            peerText.insert(peerText.length, " <p>remote editor newer</p>");
            peer.commit();
            const remote = peer.export({ mode: "update", from: server.oplogVersion() });
            server.import(remote);
            relayedRemote = true;
            onMessage({type:'doc-update', update:encode(remote)});
          }
        });
      }
    }
    return {ok:true};
  }, sendLive() { return {ok:true}; }, close() {} };
}`);
// Observe the real session's persistence boundary without changing how Reader
// creates, hydrates, or synchronizes it.
writeFileSync(collab, `
export * from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import { join as realJoin } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
export function join(options) {
  const session = realJoin({ ...options, onState(state) {
    window.collabState = state;
    options.onState?.(state);
  } });
  window.testSession = session;
  return session;
}
`);
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
      if (id === "../lib/collab.js") return collab;
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
  await tab.resize(1400, 900);
  await tab.navigate(`${origin}/docs/paper`);
  // The base lands from doc-state first; this only proves the join started.
  await until("base source visible", () => tab.evaluate(`document.querySelector(".cm-content")?.textContent.includes("base draft")`), 20000);
  // The addition only exists in the doc-rows frame's update. If `rows()`
  // threw before importing it, this text never arrives and the wait times
  // out -- which is the failure this test is meant to catch.
  await until("doc-rows addition visible", () => tab.evaluate(`document.querySelector('.cm-content')?.textContent.includes('row that only doc-rows carries')`), 10000);
  const sourcePoint = await tab.evaluate(`(() => { const r = document.querySelector('.cm-content').getBoundingClientRect(); return { x: r.left + 8, y: r.top + 8 }; })()`);
  await tab.command("Input.dispatchMouseEvent", { type: "mousePressed", x: sourcePoint.x, y: sourcePoint.y, button: "left", clickCount: 1 });
  await tab.command("Input.dispatchMouseEvent", { type: "mouseReleased", x: sourcePoint.x, y: sourcePoint.y, button: "left", clickCount: 1 });
  await tab.command("Input.insertText", { text: " <p>local buffered</p>" });
  await until("local edit sent", () => tab.evaluate(`window.roomControl?.state().localVector && document.querySelector('.cm-content')?.textContent.includes('local buffered')`), 10000);
  await tab.evaluate("window.testSession.persist()");
  const opensBeforeReconnect = await tab.evaluate("window.roomControl.state().opens");
  await tab.evaluate("window.roomControl.drop()");
  await tab.evaluate("window.roomControl.resume()");
  await until("reconnect joins the head", () => tab.evaluate(`window.roomControl?.state().opens > ${opensBeforeReconnect}`), 10000);
  await until("empty catch-up is acknowledged", () => tab.evaluate(`(() => { const s = window.roomControl?.state(); return s?.catchupWasEmpty && s.ackedSeq === s.catchupSeq; })()`), 10000);
  await until("remote newer edit relayed", () => tab.evaluate(`document.querySelector('.cm-content')?.textContent.includes('remote editor newer')`), 10000);
  await tab.evaluate("window.testSession.persist()");
  assert.equal(await tab.evaluate("window.collabState.localPending"), 0);
  assert.equal(await tab.evaluate("window.collabState.localError"), "");
  assert.equal(await tab.evaluate("window.collabState.pending"), 1, "empty catch-up acknowledgement leaves local work awaiting durability");
  assert.notEqual(await tab.evaluate("window.roomControl.state().localVector"), await tab.evaluate("window.roomControl.state().headVector"), "the remote relay advances the head beyond the local save target");
  const saveShortcut = async () => {
    await tab.evaluate("document.querySelector('.cm-content')?.focus()");
    await tab.command("Input.dispatchKeyEvent", { type: "keyDown", key: "s", code: "KeyS", modifiers: 2, windowsVirtualKeyCode: 83 });
    await tab.command("Input.dispatchKeyEvent", { type: "keyUp", key: "s", code: "KeyS", modifiers: 2, windowsVirtualKeyCode: 83 });
  };
  const savedToasts = () => tab.evaluate(`[...document.querySelectorAll('.toast-card')].map(node => node.textContent).filter(text => text.includes('Saved on the server.'))`);
  await saveShortcut();
  assert.deepEqual(await savedToasts(), [], "an empty catch-up ack cannot claim the buffered local edit was saved");
  await tab.evaluate("window.roomControl.confirmLocal()");
  assert.equal(await tab.evaluate("window.collabState.pending"), 0, "later remote edits do not extend the local save target");
  await saveShortcut();
  await until("durable save confirmation", async () => (await savedToasts()).length > 0, 10000);
  assert.deepEqual(await tab.evaluate("window.testErrors"), [], "doc-rows must not throw while it is imported");
  console.log("doc-rows: split join, buffered reconnect, remote relay and durable save confirmation passed");
} catch (error) {
  if (tab) console.error(await tab.evaluate(`JSON.stringify({errors:window.testErrors,state:window.roomControl?.state(),toasts:[...document.querySelectorAll('.toast-card')].map(node=>node.textContent),source:[...document.querySelectorAll('.cm-content')].map(node=>({text:node.textContent,editable:node.contentEditable})),active:document.activeElement?.className})`));
  throw error;
} finally {
  await tab?.close(); http?.close(); rmSync(temp, { recursive: true, force: true });
}
