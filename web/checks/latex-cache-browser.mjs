// A legacy immutable HTTP response without fileid corrupts SwiftLaTeX's
// package filesystem. Reproduce that failure, then compile through the new
// endpoint without clearing the same browser's cache.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, readFileSync, writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join, resolve, extname } from "node:path";
import { tmpdir } from "node:os";
import { browser, until } from "../tools/browser-driver.mjs";

const root = resolve(dirname(new URL(import.meta.url).pathname), "../..");
const mirror = join(root, "latex/mirror");
if (!existsSync(join(mirror, "manifest.json"))) {
  throw new Error("This check requires the local SwiftLaTeX mirror (latex/mirror).");
}
const manifest = JSON.parse(readFileSync(join(mirror, "manifest.json"), "utf8"));
const adapter = readFileSync(join(root, "web/src/lib/latex/swiftlatex.js"), "utf8");
assert.ok(adapter.includes('new URL("packages-v2/", base)'), "the adapter must bypass legacy cache keys");
const legacy = adapter.replace('new URL("packages-v2/", base)', 'new URL("packages/", base)');
const mime = { ".js": "text/javascript", ".mjs": "text/javascript", ".json": "application/json", ".wasm": "application/wasm" };

for (const name of (process.argv[2] ? [process.argv[2]] : ["firefox", "chromium"])) {
  const directory = mkdtempSync(join(tmpdir(), "komodoc-latex-cache-"));
  let faulty = true;
  const requests = { legacy: 0, current: 0 };
  const server = createServer((request, response) => {
    const path = new URL(request.url, "http://localhost").pathname;
    if (path === "/") {
      response.writeHead(200, { "content-type": "text/html" });
      response.end(`<script type="module">
        import * as latex from '/src/lib/latex.js';
        import { create } from '/legacy.js';
        window.latex = latex;
        latex.at('/mirror/');
        window.makeLegacy = async () => create({
          name: 'swiftlatex-pdftex', base: new URL('/mirror/', location.href).href,
          distribution: (await fetch('/mirror/manifest.json').then(r => r.json())).distributions['swiftlatex-pdftex']
        });
        window.ready = true;
      </script>`);
      return;
    }
    if (path === "/legacy.js") {
      response.writeHead(200, { "content-type": "text/javascript" });
      response.end(legacy);
      return;
    }
    const packagePath = path.match(/^\/mirror\/(packages(?:-v2)?)\/(.+)$/);
    if (packagePath) {
      const old = packagePath[1] === "packages";
      requests[old ? "legacy" : "current"]++;
      const entry = manifest.packages[packagePath[2]];
      if (!entry) {
        response.writeHead(301, { "cache-control": "no-store" }).end("not found");
        return;
      }
      const headers = {
        "content-type": "application/octet-stream",
        "cache-control": old && faulty ? "public, max-age=31536000, immutable" : "no-cache",
      };
      if (!old || !faulty) headers.fileid = entry.sha256.slice(0, 16);
      response.writeHead(200, headers);
      response.end(readFileSync(join(mirror, entry.url)));
      return;
    }
    const servingRoot = path.startsWith("/src/") ? join(root, "web/src") : mirror;
    const relative = path.startsWith("/src/") ? path.slice(5) : path.slice("/mirror/".length);
    const file = resolve(servingRoot, relative);
    if (!file.startsWith(servingRoot + "/") || !existsSync(file)) {
      response.writeHead(404).end("not found");
      return;
    }
    response.writeHead(200, { "content-type": mime[extname(file)] || "application/octet-stream" });
    response.end(readFileSync(file));
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const base = `http://127.0.0.1:${server.address().port}`;
  let tab;
  try {
    tab = await browser(name, join(directory, "profile"), 20000 + Math.floor(Math.random() * 10000));
    await tab.navigate(base);
    await until("LaTeX harness", () => tab.evaluate("Boolean(window.ready)"));
    // The old engine caches every package at /tex/null. The next invocation
    // then reads a different package where it expects the format dump.
    const bad = JSON.parse(await tab.evaluate(`(async () => {
      window.tree = { main: 'main.tex', texts: { 'main.tex': ${JSON.stringify("\\documentclass{article}\n\\begin{document}\nFirst preview.\n\\end{document}")} }, assets: {} };
      const engine = await window.makeLegacy();
      await engine.compile(window.tree);
      const result = await engine.compile(window.tree);
      engine.close();
      return JSON.stringify({ pdf: result.pdf?.byteLength || 0, log: result.log });
    })()`));
    assert.equal(bad.pdf, 0);
    assert.match(bad.log, /Fatal format file error/, "reproduce the reported failure");
    faulty = false;
    const cached = JSON.parse(await tab.evaluate(`(async () => {
      const r = await fetch('/mirror/packages/pdftex/10/swiftlatexpdftex.fmt');
      return JSON.stringify({ fileid: r.headers.get('fileid'), bytes: (await r.arrayBuffer()).byteLength });
    })()`));
    assert.equal(cached.fileid, null, "the browser still has the obsolete response headers");
    assert.ok(cached.bytes > 1000000);
    const oldRequests = requests.legacy;
    await tab.evaluate("window.latex.choose('swiftlatex-pdftex')");
    for (const revision of ["First preview.", "Changed preview.", "Changed again."]) {
      const result = JSON.parse(await tab.evaluate(`(async () => {
        const source = ${JSON.stringify("\\documentclass{article}\n\\begin{document}\n")} + ${JSON.stringify(revision)} + ${JSON.stringify("\n\\end{document}")};
        const result = await window.latex.compile({ ...window.tree, texts: { 'main.tex': source } });
        return JSON.stringify({ pdf: result.pdf?.byteLength || 0, bytes: result.pdf ? Array.from(result.pdf) : [], log: result.log });
      })()`));
      assert.ok(result.pdf > 1000, result.log);
      const output = join(directory, "revision.pdf");
      writeFileSync(output, Buffer.from(result.bytes));
      const text = execFileSync("pdftotext", [output, "-"], { encoding: "utf8" });
      assert.ok(text.includes(revision), `PDF must contain the current revision: ${text}`);
    }
    assert.equal(requests.legacy, oldRequests, "the corrected compiler never uses legacy cache keys");
    assert.ok(requests.current > 0);
    console.log(`${name}: reproduced fatal format error; legacy cache retained; three PDF revisions passed via packages-v2`);
  } finally {
    await tab?.close();
    await new Promise((done) => server.close(done));
    rmSync(directory, { recursive: true, force: true });
  }
}
