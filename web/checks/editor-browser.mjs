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
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};
import { undoDepth } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/commands/dist/index.js"))};
import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};
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
} finally {
  socket?.close();
  browser?.kill();
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  rmSync(temporary, { recursive: true, force: true });
}
