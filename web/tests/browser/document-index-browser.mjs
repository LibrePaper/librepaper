import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { removeTemporary, stopBrowserProcess } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-document-index-check-"));
const output = join(temporary, "build"), profile = join(temporary, "chrome"), entry = join(temporary, "entry.js");
const port = 37000 + Math.floor(Math.random() * 1000);
const imports = (path) => JSON.stringify(join(root, path));

const project = {
  "main.tex": "\\documentclass{article}\n\\begin{document}\n\\input{intro}\nSee \\ref{fig:a} and \\ref{fig:missing}.\n\\end{document}\n",
  "intro.tex": "\\section{Intro}\n\\begin{figure}\n\\caption{A plot}\\label{fig:a}\n\\end{figure}\n",
};

// The browser side. Written as a raw string so its backslashes reach the
// browser unchanged; it contains no backticks and no template substitutions.
const pageSource = String.raw`
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
window.indexReady = async () => { await tick(); return Boolean(document.body); };
const press = (view, init) => view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
const readText = (session, id) => session.doc.getMap("files").get(id).toString();
// Puts the caret inside the name in a needle such as \ref{fig:a}.
const placeCaret = (view, needle, name) => {
  const at = view.state.doc.toString().indexOf(needle);
  if (at < 0) throw new Error("no " + needle + " in the document");
  view.focus();
  view.dispatch({ selection: { anchor: at + needle.indexOf(name) + 2 } });
};
// Presses F2 and renames the label at the caret to newName through the panel.
const renameThrough = async (view, newName) => {
  press(view, { key: "F2", code: "F2" });
  await pause(100);
  const input = document.querySelector(".cm-index-rename input");
  if (!input) return false;
  input.value = newName;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true, cancelable: true }));
  // A synthetic keydown does not submit the form the way a real Enter does, so
  // submit it here when the Enter did nothing and the panel is still open.
  if (document.querySelector(".cm-index-rename")) input.form.requestSubmit();
  return true;
};

const scenarios = {
  definition: async ({ view, session, ids }) => {
    placeCaret(view, "\\ref{fig:a}", "fig:a");
    press(view, { key: "F12", code: "F12" });
    await pause(200);
    return { doc: view.state.doc.toString(), head: view.state.selection.main.head, intro: readText(session, ids["intro.tex"]) };
  },
  diagnostics: async ({ view }) => {
    const collect = () => {
      const out = [];
      forEachDiagnostic(view.state, (d, from, to) => out.push({
        source: d.source,
        message: d.message,
        covered: view.state.doc.sliceString(from, to),
        actions: d.actions ? d.actions.length : 0,
      }));
      return out;
    };
    let found = [];
    for (let attempt = 0; attempt < 60; attempt++) {
      await pause(50);
      found = collect();
      if (found.some((d) => d.source === "librepaper-index")) break;
    }
    return found;
  },
  references: async ({ view }) => {
    placeCaret(view, "\\ref{fig:a}", "fig:a");
    press(view, { key: "F12", code: "F12", shiftKey: true });
    await pause(200);
    const panel = document.querySelector(".cm-index-references");
    return {
      panel: Boolean(panel),
      rows: panel ? panel.querySelectorAll("ul li button").length : 0,
      buttons: panel ? panel.querySelectorAll("button").length : 0,
    };
  },
  rename: async ({ view, session, ids }) => {
    placeCaret(view, "\\ref{fig:a}", "fig:a");
    const opened = await renameThrough(view, "fig:plot");
    await pause(300);
    return { opened, main: view.state.doc.toString(), intro: readText(session, ids["intro.tex"]) };
  },
  "rename-after-typing": async ({ view, session, ids }) => {
    const at = view.state.doc.toString().indexOf("\nSee ") + 1;
    view.dispatch({ changes: { from: at, insert: "Hello. " }, annotations: Transaction.userEvent.of("input.type") });
    placeCaret(view, "\\ref{fig:a}", "fig:a");
    const opened = await renameThrough(view, "fig:z");
    await pause(300);
    return { opened, main: view.state.doc.toString(), intro: readText(session, ids["intro.tex"]) };
  },
};

window.indexRun = async (name) => {
  const scenario = scenarios[name];
  if (!scenario) throw new Error("no scenario " + name);
  const session = joinSession({ send: () => {}, mayEdit: true });
  let host = null, editor = null;
  try {
    const ids = {};
    for (const [path, body] of Object.entries(project)) ids[path] = session.addText(path, body);
    host = document.createElement("section");
    document.body.append(host);
    editor = createClassComponent({ component: Editor, target: host, props: { session, format: "latex", file: ids["main.tex"], entry: "main.tex" } });
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
  "import { Transaction } from " + imports("web/node_modules/@codemirror/state/dist/index.js") + ";",
  "import { forEachDiagnostic } from " + imports("web/node_modules/@codemirror/lint/dist/index.js") + ";",
  "import { createClassComponent } from " + imports("web/node_modules/svelte/src/legacy/legacy-client.js") + ";",
  "import { join as joinSession } from " + imports("web/src/lib/collab.js") + ";",
  "import Editor from " + imports("web/src/components/Editor.svelte") + ";",
  "globalThis.LIBREPAPER_MODULES = {};",
  "const project = " + JSON.stringify(project) + ";",
  pageSource,
].join("\n"));

const checks = {
  definition(result) {
    const intro = project["intro.tex"];
    assert.equal(result.doc, intro, "definition: the editor should show intro.tex");
    assert.equal(result.intro, intro, "definition: intro.tex in the session should be unchanged");
    assert.equal(result.head, intro.indexOf("\\label{fig:a}") + "\\label{".length, "definition: the caret should sit on fig:a in the label");
  },
  diagnostics(result) {
    const mine = result.filter((diagnostic) => diagnostic.source === "librepaper-index");
    assert.equal(mine.length, 1, "diagnostics: expected one librepaper-index diagnostic: " + JSON.stringify(result));
    assert.ok(mine[0].covered.includes("fig:missing"), "diagnostics: the range should cover fig:missing, got " + JSON.stringify(mine[0].covered));
    assert.ok(mine[0].actions >= 1, "diagnostics: expected at least one suggestion");
  },
  references(result) {
    assert.ok(result.panel, "references: the .cm-index-references panel did not open");
    assert.equal(result.rows, 2, "references: expected one button per location (the ref and the label): " + JSON.stringify(result));
  },
  rename(result) {
    assert.equal(result.opened, true, "rename: the rename panel did not open");
    assert.ok(result.main.includes("\\ref{fig:plot}"), "rename: main.tex should reference fig:plot");
    assert.ok(!result.main.includes("\\ref{fig:a}"), "rename: main.tex should no longer reference fig:a");
    assert.ok(result.intro.includes("\\label{fig:plot}"), "rename: intro.tex should define fig:plot");
  },
  "rename-after-typing"(result) {
    assert.equal(result.opened, true, "rename-after-typing: the rename panel did not open");
    assert.ok(result.main.includes("Hello. See \\ref{fig:z}"), "rename-after-typing: expected a rename with fresh offsets, got " + JSON.stringify(result.main));
  },
};

let server, browser, socket;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "document-index-check.js" } } });
  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); }
    else { response.setHeader("Content-Type", "text/html"); response.end("<body><script type=\"module\" src=\"/document-index-check.js\"></script></body>"); }
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
  for (let attempt = 0; attempt < 100 && !(await evaluate("typeof indexRun === 'function' && indexReady()")); attempt++) await wait(50);

  for (const [name, check] of Object.entries(checks)) {
    check(await evaluate(`indexRun(${JSON.stringify(name)})`));
    console.log(`document-index-browser: ${name} ok`);
  }
} finally { socket?.close(); await stopBrowserProcess(browser); server?.close(); removeTemporary(temporary); }
