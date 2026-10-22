import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { removeTemporary, stopBrowserProcess } from "../helpers/browser-driver.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-completion-check-"));
const output = join(temporary, "build"), profile = join(temporary, "chrome"), entry = join(temporary, "entry.js");
const port = 37000 + Math.floor(Math.random() * 1000);
const typstWasm = process.env.LIBREPAPER_TYPST_WASM || join(root, "web/wasm/typst.wasm");
// The renderer checks a module against the digest in its URL, as the mirror lays it out.
const digestUrl = (file, name) => `/wasm/${createHash("sha256").update(readFileSync(file)).digest("hex")}/${name}.wasm`;
const bibliographyUrl = digestUrl(join(root, "web/wasm/bibliography.wasm"), "bibliography");
const typstUrl = existsSync(typstWasm) ? digestUrl(typstWasm, "typst") : "/wasm/missing/typst.wasm";
const imports = (path) => JSON.stringify(join(root, path));
writeFileSync(entry, [
  "import { tick } from " + imports("web/node_modules/svelte/src/index-client.js") + ";",
  "import { EditorView } from " + imports("web/node_modules/@codemirror/view/dist/index.js") + ";",
  "import { Transaction } from " + imports("web/node_modules/@codemirror/state/dist/index.js") + ";",
  "import { acceptCompletion, startCompletion } from " + imports("web/node_modules/@codemirror/autocomplete/dist/index.js") + ";",
  "import { createClassComponent } from " + imports("web/node_modules/svelte/src/legacy/legacy-client.js") + ";",
  "import { join as joinSession } from " + imports("web/src/lib/collab.js") + ";",
  "import Editor from " + imports("web/src/components/Editor.svelte") + ";",
  `globalThis.LIBREPAPER_MODULES = { bibliography: ${JSON.stringify(bibliographyUrl)}, typst: ${JSON.stringify(typstUrl)} };`,
  "const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));",
  "window.completionReady = async () => { await tick(); return Boolean(document.body); };",
  `window.completionRun = async (spec) => {
    const session = joinSession({ send: () => {}, mayEdit: true });
    try {
      const ids = {};
      for (const [path, body] of Object.entries(spec.texts)) ids[path] = session.addText(path, body);
      for (const path of spec.assets || []) session.putAsset(path, "0".repeat(64));
      session.setMain(ids[spec.file]);
      const host = document.createElement("section");
      document.body.append(host);
      const editor = createClassComponent({ component: Editor, target: host, props: { session, format: spec.format, file: ids[spec.file] } });
      try {
        await tick();
        let content = null;
        for (let attempt = 0; attempt < 100 && !content; attempt++) {
          content = host.querySelector(".cm-editor");
          if (!content) await pause(50);
        }
        if (!content) throw new Error("the editor did not mount");
        const view = EditorView.findFromDOM(content);
        const at = spec.at ?? view.state.doc.length;
        view.focus();
        view.dispatch({ changes: { from: at, insert: spec.type }, selection: { anchor: at + spec.type.length }, annotations: Transaction.userEvent.of("input.type") });
        view.focus();
        const deadline = Date.now() + spec.timeout;
        let popup = "";
        // Sources that load something first (the bibliography, the typst
        // module) answer nothing on the first keystroke. Asking again is what
        // Ctrl-Space does once they are ready.
        for (let attempt = 0; Date.now() < deadline; attempt++) {
          await pause(50);
          popup = document.querySelector(".cm-tooltip-autocomplete")?.textContent || "";
          if (popup.includes(spec.expect)) break;
          if (!popup && attempt % 10 === 9) startCompletion(view);
        }
        const result = { popup, text: view.state.doc.toString(), accepted: null };
        if (spec.accept && popup.includes(spec.expect)) {
          await pause(100);
          result.accepted = acceptCompletion(view);
          await pause(100);
          result.text = view.state.doc.toString();
        }
        return result;
      } finally {
        editor.$destroy();
        host.remove();
      }
    } finally {
      session.leave();
    }
  };`,
  `window.completionTypstSupport = async () => {
    const response = await fetch(${JSON.stringify(typstUrl)});
    if (!response.ok) return "missing";
    const module = await WebAssembly.compile(await response.arrayBuffer());
    return WebAssembly.Module.exports(module).some((item) => item.name === "complete") ? "ok" : "no-complete";
  };`,
].join("\n"));

