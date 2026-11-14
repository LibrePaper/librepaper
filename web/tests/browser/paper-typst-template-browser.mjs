// A browser-level check for a Typst template in the paper view. A template's
// opening output (its title, authors and abstract) is drawn once, under the
// Preamble pill, and not around every block. The body's aligned content is
// drawn, and the caret in the preamble shows its source, which the header
// block stands in for.
//
// The Editor is mounted with `paper` and `visual` on a Typst file, with the real
// typst.wasm and typst-syntax.wasm served under digest-bearing URLs, which the
// renderer requires.

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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-paper-typst-template-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const katexDist = join(root, "web/node_modules/katex/dist");
const port = 19000 + Math.floor(Math.random() * 1000);
const typstWasm = process.env.LIBREPAPER_TYPST_WASM || join(root, "web/wasm/typst.wasm");
const typstBytes = readFileSync(typstWasm);
const typstDigest = createHash("sha256").update(typstBytes).digest("hex");
const typstSyntaxWasm = process.env.LIBREPAPER_TYPST_SYNTAX_WASM || join(root, "web/wasm/typst-syntax.wasm");
const typstSyntaxBytes = readFileSync(typstSyntaxWasm);
const typstSyntaxDigest = createHash("sha256").update(typstSyntaxBytes).digest("hex");

const sample = [
  "#let article(title: none, authors: (), abstract: none, body) = {",
  "  align(center, text(17pt, weight: \"bold\", title))",
  "  align(center, authors.join(\", \"))",
  "  block[*Abstract.* #abstract]",
  "  body",
  "}",
  "#show: article.with(title: [A Typst Paper], authors: (\"Ada\", \"Grace\"), abstract: [Short abstract.])",
  "",
  "First paragraph.",
  "",
  "= A section",
  "",
  "#align(center)[Centred words.]",
  "",
  "Last paragraph.",
].join("\n");

// Runs in the page. It refers only to names the generated entry defines
// (view, tick), so its source is copied into the entry as it stands.
async function templateCheck() {
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  const settle = async () => {
    await tick();
    await wait(300);
  };
  const poll = async (test) => {
    for (let attempt = 0; attempt < 150; attempt++) {
      if (test()) return true;
      await wait(100);
    }
    return false;
  };
  const editor = view.dom;
  const lineTexts = () => Array.from(editor.querySelectorAll(".cm-line")).map((node) => node.textContent);
  const drawnBlock = () => editor.querySelector(".cm-lp-paper header.title-block");
  const count = (text, part) => text.split(part).length - 1;
  const papers = () => Array.from(editor.querySelectorAll(".cm-lp-paper"));
  const hasHeader = (node) => Boolean(node.querySelector("header.title-block"));

  view.dispatch({ selection: { anchor: view.state.doc.length } });
  await settle();
  const drawn = await poll(drawnBlock);
  const source = editor.querySelector(".cm-content").textContent;
  const first = papers().find((node) => node.textContent.includes("First paragraph.") && !hasHeader(node));
  const heading = papers().find((node) => node.querySelector("h2"));
  const centredPaper = papers().find((node) => node.textContent.includes("Centred words.") && !hasHeader(node));
  const away = {
    headerText: editor.querySelector(".cm-lp-paper header.title-block")?.textContent ?? null,
    pills: editor.querySelectorAll(".cm-lp-paper-header .cm-lp-fold-preamble").length,
    pillDetail: editor.querySelector(".cm-lp-paper-header .cm-lp-fold-detail")?.textContent ?? null,
    loosePills: Array.from(editor.querySelectorAll(".cm-lp-fold-preamble")).filter((node) => !node.closest(".cm-lp-paper-header")).length,
    abstracts: count(source, "Short abstract."),
    titles: count(source, "A Typst Paper"),
    first: first ? first.textContent.trim() : null,
    heading: heading ? heading.textContent.trim() : null,
    centred: centredPaper ? centredPaper.querySelector("div")?.getAttribute("style") ?? null : null,
    ruleShown: lineTexts().some((text) => text.includes("#show: article.with")),
  };

  // The caret moves into the preamble: its source shows, and the header block is gone.
  view.dispatch({ selection: { anchor: 3 } });
  await settle();
  const insideLines = lineTexts();
  const inside = {
    sourceShown: insideLines.some((text) => text.includes("#show: article.with")) && insideLines.some((text) => text.includes("#let article(")),
    headerGone: !drawnBlock(),
  };

  // The caret goes back to the end: the header block is drawn again.
  view.dispatch({ selection: { anchor: view.state.doc.length } });
  await settle();
  const back = { redrawn: Boolean(drawnBlock()) };

  return { drawn: Boolean(drawn), away, inside, back };
}

