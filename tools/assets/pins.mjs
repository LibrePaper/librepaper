#!/usr/bin/env node

// Fetch and move the repository's pinned browser inputs. Every download is
// checked against a lock or a release checksum before it reaches the worktree.
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseAssetsLock, PINNED_MODULES } from "./pins-lock.mjs";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const shaPattern = /^[a-f0-9]{64}$/;

async function downloadVerified(url, expected, label) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${label}: ${url} -> ${response.status} ${response.statusText}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const got = digest(bytes);
  if (got !== expected) throw new Error(`${label}: bytes at ${url} do not match the pin.\n  expected ${expected}\n  received ${got}\nNothing was written.`);
  return bytes;
}

async function fetchModules(root, only) {
  const { modules } = await parseAssetsLock(root);
  if (only && !PINNED_MODULES.has(only)) throw new Error(`unknown module ${only}; expected one of ${[...PINNED_MODULES.keys()].join(", ")}`);
  const out = join(root, "web/wasm");
  await mkdir(out, { recursive: true });
  let fetched = 0;
  for (const { module, repo, tag, sha256 } of modules) {
    if (only && only !== module) continue;
    const target = join(out, module);
    if (existsSync(target) && digest(await readFile(target)) === sha256) {
      console.log(`${module.padEnd(20)} ok`);
      continue;
    }
    const url = `https://github.com/LibrePaper/${repo}/releases/download/${tag}/${module}`;
    const bytes = await downloadVerified(url, sha256, module);
    if (module === "typst.wasm") {
      const instantiated = await WebAssembly.instantiate(bytes, {});
      const requiresFonts = instantiated.instance.exports.default_fonts_required;
      if (typeof requiresFonts === "function" && requiresFonts() !== 0) throw new Error(`${module} at ${url} requires external fonts; pin an embedded-font build`);
    }
    await writeFile(target, bytes);
    fetched++;
    console.log(`${module.padEnd(20)} ${(bytes.length / 1024).toFixed(0)} KiB  ${tag}`);
  }
  if (!fetched && !only) console.log("every pinned module was already in place");
}

