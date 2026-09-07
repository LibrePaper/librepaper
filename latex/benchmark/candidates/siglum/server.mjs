#!/usr/bin/env node
import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { extname, join, normalize } from "node:path";

const root = new URL(".", import.meta.url).pathname;
const port = Number(process.argv[process.argv.indexOf("--port") + 1] || 8701);
const tally = { total: 0, files: {} };
const types = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8", ".wasm": "application/wasm",
  ".gz": "application/octet-stream", ".png": "image/png", ".pdf": "application/pdf",
};

function fileFor(url) {
  const relative = url === "/" ? "harness/index.html" : url.replace(/^\//, "");
  const mapped = relative.startsWith("src/") ? join("src", relative) : relative;
  // The checkout is nested as src/src; expose its package source at /src.
  const path = normalize(join(root, mapped));
  return path.startsWith(root) && existsSync(path) && statSync(path).isFile() ? path : null;
}
function headers(type) {
  return {
    "content-type": type,
    "cross-origin-opener-policy": "same-origin",
    "cross-origin-embedder-policy": "require-corp",
    "cross-origin-resource-policy": "same-origin",
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
  };
}
const server = createServer((request, response) => {
  const url = decodeURIComponent((request.url || "/").split("?")[0]);
  if (url === "/__bytes") {
    response.writeHead(200, headers("application/json"));
    response.end(JSON.stringify(tally));
    return;
  }
  if (url === "/__reset") {
    tally.total = 0; tally.files = {};
    response.writeHead(200, headers("application/json"));
    response.end("{}");
    return;
  }
  const path = fileFor(url);
  if (!path) {
    response.writeHead(404, headers("text/plain"));
    response.end("not found");
    return;
  }
  const body = readFileSync(path);
  tally.total += body.length;
  tally.files[url] = (tally.files[url] || 0) + body.length;
  response.writeHead(200, headers(types[extname(path)] || "application/octet-stream"));
  response.end(body);
});
server.listen(port, "127.0.0.1", () => console.log(`siglum harness on ${port}`));
