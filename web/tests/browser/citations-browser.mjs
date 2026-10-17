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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-citations-check-"));
const output = join(temporary, "build"), profile = join(temporary, "chrome"), entry = join(temporary, "entry.js");
const port = 22000 + Math.floor(Math.random() * 1000);
const imports = (path) => JSON.stringify(join(root, path));
writeFileSync(entry, [
  "import { tick } from " + imports("web/node_modules/svelte/src/index-client.js") + ";",
  "import { EditorView } from " + imports("web/node_modules/@codemirror/view/dist/index.js") + ";",
  "import { Transaction } from " + imports("web/node_modules/@codemirror/state/dist/index.js") + ";",
  "import { acceptCompletion } from " + imports("web/node_modules/@codemirror/autocomplete/dist/index.js") + ";",
  "import { createClassComponent } from " + imports("web/node_modules/svelte/src/legacy/legacy-client.js") + ";",
  "import { join as joinSession } from " + imports("web/src/lib/collab.js") + ";",
  "import { projectDirectory } from " + imports("web/src/lib/projection.js") + ";",
  "import { configure as configureDesktop } from " + imports("web/src/lib/desktop/client.js") + ";",
  "import Editor from " + imports("web/src/components/Editor.svelte") + ";",
  "globalThis.LIBREPAPER_MODULES = { bibliography: '/bibliography.wasm' };",
  "const session = joinSession({ send: () => {}, mayEdit: true });",
  "const main = session.addText('paper.md', '---\\nbibliography: refs.bib\\n---\\n\\nSee ');",
  "session.addText('refs.bib', '@article{smith2020, author={Jane Smith}, title={A Study of Rivers}, year={2020}, journal={Nature}}');",
  "session.setMain(main);",
  "createClassComponent({ component: Editor, target: document.body, props: { session, format: 'markdown', file: main } });",
  "window.citationReady = async () => { await tick(); return Boolean(document.querySelector('.cm-editor')); };",
  "window.citationPrepare = () => { const view = EditorView.findFromDOM(document.querySelector('.cm-editor')); view.dispatch({ selection: { anchor: view.state.doc.length } }); view.focus(); };",
  "window.citationState = () => { const view = EditorView.findFromDOM(document.querySelector('.cm-editor')); return { popup: Boolean(document.querySelector('.cm-tooltip-autocomplete')), popupText: document.querySelector('.cm-tooltip-autocomplete')?.textContent || '', text: view.state.doc.toString() }; };",
  `window.zoteroImportMainCheck = async (missingMain) => {
    const value = joinSession({ send: () => {}, mayEdit: true });
    const main = value.addText("paper.md", "See ");
    if (missingMain) {
      value.doc.getMap("meta").delete("main");
      value.doc.commit();
    }
    const fetchBefore = globalThis.fetch;
    const zoteroCalls = [];
    const bibliographyReports = [];
    const baseMeta = [...value.doc.getMap("meta").entries()];
    const host = document.createElement("section");
    document.body.append(host);
    const editor = createClassComponent({ component: Editor, target: host, props: {
      session: value, format: "markdown", file: main, me: { who: "me", author: "Me" },
      analyze: async () => ({ entries: [], diagnostics: [] }),
      onbibliography: (report) => bibliographyReports.push(report),
    } });
    try {
      configureDesktop({ project: "paper", origin: location.origin });
      window.__LIBREPAPER_DESKTOP__ = { base: "http://lp-local.localhost/", version: "test" };
      globalThis.fetch = async (input, init) => {
        const url = String(input);
        if (url.endsWith("/librepaper/local/capabilities")) return new Response(JSON.stringify({ builders: [] }), { status: 200, headers: { "content-type": "application/json" } });
        if (url.includes("/zotero/")) zoteroCalls.push({ url, authorization: init?.headers?.Authorization || "" });
        if (url.includes("/zotero/search?")) return new Response(JSON.stringify({ entries: [{
          citation_key: "Riv2024", authors: ["Jane Smith"], title: "Rivers", year: "2024", item_type: "article", zotero_item: "ITEM123",
        }] }), { status: 200, headers: { "content-type": "application/json" } });
        if (url.includes("/zotero/items/ITEM123")) return new Response(JSON.stringify({
          citation_key: "Riv2024", zotero_item: "ITEM123",
          bibtex: "@article{Riv2024,\\n  title={Rivers},\\n  x-librepaper-zotero-item={ITEM123}\\n}\\n",
        }), { status: 200, headers: { "content-type": "application/json" } });
        return fetchBefore(input, init);
      };
      await tick();
      editor.startTracking();
      await tick();
      const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
      const at = view.state.doc.length;
      view.dispatch({ changes: { from: at, insert: "@Riv" }, selection: { anchor: at + 4 }, annotations: Transaction.userEvent.of("input.type") });
      view.focus();
      let popup = "";
      for (let attempt = 0; attempt < 100; attempt++) {
        await new Promise((resolve) => setTimeout(resolve, 50));
        popup = host.querySelector(".cm-tooltip-autocomplete")?.textContent || "";
        if (popup.includes("Rivers")) break;
      }
      if (!popup.includes("Rivers")) throw new Error("Zotero fixture did not appear in the citation picker: " + JSON.stringify({ popup, calls: zoteroCalls, text: view.state.doc.toString() }));
      await new Promise((resolve) => setTimeout(resolve, 100));
      if (!acceptCompletion(view)) throw new Error("the Zotero picker could not accept its selected entry");
      for (let attempt = 0; attempt < 100; attempt++) {
        await new Promise((resolve) => setTimeout(resolve, 50));
        if (value.textOf(main).toString().includes("@Riv2024") && value.doc.getMap("changes").size > 0) break;
      }
      if (!value.textOf(main).toString().includes("@Riv2024")) throw new Error("tracked Zotero import did not reach the document: " + JSON.stringify({ text: value.textOf(main).toString(), calls: zoteroCalls, reports: bibliographyReports }));
      const projection = projectDirectory(value.doc, null);
      const entries = [...value.doc.getMap("changes").values()];
      return {
        mainState: missingMain ? "absent" : "explicit",
        text: value.textOf(main).toString(),
        mainId: main,
        bib: [...projection.files].find(([path]) => path === "references.bib")?.[1].id || "",
        projectedMain: projection.mainId,
        projectedPath: projection.main,
        bibliography: projection.texts.get("references.bib") || "",
        metadataUnchanged: JSON.stringify([...value.doc.getMap("meta").entries()]) === JSON.stringify(baseMeta),
        changeCount: entries.length,
        changeFiles: [...new Set(entries.map((entry) => (typeof entry.get === "function" ? entry.get("file") : entry.file)))],
      };
    } finally {
      globalThis.fetch = fetchBefore;
      delete window.__LIBREPAPER_DESKTOP__;
      editor.$destroy();
      host.remove();
      value.leave();
    }
  };`,
].join("\n"));
let server, browser, socket;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "citations-check.js" } } });
  server = createServer((request, response) => {
    if (request.url === "/bibliography.wasm") { response.setHeader("Content-Type", "application/wasm"); response.end(readFileSync(join(root, "web/wasm/bibliography.wasm"))); return; }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); }
    else { response.setHeader("Content-Type", "text/html"); response.end("<body><script type=\"module\" src=\"/citations-check.js\"></script></body>"); }
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
  for (const missingMain of [true, false]) {
    const zotero = await evaluate(`zoteroImportMainCheck(${missingMain})`);
    assert.match(zotero.text, /bibliography: references\.bib/);
    assert.match(zotero.text, /@Riv2024/);
    assert.notEqual(zotero.bib, "", "the Zotero import did not create references.bib");
    assert.match(zotero.bibliography, /x-librepaper-zotero-item=\{ITEM123\}/);
    assert.equal(zotero.projectedMain, zotero.mainId, "creating the bibliography displaced the projected paper main");
    assert.equal(zotero.projectedPath, "paper.md");
    assert.equal(zotero.metadataUnchanged, true, "the tracked import changed metadata");
    assert.ok(zotero.changeCount > 0, "the tracked import recorded no change");
    assert.deepEqual(zotero.changeFiles, [zotero.mainId], "the tracked import recorded a change in another file");
  }
  console.log("citations-browser: tracked Zotero imports are recorded as changes and preserve implicit and explicit main metadata");
} finally { socket?.close(); await stopBrowserProcess(browser); server?.close(); removeTemporary(temporary); }
