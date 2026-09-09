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
const out = join(root, "web/dist/wasm");
const only = process.argv[2];

const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

const entries = (await readFile(lock, "utf8"))
  .split("\n")
  .map((line) => line.replace(/#.*$/, "").trim())
  .filter(Boolean)
  .map((line) => {
    const [module, repo, tag, sha256] = line.split(/\s+/);
    return { module, repo, tag, sha256 };
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

for (const { module, repo, tag, sha256 } of entries) {
  if (only && only !== module) continue;
  const target = join(out, module);
  if (existsSync(target) && digest(await readFile(target)) === sha256) {
    console.log(`${module.padEnd(20)} ok`);
    continue;
  }
  const url = `https://github.com/LibrePaper/${repo}/releases/download/${tag}/${module}`;
  const response = await fetch(url);
  if (!response.ok) {
    console.error(`${module}: ${url} -> ${response.status} ${response.statusText}`);
    process.exit(1);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  const got = digest(bytes);
  if (got !== sha256) {
    console.error(
      `${module}: the bytes at ${url} are not what wasm-modules.lock pins.\n` +
        `  expected ${sha256}\n  received ${got}\n` +
        `Nothing was written. Either the release moved, or the lock is stale.`,
    );
    process.exit(1);
  }
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, bytes);
  fetched += 1;
  console.log(`${module.padEnd(20)} ${(bytes.length / 1024).toFixed(0)} KiB  ${tag}`);
}

if (fetched === 0 && !only) console.log("every pinned module was already in place");
