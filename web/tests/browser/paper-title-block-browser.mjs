// A browser-level check for the title block of a header in the paper view. An Org
// header and a Quarto front matter that set a title are each drawn as a Preamble
// pill followed by the title block. The pill names the header's other lines and
// is absent when there are none. The caret in the header, or a click on the pill,
// shows the source, and when the caret leaves, the block is drawn again.
//
// The Editors are mounted with `paper` and `visual` on an Org file and a Quarto
// file, with the real org.wasm and markdown.wasm served under digest-bearing URLs,
// which the renderer requires.

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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-paper-title-block-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const katexDist = join(root, "web/node_modules/katex/dist");
const port = 19000 + Math.floor(Math.random() * 1000);
const orgWasm = process.env.LIBREPAPER_ORG_WASM || join(root, "web/wasm/org.wasm");
const orgBytes = readFileSync(orgWasm);
const orgDigest = createHash("sha256").update(orgBytes).digest("hex");
const markdownWasm = process.env.LIBREPAPER_MARKDOWN_WASM || join(root, "web/wasm/markdown.wasm");
const markdownBytes = readFileSync(markdownWasm);
const markdownDigest = createHash("sha256").update(markdownBytes).digest("hex");

const samples = {
  org: [
    "#+TITLE: Learn LibrePaper with Org",
    "#+AUTHOR: LibrePaper",
    "#+bibliography: references.bib",
    "",
    "Welcome to LibrePaper.",
    "",
    "* First section",
    "",
    "Body text.",
  ].join("\n"),
  quarto: [
    "---",
    "title: A Quarto Paper",
    "author: Ada",
    "bibliography: references.bib",
    "---",
    "",
    "Welcome to Quarto.",
    "",
    "# First section",
    "",
    "Body text.",
  ].join("\n"),
};

// Runs in the page. It refers only to names the generated entry defines
// (views, tick, samples), so its source is copied into the entry as it stands.
async function titleCheck(name) {
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
  const host = document.querySelector(name === "org" ? "#org-host" : "#quarto-host");
  const view = views[name];
  const headerLines = name === "org"
    ? ["#+TITLE: Learn LibrePaper with Org", "#+AUTHOR: LibrePaper", "#+bibliography: references.bib"]
    : ["title: A Quarto Paper", "author: Ada", "bibliography: references.bib"];
  const firstLine = name === "org" ? "#+TITLE" : "title: A Quarto Paper";
  const lineTexts = () => Array.from(host.querySelectorAll(".cm-line")).map((node) => node.textContent);
  const titleText = (selector) => host.querySelector(`.cm-lp-paper header.title-block ${selector}`)?.textContent ?? null;
  const drawnBlock = () => host.querySelector(".cm-lp-paper header.title-block");
  const pillCount = () => host.querySelectorAll(".cm-lp-fold-preamble").length;

  const drawn = await poll(drawnBlock);
  const pillNode = host.querySelector(".cm-lp-paper-header .cm-lp-fold-preamble");
  const paperNode = host.querySelector(".cm-lp-paper-header > .cm-lp-paper");
  const away = {
    title: titleText("h1.title"),
    author: titleText("p.author"),
    pills: pillCount(),
    loosePills: Array.from(host.querySelectorAll(".cm-lp-fold-preamble")).filter((node) => !node.closest(".cm-lp-paper-header")).length,
    pillDetail: host.querySelector(".cm-lp-paper-header .cm-lp-fold-detail")?.textContent ?? null,
    pillFirst: Boolean(pillNode && paperNode) && Boolean(pillNode.compareDocumentPosition(paperNode) & Node.DOCUMENT_POSITION_FOLLOWING),
    keywordShown: lineTexts().some((text) => text.includes(firstLine)),
  };

  // The caret moves into the header: its source lines show, and the block and the pill are gone.
  view.dispatch({ selection: { anchor: 3 } });
  await settle();
  const insideLines = lineTexts();
  const inside = {
    sourceShown: headerLines.every((line) => insideLines.some((text) => text.includes(line))),
    blockGone: !drawnBlock(),
    pillGone: pillCount() === 0,
  };

  // The caret leaves again: the block is drawn again.
  view.dispatch({ selection: { anchor: view.state.doc.length } });
  await settle();
  const back = { redrawn: Boolean(drawnBlock()) };

  // A click on the pill reveals the source, then the caret goes back to the end.
  const label = host.querySelector(".cm-lp-paper-header .cm-lp-fold-label");
  label?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
  await settle();
  const clickLines = lineTexts();
  const click = {
    sourceShown: clickLines.some((text) => text.includes(firstLine)),
    blockGone: !drawnBlock(),
  };
  view.dispatch({ selection: { anchor: view.state.doc.length } });
  await settle();

  // The header's other line goes: only the title lines remain, and the pill goes with it.
  const removed = name === "org" ? "#+bibliography: references.bib\n" : "bibliography: references.bib\n";
  const from = view.state.doc.toString().indexOf(removed);
  view.dispatch({ changes: { from, to: from + removed.length } });
  view.dispatch({ selection: { anchor: view.state.doc.length } });
  await settle();
  await poll(drawnBlock);
  const bare = {
    title: titleText("h1.title"),
    pills: pillCount(),
    wrapped: Boolean(host.querySelector(".cm-lp-paper-header")),
  };

  return { drawn: Boolean(drawn), away, inside, back, click, bare };
}

