// Move one renderer pin, explicitly. Both Cargo git dependencies and the
// browser lock are changed together; no release discovery or "latest" lookup.
import { readFile, writeFile, readdir } from "node:fs/promises";
import { join } from "node:path";

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

const root = rootValue || new URL("../..", import.meta.url).pathname;
const lockPath = join(root, "wasm-modules.lock");
const lock = await readFile(lockPath, "utf8");
const expected = new Map([["markdown.wasm", "wasm-markdown"], ["bibliography.wasm", "wasm-bibliography"], ["citations.wasm", "wasm-bibliography"], ["typst.wasm", "wasm-typst"]]);
const rows = [];
for (const raw of lock.split("\n")) {
  const line = raw.replace(/#.*$/, "").trim();
  if (!line) continue;
  const fields = line.split(/\s+/);
  if (fields.length !== 4 || !/^[a-f0-9]{64}$/.test(fields[3])) throw new Error("wasm-modules.lock contains a malformed entry");
  rows.push(fields);
}
if (rows.length !== expected.size || new Set(rows.map(([module]) => module)).size !== expected.size || rows.some(([module, entryRepo]) => expected.get(module) !== entryRepo)) throw new Error("wasm-modules.lock must contain exactly the four required renderer modules");
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

const files = [];
async function walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory() && entry.name !== "target") await walk(path);
    else if (entry.isFile() && entry.name === "Cargo.toml") files.push(path);
  }
}
await walk(join(root, "crates"));
files.push(join(root, "Cargo.toml"));
const cargoPattern = new RegExp(`(wasm-${repo.slice(5)}\\s*=\\s*\\{[^}]*?\\btag\\s*=\\s*")([^"]+)(")`, "g");
let nativeCount = 0;
const updates = [];
for (const path of files) {
  const text = await readFile(path, "utf8");
  const changed = text.replace(cargoPattern, (_, before, _old, after) => { nativeCount++; return `${before}${tag}${after}`; });
  if (changed !== text) updates.push([path, changed]);
}
if (!nativeCount) throw new Error(`${repo}: no native Cargo dependency with an explicit tag`);
// All checks, including the release manifest, happen before writing anything.
// This keeps a failed pin move from leaving native and browser pins divergent.
for (const [path, changed] of updates) await writeFile(path, changed);
await writeFile(lockPath, changedLock);
console.log(`${repo}: updated ${nativeCount} native dependency tag(s) and browser lock to ${tag}`);
console.log("Run `cargo update -p <native-renderer>` (or cargo check) and `make wasm` before committing.");
