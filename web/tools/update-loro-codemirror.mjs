// Moves the loro-codemirror pin to an explicitly named commit.
//
// Never resolves a branch or a "latest": a pin that can move on its own is not
// a pin. Give it the full sha you pushed, it fetches those bytes, and it
// rewrites the lock with the digests they actually have. Reviewing the diff is
// how you see what changed in the binding.
//
//   node web/tools/update-loro-codemirror.mjs <full 40-char sha>

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = new URL("../..", import.meta.url).pathname;
const lock = join(root, "loro-codemirror.lock");

const commit = process.argv[2];
if (!/^[a-f0-9]{40}$/.test(commit || "")) {
  throw new Error("usage: update-loro-codemirror.mjs <full 40-character commit sha>");
}

const text = await readFile(lock, "utf8");
const repository = (text.match(/^repository\s+(\S+)$/m) || [])[1];
if (!repository) throw new Error("loro-codemirror.lock does not name a repository");

const paths = text
  .split("\n")
  .map((line) => line.replace(/#.*$/, "").trim())
  .filter((line) => line && !line.startsWith("repository ") && !line.startsWith("commit "))
  .map((line) => line.split(/\s+/)[0]);

const digests = new Map();
for (const path of paths) {
  const url = `https://raw.githubusercontent.com/${repository}/${commit}/${path}`;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${path}: ${url} -> ${response.status} ${response.statusText}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  digests.set(path, createHash("sha256").update(bytes).digest("hex"));
}

const width = Math.max(...paths.map((path) => path.length)) + 2;
const updated = text
  .split("\n")
  .map((line) => {
    if (/^commit\s+/.test(line)) return `commit      ${commit}`;
    const bare = line.replace(/#.*$/, "").trim();
    const path = bare.split(/\s+/)[0];
    if (!bare || !digests.has(path)) return line;
    return `${path.padEnd(width)}${digests.get(path)}`;
  })
  .join("\n");

await writeFile(lock, updated);
console.log(`loro-codemirror.lock now pins ${repository} at ${commit}`);
for (const [path, sha] of digests) console.log(`  ${path.padEnd(width)}${sha}`);
