// A browser-level check for the title block of an Org file. An Org header with a
// title and an author is drawn as a title block in the paper view, in place of
// the Preamble pill that used to hide the keyword lines. The caret inside the
// block shows its source, and when the caret leaves, the block is drawn again.
//
// The Editor is mounted with `paper` and `visual` on an Org file, with the real
// org.wasm served under a digest-bearing URL, which the renderer requires.

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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-paper-org-title-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const katexDist = join(root, "web/node_modules/katex/dist");
const port = 19000 + Math.floor(Math.random() * 1000);
const orgWasm = process.env.LIBREPAPER_ORG_WASM || join(root, "web/wasm/org.wasm");
const orgBytes = readFileSync(orgWasm);
const digest = createHash("sha256").update(orgBytes).digest("hex");

const sample = [
  "#+TITLE: Learn LibrePaper with Org",
  "#+AUTHOR: LibrePaper",
  "#+bibliography: references.bib",
  "",
  "Welcome to LibrePaper.",
  "",
  "* First section",
  "",
  "Body text.",
].join("\n");

// Runs in the page. It refers only to names the generated entry defines
// (view, tick, samples), so its source is copied into the entry as it stands.
async function titleCheck() {
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
  const titleText = (selector) => document.querySelector(`.cm-editor .cm-lp-paper header.title-block ${selector}`)?.textContent ?? null;
  const lineTexts = () => Array.from(document.querySelectorAll(".cm-editor .cm-line")).map((node) => node.textContent);

  const drawn = await poll(() => document.querySelector(".cm-editor .cm-lp-paper header.title-block"));
  const awayLines = lineTexts();
  const away = {
    title: titleText("h1.title"),
    author: titleText("p.author"),
    pills: document.querySelectorAll(".cm-editor .cm-lp-fold-preamble").length,
    keywordShown: awayLines.some((text) => text.includes("#+TITLE") || text.includes("#+bibliography")),
    first: papers()[0]?.querySelector("header.title-block") != null,
  };

  // The caret moves into the header: its three source lines show, and the block is gone.
  view.dispatch({ selection: { anchor: 3 } });
  await settle();
  const insideLines = lineTexts();
  const inside = {
    sourceShown: insideLines.some((text) => text.includes("#+TITLE: Learn LibrePaper with Org"))
      && insideLines.some((text) => text.includes("#+AUTHOR: LibrePaper"))
      && insideLines.some((text) => text.includes("#+bibliography: references.bib")),
    blockGone: !document.querySelector(".cm-editor header.title-block"),
  };

  // The caret leaves again: the block is drawn again.
  view.dispatch({ selection: { anchor: view.state.doc.length } });
  await settle();
  const back = { redrawn: Boolean(document.querySelector(".cm-editor .cm-lp-paper header.title-block h1.title")) };

  return { drawn: Boolean(drawn), away, inside, back, docUnchanged: view.state.doc.toString() === source };
}

const pageSource = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};`,
  `import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};`,
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `const samples = ${JSON.stringify({ paper: sample })};`,
  `globalThis.LIBREPAPER_MODULES = { org: "/wasm/${digest}/org.wasm" };`,
  `document.documentElement.dataset.theme = "librepaper";`,
  `document.documentElement.dataset.palette = "librepaper";`,
  `document.documentElement.dataset.mode = "light";`,
  `const host = document.createElement("main");`,
  `host.style.cssText = "height: 100vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(host);`,
  `const session = joinSession({ send: () => {}, mayEdit: true });`,
  `const file = session.addText("paper.org", samples.paper);`,
  `createClassComponent({`,
  `  component: Editor,`,
  `  target: host,`,
  `  props: { session, format: "org", file, visual: true, paper: true },`,
  `});`,
  `await tick();`,
  `const view = EditorView.findFromDOM(document.querySelector(".cm-editor"));`,
  `view.dispatch({ selection: { anchor: view.state.doc.length } });`,
  `window.titleCheck = ${titleCheck.toString()};`,
  `window.titleCheckReady = true;`,
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
      lib: { entry, formats: ["es"], fileName: () => "paper-org-title-check.js", cssFileName: "paper-org-title-check" },
    },
  });

  server = createServer((request, response) => {
    const path = decodeURIComponent(request.url.split("?")[0]);
    if (path === `/wasm/${digest}/org.wasm`) {
      response.setHeader("Content-Type", "application/wasm");
      response.end(orgBytes);
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
    response.end('<!doctype html><body><link rel="stylesheet" href="/paper-org-title-check.css"><script type="module" src="/paper-org-title-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local paper org title server did not start");

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
    ready = await evaluate("Boolean(window.titleCheckReady && document.querySelector('.cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editor did not mount in Chromium");

  const result = await evaluate("titleCheck()");

  assert.equal(result.drawn, true, "the paper view drew no title block");
  assert.equal(result.away.title, "Learn LibrePaper with Org", "the title block does not show the title");
  assert.equal(result.away.author, "LibrePaper", "the title block does not show the author");
  assert.equal(result.away.pills, 0, "the header is still folded behind a Preamble pill");
  assert.equal(result.away.keywordShown, false, "the keyword lines show while the caret is away from the header");
  assert.equal(result.away.first, true, "the title block is not in the first paper block");
  console.log("paper-org-title-browser: an Org header is drawn as a title block in place of the Preamble pill");

  assert.equal(result.inside.sourceShown, true, "the caret in the header does not show its source lines");
  assert.equal(result.inside.blockGone, true, "the title block is still drawn while the caret is in the header");
  console.log("paper-org-title-browser: the caret in the header shows its source lines");

  assert.equal(result.back.redrawn, true, "the title block was not drawn again after the caret left the header");
  assert.equal(result.docUnchanged, true, "the title check changed the document");
  console.log("paper-org-title-browser: the title block is drawn again when the caret leaves the header");

  assert.equal(errors.length, 0, ["Browser errors or exceptions during the paper org title check:", ...errors].join("\n"));
  console.log("paper-org-title-browser: no browser errors or exceptions");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
