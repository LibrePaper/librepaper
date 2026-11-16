import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { removeTemporary, stopBrowserProcess } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-source-grammar-check-"));
const output = join(temporary, "build"), profile = join(temporary, "chrome"), entry = join(temporary, "entry.js");
const port = 37000 + Math.floor(Math.random() * 1000);
const typstWasm = process.env.LIBREPAPER_TYPST_WASM || join(root, "web/wasm/typst.wasm");
const syntaxWasm = process.env.LIBREPAPER_TYPST_SYNTAX_WASM || join(root, "web/wasm/typst-syntax.wasm");
const syntaxUrl = existsSync(syntaxWasm) ? "/wasm/" + createHash("sha256").update(readFileSync(syntaxWasm)).digest("hex") + "/typst-syntax.wasm" : "/wasm/missing/typst-syntax.wasm";
// The renderer checks a module against the digest in its URL, as the mirror lays it out.
const typstUrl = existsSync(typstWasm) ? "/wasm/" + createHash("sha256").update(readFileSync(typstWasm)).digest("hex") + "/typst.wasm" : "/wasm/missing/typst.wasm";
const imports = (path) => JSON.stringify(join(root, path));


const texts = {
  "latex-highlight": { path: "main.tex", format: "latex", text: "\\section{Intro}\nText $x^2$ and \\verb|$| here.\n\\begin{verbatim}\n$ not math\n\\end{verbatim}\n" },
  "latex-autoclose": { path: "main.tex", format: "latex", text: "\\documentclass{article}\n" },
  "markdown-div-fold": { path: "notes.md", format: "markdown", text: "# Title\n\n::: {.callout-note}\nSome *text* and $x$ and [@doe99].\n:::\n\nAfter.\n" },
  "typst-fold": { path: "doc.typ", format: "typst", text: "= Intro\nText\n\n#let f(x) = {\n  x + 1\n}\n= Next\n" },
};

// The browser side, a raw string so backslashes arrive unchanged (no backticks, no substitutions).
const pageSource = String.raw`
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
window.grammarReady = async () => { await tick(); return Boolean(document.body); };
const press = (view, init) => view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
const readText = (session, id) => session.doc.getMap("files").get(id).toString();
// Names of the syntax nodes from the innermost one at pos up to the root.
const chainAt = (view, pos) => {
  const names = [];
  for (let node = syntaxTree(view.state).resolveInner(pos, 1); node; node = node.parent) names.push(node.name);
  return names;
};
const typeChar = (view, ch) => {
  const head = view.state.selection.main.head;
  view.dispatch(view.state.update({ changes: { from: head, insert: ch }, selection: { anchor: head + 1 }, userEvent: "input.type" }));
};
const poll = async (get, ms) => {
  let value = null;
  for (let waited = 0; waited < ms; waited += 100) {
    value = get();
    if (value) return value;
    await pause(100);
  }
  return get();
};

const scenarios = {
  "latex-highlight": async ({ view }) => {
    const doc = view.state.doc.toString();
    ensureSyntaxTree(view.state, view.state.doc.length, 5000);
    await pause(200);
    const math = chainAt(view, doc.indexOf("x^2"));
    const verbatim = chainAt(view, doc.indexOf("$ not math") + 2);
    const section = chainAt(view, doc.indexOf("\\section") + 1);
    const firstLine = view.dom.querySelector(".cm-line");
    const classes = [...firstLine.querySelectorAll("span")].map((span) => span.className).filter(Boolean);
    return { math, verbatim, section, classes };
  },
  "latex-autoclose": async ({ view, session, ids }) => {
    view.focus();
    view.dispatch({ selection: { anchor: view.state.doc.length } });
    for (const ch of "\\begin{itemize") typeChar(view, ch);
    const before = view.state.doc.toString();
    // Longer than the undo manager's merge interval, so the } is its own step.
    await pause(1300);
    typeChar(view, "}");
    await pause(300);
    const typed = view.state.doc.toString();
    const head = view.state.selection.main.head;
    const crdt = readText(session, ids["main.tex"]);
    press(view, { key: "z", code: "KeyZ", ctrlKey: true });
    await pause(300);
    return { before, typed, head, crdt, undone: view.state.doc.toString(), undoneCrdt: readText(session, ids["main.tex"]) };
  },
  "markdown-div-fold": async ({ view }) => {
    ensureSyntaxTree(view.state, view.state.doc.length, 5000);
    const line = view.state.doc.line(3), closing = view.state.doc.line(5);
    const range = foldable(view.state, line.from, line.to);
    const names = new Set();
    syntaxTree(view.state).iterate({ enter: (node) => { names.add(node.name); } });
    return { range, expectFrom: line.to, expectTo: closing.from, names: [...names] };
  },
  "typst-fold": async ({ view }) => {
    const doc = view.state.doc;
    const letLine = doc.line(4), heading = doc.line(1);
    // The wasm language is active once the tree has a Heading node.
    const hasHeading = () => { let found = false; syntaxTree(view.state).iterate({ enter: (node) => { if (node.name === "Heading") found = true; } }); return found; };
    const active = await poll(() => hasHeading() || null, 10000);
    const range = await poll(() => foldable(view.state, letLine.from, letLine.to), 10000);
    const headingRange = await poll(() => foldable(view.state, heading.from, heading.to), 10000);
    return { active: !!active, range, headingRange, covered: range ? doc.sliceString(range.from, range.to) : null, headingEnd: heading.to, letFrom: letLine.from };
  },
};

window.grammarRun = async (name, spec) => {
  const scenario = scenarios[name];
  if (!scenario) throw new Error("no scenario " + name);
  const session = joinSession({ send: () => {}, mayEdit: true });
  let host = null, editor = null;
  try {
    const ids = {};
    ids[spec.path] = session.addText(spec.path, spec.text);
    host = document.createElement("section");
    document.body.append(host);
    editor = createClassComponent({ component: Editor, target: host, props: { session, format: spec.format, file: ids[spec.path], entry: spec.path } });
    await tick();
    let content = null;
    for (let attempt = 0; attempt < 100 && !content; attempt++) {
      content = host.querySelector(".cm-editor");
      if (!content) await pause(50);
    }
    if (!content) throw new Error("the editor did not mount");
    const view = EditorView.findFromDOM(content);
    await pause(600);
    return await scenario({ view, session, ids });
  } finally {
    editor?.$destroy();
    host?.remove();
    session.leave();
  }
};
`;

