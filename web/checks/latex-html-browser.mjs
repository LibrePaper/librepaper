// Actual LaTeXML WASM through the application's adapter. The overlay serves
// freshly built engine files beside the existing mirror's verified bundles;
// no release, deployment, or existing mirror is modified by this check.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { readFileSync, writeFileSync, readdirSync, mkdirSync, symlinkSync, mkdtempSync, rmSync } from "node:fs";
import { resolve, join, extname } from "node:path";
import { tmpdir } from "node:os";
import { browser, until } from "../tools/browser-driver.mjs";

const root = resolve(import.meta.dirname, "../..");
const engineRoot = resolve(process.env.LATEXML_DIST || join(root, "../wasm-latex/wasm-build/dist"));
const mirror = resolve(process.env.MIRROR || join(root, "../wasm-latex/mirror"));
const mirrorUrl = process.env.LATEXML_MIRROR_URL;
const manifest = mirrorUrl
  ? await (await fetch(new URL("manifest.json", mirrorUrl))).json()
  : JSON.parse(readFileSync(join(mirror, "manifest.json"), "utf8"));
const release = structuredClone(manifest.releases[manifest.default_release]);
// Exercise the actual deployment manifest when checking an assembled mirror.
// Otherwise overlay freshly built artifacts for engine development.
const legacyRelease = Object.keys(manifest.releases).find(id => !manifest.releases[id].engines?.latexml);
const useMirror = Boolean(mirrorUrl) || process.env.LATEXML_USE_MIRROR === "1";
const engineFiles = [
  "latexml.worker.js", "latexml.js", "latexml.wasm", "kpse-resolve.js", "bundle-mode.js", "latexml.css",
  "LaTeXML-blue.css", "LaTeXML-marginpar.css", "LaTeXML-navbar-left.css", "LaTeXML-navbar-right.css",
  "ltx-amsart.css", "ltx-apj.css", "ltx-article.css", "ltx-book.css", "ltx-listings.css",
  "ltx-report.css", "ltx-svjour.css", "ltx-ulem.css",
];
if (useMirror) {
  assert.ok(release.engines.latexml, "the deployment mirror must advertise LaTeXML");
} else {
  release.engines.latexml = { worker: "../../latexml-test/latexml.worker.js", files: engineFiles };
  release.files = { ...release.files, ...Object.fromEntries(engineFiles.map((name) => {
    const bytes = readFileSync(join(engineRoot, name));
    return [name, { url: `latexml-test/${name}`, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") }];
  })) };
  manifest.releases[manifest.default_release] = release;
}
const contentTypes = { ".js": "text/javascript", ".wasm": "application/wasm", ".json": "application/json", ".css": "text/css" };
let bundleRequests = 0;
const server = createServer(async (req, res) => {
  try {
    const pathname = new URL(req.url, "http://localhost").pathname;
    if (pathname === "/") {
      res.setHeader("content-type", "text/html");
      res.end('<!doctype html><meta charset="utf-8"><script type="module">import {compile,cancel} from "/src/lib/latex/html.js";window.compile=compile;window.cancel=cancel;</script>');
      return;
    }
    // Workers require same-origin scripts. Proxy the hosted mirror just as
    // the application does, while retaining the published bytes and manifest.
    if (mirrorUrl && pathname.startsWith("/latex/")) {
      const response = await fetch(new URL(pathname.slice(7), mirrorUrl));
      res.writeHead(response.status, { "content-type": response.headers.get("content-type") || "application/octet-stream" });
      res.end(Buffer.from(await response.arrayBuffer()));
      return;
    }
    if (pathname === "/latex/manifest.json") {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(manifest));
      return;
    }
    let directory, relative;
    if (pathname.startsWith("/src/")) { directory = join(root, "web/src"); relative = pathname.slice(5); }
    else if (pathname.startsWith("/latex/latexml-test/")) { directory = engineRoot; relative = pathname.slice(20); }
    else if (pathname.startsWith("/latex/")) { directory = mirror; relative = pathname.slice(7); if (relative.endsWith(".tar")) bundleRequests++; }
    else { res.writeHead(404).end(); return; }
    const path = resolve(directory, relative);
    if (!path.startsWith(`${directory}/`)) { res.writeHead(403).end(); return; }
    res.setHeader("content-type", contentTypes[extname(path)] || "application/octet-stream");
    res.end(readFileSync(path));
  } catch (error) { res.writeHead(404).end(error.message); }
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const base = `http://127.0.0.1:${server.address().port}`;
const scratch = mkdtempSync(join(tmpdir(), "librepaper-latexml-browser-"));
let tab;
let app;
try {
  tab = await browser(process.env.BROWSER || "chromium", join(scratch, "profile"), 19000 + Math.floor(Math.random() * 1000));
  await tab.navigate(base);
  await until("adapter ready", () => tab.evaluate("typeof window.compile === 'function'"));
  const source = String.raw`\documentclass{article}
\usepackage{amsmath,graphicx}
\newcommand{\greeting}{First}
\begin{document}
\section{Browser conversion}
\greeting{} live paragraph.
\input{parts/section}
\begin{equation}\label{eq:test} y=\frac{x^2}{2}\end{equation}
See equation~\eqref{eq:test}.
\includegraphics{pixel.png}
\end{document}`;
  const png = Array.from(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aJXcAAAAASUVORK5CYII=", "base64"));
  async function render(text, chapter = "Included chapter content.", settings = {}) {
    const tree = { main: "main.tex", texts: { "main.tex": text, ...(chapter === null ? {} : { "parts/section.tex": chapter }) } };
    await tab.evaluate(`window.result = null; window.failure = null;
      window.compile({...${JSON.stringify(tree)},assets:{'pixel.png':Uint8Array.from(${JSON.stringify(png)})}}, {base:${JSON.stringify(`${base}/latex/`)},settings:${JSON.stringify(settings)}})
      .then(value=>window.result=value,error=>window.failure=error.message); true`);
    await until("LaTeXML conversion", () => tab.evaluate("window.result !== null || window.failure !== null"), 245000);
    const error = await tab.evaluate("window.failure");
    assert.equal(error, null);
    return JSON.parse(await tab.evaluate("JSON.stringify(window.result)"));
  }
  const first = await render(source);
  assert.equal(first.ok, true, first.log);
  assert.doesNotMatch(first.log || "", /no precompiled kernel dump found/, "the release must embed its kernel snapshots");
  assert.match(first.html, /First\s*(?:<[^>]+>\s*)*live paragraph/);
  assert.match(first.html, /Included chapter content/);
  assert.match(first.html, /<math\b/);
  assert.match(first.html, /data:image\/png;base64,/);
  assert.match(first.html, /<style\b/);
  const fetched = bundleRequests;
  const second = await render(source.replace("{First}", "{Second}"));
  assert.equal(second.ok, true, second.log);
  assert.match(second.html, /Second\s*(?:<[^>]+>\s*)*live paragraph/);
  assert.doesNotMatch(second.html, /First live paragraph/);
  assert.equal(bundleRequests, fetched, "warm edits reuse already downloaded packages");
  const missing = await render(source, null);
  assert.doesNotMatch(missing.html || "", /Included chapter content/, "deleted inputs must not survive a worker reuse");
  assert.ok(!missing.ok || /section|missing|not found/i.test(missing.log), "missing input must be diagnosed");
  console.log(`latex-html-browser: HTML, MathML, images, includes, warm edits and file deletion passed (cold ${first.seconds.toFixed(2)}s; warm ${second.seconds.toFixed(2)}s)`);

  if (legacyRelease) {
    const result = await render(source, "Included chapter content.", { release: legacyRelease });
    assert.equal(result.ok, true, "documents pinned before LaTeXML can render HTML from the current release");
    console.log("latex-html-browser: retained PDF release pin with current HTML renderer passed");
  }
  if (process.env.LIBREPAPER_BIN) {
    const port = 22000 + Math.floor(Math.random() * 1000);
    const appBase = `http://localhost:${port}`;
    const data = join(scratch, "data");
    const localMirror = join(scratch, "mirror");
    if (!mirrorUrl) {
      mkdirSync(localMirror);
      for (const name of readdirSync(mirror)) {
        if (name !== "manifest.json") symlinkSync(join(mirror, name), join(localMirror, name));
      }
      symlinkSync(engineRoot, join(localMirror, "latexml-test"));
      writeFileSync(join(localMirror, "manifest.json"), JSON.stringify(manifest));
    }
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LIBREPAPER_")));
    app = spawn(resolve(process.env.LIBREPAPER_BIN), ["serve", "--port", String(port), "--data", data,
      "--publishers", "anyone", "--commenters", "anyone", "--latex", mirrorUrl || localMirror], { env, stdio: ["ignore", "ignore", "pipe"] });
    let appLog = "";
    app.stderr.on("data", (bytes) => { appLog += bytes; });
    await until("app startup", async () => {
      if (app.exitCode !== null) throw new Error(appLog);
      return (await fetch(`${appBase}/api/config`)).ok;
    });
    await tab.navigate(appBase);
    await until("app page", () => tab.evaluate(`location.origin === ${JSON.stringify(appBase)} && document.readyState === 'complete'`));
    const appSource = String.raw`\documentclass{article}\begin{document}First app paragraph. $y=x^2$\end{document}`;
    const body = JSON.stringify({ title: "LaTeXML preview", source_format: "latex", source: appSource });
    const created = JSON.parse(await tab.evaluate(`(async () => {
      const response = await fetch('/api/documents', {method:'POST',headers:{'content-type':'application/json','x-librepaper-client':'1'},body:${JSON.stringify(body)}});
      return JSON.stringify({status:response.status,body:await response.json()});
    })()`));
    assert.equal(created.status, 201, JSON.stringify(created.body));
    const { slug } = created.body;
    await tab.resize(1300, 900);
    await tab.navigate(`${appBase}/docs/${slug}`);
    await until("initial PDF", async () => (await tab.text()).includes("First app paragraph"), 245000);
    console.log("latex-html-browser: app PDF ready");
    async function choose(label) {
      await tab.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='View' && b.getClientRects().length).click()`);
      await until(`${label} menu item`, () => tab.evaluate(`Array.from(document.querySelectorAll('[role="menuitem"]')).some(e=>e.textContent.replace('✓','').trim()===${JSON.stringify(label)} && e.getClientRects().length)`));
      await tab.evaluate(`(() => {
        const item = Array.from(document.querySelectorAll('[role="menuitem"]')).find(e=>e.textContent.replace('✓','').trim()===${JSON.stringify(label)} && e.getClientRects().length);
        item.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse',button:0}));
        item.click();
      })()`);
    }
    await choose("HTML");
    await until("HTML frame", () => tab.evaluate(`document.querySelector('iframe').src.includes('/raw/')`));
    await until("HTML preview", async () => (await tab.text()).includes("First app paragraph"), 245000);
    for (const marker of ["Second app paragraph", "Third app paragraph"]) {
      await tab.insert(appSource.replace("First app paragraph", marker), true);
      await until(marker, async () => (await tab.text()).includes(marker), 245000);
    }
    await tab.navigate(`${appBase}/docs/${slug}`);
    await until("remembered HTML", () => tab.evaluate(`document.querySelector('iframe')?.src.includes('/raw/')`));
    await until("remembered content", async () => (await tab.text()).includes("Third app paragraph"), 245000);
    await choose("PDF");
    await until("PDF frame", () => tab.evaluate(`document.querySelector('iframe').src.includes('/pdf/')`));
    await until("PDF preview after switch", async () => (await tab.text()).includes("Third app paragraph"), 245000);
    console.log("latex-html-browser: View menu, successive edits, remembered choice and PDF switch passed");
  }
} catch (error) {
  if (tab) {
    console.error("Page at failure:", await tab.evaluate("JSON.stringify({text:document.body.innerText,frames:Array.from(document.querySelectorAll('iframe')).map(f=>f.src),storage:{...localStorage}})").catch(() => "unavailable"));
  }
  throw error;
} finally {
  await tab?.close();
  app?.kill();
  await new Promise((done) => server.close(done));
  rmSync(scratch, { recursive: true, force: true });
}
