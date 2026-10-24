import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { inflateRawSync } from "node:zlib";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { removeTemporary, stopBrowserProcess } from "../helpers/browser-driver.mjs";

// Reads one entry of a zip as text. It walks the central directory, which is always complete, unlike local headers.
function zipEntry(bytes, name) {
  const buffer = Buffer.from(bytes);
  let end = -1;
  for (let at = buffer.length - 22; at >= 0; at--) if (buffer.readUInt32LE(at) === 0x06054b50) { end = at; break; }
  assert.ok(end >= 0, "the zip has no end of central directory record");
  let at = buffer.readUInt32LE(end + 16);
  for (let left = buffer.readUInt16LE(end + 10); left > 0; left--) {
    assert.equal(buffer.readUInt32LE(at), 0x02014b50, "the zip central directory is damaged");
    const method = buffer.readUInt16LE(at + 10), size = buffer.readUInt32LE(at + 20);
    const nameLength = buffer.readUInt16LE(at + 28), extraLength = buffer.readUInt16LE(at + 30), commentLength = buffer.readUInt16LE(at + 32);
    const local = buffer.readUInt32LE(at + 42);
    if (buffer.toString("utf8", at + 46, at + 46 + nameLength) === name) {
      assert.equal(buffer.readUInt32LE(local), 0x04034b50, "the zip local header is damaged");
      const start = local + 30 + buffer.readUInt16LE(local + 26) + buffer.readUInt16LE(local + 28);
      const data = buffer.subarray(start, start + size);
      assert.ok(method === 0 || method === 8, `the zip entry ${name} uses unsupported method ${method}`);
      return (method === 8 ? inflateRawSync(data) : data).toString("utf8");
    }
    at += 46 + nameLength + extraLength + commentLength;
  }
  throw new Error(`the zip has no ${name} entry`);
}

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const names = ["markdown", "citations", "typst", "pandoc"];
const files = Object.fromEntries(names.map((name) => [name, join(root, "web/wasm", name + ".wasm")]));
const missing = names.filter((name) => !existsSync(files[name]));
if (missing.includes("pandoc") || missing.includes("typst")) {
  console.log(`exports-browser: no ${missing.join(", ")} module in web/wasm; skipping (fetch the wasm modules first)`);
  process.exit(0);
}

const temporary = mkdtempSync(join(tmpdir(), "librepaper-exports-check-"));
const output = join(temporary, "build"), profile = join(temporary, "chrome"), entry = join(temporary, "entry.js");
const port = 38000 + Math.floor(Math.random() * 1000);
// The renderer checks a module against the digest in its URL, as the mirror lays it out.
const urls = {}, served = new Map();
for (const name of names) {
  if (!existsSync(files[name])) continue;
  const bytes = readFileSync(files[name]);
  urls[name] = `/wasm/${createHash("sha256").update(bytes).digest("hex")}/${name}.wasm`;
  served.set(urls[name], bytes);
}
const imports = (path) => JSON.stringify(join(root, path));
writeFileSync(entry, [
  "import * as renderers from " + imports("web/src/lib/renderers.js") + ";",
  "import { pandoc } from " + imports("web/src/lib/pandoc/client.js") + ";",
  "import { exportFile, exportAvailability } from " + imports("web/src/lib/reader/exports.js") + ";",
  `globalThis.LIBREPAPER_MODULES = ${JSON.stringify(urls)};`,
  `const base64 = (bytes) => { let text = ""; for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000)); return btoa(text); };
  window.exportReady = () => true;
  window.exportAvailable = (format, target) => exportAvailability(format, target);
  window.exportRun = async (spec) => {
    const started = performance.now();
    try {
      const tree = { main: spec.main, texts: spec.texts, assets: Object.fromEntries(Object.entries(spec.assets || {}).map(([path, data]) => [path, Uint8Array.from(atob(data), (c) => c.charCodeAt(0))])), urls: {} };
      const { blob, name } = await exportFile({ target: spec.target, format: spec.format, tree, title: "T", slug: "t", preview: null, previewCurrent: false, render: renderers.render, run: pandoc });
      const bytes = new Uint8Array(await blob.arrayBuffer());
      return {
        type: blob.type, size: bytes.length, name, ms: performance.now() - started,
        head: String.fromCharCode(...bytes.subarray(0, 4)),
        text: spec.target === "html" ? new TextDecoder().decode(bytes) : "",
        base64: spec.target === "docx" ? base64(bytes) : "",
        error: null,
      };
    } catch (error) {
      return { error: String(error?.message || error), ms: performance.now() - started };
    }
  };`,
].join("\n"));

const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
const bib = readFileSync(join(root, "docs/examples/tutorial-markdown/references.bib"), "utf8").split(/\n(?=@)/).find((entry) => entry.includes("lovelace1843")).trim() + "\n";
const markdown = "---\ntitle: Notes\nbibliography: references.bib\nbibliography-style: apa\n---\n\n# Analytical Engine\n\nA paragraph with math $x^2$ and a citation [@lovelace1843].\n";
const paper = { format: "markdown", main: "main.md", texts: { "main.md": markdown, "references.bib": bib } };

