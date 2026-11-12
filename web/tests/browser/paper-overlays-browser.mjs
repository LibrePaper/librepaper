// A browser-level check for overlays inside paper blocks. The paper view draws
// every block the caret is not in as rendered HTML, and a decoration cannot
// reach into that HTML, so overlays.js paints copies: a comment mark, a
// tracked insertion and the text it removed are checked here in a block away
// from the caret, and then in the block holding the caret, where the source
// shows them as it always does.
//
// The Editor is mounted with `paper` and `visual` on a Markdown file, with the
// real markdown.wasm served under a digest-bearing URL, which the renderer
// requires.

import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, join, sep } from "node:path";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { stopBrowserProcess, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-paper-overlays-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const katexDist = join(root, "web/node_modules/katex/dist");
const port = 19000 + Math.floor(Math.random() * 1000);
const markdownWasm = process.env.LIBREPAPER_MARKDOWN_WASM || join(root, "web/wasm/markdown.wasm");
const markdownBytes = readFileSync(markdownWasm);
const digest = createHash("sha256").update(markdownBytes).digest("hex");

const sample = [
  "# Title",
  "",
  "First paragraph with *emphasis* and a [link](https://example.org/x) here.",
  "",
  "Second paragraph has a tracked insertion here.",
  "",
  "Caret paragraph.",
].join("\n");

// Runs in the page. It refers only to names the generated entry defines
// (view, tick, setCommentMarks, setChangeMarks, samples), so its source is
// copied into the entry as it stands.
async function overlayCheck() {
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  const settle = async () => {
    await tick();
    await wait(300);
  };
  const poll = async (test) => {
    for (let attempt = 0; attempt < 50; attempt++) {
      if (test()) return true;
      await wait(100);
    }
    return false;
  };
  const source = samples.paper;
  const papers = () => Array.from(document.querySelectorAll(".cm-editor .cm-lp-paper"));
  const paperWith = (text) => papers().find((node) => node.textContent.includes(text)) ?? null;

  const rendered = await poll(() => paperWith("First paragraph"));
  const emphasisAt = source.indexOf("emphasis");
  const insertionAt = source.indexOf("tracked insertion");
  const gapAt = source.indexOf("Second paragraph has a ") + "Second paragraph has a ".length;
  view.dispatch({
    effects: [
      setCommentMarks.of([{ id: "c1", from: emphasisAt, to: emphasisAt + "emphasis".length, highlight: true, color: "#ffcc00", selected: false }]),
      setChangeMarks.of({
        changes: [{ id: "ch1", who: "ada", author: "Ada", from: insertionAt, to: insertionAt + "tracked insertion".length }],
        gaps: [{ id: "g1", at: gapAt, fragments: [{ change: "ch1", text: "deleted " }] }],
        me: { who: "bob" },
        selected: null,
      }),
    ],
  });
  await settle();

  const first = paperWith("First paragraph");
  const second = paperWith("Second paragraph");
  const away = {
    rendered,
    commentText: first?.querySelector(".cm-comment-mark-highlight[data-comment-id='c1']")?.textContent ?? null,
    commentInEmphasis: Boolean(first?.querySelector("em .cm-comment-mark-highlight, .cm-comment-mark-highlight em")),
    commentStyle: first?.querySelector(".cm-comment-mark-highlight")?.getAttribute("style") ?? null,
    changeText: second?.querySelector(".change-added[data-change='ch1']")?.textContent ?? null,
    removedText: second?.querySelector("del.change-removed[data-change='ch1']")?.textContent ?? null,
    firstText: first?.textContent ?? null,
    docUnchanged: view.state.doc.toString() === source,
  };

  // A click on the painted comment selects the comment, and the block stays drawn.
  const painted = first.querySelector(".cm-comment-mark-highlight");
  const headBefore = view.state.selection.main.head;
  painted.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
  painted.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 0 }));
  await settle();
  const click = {
    selected: [...window.commentClicks],
    headKept: view.state.selection.main.head === headBefore,
    stillRendered: Boolean(paperWith("First paragraph")),
  };

  // A painted copy is repainted when the text under it changes: a word typed in the block of the
  // caret leaves the paintings in the other blocks alone.
  const before = second.querySelector(".change-added") ;
  view.dispatch({ changes: { from: source.length, insert: "!" } });
  await settle();
  const after = paperWith("Second paragraph")?.querySelector(".change-added[data-change='ch1']");
  const typing = { kept: Boolean(before && after), text: after?.textContent ?? null };
  view.dispatch({ changes: { from: source.length, to: source.length + 1 } });
  await settle();

  // The caret enters the first block: its source shows the mark itself, and no painted copy remains there.
  view.dispatch({ selection: { anchor: emphasisAt } });
  await settle();
  const lines = Array.from(document.querySelectorAll(".cm-editor .cm-line"));
  const line = lines.find((node) => node.textContent.includes("First paragraph"));
  const sourceView = {
    firstIsSource: Boolean(line) && !paperWith("First paragraph"),
    mark: line?.querySelector(".cm-comment-mark")?.textContent ?? null,
    painted: line?.querySelectorAll("[data-lp-overlay]").length ?? -1,
    otherStillPainted: Boolean(paperWith("Second paragraph")?.querySelector("[data-lp-overlay]")),
  };

  // The caret leaves again: the block is drawn and painted anew.
  view.dispatch({ selection: { anchor: view.state.doc.length } });
  await settle();
  const back = {
    commentText: paperWith("First paragraph")?.querySelector(".cm-comment-mark-highlight")?.textContent ?? null,
    marks: paperWith("First paragraph")?.querySelectorAll("[data-lp-overlay]").length ?? -1,
  };
  return { away, click, typing, sourceView, back };
}