async function parseLoroLock(root) {
  const text = await readFile(join(root, "web/loro-codemirror.lock"), "utf8");
  const lines = text.split(/\r?\n/).map((line) => line.replace(/#.*$/, "").trim()).filter(Boolean);
  const directive = (name) => {
    const found = lines.filter((line) => line.startsWith(`${name} `));
    if (found.length !== 1) throw new Error(`loro-codemirror.lock must name exactly one ${name}`);
    return found[0].slice(name.length).trim();
  };
  const repository = directive("repository");
  const commit = directive("commit");
  if (!/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(repository)) throw new Error(`loro-codemirror.lock: "${repository}" is not an owner/repository`);
  if (!/^[a-f0-9]{40}$/.test(commit)) throw new Error(`loro-codemirror.lock: "${commit}" is not a full 40-character commit sha`);
  const entries = lines.filter((line) => !line.startsWith("repository ") && !line.startsWith("commit ")).map((line) => {
    const [path, sha256, ...extra] = line.split(/\s+/);
    if (extra.length || !shaPattern.test(sha256 || "")) throw new Error(`${path || "lock entry"}: expected "path sha256"`);
    return { path, sha256 };
  });
  const required = ["src/index.ts", "src/sync.ts", "src/undo.ts", "src/awareness.ts", "src/ephemeral.ts", "src/utils.ts", "LICENSE"];
  const missing = required.filter((path) => !entries.some((entry) => entry.path === path));
  if (missing.length) throw new Error(`loro-codemirror.lock is missing: ${missing.join(", ")}`);
  return { text, repository, commit, entries };
}

async function fetchLoro(root) {
  const { repository, commit, entries } = await parseLoroLock(root);
  const out = join(root, "web/vendor/loro-codemirror");
  await mkdir(out, { recursive: true });
  let fetched = 0;
  for (const { path, sha256 } of entries) {
    const name = basename(path);
    const target = join(out, name);
    if (existsSync(target) && digest(await readFile(target)) === sha256) { console.log(`${name.padEnd(16)} ok`); continue; }
    const url = `https://raw.githubusercontent.com/${repository}/${commit}/${path}`;
    const bytes = await downloadVerified(url, sha256, path);
    await writeFile(target, bytes);
    fetched++;
    console.log(`${name.padEnd(16)} ${(bytes.length / 1024).toFixed(1)} KiB  ${commit.slice(0, 8)}`);
  }
  if (!fetched) console.log(`every pinned file was already in place (${commit.slice(0, 8)})`);
}

function parseSums(text) {
  const sums = new Map();
  for (const line of text.split(/\r?\n/)) {
    const fields = line.trim().split(/\s+/);
    if (!line.trim()) continue;
    if (fields.length !== 2 || !shaPattern.test(fields[0])) throw new Error(`invalid SHA256SUMS line: ${line}`);
    if (sums.has(fields[1])) throw new Error(`duplicate SHA256SUMS entry: ${fields[1]}`);
    sums.set(fields[1], fields[0]);
  }
  return sums;
}

async function updateWasmPin(args) {
  const allowed = ["--repo", "--tag", "--root", "--sums-file"];
  const values = new Map();
  for (let i = 0; i < args.length; i += 2) {
    const name = args[i];
    const value = args[i + 1];
    if (!allowed.includes(name) || !value || value.startsWith("--") || values.has(name)) throw new Error("invalid wasm pin options");
    values.set(name, value);
  }
  const repo = values.get("--repo");
  const tag = values.get("--tag");
  const root = values.get("--root") || ROOT;
  const sumsFile = values.get("--sums-file");
  if (!repo || !tag ||
      !/^wasm-(markdown|bibliography|typst)$/.test(repo) || !/^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(tag)) {
    throw new Error("usage: pins update wasm --repo wasm-markdown|wasm-bibliography|wasm-typst --tag vX.Y.Z [--root DIR] [--sums-file FILE]");
  }
  const { text, modules } = await parseAssetsLock(root);
  if (!modules.some(({ repo: entryRepo }) => entryRepo === repo)) throw new Error(`${repo}: no lock entry`);
  const sumsText = sumsFile ? await readFile(sumsFile, "utf8") : await (async () => {
    const response = await fetch(`https://github.com/LibrePaper/${repo}/releases/download/${tag}/SHA256SUMS`);
    if (!response.ok) throw new Error(`${repo} ${tag}: release has no SHA256SUMS (${response.status})`);
    return response.text();
  })();
  const sums = parseSums(sumsText);
  const updated = text.replace(new RegExp(`^(\\s*)(\\S+)(\\s+${repo}\\s+)\\S+(\\s+)([a-f0-9]{64})(.*)$`, "gm"), (_, indent, module, middle, gap, oldSha, rest) => {
    const sha = sums.get(module);
    if (!sha) throw new Error(`${repo} ${tag}: SHA256SUMS has no ${module}`);
    return `${indent}${module}${middle}${tag}${gap}${sha}${rest}`;
  });
  await writeFile(join(root, "assets.lock"), updated);
  console.log(`${repo}: updated assets.lock to ${tag}`);
  console.log("Run `node tools/assets/pins.mjs fetch` before committing.");
}

function option(args, name) { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : undefined; }

function fetchArguments(args) {
  const rest = [...args];
  const rootAt = rest.indexOf("--root");
  let root = ROOT;
  if (rootAt >= 0) {
    root = rest[rootAt + 1];
    if (!root || root.startsWith("--")) throw new Error("fetch --root requires a directory");
    rest.splice(rootAt, 2);
  }
  if (rest.some((arg) => arg.startsWith("--")) || rest.length > 1) throw new Error("usage: pins fetch [module.wasm] [--root DIR]");
  return { root, only: rest[0] };
}

async function updateLoroPin(root, commit) {
  if (!/^[a-f0-9]{40}$/.test(commit || "")) throw new Error("usage: pins update loro <full 40-character commit sha>");
  const lockPath = join(root, "web/loro-codemirror.lock");
  const { text, repository, entries } = await parseLoroLock(root);
  const digests = new Map();
  for (const { path } of entries) {
    const url = `https://raw.githubusercontent.com/${repository}/${commit}/${path}`;
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${path}: ${url} -> ${response.status} ${response.statusText}`);
    digests.set(path, digest(Buffer.from(await response.arrayBuffer())));
  }
  const width = Math.max(...entries.map(({ path }) => path.length)) + 2;
  const updated = text.split(/\r?\n/).map((line) => {
    if (/^commit\s+/.test(line)) return `commit      ${commit}`;
    const path = line.replace(/#.*$/, "").trim().split(/\s+/)[0];
    return digests.has(path) ? `${path.padEnd(width)}${digests.get(path)}` : line;
  }).join("\n");
  await writeFile(lockPath, updated);
  console.log(`loro-codemirror.lock now pins ${repository} at ${commit}`);
  for (const [path, sha] of digests) console.log(`  ${path.padEnd(width)}${sha}`);
}

async function updateLatexPin(args) {
  let rest = [...args];
  const rootAt = rest.indexOf("--root");
  const root = rootAt >= 0 ? rest.splice(rootAt, 2)[1] : ROOT;
  if ((rootAt >= 0 && (!root || root.startsWith("--"))) || rest.includes("--root") ||
      rest.some((arg) => arg.startsWith("--")) || rest.length > 1) throw new Error("usage: pins update latex [--root DIR] [mirror-dir]");
  const mirrorDir = rest[0] || "../wasm-latex/mirror";
  const entries = await readdir(mirrorDir, { withFileTypes: true });
  const hexDirs = entries.filter((entry) => entry.isDirectory() && shaPattern.test(entry.name));
  if (hexDirs.length !== 1) throw new Error(`Mirror must contain exactly one release directory (64 hex chars), found ${hexDirs.length}`);
  const releaseId = hexDirs[0].name;
  const releaseDir = join(mirrorDir, releaseId);
  const manifestSha = digest(await readFile(join(releaseDir, "MANIFEST.json")));
  if (manifestSha !== releaseId) throw new Error(`MANIFEST.json sha256 (${manifestSha}) does not match directory name (${releaseId})`);
  const releaseJson = JSON.parse(await readFile(join(releaseDir, "release.json"), "utf8"));
  const sourceUrl = releaseJson?.source?.corresponding_source?.url;
  if (!sourceUrl) throw new Error("release.json missing source.corresponding_source.url");
  const match = sourceUrl.match(/\/releases\/download\/([^/]+)\//);
  if (!match) throw new Error(`Could not extract tag from source URL: ${sourceUrl}`);
  const lockPath = join(root, "assets.lock");
  const { text, latex } = await parseAssetsLock(root);
  const lines = text.split(/\r?\n/);
  const i = lines.findIndex((line) => /^\s*latex\s/.test(line.replace(/#.*$/, "")));
  const matchLine = lines[i].match(/^(\s*)(\S+)(\s+)(\S+)(\s+)(\S+)(\s+)([a-f0-9]{64})(.*)$/);
  if (!matchLine) throw new Error("latex row in assets.lock is malformed");
  const [, indent, module, s1, repo, s2, oldTag, s3, oldSha, restLine] = matchLine;
  if (repo !== latex.repo) throw new Error("latex row in assets.lock is malformed");
  if (oldTag === match[1] && oldSha === manifestSha) { console.log(`latex: already pinned ${match[1]} ${manifestSha}`); return; }
  lines[i] = `${indent}${module}${s1}${repo}${s2}${match[1]}${s3}${manifestSha}${restLine}`;
  await writeFile(lockPath, lines.join("\n"));
  console.log(`latex: pinned ${match[1]} ${manifestSha}`);
}

function usage() {
  return `usage: node tools/assets/pins.mjs <command>\n\n  fetch [module.wasm]             fetch pinned wasm and CodeMirror inputs\n  update wasm --repo <repo> --tag <tag> [--root DIR] [--sums-file FILE]\n  update loro <full-40-character-sha>\n  update latex [mirror-dir]`;
}

async function main() {
  const [command = "help", ...args] = process.argv.slice(2);
  if (command === "fetch") {
    const { root, only } = fetchArguments(args);
    await fetchModules(root, only);
    if (!only) await fetchLoro(root);
  } else if (command === "update" && args[0] === "wasm") await updateWasmPin(args.slice(1));
  else if (command === "update" && args[0] === "loro") {
    const commit = args[1];
    const rest = args.slice(2);
    const root = option(rest, "--root") || ROOT;
    if (rest.length && (rest.length !== 2 || rest[0] !== "--root" || !rest[1])) throw new Error("usage: pins update loro <full-40-character-sha> [--root DIR]");
    await updateLoroPin(root, commit);
  }
  else if (command === "update" && args[0] === "latex") await updateLatexPin(args.slice(1));
  else if (command === "help" || command === "-h" || command === "--help") console.log(usage());
  else throw new Error(usage());
}

main().catch((error) => { console.error(error.message); process.exitCode = 1; });
