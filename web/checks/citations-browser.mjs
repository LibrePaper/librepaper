import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

const root = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const temporary = mkdtempSync(join(tmpdir(), "komodoc-citations-check-"));
const output = join(temporary, "build"), profile = join(temporary, "chrome"), entry = join(temporary, "entry.js");
const port = 22000 + Math.floor(Math.random() * 1000);
const imports = (path) => JSON.stringify(join(root, path));
writeFileSync(entry, [
  "import { tick } from " + imports("web/node_modules/svelte/src/index-client.js") + ";",
  "import { EditorView } from " + imports("web/node_modules/@codemirror/view/dist/index.js") + ";",
  "import { createClassComponent } from " + imports("web/node_modules/svelte/src/legacy/legacy-client.js") + ";",
  "import { join as joinSession } from " + imports("web/src/lib/collab.js") + ";",
  "import Editor from " + imports("web/src/components/Editor.svelte") + ";",
  "globalThis.KOMODOC_MODULES = { bibliography: '/bibliography.wasm' };",
  "const session = joinSession({ send: () => {}, mayEdit: true });",
  "const main = session.addText('paper.md', '---\\nbibliography: refs.bib\\n---\\n\\nSee ');",
  "session.addText('refs.bib', '@article{smith2020, author={Jane Smith}, title={A Study of Rivers}, year={2020}, journal={Nature}}');",
  "session.setMain(main);",
  "createClassComponent({ component: Editor, target: document.body, props: { session, format: 'markdown', file: main } });",
  "window.citationReady = async () => { await tick(); return Boolean(document.querySelector('.cm-editor')); };",
  "window.citationPrepare = () => { const view = EditorView.findFromDOM(document.querySelector('.cm-editor')); view.dispatch({ selection: { anchor: view.state.doc.length } }); view.focus(); };",
  "window.citationState = () => { const view = EditorView.findFromDOM(document.querySelector('.cm-editor')); return { popup: Boolean(document.querySelector('.cm-tooltip-autocomplete')), popupText: document.querySelector('.cm-tooltip-autocomplete')?.textContent || '', text: view.state.doc.toString() }; };",
].join("\n"));
let server, browser, socket;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "citations-check.js" } } });
  server = createServer((request, response) => {
    if (request.url === "/bibliography.wasm") { response.setHeader("Content-Type", "application/wasm"); response.end(readFileSync(join(root, "web/dist/wasm/bibliography.wasm"))); return; }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); }
    else { response.setHeader("Content-Type", "text/html"); response.end("<body><script type=\"module\" src=\"/citations-check.js\"></script></body>"); }
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const httpPort = server.address().port;
  browser = spawn("chromium", ["--headless=new", "--no-sandbox", "--disable-gpu", "--user-data-dir=" + profile, "--remote-debugging-port=" + port, "about:blank"], { stdio: "ignore" });
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
  for (let attempt = 0; attempt < 100 && !(await evaluate("typeof citationReady === 'function' && citationReady()")); attempt++) await wait(50);
  await evaluate("citationPrepare()");
  await send("Input.insertText", { text: "@" });
  await send("Input.insertText", { text: "Rivers" });
  let state;
  for (let attempt = 0; attempt < 100; attempt++) { state = await evaluate("citationState()"); if (state.popup) break; await wait(50); }
  assert.equal(state.popup, true, "typing @ and Rivers should open the picker");
  assert.match(state.popupText, /Smith.*Rivers/);
  assert.equal(state.text, "---\nbibliography: refs.bib\n---\n\nSee @Rivers");
  await wait(100);
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 });
  await wait(300);
  state = await evaluate("citationState()");
  assert.equal(state.popup, false, "accepting a citation should close the picker");
  assert.equal(state.text, "---\nbibliography: refs.bib\n---\n\nSee @smith2020");
  await send("Input.insertText", { text: " " });
  await wait(300);
  state = await evaluate("citationState()");
  assert.equal(state.popup, false, "a space after an accepted key should not reopen the picker");
  assert.equal(state.text, "---\nbibliography: refs.bib\n---\n\nSee @smith2020 ");
  console.log("citations-browser: analyzer-backed @ Rivers picker opened, inserted Smith, and stayed closed");
} finally { socket?.close(); browser?.kill(); server?.close(); rmSync(temporary, { recursive: true, force: true }); }