const pageSource = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};`,
  `import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};`,
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `const samples = ${JSON.stringify(samples)};`,
  `globalThis.LIBREPAPER_MODULES = { org: "/wasm/${orgDigest}/org.wasm", markdown: "/wasm/${markdownDigest}/markdown.wasm" };`,
  `document.documentElement.dataset.theme = "librepaper";`,
  `document.documentElement.dataset.palette = "librepaper";`,
  `document.documentElement.dataset.mode = "light";`,
  `const orgHost = document.createElement("main");`,
  `orgHost.id = "org-host";`,
  `orgHost.style.cssText = "height: 50vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(orgHost);`,
  `const quartoHost = document.createElement("main");`,
  `quartoHost.id = "quarto-host";`,
  `quartoHost.style.cssText = "height: 50vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(quartoHost);`,
  `const session = joinSession({ send: () => {}, mayEdit: true });`,
  `const orgFile = session.addText("paper.org", samples.org);`,
  `const quartoFile = session.addText("paper.qmd", samples.quarto);`,
  `createClassComponent({`,
  `  component: Editor,`,
  `  target: orgHost,`,
  `  props: { session, format: "org", file: orgFile, visual: true, paper: true },`,
  `});`,
  `createClassComponent({`,
  `  component: Editor,`,
  `  target: quartoHost,`,
  `  props: { session, format: "quarto", file: quartoFile, visual: true, paper: true },`,
  `});`,
  `await tick();`,
  `const views = {`,
  `  org: EditorView.findFromDOM(document.querySelector("#org-host .cm-editor")),`,
  `  quarto: EditorView.findFromDOM(document.querySelector("#quarto-host .cm-editor")),`,
  `};`,
  `views.org.dispatch({ selection: { anchor: views.org.state.doc.length } });`,
  `views.quarto.dispatch({ selection: { anchor: views.quarto.state.doc.length } });`,
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
      lib: { entry, formats: ["es"], fileName: () => "paper-title-block-check.js", cssFileName: "paper-title-block-check" },
    },
  });

  const modules = {
    [`/wasm/${orgDigest}/org.wasm`]: orgBytes,
    [`/wasm/${markdownDigest}/markdown.wasm`]: markdownBytes,
  };
  server = createServer((request, response) => {
    const path = decodeURIComponent(request.url.split("?")[0]);
    if (modules[path]) {
      response.setHeader("Content-Type", "application/wasm");
      response.end(modules[path]);
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
    response.end('<!doctype html><body><link rel="stylesheet" href="/paper-title-block-check.css"><script type="module" src="/paper-title-block-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local paper title block server did not start");

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
    ready = await evaluate("Boolean(window.titleCheckReady && document.querySelector('#org-host .cm-editor') && document.querySelector('#quarto-host .cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editors did not mount in Chromium");

  const results = {
    org: await evaluate('titleCheck("org")'),
    quarto: await evaluate('titleCheck("quarto")'),
  };
  const expected = {
    org: { title: "Learn LibrePaper with Org", author: "LibrePaper", detail: "#+BIBLIOGRAPHY" },
    quarto: { title: "A Quarto Paper", author: "Ada", detail: "bibliography" },
  };

  for (const name of ["org", "quarto"]) {
    const result = results[name];
    const want = expected[name];
    assert.equal(result.drawn, true, `${name}: the paper view drew no title block`);
    assert.equal(result.away.title, want.title, `${name}: the title block does not show the title`);
    assert.equal(result.away.author, want.author, `${name}: the title block does not show the author`);
    assert.equal(result.away.pills, 1, `${name}: the header is not drawn under exactly one Preamble pill`);
    assert.equal(result.away.loosePills, 0, `${name}: a Preamble pill is drawn outside the title block`);
    assert.equal(result.away.pillFirst, true, `${name}: the Preamble pill does not come before the title block`);
    assert.equal(result.away.pillDetail, want.detail, `${name}: the Preamble pill does not name the header's other lines`);
    assert.equal(result.away.keywordShown, false, `${name}: the header's source lines show while the caret is away from the header`);
    console.log(`paper-title-block-browser: ${name}: a header is drawn as a Preamble pill followed by the title block`);

    assert.equal(result.inside.sourceShown, true, `${name}: the caret in the header does not show its source lines`);
    assert.equal(result.inside.blockGone, true, `${name}: the title block is still drawn while the caret is in the header`);
    assert.equal(result.inside.pillGone, true, `${name}: the Preamble pill is still drawn while the caret is in the header`);
    assert.equal(result.back.redrawn, true, `${name}: the title block was not drawn again after the caret left the header`);
    console.log(`paper-title-block-browser: ${name}: the caret in the header shows its source, and the block is drawn again when it leaves`);

    assert.equal(result.click.sourceShown, true, `${name}: a click on the Preamble pill does not show the source`);
    assert.equal(result.click.blockGone, true, `${name}: the title block is still drawn after a click on the Preamble pill`);
    console.log(`paper-title-block-browser: ${name}: a click on the Preamble pill shows the source`);

    assert.equal(result.bare.title, want.title, `${name}: the title block does not show the title after the other line is removed`);
    assert.equal(result.bare.pills, 0, `${name}: a Preamble pill is drawn though the header has no other lines`);
    assert.equal(result.bare.wrapped, false, `${name}: the title block is still wrapped under a pill though the header has no other lines`);
    console.log(`paper-title-block-browser: ${name}: without other header lines the title block is drawn alone`);
  }

  assert.equal(errors.length, 0, ["Browser errors or exceptions during the paper title block check:", ...errors].join("\n"));
  console.log("paper-title-block-browser: no browser errors or exceptions");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
