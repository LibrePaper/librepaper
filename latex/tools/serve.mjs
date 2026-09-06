#!/usr/bin/env node
// The mirror, served, so a browser can be measured against it.
//
// In a deployment the mirror is a bucket behind `--latex <url>` and this file
// does not exist. It exists for the tests, and it does three things a plain
// static server does not.
//
// It counts. Every response's bytes are recorded per path, and `/__bytes`
// hands the tally back, which is how "bytes fetched up front" and "bytes
// fetched to compile this document" become numbers rather than guesses.
// `/__reset` starts a new tally, which is how a cold cache is simulated
// without restarting anything.
//
// It speaks SwiftLaTeX's package protocol. That engine does not fetch a URL a
// caller chose; it builds `<endpoint>/<engine>/<format>/<name>` itself, in C,
// over synchronous XHR, and wants a `fileid` header back and a 301 -- not a
// 404 -- to mean "no such file". So `/packages/...` answers from the mirror's
// package half. With `--record`, a name the mirror does not have is fetched
// once from upstream or found on this machine and written in, which is how
// the package list was collected in the first place.
//
// And it serves the corpus and `web/src` beside the mirror, so the headless
// check can load the real modules from source rather than a build. Nothing
// here is reachable from a deployment.
//
//     node latex/serve.mjs [--port 8300] [--mirror latex/mirror] [--record]

import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join, normalize } from "node:path";
import { createHash } from "node:crypto";
import { readManifest, addPackages } from "./mirror.mjs";

const HERE = dirname(new URL(import.meta.url).pathname);
// This file is latex/tools/serve.mjs, so the repository is two directories up.
const REPO = dirname(dirname(HERE));

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const at = argv.indexOf(name);
  return at < 0 ? fallback : argv[at + 1];
};

const MIRROR = flag("--mirror", join(REPO, "latex", "mirror"));
const RECORD = argv.includes("--record");

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
};
const typeOf = (path) => TYPES[path.slice(path.lastIndexOf("."))] || "application/octet-stream";

/// What has been served since the last reset, in bytes, by path. A compile's
/// download is the difference between two readings.
const tally = { total: 0, files: {} };

/// The roots served, in the order tried. Every path is normalised and then
/// checked to be inside its root, because this reads files off a developer's
/// disk and a `..` must not walk out of the tree even in a test server.
const ROOTS = [
  ["/mirror/", MIRROR],
  ["/src/", join(REPO, "web", "src")],
  ["/examples/", join(REPO, "latex", "corpus")],
  ["/", join(REPO, "latex", "harness")],
];

function resolve(url) {
  for (const [prefix, root] of ROOTS) {
    if (!url.startsWith(prefix)) continue;
    const path = normalize(join(root, decodeURIComponent(url.slice(prefix.length))));
    if (!path.startsWith(root)) return null;
    if (existsSync(path) && statSync(path).isFile()) return path;
  }
  return null;
}

/// SwiftLaTeX's kpathsea callback, answered from the mirror.
async function servePackage(url, response) {
  // `<engine>/<format code>/<name>`; the format code is the engine's idea of
  // what kind of file it wants and does not change the answer.
  const namespace = url.startsWith("/packages-v2/") ? "/packages-v2/" : "/packages/";
  const parts = url.slice(namespace.length).split("/");
  const engine = parts[0] === "xetex" ? "xetex" : "pdftex";
  const name = parts[parts.length - 1];
  // The format code is part of the key. A font is asked for as `cmr10`, and
  // only the code says whether that is a `.tfm` or a `.pfb`.
  const format = parts.length > 2 ? parts[1] : "0";
  const key = `${engine}/${format}/${name}`;
  let manifest = readManifest(MIRROR);
  let entry = manifest.packages?.[key];
  // Recording fetches a name the mirror does not have, and asks upstream
  // once more for one it has only from the TeX Live on this machine, since
  // the engine's format may be older than that file expects.
  if (RECORD && !manifest.absent?.[key] && (!entry || entry.from !== "upstream")) {
    await addPackages([name], engine, format, MIRROR, { replace: Boolean(entry) });
    manifest = readManifest(MIRROR);
    entry = manifest.packages?.[key];
  }
  if (!entry) {
    // 301, not 404: the engine reads a 301 as "this file does not exist" and
    // caches the absence. A 404 it treats as a network failure and retries.
    response.writeHead(301, { "content-type": "text/plain" });
    response.end("no such file");
    return;
  }
  const bytes = readFileSync(join(MIRROR, entry.url));
  count(entry.url, bytes.length);
  response.writeHead(200, {
    "content-type": "application/octet-stream",
    // The engine saves the file under this name in its own filesystem, so it
    // has to be stable for the same bytes and distinct for different ones.
    fileid: entry.sha256.slice(0, 32),
    "access-control-expose-headers": "fileid",
    "access-control-allow-origin": "*",
    // A fixed alias must be revalidated: the manifest can change its bytes.
    "cache-control": "no-cache",
  });
  response.end(bytes);
}

function count(path, bytes) {
  tally.total += bytes;
  tally.files[path] = (tally.files[path] || 0) + bytes;
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
    // The engine builds this URL itself, from the base it was given, so it
    // arrives under the mirror rather than beside it.
    if (url.startsWith("/mirror/packages/") || url.startsWith("/mirror/packages-v2/")) {
      return await servePackage(url.slice("/mirror".length), response);
    }

    const path = resolve(url === "/" ? "/index.html" : url);
    if (!path) {
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("not found");
      return;
    }
    const bytes = readFileSync(path);
    count(url, bytes.length);
    response.writeHead(200, {
      "content-type": typeOf(path),
      "access-control-allow-origin": "*",
      // The digest is in the path for everything under /mirror/, so a year is
      // safe there. Everything else is a developer's working copy.
      "cache-control": url.startsWith("/mirror/")
        ? "public, max-age=31536000, immutable"
        : "no-store",
    });
    response.end(bytes);
  } catch (error) {
    response.writeHead(500, { "content-type": "text/plain" });
    response.end(String(error));
  }
});

const PORT = Number(flag("--port", "8300"));
server.listen(PORT, () => {
  console.log(`latex: mirror on http://localhost:${PORT}/mirror/${RECORD ? " (recording)" : ""}`);
});

export { server };
