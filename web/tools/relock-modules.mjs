// Re-pins wasm-modules.lock to whatever each repository's release currently
// holds, by reading the SHA256SUMS published beside the modules.
//
// The digests are not computed here from downloaded bytes: they are taken from
// the release, and `make wasm` is what checks the bytes against them. Two steps
// rather than one, so that re-pinning is a small reviewable diff and verifying
// is something every build does.

import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = new URL("../..", import.meta.url).pathname;
const lock = join(root, "wasm-modules.lock");

const text = await readFile(lock, "utf8");
const sums = new Map();

for (const line of text.split("\n")) {
  const bare = line.replace(/#.*$/, "").trim();
  if (!bare) continue;
  const [, repo, tag] = bare.split(/\s+/);
  const key = `${repo}@${tag}`;
  if (sums.has(key)) continue;
  const url = `https://github.com/LibrePaper/${repo}/releases/download/${tag}/SHA256SUMS`;
  const response = await fetch(url);
  if (!response.ok) {
    console.error(`${repo} ${tag}: ${url} -> ${response.status} ${response.statusText}`);
    process.exit(1);
  }
  const map = new Map();
  for (const entry of (await response.text()).split("\n")) {
    const [sha, name] = entry.trim().split(/\s+/);
    if (sha && name) map.set(name, sha);
  }
  sums.set(key, map);
}

const updated = text
  .split("\n")
  .map((line) => {
    const bare = line.replace(/#.*$/, "").trim();
    if (!bare) return line;
    const [module, repo, tag, was] = bare.split(/\s+/);
    const now = sums.get(`${repo}@${tag}`)?.get(module);
    if (!now) {
      console.error(`${module}: ${repo} ${tag} publishes no such file`);
      process.exit(1);
    }
    if (now !== was) console.log(`${module.padEnd(20)} ${was.slice(0, 12)} -> ${now.slice(0, 12)}`);
    return `${module.padEnd(20)} ${repo.padEnd(19)} ${tag.padEnd(9)} ${now}`;
  })
  .join("\n");

await writeFile(lock, updated);