writeFileSync(entry, [
  "import { tick } from " + imports("web/node_modules/svelte/src/index-client.js") + ";",
  "import { EditorView } from " + imports("web/node_modules/@codemirror/view/dist/index.js") + ";",
  "import { syntaxTree, ensureSyntaxTree, foldable } from " + imports("web/node_modules/@codemirror/language/dist/index.js") + ";",
  "import { createClassComponent } from " + imports("web/node_modules/svelte/src/legacy/legacy-client.js") + ";",
  "import { join as joinSession } from " + imports("web/src/lib/collab.js") + ";",
  "import Editor from " + imports("web/src/components/Editor.svelte") + ";",
  "globalThis.LIBREPAPER_MODULES = { typst: " + JSON.stringify(typstUrl) + ", \"typst-syntax\": " + JSON.stringify(syntaxUrl) + " };",
  pageSource,
].join("\n"));

const checks = {
  "latex-highlight"(r) {
    assert.ok(r.math.includes("InlineMath"), "latex-highlight: x^2 should sit in InlineMath: " + JSON.stringify(r.math));
    assert.ok(r.verbatim.includes("VerbBody"), "latex-highlight: the verbatim text should be a VerbBody: " + JSON.stringify(r.verbatim));
    assert.ok(!r.verbatim.includes("InlineMath"), "latex-highlight: $ inside verbatim must not open math: " + JSON.stringify(r.verbatim));
    assert.ok(r.section.includes("SectionName"), "latex-highlight: \\section should be a SectionName: " + JSON.stringify(r.section));
    assert.ok(r.classes.length > 0, "latex-highlight: the first line should carry highlight classes");
  },
  "latex-autoclose"(r) {
    assert.equal(r.before, "\\documentclass{article}\n\\begin{itemize", "latex-autoclose: nothing should close before the }: " + JSON.stringify(r.before));
    assert.ok(r.typed.endsWith("\\begin{itemize}\n\\end{itemize}"), "latex-autoclose: expected \\end{itemize} after the }, got " + JSON.stringify(r.typed));
    assert.equal(r.head, r.typed.length - "\n\\end{itemize}".length, "latex-autoclose: the caret should sit right after \\begin{itemize}");
    assert.equal(r.crdt, r.typed, "latex-autoclose: the collaborative text should match the editor");
    // Assumes one undo step removes the } and the \end together. The 1.3 s pause before the } keeps it out of the typing's step.
    assert.ok(!r.undone.includes("\\end{itemize}"), "latex-autoclose: undo should remove \\end{itemize}: " + JSON.stringify(r.undone));
    assert.ok(!r.undone.includes("\\begin{itemize}"), "latex-autoclose: undo should remove the } as well: " + JSON.stringify(r.undone));
    assert.equal(r.undoneCrdt, r.undone, "latex-autoclose: the collaborative text should match the editor after undo");
  },
  "markdown-div-fold"(r) {
    assert.ok(r.range, "markdown-div-fold: the ::: line should be foldable");
    assert.deepEqual(r.range, { from: r.expectFrom, to: r.expectTo }, "markdown-div-fold: wrong fold range");
    for (const name of ["FencedDiv", "InlineMath", "Citation"]) assert.ok(r.names.includes(name), "markdown-div-fold: no " + name + " node in " + JSON.stringify(r.names));
  },
  "typst-fold"(r) {
    assert.ok(r.active, "typst-fold: the wasm language never produced a Heading node (is web/wasm/typst-syntax.wasm present?)");
    assert.ok(r.range, "typst-fold: the #let block never became foldable (is web/wasm/typst.wasm present?)");
    assert.equal(r.covered, "\n  x + 1\n", "typst-fold: wrong fold range " + JSON.stringify(r.range));
    assert.ok(r.headingRange, "typst-fold: the heading should be foldable");
    assert.equal(r.headingRange.from, r.headingEnd, "typst-fold: the heading fold should start at the end of its line");
    assert.ok(r.headingRange.to > r.letFrom, "typst-fold: the heading fold should cover its section");
  },
};

