// Harper in a real browser: the pinned wasm loads in its worker, a grammar
// slip is underlined, a fix from the hover card is applied, a word added to
// the dictionary stops being underlined, and a document that declares another
// language gets the browser's spellcheck instead.
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
const wasmPath = join(root, "web/wasm/harper.wasm");
if (!existsSync(wasmPath)) {
  console.log("harper-browser: web/wasm/harper.wasm missing, skipped");
  process.exit(0);
}
const temporary = mkdtempSync(join(tmpdir(), "librepaper-harper-check-"));
const output = join(temporary, "build"), profile = join(temporary, "chrome"), entry = join(temporary, "entry.js");
const port = 23000 + Math.floor(Math.random() * 1000);
// The worker checks the module against the digest in its URL, as the mirror lays it out.
const harperBytes = readFileSync(wasmPath);
const harperUrl = `/wasm/${createHash("sha256").update(harperBytes).digest("hex")}/harper.wasm`;
const imports = (path) => JSON.stringify(join(root, path));
writeFileSync(entry, [
  "import { tick } from " + imports("web/node_modules/svelte/src/index-client.js") + ";",
  "import { EditorView } from " + imports("web/node_modules/@codemirror/view/dist/index.js") + ";",
  "import { createClassComponent } from " + imports("web/node_modules/svelte/src/legacy/legacy-client.js") + ";",
  "import { join as joinSession } from " + imports("web/src/lib/collab.js") + ";",
  "import Editor from " + imports("web/src/components/Editor.svelte") + ";",
  `globalThis.LIBREPAPER_MODULES = { harper: ${JSON.stringify(harperUrl)} };`,
  "const view = (host) => EditorView.findFromDOM(host.querySelector('.cm-editor'));",
  "window.mount = async (name, text) => {",
  "  const host = document.createElement('section'); host.id = name; document.body.append(host);",
  "  const session = joinSession({ send: () => {}, mayEdit: true });",
  "  const file = session.addText(name + '.md', text);",
  "  createClassComponent({ component: Editor, target: host, props: { session, format: 'markdown', file, entry: name + '.md', proofing: 'harper' } });",
  "  await tick(); return Boolean(host.querySelector('.cm-editor'));",
  "};",
  "window.marks = (name) => [...document.querySelectorAll('#' + name + ' .cm-harper-grammar, #' + name + ' .cm-harper-spelling')].map((node) => node.className.includes('spelling') ? 'spelling:' + node.textContent : 'grammar:' + node.textContent);",
  "window.textOf = (name) => view(document.getElementById(name)).state.doc.toString();",
  "window.spellcheckOf = (name) => document.querySelector('#' + name + ' .cm-content').getAttribute('spellcheck');",
  "window.point = (name, word) => { const v = view(document.getElementById(name)); const box = v.coordsAtPos(v.state.doc.toString().indexOf(word) + 1); return { x: box.left + 1, y: (box.top + box.bottom) / 2 }; };",
  "window.cardButtons = () => [...document.querySelectorAll('.cm-harper-card button')].map((node) => node.textContent);",
  "window.press = (label) => { const button = [...document.querySelectorAll('.cm-harper-card button')].find((node) => node.textContent === label); if (!button) return false; button.click(); return true; };",
  "window.stored = () => localStorage.getItem('librepaper-harper-words');",
].join("\n"));
let server, browser, socket;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "harper-check.js" } } });
  server = createServer((request, response) => {
    if (request.url === harperUrl) { response.setHeader("Content-Type", "application/wasm"); response.end(harperBytes); return; }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); }
    else { response.setHeader("Content-Type", "text/html"); response.end("<body><script type=\"module\" src=\"/harper-check.js\"></script></body>"); }
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
  const until = async (expression, ok, attempts = 600) => { let value; for (let attempt = 0; attempt < attempts; attempt++) { value = await evaluate(expression); if (ok(value)) return value; await wait(50); } return value; };
  await send("Page.navigate", { url: "http://127.0.0.1:" + httpPort + "/" });
  await until("typeof mount === 'function'", Boolean, 100);

  assert.equal(await evaluate("mount('english', 'Its a known issue and we could of done better. The estimand is `cod of` here.')"), true);
  // The engine loads in the worker on the first check, which takes seconds.
  const started = Date.now();
  let marks = await until("marks('english')", (list) => list.length >= 3);
  console.log(`harper-browser: first underlines after ${Date.now() - started} ms: ${JSON.stringify(marks)}`);
  assert.ok(marks.includes("grammar:Its"), "its/it's should be underlined");
  assert.ok(marks.includes("grammar:could of"), "could of should be underlined");
  assert.ok(marks.includes("spelling:estimand"), "estimand is not in Harper's dictionary");
  assert.ok(!marks.some((mark) => mark.endsWith("cod of")), "inline code is markup and is never checked");
  assert.equal(await evaluate("spellcheckOf('english')"), "false", "Harper replaces the browser's spellcheck");

  let point = await evaluate("point('english', 'could of')");
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: point.x, y: point.y });
  const buttons = await until("cardButtons()", (list) => list.length > 0, 100);
  assert.ok(buttons.includes("could have"), "the card offers Harper's fix: " + JSON.stringify(buttons));
  assert.equal(await evaluate("press('could have')"), true);
  assert.match(await evaluate("textOf('english')"), /we could have done better/);
  marks = await until("marks('english')", (list) => !list.includes("grammar:could of"));
  assert.ok(!marks.includes("grammar:could of"), "the fixed slip is no longer underlined");
  console.log("harper-browser: a fix from the hover card is applied");

  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 1, y: 1 });
  await wait(300);
  point = await evaluate("point('english', 'estimand')");
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: point.x, y: point.y });
  await until("cardButtons()", (list) => list.includes("Add to dictionary"), 100);
  assert.equal(await evaluate("press('Add to dictionary')"), true);
  marks = await until("marks('english')", (list) => !list.includes("spelling:estimand"));
  assert.ok(!marks.includes("spelling:estimand"), "an added word is no longer underlined");
  assert.deepEqual(JSON.parse(await evaluate("stored()")), ["estimand"]);
  console.log("harper-browser: an added word stops being underlined and is remembered");

  assert.equal(await evaluate("mount('french', '---\\nlang: fr\\n---\\n\\nNous avons un probleme ici.')"), true);
  await wait(1500);
  assert.deepEqual(await evaluate("marks('french')"), [], "Harper never checks a document that declares another language");
  assert.equal(await evaluate("spellcheckOf('french')"), "true", "a French document gets the browser's spellcheck");
  console.log("harper-browser: a French document gets the browser's spellcheck instead");
} finally { socket?.close(); await stopBrowserProcess(browser); server?.close(); removeTemporary(temporary); }
