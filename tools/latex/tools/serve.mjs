// A server for a LaTeX mirror on disk or at a URL and the harness page that drives
// it, for the browser checks. The mirror is
// built and deployed by the wasm-latex repository (`make mirror`, `make push`
// there; layout in wasm-latex/docs/release.md), and this serves it the way the
// asset mirror does -- everything under /mirror/ is immutable and cached for a
// year -- so what a check sees here is what a browser sees there. The mirror
// directory holds one or more release directories, `<sha256>/`; a page
// pins one by using `/mirror/<sha256>/` as its LaTeX mirror URL.
//
// `/__bytes` reports bytes served since the last `/__reset`, by path, so a
// check can measure what a compile downloaded.
//
//     node tools/latex/tools/serve.mjs [--port 8300] [--mirror ../wasm-latex/mirror]
//
// The harness and the mirror share one origin because a nested engine Worker
// must be same-origin with the document that creates it.

import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { fetchMirror, mirrorTarget, resolveLocalFile } from "./serve-paths.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = dirname(dirname(dirname(HERE)));

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const at = argv.indexOf(name);
  return at >= 0 && argv[at + 1] !== undefined ? argv[at + 1] : fallback;
};
const MIRROR = flag("--mirror", join(REPO, "..", "wasm-latex", "mirror"));
const REMOTE = /^https?:\/\//i.test(MIRROR) ? MIRROR.replace(/\/?$/, '/') : null;

const TYPES = {
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".png": "image/png",
  ".pdf": "application/pdf",
  ".svelte": "text/javascript; charset=utf-8",
  ".tar": "application/x-tar",
  ".gz": "application/gzip",
};
const typeOf = (path) => TYPES[path.slice(path.lastIndexOf("."))] || "application/octet-stream";

/// What has been served since the last reset, in bytes, by path.
const tally = { total: 0, files: {} };
function count(path, bytes) {
  tally.total += bytes;
  tally.files[path] = (tally.files[path] || 0) + bytes;
}

/// The roots served, in the order tried. Every path is normalised and then
/// checked to be inside its root, because this reads files off a developer's
/// disk and a `..` must not walk out of the tree even in a test server.
const ROOTS = [
  ...(!REMOTE ? [["/mirror/", MIRROR]] : []),
  ["/src/", join(REPO, "web", "src")],
  ["/examples/", join(REPO, "tools", "latex", "corpus")],
  ["/", join(REPO, "tools", "latex", "harness")],
];

function resolvePath(url) {
  for (const [prefix, root] of ROOTS) {
    if (!url.startsWith(prefix)) continue;
    const relative = prefix === "/" && url === "/" ? "index.html" : url.slice(prefix.length);
    return resolveLocalFile(root, relative);
  }
  return null;
}

function cacheControl(url) {
  if (url.startsWith("/mirror/")) return "public, max-age=31536000, immutable";
  return "no-store";
}

const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, "http://localhost").pathname;
    if (url === "/__bytes") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify(tally));
      return;
    }
    if (url === "/__reset") {
      tally.total = 0;
      tally.files = {};
      response.writeHead(200, { "content-type": "application/json" });
      response.end("{}");
      return;
    }
    if (REMOTE && url.startsWith("/mirror/")) {
      const target = mirrorTarget(REMOTE, url.slice("/mirror/".length));
      if (!target) {
        response.writeHead(400, { "content-type": "text/plain" });
        response.end("invalid mirror path");
        return;
      }
      const upstream = await fetchMirror(target, {
        signal: AbortSignal.timeout(120000),
      });
      if (!upstream) {
        response.writeHead(502, { "content-type": "text/plain" });
        response.end("mirror redirects are not followed");
        return;
      }
      const bytes = Buffer.from(await upstream.arrayBuffer());
      count(url, bytes.length);
      response.writeHead(upstream.status, {
        'content-type': upstream.headers.get('content-type') || typeOf(url),
        'cache-control': upstream.headers.get('cache-control') || cacheControl(url),
      });
      response.end(bytes);
      return;
    }
    const path = resolvePath(url);
    if (!path) {
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("not found");
      return;
    }
    const bytes = readFileSync(path);
    count(url, bytes.length);
    response.writeHead(200, {
      "content-type": typeOf(path),
      "cache-control": cacheControl(url),
    });
    response.end(bytes);
  } catch (error) {
    response.writeHead(500, { "content-type": "text/plain" });
    response.end(String(error));
  }
});

const PORT = Number(flag("--port", "8300"));
server.listen(PORT, "127.0.0.1", () => {
  console.log(`latex: mirror on http://127.0.0.1:${server.address().port}/mirror/ from ${MIRROR}`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => process.exit(0));
}

export { server };
