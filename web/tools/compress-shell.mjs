// Brotli for the shell's own assets, written beside them at build time.
//
// The renderers arrive pre-compressed and the server has always served them
// that way; everything the bundler produces was served raw. That is the
// reader bundle, the CodeMirror chunk and the stylesheet -- the megabyte a
// browser has to have before the page does anything -- going over the wire
// uncompressed, on a server with no compression middleware at all.
//
// Compressing here rather than in the server keeps that property: the bytes
// are computed once, by the build, and embedded. A request costs a lookup,
// not a compression. Quality 11 is affordable exactly because it is paid
// once, and it is what a CDN would do to these files anyway.
//
// Only `assets/` and `fonts/`. Every file under them is named for a digest of
// its contents and is immutable, so the compressed copy can never fall out of
// step with the original. The pages are deliberately excluded: the server
// rewrites `__MODULES__` into each one as it serves it, so a page compressed
// here would be a page served with the placeholder still in it.
import { brotliCompressSync, constants } from "node:zlib";
import { readFileSync, writeFileSync, statSync } from "node:fs";
import { readdir } from "node:fs/promises";
import { resolve, join } from "node:path";

const DIST = resolve(import.meta.dirname, "..", "dist");
const ROOTS = ["assets", "fonts"];
// Compressing a file that is already compressed -- a woff2, a png -- spends
// build time to make it marginally larger. These are the ones that shrink.
const WANTED = /\.(js|mjs|css|svg|json|map|txt|ico|wasm)$/;
// Below about a kilobyte the header costs more than the saving, and the
// server would rather serve the original than look for a copy that is no
// smaller.
const FLOOR = 1024;

async function* files(dir) {
  let entries;
  try { entries = await readdir(dir, { withFileTypes: true }); } catch { return; }
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* files(path);
    else yield path;
  }
}

let count = 0;
let raw = 0;
let small = 0;
for (const root of ROOTS) {
  for await (const path of files(join(DIST, root))) {
    if (!WANTED.test(path) || path.endsWith(".br")) continue;
    if (statSync(path).size < FLOOR) continue;
    const body = readFileSync(path);
    const compressed = brotliCompressSync(body, {
      params: {
        [constants.BROTLI_PARAM_QUALITY]: 11,
        [constants.BROTLI_PARAM_SIZE_HINT]: body.length,
      },
    });
    // A file brotli cannot improve is left without a copy, so the server
    // serves the original rather than something larger.
    if (compressed.length >= body.length) continue;
    writeFileSync(`${path}.br`, compressed);
    count += 1;
    raw += body.length;
    small += compressed.length;
  }
}

const mb = (n) => (n / 1024 / 1024).toFixed(2);
console.log(
  `brotli: ${count} shell assets, ${mb(raw)} MB -> ${mb(small)} MB` +
    (raw ? ` (${Math.round((1 - small / raw) * 100)}% smaller)` : ""),
);
