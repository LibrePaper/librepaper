import { createServer } from "node:http";
import { readFileSync, statSync, openSync, readSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, relative } from "node:path";
import { createHash } from "node:crypto";
import { createTinytexResolver } from "./tinytex-files.mjs";

const root = fileURLToPath(new URL(".", import.meta.url));
const wasmRoot = realpathSync(join(root, "../wasmtex/source"));
const disk = join(root, "../tinytex-cheerpx/assets/rootfs.ext2");
const diskStat = statSync(disk), fd = openSync(disk, "r");
const manifest = JSON.parse(readFileSync(join(root, "../wasmtex/downloads/manifest-2025.json")));
const expected = new Map(manifest.files.map(file => [file.name, file]));
const verified = new Map(), packages = new Map(), cachedResponses = new Map();
const tinytex = createTinytexResolver();
const metadata = {
  engineRelease: manifest.releaseId,
  engineSourceRevision: manifest.buildReceipts.find(r => r.family === "xetex").sourceRevision,
  wrapperCheckout: "44c5861fcdf729838205b00b96ac9509bc7fb677",
  texliveSnapshot: "TinyTeX-based frozen TeX Live 2025 final",
  packageTree: { sha256: tinytex.treeIdentity, files: tinytex.count, receipt: tinytex.receipt },
  supplementarySnapshot: "2025-92e10d3241a312f0 (WASM ICU data/font index only)",
  biberImage: JSON.parse(readFileSync(join(root, "../tinytex-cheerpx/assets/receipt.json"))),
};
let counts = { bytes: 0, requests: 0 };
const lifetime = { bytes: 0, requests: 0 };
const sha = data => createHash("sha256").update(data).digest("hex");
async function upstream(path, engine) {
  if (cachedResponses.has(path)) return cachedResponses.get(path);
  const job = (async () => {
    const url = engine ? "https://corca-ai.github.io/wasmtex" + path
      : "https://texlive.corca.ai/snapshots/2025-92e10d3241a312f0/2025/" + path.slice(6);
    const response = await fetch(url, { signal: AbortSignal.timeout(60000) });
    const body = Buffer.from(await response.arrayBuffer());
    if (response.ok) {
      const digest = sha(body);
      if (engine) {
        const name = path.slice("/wasmtex/2025/".length), pin = expected.get(name);
        if (!pin || pin.sha256 !== digest || pin.bytes !== body.length) throw Error("Engine checksum mismatch: " + name);
        verified.set(name, { sha256: digest, bytes: body.length });
      } else packages.set(path, { sha256: digest, bytes: body.length });
    }
    return { status: response.status, body, fileid: response.headers.get("fileid") };
  })();
  cachedResponses.set(path, job);
  try { return await job; } catch (e) { cachedResponses.delete(path); throw e; }
}
export const server = createServer(async (req, res) => {
  res.setHeader("Cross-Origin-Opener-Policy", "same-origin");
  res.setHeader("Cross-Origin-Embedder-Policy", "require-corp");
  res.setHeader("Cache-Control", "no-store");
  try {
    const path = new URL(req.url, "http://localhost").pathname;
    if (path === "/__reset") { counts = { bytes: 0, requests: 0 }; return res.end(); }
    if (path === "/__bytes" || path === "/__metadata") {
      res.setHeader("Content-Type", "application/json");
      return res.end(JSON.stringify(path === "/__bytes" ? { ...counts, lifetime } :
        { ...metadata, verifiedEngineAssets: Object.fromEntries(verified), packageFiles: Object.fromEntries(packages) }));
    }
    let body;
    if (path === "/assets/rootfs.ext2") {
      res.setHeader("Accept-Ranges", "bytes");
      res.setHeader("Last-Modified", diskStat.mtime.toUTCString());
      res.setHeader("Content-Type", "application/octet-stream");
      if (req.method === "HEAD") { res.setHeader("Content-Length", diskStat.size); return res.end(); }
      const match = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range || "");
      if (!match) { res.writeHead(400); return res.end("Range required"); }
      const start = Number(match[1]), end = Math.min(Number(match[2] || diskStat.size - 1), diskStat.size - 1);
      if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start > end || end - start >= 16 * 1024 * 1024) {
        res.writeHead(416); return res.end();
      }
      body = Buffer.alloc(end - start + 1);
      readSync(fd, body, 0, body.length, start);
      res.statusCode = 206;
      res.setHeader("Content-Range", `bytes ${start}-${end}/${diskStat.size}`);
    } else if (path.startsWith("/wasmtex/2025/") || path.startsWith("/2025/")) {
      const lookup = /^\/2025\/pdftex\/(\d+)\/(.+)$/.exec(path);
      let response;
      if (lookup && lookup[2] !== "xetexfontlist.txt") {
        const found = tinytex.resolve(lookup[1], decodeURIComponent(lookup[2]));
        response = { status: found ? 200 : 404, body: found ? Buffer.from(found.bytes) : Buffer.alloc(0), fileid: found?.fileid };
        if (found) packages.set(path, { sha256: found.sha256, bytes: response.body.length, source: relative(tinytex.root, found.sourcePath) });
      } else response = await upstream(path, path.startsWith("/wasmtex/"));
      body = response.body;
      res.statusCode = response.status;
      if (response.fileid) res.setHeader("fileid", response.fileid);
      res.setHeader("Content-Type", path.endsWith(".js") ? "text/javascript" : path.endsWith(".wasm") ? "application/wasm" : "application/octet-stream");
    } else {
      let file;
      if (path === "/") file = join(root, "index.html");
      else if (path === "/biber-bridge.js") file = join(root, "../tinytex-cheerpx/biber-bridge.js");
      else if (path.startsWith("/source/lib/")) {
        file = realpathSync(join(wasmRoot, decodeURIComponent(path.slice(8))));
        const rel = relative(wasmRoot, file);
        if (rel.startsWith("..") || rel.startsWith("/")) throw Error("Invalid source path");
      }
      if (!file) { res.writeHead(404); return res.end(); }
      body = readFileSync(file);
      res.setHeader("Content-Type", file.endsWith(".js") ? "text/javascript" : "text/html");
    }
    counts.bytes += body.length; counts.requests++;
    lifetime.bytes += body.length; lifetime.requests++;
    res.setHeader("Content-Length", body.length);
    res.end(body);
  } catch (error) {
    res.writeHead(error.code === "ENOENT" ? 404 : 502);
    res.end(String(error));
  }
});
server.listen(8706, "127.0.0.1", () => console.log("Hybrid: http://127.0.0.1:8706"));