const pageSource = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};`,
  `import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};`,
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `const sample = ${JSON.stringify(sample)};`,
  `globalThis.LIBREPAPER_MODULES = { typst: "/wasm/${typstDigest}/typst.wasm", "typst-syntax": "/wasm/${typstSyntaxDigest}/typst-syntax.wasm" };`,
  `document.documentElement.dataset.theme = "librepaper";`,
  `document.documentElement.dataset.palette = "librepaper";`,
  `document.documentElement.dataset.mode = "light";`,
  `const host = document.createElement("main");`,
  `host.id = "typst-host";`,
  `host.style.cssText = "height: 100vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(host);`,
  `const session = joinSession({ send: () => {}, mayEdit: true });`,
  `const file = session.addText("paper.typ", sample);`,
  `createClassComponent({`,
  `  component: Editor,`,
  `  target: host,`,
  `  props: { session, format: "typst", file, visual: true, paper: true },`,
  `});`,
  `await tick();`,
  `const view = EditorView.findFromDOM(document.querySelector("#typst-host .cm-editor"));`,
  `view.dispatch({ selection: { anchor: view.state.doc.length } });`,
  `window.templateCheck = ${templateCheck.toString()};`,
  `window.templateCheckReady = true;`,
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
      lib: { entry, formats: ["es"], fileName: () => "paper-typst-template-check.js", cssFileName: "paper-typst-template-check" },
    },
  });

  const modules = {
    [`/wasm/${typstDigest}/typst.wasm`]: typstBytes,
    [`/wasm/${typstSyntaxDigest}/typst-syntax.wasm`]: typstSyntaxBytes,
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
    response.end('<!doctype html><body><link rel="stylesheet" href="/paper-typst-template-check.css"><script type="module" src="/paper-typst-template-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local paper typst template server did not start");

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
    ready = await evaluate("Boolean(window.templateCheckReady && document.querySelector('#typst-host .cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "The Editor did not mount in Chromium");

  const result = await evaluate("templateCheck()");

  assert.equal(result.drawn, true, "the paper view drew no title block for the template's header");
  console.log("paper-typst-template-browser: the template's header is drawn as a title block");

  for (const part of ["A Typst Paper", "Ada, Grace", "Short abstract."]) {
    assert.ok(result.away.headerText?.includes(part), `the title block does not show "${part}"`);
  }
  assert.equal(result.away.pills, 1, "the header is not drawn under exactly one Preamble pill");
  assert.equal(result.away.loosePills, 0, "a Preamble pill is drawn outside the header");
  assert.equal(result.away.pillDetail, "#let · #show", "the Preamble pill does not name the preamble's other lines");
  assert.equal(result.away.ruleShown, false, "the template's rule shows while the caret is away from the preamble");
  console.log("paper-typst-template-browser: the header is drawn under one Preamble pill, and its source stays hidden");

  assert.equal(result.away.abstracts, 1, "the template's output is repeated around the blocks (abstract)");
  assert.equal(result.away.titles, 1, "the template's output is repeated around the blocks (title)");
  console.log("paper-typst-template-browser: the template's output is drawn once, not around every block");

  assert.equal(result.away.first, "First paragraph.", "the first paragraph is not drawn as its block");
  assert.equal(result.away.heading, "A section", "the section heading is not drawn as its block");
  assert.match(result.away.centred ?? "", /text-align: center/, "the aligned content is not drawn centred");
  console.log("paper-typst-template-browser: the body blocks are drawn, and aligned content is drawn");

  assert.equal(result.inside.sourceShown, true, "the caret in the preamble does not show its source lines");
  assert.equal(result.inside.headerGone, true, "the title block is still drawn while the caret is in the preamble");
  assert.equal(result.back.redrawn, true, "the title block was not drawn again after the caret left the preamble");
  console.log("paper-typst-template-browser: the caret in the preamble shows its source, and the header is drawn again when it leaves");

  assert.equal(errors.length, 0, ["Browser errors or exceptions during the paper Typst template check:", ...errors].join("\n"));
  console.log("paper-typst-template-browser: no browser errors or exceptions");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
