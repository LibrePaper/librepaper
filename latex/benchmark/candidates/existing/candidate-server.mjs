#!/usr/bin/env node
import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join, normalize } from "node:path";
const root = new URL("../../../../", import.meta.url).pathname;
const candidate = new URL("./candidate/", import.meta.url).pathname;
const mirror = join(root, "latex/mirror");
const manifest = JSON.parse(readFileSync(join(mirror, "manifest.json"), "utf8"));
const mime = (p) => p.endsWith(".js") ? "text/javascript" : p.endsWith(".html") ? "text/html" : p.endsWith(".json") ? "application/json" : p.endsWith(".wasm") ? "application/wasm" : "application/octet-stream";
const safe = (base, rel) => { const p = normalize(join(base, decodeURIComponent(rel))); return p.startsWith(base) && existsSync(p) && statSync(p).isFile() ? p : null; };
const html = `<!doctype html><meta charset=utf-8><script type=module>
import { create } from '/candidate/lib/latex/texlyre.js';
globalThis.candidateCreate = create; globalThis.candidateReady = true;
</script>`;
const server = createServer((req, res) => {
  const u = new URL(req.url, "http://localhost");
  if (u.pathname === "/") { res.writeHead(200, { "content-type": "text/html", "access-control-allow-origin": "*" }); res.end(html); return; }
  let path = null;
  if (u.pathname.startsWith("/candidate/")) path = safe(candidate, u.pathname.slice("/candidate/".length));
  else if (u.pathname.startsWith("/src/")) path = safe(join(root, "web/src"), u.pathname.slice("/src/".length));
  else if (u.pathname.startsWith("/mirror/") && !u.pathname.startsWith("/mirror/packages/")) path = safe(mirror, u.pathname.slice("/mirror/".length));
  else if (u.pathname.startsWith("/mirror/packages/")) {
    const parts = u.pathname.slice("/mirror/packages/".length).split("/");
    const key = `${parts[0]}/${parts[1]}/${parts.slice(2).join("/")}`;
    const entry = manifest.packages?.[key];
    if (!entry) { res.writeHead(301, { "access-control-allow-origin": "*" }); res.end("no such file"); return; }
    path = join(mirror, entry.url);
  }
  if (!path) { res.writeHead(404, { "access-control-allow-origin": "*" }); res.end("not found"); return; }
  const bytes = readFileSync(path); res.writeHead(200, { "content-type": mime(path), "access-control-allow-origin": "*", "cache-control": "no-store" }); res.end(bytes);
});
server.listen(8703, "127.0.0.1");