let server, browser, socket;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error", build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "exports-check.js" } } });
  server = createServer((request, response) => {
    if (served.has(request.url)) { response.setHeader("Content-Type", "application/wasm"); response.end(served.get(request.url)); return; }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); }
    else { response.setHeader("Content-Type", "text/html"); response.end("<body><script type=\"module\" src=\"/exports-check.js\"></script></body>"); }
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
  for (let attempt = 0; attempt < 100 && !(await evaluate("typeof exportReady === 'function' && exportReady()")); attempt++) await wait(50);

  // The first Pandoc call compiles the 61 MB module, so each case gets two minutes.
  const run = async (name, spec) => {
    const result = await Promise.race([
      evaluate(`exportRun(${JSON.stringify(spec)})`),
      wait(120000).then(() => ({ error: "timed out after 120 s" })),
    ]);
    assert.equal(result.error, null, `${name}: the export failed: ${result.error}`);
    console.log(`exports-browser: ${name} ok (${Math.round(result.ms)} ms, ${result.size} bytes)`);
    return result;
  };
  const isZip = (result) => Buffer.from(result.base64, "base64");

  const pdf = await run("Markdown to PDF", { ...paper, target: "pdf" });
  assert.equal(pdf.head, "%PDF", "Markdown PDF does not start with %PDF");
  assert.equal(pdf.type, "application/pdf", "Markdown PDF has the wrong type");
  assert.equal(pdf.name, "t.pdf", "Markdown PDF has the wrong file name");

  const docx = await run("Markdown to DOCX", { ...paper, target: "docx" });
  const zip = isZip(docx);
  assert.equal(zip.subarray(0, 2).toString("latin1"), "PK", "Markdown DOCX is not a zip");
  assert.ok(zip.includes("word/document.xml"), "Markdown DOCX has no word/document.xml entry");
  assert.equal(docx.name, "t.docx", "Markdown DOCX has the wrong file name");
  assert.ok(zipEntry(zip, "word/document.xml").includes("<m:oMath"), "Markdown DOCX lost the math: word/document.xml has no <m:oMath");
  console.log("exports-browser: Markdown DOCX math survives ok");
  console.log(`exports-browser: timing: first Pandoc conversion ${Math.round(pdf.ms)} ms (PDF route), second ${Math.round(docx.ms)} ms (DOCX route)`);

  const html = await run("Markdown to HTML", { ...paper, target: "html" });
  assert.ok(html.text.includes("Analytical Engine"), "Markdown HTML lacks the heading text");
  assert.ok(html.text.includes("Lovelace"), "Markdown HTML lacks the cited author");

  const figure = await run("Markdown in a folder with an image to PDF", {
    format: "markdown", main: "paper/main.md", target: "pdf",
    texts: { "paper/main.md": "# Figure\n\n![fig](fig.png)\n" }, assets: { "paper/fig.png": png },
  });
  assert.equal(figure.head, "%PDF", "Markdown with an image does not produce a PDF");

  const page5 = { format: "html", main: "index.html", texts: { "index.html": "<h1>Hello</h1><p>World</p>" } };
  const htmlPdf = await run("HTML to PDF", { ...page5, target: "pdf" });
  assert.equal(htmlPdf.head, "%PDF", "HTML PDF does not start with %PDF");
  const htmlDocx = await run("HTML to DOCX", { ...page5, target: "docx" });
  assert.equal(isZip(htmlDocx).subarray(0, 2).toString("latin1"), "PK", "HTML DOCX is not a zip");

  const typstDocx = await run("Typst to DOCX", { format: "typst", main: "main.typ", target: "docx", texts: { "main.typ": "= Title\nSome text.\n" } });
  assert.equal(isZip(typstDocx).subarray(0, 2).toString("latin1"), "PK", "Typst DOCX is not a zip");

  const typstMath = "= Math heading\n\nA paragraph with inline math $x^2 + 1$ and a display equation.\n\n$ sum_(i=1)^n i $\n";
  const typstPaper = { format: "typst", main: "main.typ", texts: { "main.typ": typstMath } };
  const typstPdf = await run("Typst math to PDF", { ...typstPaper, target: "pdf" });
  assert.equal(typstPdf.head, "%PDF", "Typst math PDF does not start with %PDF");
  const typstHtml = await run("Typst math to HTML", { ...typstPaper, target: "html" });
  assert.ok(typstHtml.text.includes("Math heading"), "Typst math HTML lacks the heading text");
  const typstMathDocx = await run("Typst math to DOCX", { ...typstPaper, target: "docx" });
  const typstMathZip = isZip(typstMathDocx);
  assert.equal(typstMathZip.subarray(0, 2).toString("latin1"), "PK", "Typst math DOCX is not a zip");
  console.log(`exports-browser: Typst DOCX math: ${zipEntry(typstMathZip, "word/document.xml").includes("<m:oMath") ? "present" : "absent"}`);

  const quarto = { format: "quarto", main: "main.qmd", texts: { "main.qmd": "---\ntitle: Quarto check\n---\n\nA Quarto paragraph.\n\n```{r}\n1 + 1\n```\n" } };
  const quartoPdf = await run("Quarto to PDF", { ...quarto, target: "pdf" });
  assert.equal(quartoPdf.head, "%PDF", "Quarto PDF does not start with %PDF");
  const quartoHtml = await run("Quarto to HTML", { ...quarto, target: "html" });
  assert.ok(quartoHtml.text.includes("Quarto check") || quartoHtml.text.includes("A Quarto paragraph"), "Quarto HTML lacks the title and the paragraph text");
  const quartoDocx = await run("Quarto to DOCX", { ...quarto, target: "docx" });
  assert.equal(isZip(quartoDocx).subarray(0, 2).toString("latin1"), "PK", "Quarto DOCX is not a zip");

  const unavailable = await evaluate(`exportAvailable("text", "pdf")`);
  assert.equal(unavailable.ok, false, "a plain text document should not be exportable as PDF");
  console.log("exports-browser: availability of text to PDF refused ok");
} finally { socket?.close(); await stopBrowserProcess(browser); server?.close(); removeTemporary(temporary); }
