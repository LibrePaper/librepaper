// Fetches the browser renderers named in assets.lock, and verifies them.
//
// The modules are built and released by their own repositories, so this is
// how they arrive: by tag, over HTTPS, checked against the digest recorded in
// the lock. A module whose bytes do not hash to what the lock says is not
// written -- a build that silently served a different renderer than the one it
// pinned would be worse than a build that failed.
//
// They land in web/wasm/, which is not embedded in the binary: the modules are
// published to the asset mirror (see tools/push-mirrors.mjs) and browsers load
// them from there. Tests and tools read them from this directory.
//
// Already-correct files are left alone, so this is cheap to run on every build
// and downloads nothing when nothing moved.

import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const lock = join(root, "assets.lock");
const out = join(root, "web/wasm");
const only = process.argv[2];

const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

const rows = (await readFile(lock, "utf8"))
  .split("\n")
  .map((line) => line.replace(/#.*$/, "").trim())
  .filter(Boolean)
  .map((line) => {
    const [module, repo, tag, sha256, ...extra] = line.split(/\s+/);
    if (extra.length || !/^[a-f0-9]{64}$/.test(sha256 || "")) {
      throw new Error(`${module || "lock entry"}: expected module repo tag sha256`);
    }
    return { module, repo, tag, sha256 };
  });

const expected = new Map([
  ["markdown.wasm", "wasm-markdown"],
  ["bibliography.wasm", "wasm-bibliography"],
  ["citations.wasm", "wasm-bibliography"],
  ["typst.wasm", "wasm-typst"],
]);
// The lock pins four wasm modules plus one `latex` row: the LaTeX mirror release
// directory `latex/<sha256>/`. The latex row names no file here; it is fetched
// by the browser from the asset mirror, not by this tool.
const latexRows = rows.filter(({ module }) => module === "latex");
const entries = rows.filter(({ module }) => module !== "latex");
if (latexRows.length !== 1 || latexRows[0].repo !== "wasm-latex") {
  throw new Error("assets.lock must contain exactly one latex row from wasm-latex");
}
if (entries.length !== expected.size || new Set(entries.map(({ module }) => module)).size !== expected.size || entries.some(({ module, repo }) => expected.get(module) !== repo)) {
  throw new Error("assets.lock must contain exactly the four required wasm modules plus one latex row");
}

await mkdir(out, { recursive: true });
let fetched = 0;

for (const { module, repo, tag, sha256 } of entries) {
  if (only && only !== module) continue;
  const target = join(out, module);
  if (existsSync(target) && digest(await readFile(target)) === sha256) {
    console.log(`${module.padEnd(20)} ok`);
    continue;
  }
  const url = `https://github.com/LibrePaper/${repo}/releases/download/${tag}/${module}`;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${module}: ${url} -> ${response.status} ${response.statusText}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const got = digest(bytes);
  if (got !== sha256) {
    throw new Error(`${module}: the bytes at ${url} are not what assets.lock pins.\n  expected ${sha256}\n  received ${got}\nNothing was written. Either the release moved, or the lock is stale.`);
  }
  if (module === "typst.wasm") {
    const instantiated = await WebAssembly.instantiate(bytes, {});
    const requiresFonts = instantiated.instance.exports.default_fonts_required;
    if (typeof requiresFonts === "function" && requiresFonts() !== 0) {
      throw new Error(`${module} at ${url} requires external fonts; pin an embedded-font build`);
    }
  }
  await writeFile(target, bytes);
  fetched += 1;
  console.log(`${module.padEnd(20)} ${(bytes.length / 1024).toFixed(0)} KiB  ${tag}`);
}

if (fetched === 0 && !only) console.log("every pinned module was already in place");
