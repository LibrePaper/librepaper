// Fetches the browser renderers named in wasm-modules.lock, and verifies them.
//
// The modules are built and released by their own repositories now, so this is
// how they arrive: by tag, over HTTPS, checked against the digest recorded in
// the lock. A module whose bytes do not hash to what the lock says is not
// written -- a build that silently served a different renderer than the one it
// pinned would be worse than a build that failed.
//
// Already-correct files are left alone, so this is cheap to run on every build
// and downloads nothing when nothing moved.

import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";

const root = new URL("../..", import.meta.url).pathname;
const lock = join(root, "wasm-modules.lock");
const assetsLock = join(root, "typst-assets.lock");
const assetsCache = join(root, "web/.cache/typst-assets");
const out = join(root, "web/dist/wasm");
const only = process.argv[2];

const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

const entries = (await readFile(lock, "utf8"))
  .split("\n")
  .map((line) => line.replace(/#.*$/, "").trim())
  .filter(Boolean)
  .map((line) => {
    const [module, repo, tag, sha256, brotliSha256, ...extra] = line.split(/\s+/);
    if (extra.length || !/^[a-f0-9]{64}$/.test(sha256 || "") || !/^[a-f0-9]{64}$/.test(brotliSha256 || "")) {
      throw new Error(`${module || "lock entry"}: expected module repo tag sha256 brotli_sha256`);
    }
    return { module, repo, tag, sha256, brotliSha256 };
  });

const expected = new Map([
  ["markdown.wasm", "wasm-markdown"],
  ["bibliography.wasm", "wasm-bibliography"],
  ["citations.wasm", "wasm-bibliography"],
  ["typst.wasm", "wasm-typst"],
]);
if (entries.length !== expected.size || new Set(entries.map(({ module }) => module)).size !== expected.size || entries.some(({ module, repo }) => expected.get(module) !== repo)) {
  throw new Error("wasm-modules.lock must contain exactly the four required renderer modules");
}

await mkdir(out, { recursive: true });
let fetched = 0;

for (const { module, repo, tag, sha256, brotliSha256 } of entries) {
  if (only && only !== module) continue;
  const representations = [
    { name: module, sha256 },
    { name: `${module}.br`, sha256: brotliSha256 },
  ];
  if ((await Promise.all(representations.map(async ({ name, sha256 }) => {
    const target = join(out, name);
    return existsSync(target) && digest(await readFile(target)) === sha256;
  }))).every(Boolean)) {
    console.log(`${module.padEnd(20)} ok`);
    continue;
  }
  const fetchedRepresentations = [];
  for (const representation of representations) {
    const url = `https://github.com/LibrePaper/${repo}/releases/download/${tag}/${representation.name}`;
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${representation.name}: ${url} -> ${response.status} ${response.statusText}`);
    const bytes = Buffer.from(await response.arrayBuffer());
    const got = digest(bytes);
    if (got !== representation.sha256) {
      throw new Error(`${representation.name}: the bytes at ${url} are not what wasm-modules.lock pins.\n  expected ${representation.sha256}\n  received ${got}\nNothing was written. Either the release moved, or the lock is stale.`);
    }
    fetchedRepresentations.push({ ...representation, bytes });
  }
  for (const { name, bytes } of fetchedRepresentations) {
    const target = join(out, name);
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, bytes);
  }
  fetched += 1;
  console.log(`${module.padEnd(20)} ${(fetchedRepresentations[0].bytes.length / 1024).toFixed(0)} KiB raw, ${(fetchedRepresentations[1].bytes.length / 1024).toFixed(0)} KiB br  ${tag}`);
}

// The external-font Typst build is a separate browser mirror. Keep its
// verified bytes out of web/dist: the native renderer above remains embedded
// in the server binary for the CLI and local compilation paths.
const assetsPin = JSON.parse(await readFile(assetsLock, "utf8"));
if (assetsPin.url || assetsPin.sha256 || assetsPin.fontsSha256) {
  if (!/^https:\/\/typst\.librepaper\.workers\.dev\/[a-f0-9]{64}\/typst\.wasm$/.test(assetsPin.url || "") ||
      !/^[a-f0-9]{64}$/.test(assetsPin.sha256 || "") || !/^[a-f0-9]{64}$/.test(assetsPin.fontsSha256 || "")) {
    throw new Error("typst-assets.lock must contain a complete static Typst URL, WASM SHA-256 and fontsSha256");
  }
  const wasmName = `${assetsPin.sha256}/typst.wasm`;
  const manifestName = `${assetsPin.sha256}/fonts.json`;
  const saveVerified = async (name, url, expected, responseBytes = null) => {
    const target = join(assetsCache, name);
    if (existsSync(target)) {
      const bytes = await readFile(target);
      if (digest(bytes) === expected) return bytes;
    }
    const response = responseBytes ? null : await fetch(url);
    if (response && !response.ok) throw new Error(`${url} -> ${response.status} ${response.statusText}`);
    const bytes = responseBytes || Buffer.from(await response.arrayBuffer());
    const got = digest(bytes);
    if (got !== expected) throw new Error(`Typst asset checksum mismatch at ${url}: expected ${expected}, received ${got}`);
    const targetPath = join(assetsCache, name);
    await mkdir(dirname(targetPath), { recursive: true });
    await writeFile(targetPath, bytes);
    return bytes;
  };
  await saveVerified(wasmName, assetsPin.url, assetsPin.sha256);
  const manifestUrl = new URL("fonts.json", assetsPin.url).href;
  const manifestBytes = await saveVerified(manifestName, manifestUrl, assetsPin.fontsSha256);
  let manifest;
  try { manifest = JSON.parse(manifestBytes.toString("utf8")); }
  catch { throw new Error(`invalid Typst fonts manifest at ${manifestUrl}`); }
  if (!manifest || !Array.isArray(manifest.fonts) || manifest.fonts.length === 0) throw new Error("Typst fonts manifest must contain a nonempty fonts array");
  for (const font of manifest.fonts) {
    if (!font || typeof font.url !== "string" || !/^[a-f0-9]{64}$/.test(font.sha256 || "") || !Number.isSafeInteger(font.size) || font.size <= 0) {
      throw new Error("Typst fonts manifest contains an invalid font entry");
    }
    const url = new URL(font.url, manifestUrl);
    if (url.origin !== new URL(assetsPin.url).origin) throw new Error(`Typst font URL leaves the pinned mirror: ${url.href}`);
    const name = url.pathname.replace(/^\//, "");
    const bytes = await saveVerified(name, url.href, font.sha256);
    if (bytes.byteLength !== font.size) throw new Error(`Typst font size mismatch at ${url.href}: expected ${font.size}, received ${bytes.byteLength}`);
  }
}

if (fetched === 0 && !only) console.log("every pinned module was already in place");
