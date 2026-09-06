// A browser-level regression check for the source editor.
//
// This deliberately exercises the component through its public props and DOM,
// while Yjs supplies the same shared-text changes a peer would make. It does
// not inspect the component's state cache or effects.

import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(here));
const temporary = mkdtempSync(join(tmpdir(), "komodoc-editor-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const port = 19000 + Math.floor(Math.random() * 1000);

const source = `
import * as Y from ${JSON.stringify(join(root, "web/node_modules/yjs/dist/yjs.mjs"))};
import { IndexeddbPersistence } from ${JSON.stringify(join(root, "web/node_modules/y-indexeddb/src/y-indexeddb.js"))};
import { cacheName } from ${JSON.stringify(join(root, "web/src/lib/collab-cache.js"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};
import { undoDepth } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/commands/dist/index.js"))};
import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};
import Diagnostics from ${JSON.stringify(join(root, "web/src/components/Diagnostics.svelte"))};
import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};

const session = joinSession({ send: () => {}, mayEdit: true });
const firstFile = session.addText("a.md", "alpha");
const secondFile = session.addText("b.md", "beta");
session.setMain(firstFile);
const component = createClassComponent({
  component: Editor,
  target: document.body,
  props: { session, format: "markdown", file: firstFile },
});

window.editorCheck = async () => {
  await tick();
  const firstView = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  firstView.dispatch({
    changes: { from: 0, insert: "LOCAL " },
    selection: { anchor: 3 },
  });
  const before = {
    undo: undoDepth(firstView.state),
    caret: firstView.state.selection.main.head,
    text: firstView.state.doc.toString(),
  };

  component.$set({ file: secondFile });
  await tick();
  const secondText = EditorView.findFromDOM(document.querySelector(".cm-editor")).state.doc.toString();

  // A peer changes A while this browser is looking at B.
  session.textOf(firstFile).insert(0, "REMOTE ");
  component.$set({ file: firstFile });
  await tick();
  await tick();
  const finalView = EditorView.findFromDOM(document.querySelector(".cm-editor"));
  return {
    before,
    secondText,
    after: {
      undo: undoDepth(finalView.state),
      caret: finalView.state.selection.main.head,
      text: finalView.state.doc.toString(),
    },
    sameView: firstView === finalView,
  };
};
window.collabCacheCheck = async () => {
  const slug = "cache-upgrade";
  const sessions = [];
  const create = (props = {}) => {
    const value = joinSession({ send: () => {}, mayEdit: true, ...props });
    sessions.push(value);
    return value;
  };
  const snapshot = (value) => ({ update: btoa(String.fromCharCode(...Y.encodeStateAsUpdate(value.doc))) });
  const cached = async (createdAt) => {
    let ready;
    const local = new Promise((resolve) => { ready = resolve; });
    const value = create({ slug, createdAt, onState: (state) => { if (state.local) ready(); } });
    await local;
    return value;
  };
  try {
    const server = create();
    const main = server.addText("main.md", "server");
    server.setMain(main);
    const old = create();
    Y.applyUpdate(old.doc, Y.encodeStateAsUpdate(server.doc));
    const legacyStore = new IndexeddbPersistence(cacheName(slug), old.doc);
    await legacyStore.whenSynced;
    old.textOf(main).insert(0, "unsent ");
    await legacyStore.destroy();

    const upgraded = await cached("first-creation");
    await upgraded.start(snapshot(server));
    const migrated = upgraded.text.toString();
    upgraded.text.insert(0, "new offline ");
    upgraded.leave();
    const reopened = await cached("first-creation");
    await reopened.start(snapshot(server));
    const recovered = reopened.text.toString();

    const replacement = create();
    const newMain = replacement.addText("main.md", "reseeded");
    replacement.setMain(newMain);
    const recreated = await cached("second-creation");
    await recreated.start(snapshot(replacement));
    recreated.text.insert(0, "edited ");
    const result = {
      migrated, recovered,
      files: recreated.list().length,
      main: recreated.mainId() === newMain,
      preview: recreated.tree().texts["main.md"],
    };
    const retained = create();
    const retainedStore = new IndexeddbPersistence(cacheName(slug), retained.doc);
    await retainedStore.whenSynced;
    result.legacy = retained.text.toString();
    await retainedStore.destroy();
    return result;
  } finally {
    sessions.forEach((value) => value.leave());
  }
};
window.diagnosticsCheck = async () => {
  const host = document.createElement("aside");
  document.body.append(host);
  let chosen;
  const component = createClassComponent({ component: Diagnostics, target: host, props: {
    main: "main.typ",
    diagnostics: [
      { severity: "warning", message: "General compiler warning", hints: ["Try this suggestion"], line: 0 },
      { severity: "error", message: "Unknown name", file: "chapter.typ", line: 4, column: 2 },
    ],
    canOpen: (item) => item.line > 0,
    onopen: (item) => { chosen = item; },
  } });
  await tick();
  const text = host.textContent;
  const links = host.querySelectorAll("button");
  const link = links[0]?.textContent.trim();
  links[0]?.click();
  component.$set({ diagnostics: [] });
  await tick();
  const result = { text, links: links.length, link, chosen, empty: host.textContent };
  component.$destroy();
  host.remove();
  return result;
};
window.editorCheckReady = true;
`;

writeFileSync(entry, source);
let server;
let browser;
let socket;
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    plugins: [svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "editor-check.js" },
    },
  });

  server = createServer((request, response) => {
    if (request.url === "/editor-check.js") {
      response.setHeader("Content-Type", "text/javascript");
      response.end(readFileSync(join(output, "editor-check.js")));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<body><script type="module" src="/editor-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local editor server did not start");

  browser = spawn("chromium", [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore" });
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  let browserInfo;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      browserInfo = await fetch(`http://127.0.0.1:${port}/json`).then((answer) => answer.json());
      break;
    } catch {
      await wait(100);
    }
  }
  const page = browserInfo?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });

  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    const resolve = pending.get(message.id);
    if (resolve) {
      pending.delete(message.id);
      resolve(message);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    const id = ++sequence;
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const message = await send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };

  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await evaluate("Boolean(window.editorCheckReady && document.querySelector('.cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editor did not mount in Chromium");
  const result = await evaluate("editorCheck()");
  assert.equal(result.sameView, true);
  assert.equal(result.secondText, "beta");
  assert.equal(result.before.undo, 1);
  assert.equal(result.after.undo, 1);
  assert.equal(result.before.caret + 7, result.after.caret);
  assert.equal(result.after.text, "REMOTE LOCAL alpha");
  console.log("editor-browser: file state, undo, caret, and inactive remote text preserved");
  const diagnostics = await evaluate("diagnosticsCheck()");
  assert.match(diagnostics.text, /General compiler warning/);
  assert.match(diagnostics.text, /Try this suggestion/);
  assert.match(diagnostics.text, /Unknown name/);
  assert.equal(diagnostics.links, 1);
  assert.equal(diagnostics.link, "chapter.typ:4:2");
  assert.equal(diagnostics.chosen.file, "chapter.typ");
  assert.match(diagnostics.empty, /No warnings or errors/);
  console.log("editor-browser: diagnostics, hints, source links and empty state passed");
  const cache = await evaluate("collabCacheCheck()");
  assert.equal(cache.migrated, "unsent server");
  assert.equal(cache.recovered, "new offline unsent server");
  assert.equal(cache.files, 1);
  assert.equal(cache.main, true);
  assert.equal(cache.preview, "edited reseeded");
  assert.equal(cache.legacy, "unsent server");
  console.log("editor-browser: cache upgrade preserves offline edits and isolates recreated documents");
} finally {
  socket?.close();
  browser?.kill();
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