const pageSource = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};`,
  `import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};`,
  `import { setCommentMarks } from ${JSON.stringify(join(root, "web/src/lib/source-comment-marks.js"))};`,
  `import { setChangeMarks } from ${JSON.stringify(join(root, "web/src/lib/change-marks.js"))};`,
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `const samples = ${JSON.stringify({ paper: sample })};`,
  `globalThis.LIBREPAPER_MODULES = { markdown: "/wasm/${digest}/markdown.wasm" };`,
  `document.documentElement.dataset.theme = "librepaper";`,
  `document.documentElement.dataset.palette = "librepaper";`,
  `document.documentElement.dataset.mode = "light";`,
  `window.commentClicks = [];`,
  `const host = document.createElement("main");`,
  `host.style.cssText = "height: 100vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(host);`,
  `const session = joinSession({ send: () => {}, mayEdit: true });`,
  `const file = session.addText("readme.md", samples.paper);`,
  `createClassComponent({`,
  `  component: Editor,`,
  `  target: host,`,
  `  props: { session, format: "markdown", file, visual: true, paper: true, oncomment: (id) => window.commentClicks.push(id) },`,
  `});`,
  `await tick();`,
  `const view = EditorView.findFromDOM(document.querySelector(".cm-editor"));`,
  `view.dispatch({ selection: { anchor: view.state.doc.length } });`,
  `window.overlayCheck = ${overlayCheck.toString()};`,
  `window.overlayCheckReady = true;`,
  "",
].join("\n");

writeFileSync(entry, pageSource);
let server;
let browser;
let socket;
const errors = [];
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    define: { __KATEX__: JSON.stringify("/katex/") },
    resolve: { alias: loroAlias },
    plugins: [tailwindcss(), svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "paper-overlays-check.js", cssFileName: "paper-overlays-check" },
    },
  });

  server = createServer((request, response) => {
    const path = decodeURIComponent(request.url.split("?")[0]);
    if (path === `/wasm/${digest}/markdown.wasm`) {
      response.setHeader("Content-Type", "application/wasm");
      response.end(markdownBytes);
      return;
    }
    if (path.startsWith("/katex/")) {
      const file = join(katexDist, path.slice("/katex/".length));
      if (file.startsWith(katexDist + sep) && existsSync(file)) {
        response.setHeader("Content-Type", file.endsWith(".woff2") ? "font/woff2" : contentType(file));
        response.end(readFileSync(file));
        return;
      }
      response.statusCode = 404;
      response.end();
      return;
    }
    const file = join(output, path.slice(1));
    if (path !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><body><link rel="stylesheet" href="/paper-overlays-check.css"><script type="module" src="/paper-overlays-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local paper overlays server did not start");

  browser = spawn(requireChromiumExecutable(), [
    "--headless=new",
    "--window-size=1280,2000",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore", detached: true });
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
    if (message.method === "Runtime.exceptionThrown") {
      const details = message.params.exceptionDetails;
      errors.push(details.exception?.description ?? details.text);
    }
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") {
      errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "));
    }
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
    const message = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };

  await send("Runtime.enable");
  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await evaluate("Boolean(window.overlayCheckReady && document.querySelector('.cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editor did not mount in Chromium");

  const result = await evaluate("overlayCheck()");

  assert.equal(result.away.rendered, true, "the paper view drew no rendered block");
  assert.equal(result.away.commentText, "emphasis", "the comment mark was not painted over the word it covers");
  assert.equal(result.away.commentInEmphasis, true, "the painted comment mark is not on the emphasised word");
  assert.match(result.away.commentStyle ?? "", /--comment-mark-color: #ffcc00/, "the painted comment mark lost its colour");
  console.log("paper-overlays-browser: a comment is painted over its words in a rendered block");

  assert.equal(result.away.changeText, "tracked insertion", "the tracked insertion was not painted over its words");
  assert.equal(result.away.removedText, "deleted ", "the text a change removed was not painted at its gap");
  assert.equal(result.away.docUnchanged, true, "painting changed the document");
  assert.equal(result.away.firstText.includes("*"), false, "the rendered block shows its markup");
  console.log("paper-overlays-browser: a tracked insertion and the removed text are painted");

  assert.deepEqual(result.click.selected, ["c1"], "a click on the painted comment did not select the comment");
  assert.equal(result.click.headKept, true, "a click on the painted comment moved the caret");
  assert.equal(result.click.stillRendered, true, "a click on the painted comment turned the block into source");
  console.log("paper-overlays-browser: a click on a painted comment selects it and keeps the block drawn");

  assert.equal(result.typing.kept, true, "the paintings in another block were lost when the caret's block was edited");
  assert.equal(result.typing.text, "tracked insertion");

  assert.equal(result.sourceView.firstIsSource, true, "the block holding the caret is not shown as source");
  assert.equal(result.sourceView.mark, "emphasis", "the source does not show the comment mark");
  assert.equal(result.sourceView.painted, 0, "the source line holds painted copies");
  assert.equal(result.sourceView.otherStillPainted, true, "the other block lost its paintings when the caret moved");
  assert.equal(result.back.commentText, "emphasis", "the block was not painted again after the caret left it");
  assert.equal(result.back.marks, 1, "the repainted block holds the wrong number of painted marks");
  console.log("paper-overlays-browser: the source shows the marks itself and a block is painted again when it is redrawn");

  assert.equal(errors.length, 0, ["Browser errors or exceptions during the paper overlays check:", ...errors].join("\n"));
  console.log("paper-overlays-browser: no browser errors or exceptions");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
