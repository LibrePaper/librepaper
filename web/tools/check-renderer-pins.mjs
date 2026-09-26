// Check the native git dependencies and browser release pins agree.  This is
// deliberately offline: it is safe to run from every build and CI job.
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

const rootArg = process.argv.indexOf("--root");
const root = rootArg >= 0 ? process.argv[rootArg + 1] : new URL("../..", import.meta.url).pathname;
if (!root || (rootArg >= 0 && !process.argv[rootArg + 1])) throw new Error("--root requires a directory");
const lockPath = join(root, "wasm-modules.lock");
const required = new Map([
  ["markdown.wasm", "wasm-markdown"],
  ["bibliography.wasm", "wasm-bibliography"],
  ["citations.wasm", "wasm-bibliography"],
  ["typst.wasm", "wasm-typst"],
]);

const lock = new Map();
for (const [lineNumber, raw] of (await readFile(lockPath, "utf8")).split("\n").entries()) {
  const line = raw.replace(/#.*$/, "").trim();
  if (!line) continue;
  const fields = line.split(/\s+/);
  if (fields.length !== 5) throw new Error(`${lockPath}:${lineNumber + 1}: expected module repo tag sha256 brotli_sha256`);
  const [module, repo, tag, sha256, brotliSha256] = fields;
  if (!/^[a-f0-9]{64}$/.test(sha256)) throw new Error(`${module}: invalid sha256`);
  if (!/^[a-f0-9]{64}$/.test(brotliSha256)) throw new Error(`${module}: invalid brotli sha256`);
  if (lock.has(module)) throw new Error(`${module}: duplicate lock entry`);
  lock.set(module, { repo, tag });
}

for (const [module, repo] of required) {
  const pin = lock.get(module);
  if (!pin) throw new Error(`${module}: missing from wasm-modules.lock`);
  if (pin.repo !== repo) throw new Error(`${module}: expected ${repo}, found ${pin.repo}`);
}
for (const module of lock.keys()) if (!required.has(module)) throw new Error(`${module}: unexpected browser module`);

const native = new Map();
const manifests = [join(root, "Cargo.toml")];
async function walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory() && entry.name !== "target") await walk(path);
    else if (entry.isFile() && entry.name === "Cargo.toml") manifests.push(path);
  }
}
await walk(join(root, "crates"));
for (const path of manifests) {
  const text = await readFile(path, "utf8");
  for (const match of text.matchAll(/wasm-(markdown|bibliography|typst)\s*=\s*\{([^}]*)\}/g)) {
    const repo = `wasm-${match[1]}`;
    const tag = match[2].match(/\btag\s*=\s*"([^"]+)"/)?.[1];
    if (!tag) throw new Error(`${path}: ${repo} dependency has no explicit git tag`);
    if (native.has(repo) && native.get(repo) !== tag) throw new Error(`${repo}: conflicting native tags`);
    native.set(repo, tag);
  }
}
for (const repo of new Set(required.values())) {
  const nativeTag = native.get(repo);
  const lockTags = [...lock.entries()]
    .filter(([module]) => required.get(module) === repo)
    .map(([, pin]) => pin.tag);
  const lockTag = lockTags[0];
  if (!nativeTag) throw new Error(`${repo}: no native dependency found`);
  if (lockTags.some((tag) => tag !== lockTag)) throw new Error(`${repo}: browser modules use conflicting tags`);
  if (nativeTag !== lockTag) throw new Error(`${repo}: native tag ${nativeTag} does not match browser tag ${lockTag}`);
}
console.log(`renderer pins ok (${required.size} browser modules)`);