let server, browser, socket;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "source-grammar-check.js" } } });
  server = createServer((request, response) => {
    if (request.url === typstUrl) {
      if (!existsSync(typstWasm)) { response.statusCode = 404; response.end(); return; }
      response.setHeader("Content-Type", "application/wasm"); response.end(readFileSync(typstWasm)); return;
    }
    if (request.url === syntaxUrl) {
      if (!existsSync(syntaxWasm)) { response.statusCode = 404; response.end(); return; }
      response.setHeader("Content-Type", "application/wasm"); response.end(readFileSync(syntaxWasm)); return;
    }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); }
    else { response.setHeader("Content-Type", "text/html"); response.end("<body><script type=\"module\" src=\"/source-grammar-check.js\"></script></body>"); }
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const httpPort = server.address().port;
  browser = spawn(requireChromiumExecutable(), ["--headless=new", "--no-sandbox", "--disable-gpu", "--user-data-dir=" + profile, "--remote-debugging-port=" + port, "about:blank"], { stdio: "ignore", detached: true });
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  let info;
  for (let attempt = 0; attempt < 100; attempt++) { try { info = await fetch("http://127.0.0.1:" + port + "/json").then((r) => r.json()); break; } catch { await wait(50); } }
  const page = info?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.addEventListener("open", resolve, { once: true }); socket.addEventListener("error", reject, { once: true }); });
  let sequence = 0; const pending = new Map();
  socket.addEventListener("message", (event) => { const message = JSON.parse(event.data); const resolve = pending.get(message.id); if (resolve) { pending.delete(message.id); resolve(message); } });
  const send = (method, params = {}) => new Promise((resolve) => { const id = ++sequence; pending.set(id, resolve); socket.send(JSON.stringify({ id, method, params })); });
  const evaluate = async (expression) => { const message = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }); if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails)); return message.result.result.value; };
  await send("Page.navigate", { url: "http://127.0.0.1:" + httpPort + "/" });
  for (let attempt = 0; attempt < 100 && !(await evaluate("typeof grammarRun === 'function' && grammarReady()")); attempt++) await wait(50);

  for (const [name, check] of Object.entries(checks)) {
    check(await evaluate(`grammarRun(${JSON.stringify(name)}, ${JSON.stringify(texts[name])})`));
    console.log(`source-grammar-browser: ${name} ok`);
  }
  console.log("source-grammar-browser: LaTeX grammar highlights math and verbatim, \\begin auto-closes in one undo step and reaches the CRDT, Pandoc divs fold, Typst folds from the module");
} finally { socket?.close(); await stopBrowserProcess(browser); server?.close(); removeTemporary(temporary); }