const scenarios = [
  {
    name: "LaTeX \\ref labels from another project file",
    format: "latex", file: "main.tex",
    texts: { "main.tex": "\\begin{document}\n", "chapter.tex": "\\section{Introduction}\\label{sec:intro}\n" },
    type: "\\ref{", expect: "sec:intro",
  },
  {
    name: "LaTeX environment completion inserts the end",
    format: "latex", file: "main.tex",
    texts: { "main.tex": "\\begin{document}\n" },
    type: "\\begin{ite", expect: "itemize", accept: true,
  },
  {
    name: "LaTeX command defined in a project file",
    format: "latex", file: "main.tex",
    texts: { "main.tex": "\\begin{document}\n", "macros.tex": "\\newcommand{\\realline}{\\mathbb{R}}\n" },
    type: "\\realli", expect: "realline",
  },
  {
    name: "Markdown image path from project assets",
    format: "markdown", file: "notes.md",
    texts: { "notes.md": "# Notes\n\n" }, assets: ["figs/plot.png"],
    type: "![plot](", expect: "figs/plot.png",
  },
  {
    name: "Markdown heading anchor",
    format: "markdown", file: "notes.md",
    texts: { "notes.md": "# Results Section\n\nSee below.\n\n" },
    type: "[see](#", expect: "#results-section",
  },
  {
    name: "HTML tag names",
    format: "html", file: "page.html",
    texts: { "page.html": "<html><body>" },
    type: "<di", expect: "div",
  },
  {
    name: "Quarto cell option",
    format: "quarto", file: "report.qmd",
    texts: { "report.qmd": "```{r}\n" },
    type: "#| ec", expect: "echo",
  },
  {
    name: "Quarto front matter key",
    format: "quarto", file: "report2.qmd",
    texts: { "report2.qmd": "---\n\n---\n" }, at: 4,
    type: "tit", expect: "title",
  },
  {
    name: "Citation keys from refs.bib",
    format: "markdown", file: "paper.md",
    texts: {
      "paper.md": "---\nbibliography: refs.bib\n---\n\nSee ",
      "refs.bib": "@article{smith2020, author={Jane Smith}, title={A Study of Rivers}, year={2020}, journal={Nature}}",
    },
    type: "@smi", expect: "A Study of Rivers",
  },
];
const typstScenario = {
  name: "Typst function completion",
  format: "typst", file: "main.typ",
  texts: { "main.typ": "#set page(width: 10cm)\n" },
  type: "#lin", expect: "line", timeout: 15000,
};

let server, browser, socket;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "completion-check.js" } } });
  server = createServer((request, response) => {
    if (request.url === bibliographyUrl) { response.setHeader("Content-Type", "application/wasm"); response.end(readFileSync(join(root, "web/wasm/bibliography.wasm"))); return; }
    if (request.url === typstUrl) {
      if (!existsSync(typstWasm)) { response.statusCode = 404; response.end(); return; }
      response.setHeader("Content-Type", "application/wasm"); response.end(readFileSync(typstWasm)); return;
    }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); }
    else { response.setHeader("Content-Type", "text/html"); response.end("<body><script type=\"module\" src=\"/completion-check.js\"></script></body>"); }
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
  for (let attempt = 0; attempt < 100 && !(await evaluate("typeof completionReady === 'function' && completionReady()")); attempt++) await wait(50);

  const runScenario = async (scenario) => {
    const result = await evaluate(`completionRun(${JSON.stringify({ ...scenario, timeout: scenario.timeout ?? 5000 })})`);
    assert.ok(result.popup.includes(scenario.expect), `${scenario.name}: expected "${scenario.expect}" in the completion popup: ${JSON.stringify(result.popup)}`);
    if (scenario.accept) {
      assert.equal(result.accepted, true, `${scenario.name}: the popup did not accept its selection`);
      assert.match(result.text, /\\end\{itemize\}/, `${scenario.name}: accepting did not insert the end`);
    }
    console.log("completion-browser: " + scenario.name + " ok");
  };
  let passed = 0;
  for (const scenario of scenarios) { await runScenario(scenario); passed++; }

  const typst = await evaluate("completionTypstSupport()");
  if (typst === "missing") console.log("typst completion skipped: no typst module at " + typstWasm);
  else if (typst === "no-complete") console.log("typst completion skipped: module has no complete export");
  else { await runScenario(typstScenario); passed++; }
  console.log(`completion-browser: ${passed} scenarios passed`);
} finally { socket?.close(); await stopBrowserProcess(browser); server?.close(); removeTemporary(temporary); }
