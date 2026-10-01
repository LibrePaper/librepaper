// Move one browser renderer pin in assets.lock, explicitly; no release
// discovery or "latest" lookup.
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const value = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const repo = value("--repo");
const tag = value("--tag");
const rootValue = value("--root");
const sumsFile = value("--sums-file");
if (!["--repo", "--tag"].every((name) => args.includes(name)) || args.some((arg, i) => arg.startsWith("--") && !["--repo", "--tag", "--root", "--sums-file"].includes(arg)) || !repo || !tag || !/^wasm-(markdown|bibliography|typst)$/.test(repo) || !/^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(tag)) {
  console.error("usage: node web/tools/update-module-pin.mjs --repo wasm-markdown|wasm-bibliography|wasm-typst --tag vX.Y.Z");
  process.exit(2);
}

const root = rootValue || fileURLToPath(new URL("../..", import.meta.url));
const lockPath = join(root, "assets.lock");
const lock = await readFile(lockPath, "utf8");
const expected = new Map([["markdown.wasm", "wasm-markdown"], ["bibliography.wasm", "wasm-bibliography"], ["citations.wasm", "wasm-bibliography"], ["typst.wasm", "wasm-typst"]]);
const rows = [];
for (const raw of lock.split("\n")) {
  const line = raw.replace(/#.*$/, "").trim();
  if (!line) continue;
  const fields = line.split(/\s+/);
  if (fields.length !== 4 || !/^[a-f0-9]{64}$/.test(fields[3])) throw new Error("assets.lock contains a malformed entry");
  // The `latex` row pins a mirror release directory, not a wasm module.
  if (fields[0] !== "latex") rows.push(fields);
}
if (rows.length !== expected.size || new Set(rows.map(([module]) => module)).size !== expected.size || rows.some(([module, entryRepo]) => expected.get(module) !== entryRepo)) throw new Error("assets.lock must contain exactly the four required wasm modules plus one latex row");
if (!rows.some(([, entryRepo]) => entryRepo === repo)) throw new Error(`${repo}: no lock entry`);
const sumsText = sumsFile
  ? await readFile(sumsFile, "utf8")
  : await (async () => {
      const response = await fetch(`https://github.com/LibrePaper/${repo}/releases/download/${tag}/SHA256SUMS`);
      if (!response.ok) throw new Error(`${repo} ${tag}: release has no SHA256SUMS (${response.status})`);
      return response.text();
    })();
const sums = new Map();
for (const line of sumsText.split("\n")) {
  const fields = line.trim().split(/\s+/);
  if (!line.trim()) continue;
  if (fields.length !== 2 || !/^[a-f0-9]{64}$/.test(fields[0])) throw new Error(`invalid SHA256SUMS line: ${line}`);
  if (sums.has(fields[1])) throw new Error(`duplicate SHA256SUMS entry: ${fields[1]}`);
  sums.set(fields[1], fields[0]);
}
const changedLock = lock.replace(new RegExp(`^(\\s*)(\\S+)(\\s+${repo}\\s+)\\S+(\\s+)([a-f0-9]{64})(.*)$`, "gm"), (_, indent, module, middle, gap, _old, rest) => {
  const sha = sums.get(module);
  if (!sha) throw new Error(`${repo} ${tag}: SHA256SUMS has no ${module}`);
  return `${indent}${module}${middle}${tag}${gap}${sha}${rest}`;
});

// All checks happen before writing anything.
await writeFile(lockPath, changedLock);
console.log(`${repo}: updated assets.lock to ${tag}`);
console.log("Run `tools/pins fetch` before committing.");
