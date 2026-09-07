import { createServer } from 'node:http';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { extname, resolve } from 'node:path';
import { gzipSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('.', import.meta.url));
const compressed = new Map();
let counters = { bytes: 0, uncompressedBytes: 0, requests: 0, paths: {} };
const lifetime = { bytes: 0, uncompressedBytes: 0, requests: 0 };
export const server = createServer((req, res) => {
  const url = new URL(req.url, 'http://localhost');
  if (url.pathname === '/__reset') counters = { bytes: 0, uncompressedBytes: 0, requests: 0, paths: {} };
  if (url.pathname.startsWith('/__')) {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    return res.end(JSON.stringify({ ...counters, lifetime }));
  }
  const rel = url.pathname === '/' ? 'index.html' : decodeURIComponent(url.pathname.slice(1));
  if (!(rel === 'index.html' || rel === 'worker.js' || /^assets\/(?:[^/]+|objects\/[^/]+)$/.test(rel))) { res.writeHead(404); return res.end(); }
  const path = resolve(root, rel);
  if (!path.startsWith(root) || !existsSync(path) || !statSync(path).isFile()) { res.writeHead(404); return res.end(); }
  const data = readFileSync(path);
  const gzip = /\bgzip\b/.test(req.headers['accept-encoding'] || '');
  if (gzip && !compressed.has(path)) compressed.set(path, gzipSync(data));
  const body = gzip ? compressed.get(path) : data;
  counters.bytes += body.length; counters.uncompressedBytes += data.length; counters.requests++;
  lifetime.bytes += body.length; lifetime.uncompressedBytes += data.length; lifetime.requests++;
  counters.paths[rel] = (counters.paths[rel] || 0) + body.length;
  const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json' };
  res.writeHead(200, {
    'content-type': types[extname(path)] || 'application/octet-stream',
    'content-length': body.length,
    ...(gzip ? { 'content-encoding': 'gzip' } : {}),
    'vary': 'Accept-Encoding',
    'cache-control': rel.startsWith('assets/') ? 'public, max-age=3600' : 'no-store',
    'cross-origin-opener-policy': 'same-origin', 'cross-origin-embedder-policy': 'require-corp',
  });
  res.end(body);
});
server.listen(8704, '127.0.0.1', () => console.log('TinyTeX/v86: http://127.0.0.1:8704'));
