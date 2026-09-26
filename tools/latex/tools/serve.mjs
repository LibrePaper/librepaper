// A server for a LaTeX mirror on disk or at a URL and the harness page that drives
// it, for the browser checks. The mirror is
// built and deployed by the wasm-latex repository (`make mirror`, `make push`
// there; layout and manifest in wasm-latex/docs/mirror.md), and this serves it
// the way the Cloudflare Worker does -- `manifest.json` never cached,
// `bundles.json` revalidated, everything else under /mirror/ digest-named and
// cached for a year -- so what a check sees here is what a browser sees there.
//
// `/__bytes` reports bytes served since the last `/__reset`, by path, so a
// check can measure what a compile downloaded.
//
//     node tools/latex/tools/serve.mjs [--port 8300] [--mirror ../wasm-latex/mirror]
//
// The harness and the mirror share one origin because a nested engine Worker
// must be same-origin with the document that creates it.

import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join, normalize } from "node:path";

const HERE = dirname(new URL(import.meta.url).pathname);
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

function resolve(url) {
  for (const [prefix, root] of ROOTS) {
    if (!url.startsWith(prefix)) continue;
    const relative = decodeURIComponent(url.slice(prefix.length));
    const path = prefix === "/" && relative === ""
      ? join(root, "index.html")
      : normalize(join(root, relative));
    if (!path.startsWith(root)) return null;
    if (existsSync(path) && statSync(path).isFile()) return path;
  }
  return null;
}

function cacheControl(url) {
  if (url.endsWith("/manifest.json")) return "no-store";
  if (url.endsWith("/bundles.json")) return "no-cache";
  if (url.startsWith("/mirror/")) return "public, max-age=31536000, immutable";
  return "no-store";
}

const server = createServer(async (request, response) => {
  const url = request.url.split("?")[0];
  try {
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
    if (REMOTE && url.startsWith('/mirror/')) {
      const upstream = await fetch(new URL(url.slice('/mirror/'.length), REMOTE), {
        signal: AbortSignal.timeout(120000),
      });
      const bytes = Buffer.from(await upstream.arrayBuffer());
      count(url, bytes.length);
      response.writeHead(upstream.status, {
        'content-type': upstream.headers.get('content-type') || typeOf(url),
        'access-control-allow-origin': '*',
        'cache-control': upstream.headers.get('cache-control') || cacheControl(url),
      });
      response.end(bytes);
      return;
    }
    const path = resolve(url);
    if (!path) {
      response.writeHead(404, { "content-type": "text/plain", "access-control-allow-origin": "*" });
      response.end("not found");
      return;
    }
    const bytes = readFileSync(path);
    count(url, bytes.length);
    response.writeHead(200, {
      "content-type": typeOf(path),
      "access-control-allow-origin": "*",
      "cache-control": cacheControl(url),
    });
    response.end(bytes);
  } catch (error) {
    response.writeHead(500, { "content-type": "text/plain" });
    response.end(String(error));
  }
});

const PORT = Number(flag("--port", "8300"));
server.listen(PORT, () => {
  console.log(`latex: mirror on http://localhost:${PORT}/mirror/ from ${MIRROR}`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => process.exit(0));
}

export { server };
