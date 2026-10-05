#!/usr/bin/env node

// Consolidated browser asset mirror tool: build, check, and publish asset mirrors.
// This module combines the functions of four separate tools:
// - Release shape validation and path safety
// - LaTeX mirror consumer-side validation
// - S3 publisher with integrity checking
// - Mirror staging and credential resolution

import { execFile } from "node:child_process";
import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile, copyFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { Transform } from "node:stream";
import { createGzip } from "node:zlib";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import { statSync } from "node:fs";

const runFile = promisify(execFile);

// === Release Shape Helpers ===

export function relativePathParts(path, label, kind = "relative path") {
  if (typeof path !== "string" || !path || path.trim() !== path || path.startsWith("/") || /[\\%?#:\0]/.test(path)) {
    throw new Error(`${label} has an unsafe ${kind}`);
  }
  const parts = path.split("/");
  if (parts.some((part) => !part || part === "." || part === "..")) {
    throw new Error(`${label} has an unsafe ${kind}`);
  }
  return parts;
}

export function validateReleaseShape(release) {
  if (release?.format !== 2) throw new Error(`unsupported release format: ${release?.format}`);
  if (!release.files || typeof release.files !== "object" || Array.isArray(release.files)) {
    throw new Error("release has no files map");
  }
  if (!release.engines || typeof release.engines !== "object" || Array.isArray(release.engines) || !release.engines.pdftex) {
    throw new Error("release has no pdfTeX worker specification");
  }

  for (const [name, spec] of Object.entries(release.engines)) {
    const incomplete = () => name === "pdftex"
      ? "pdfTeX engine has no valid worker inventory"
      : `engine ${name} has no valid worker inventory`;
    if (!spec || typeof spec !== "object" || typeof spec.worker !== "string" || !spec.worker.trim() ||
        !Array.isArray(spec.files) || !spec.files.length || !spec.files.includes(spec.worker)) {
      throw new Error(incomplete());
    }
    for (const file of spec.files) {
      if (typeof file !== "string" || !file.trim() || !release.files[file]) {
        throw new Error(`engine ${name} file is absent from release.json: ${file}`);
      }
    }
    const worker = release.files[spec.worker];
    if (!worker || !Number.isSafeInteger(worker.size) || worker.size <= 0 ||
        typeof worker.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(worker.sha256) ||
        typeof worker.url !== "string" || !worker.url.trim()) {
      throw new Error(`${name} worker is missing or has no valid non-empty file record`);
    }
  }
  if (!release.bundles) throw new Error(`release ${release.id} has no bundles; this mirror ships bundled releases only`);
  return release;
}

// === LaTeX release checker ===

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const ID = /^[a-f0-9]{64}$/;

const safeAssetPath = (url, what) => {
  return relativePathParts(url, what).join("/");
};

/// One release directory on disk: MANIFEST.json hashes to the directory name,
/// release.json is complete, and every engine file and bundle is present with
/// matching size and digest.
async function checkDirectory(directory) {
  const { realpath } = await import("node:fs/promises");
  const root = await realpath(resolve(directory));
  const inside = async (path, what) => {
    let actual;
    try {
      actual = await realpath(path);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      actual = resolve(path);
    }
    const rel = relative(root, actual);
    if (!rel || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
      throw new Error(`${what} escapes the release directory`);
    }
    return actual;
  };
  const release = validateReleaseShape(JSON.parse(await readFile(resolve(root, "release.json"), "utf8")));
  if (ID.test(basename(root))) {
    let manifest;
    try {
      manifest = await readFile(resolve(root, "MANIFEST.json"));
    } catch {
      throw new Error("MANIFEST.json is missing");
    }
    if (sha256(manifest) !== basename(root)) throw new Error("release directory is not the SHA-256 of MANIFEST.json");
    if (release.id !== basename(root)) throw new Error(`release.json id ${release.id} does not match its directory`);
  }

  // Every engine file present on disk with matching digest and size.
  for (const [name, spec] of Object.entries(release.engines)) {
    for (const file of spec.files || []) {
      const info = release.files?.[file];
      if (!info) throw new Error(`engine ${name}: file ${file} is absent from release.json`);
      const url = safeAssetPath(info.url, `asset path ${info.url}`);
      const bytes = await readFile(await inside(resolve(root, url), `asset path ${info.url}`));
      if (bytes.length !== info.size || sha256(bytes) !== info.sha256) {
        throw new Error(`engine ${name}: digest or size mismatch: ${info.url}`);
      }
    }
  }

  // SPEC-latex.md "The index": bundles.json's own digest must match what
  // the release pins, and every bundle path it names must actually be on
  // disk -- independent of `release.files`, which only proves the engine
  // files landed intact, not that bundles.json still agrees with them.
  const indexPath = safeAssetPath(release.bundles.index, "bundle index");
  const indexBytes = await readFile(await inside(resolve(root, indexPath), "bundle index"));
  if (sha256(indexBytes) !== release.bundles.sha256) {
    throw new Error(`bundle index digest mismatch: ${release.bundles.index}`);
  }
  const index = JSON.parse(indexBytes.toString("utf8"));
  const bundleDir = release.bundles.index.slice(0, release.bundles.index.lastIndexOf("/") + 1);
  const bundleEntries = Object.entries(index.bundles || {});
  if (bundleEntries.length !== release.bundles.count) {
    throw new Error(`release.bundles.count is ${release.bundles.count} but the index names ${bundleEntries.length}`);
  }
  for (let i = 0; i < bundleEntries.length; i += 32) {
    await Promise.all(bundleEntries.slice(i, i + 32).map(async ([name, bundle]) => {
      const path = await inside(resolve(root, safeAssetPath(bundleDir + bundle.url, `bundle path ${bundle.url}`)), `bundle path ${bundle.url}`);
      let bytes;
      try {
        bytes = await readFile(path);
      } catch {
        throw new Error(`bundle "${name}" (${bundle.url}) is missing from the mirror`);
      }
      if (bytes.length !== bundle.size || sha256(bytes) !== bundle.sha256) {
        throw new Error(`bundle asset size or digest mismatch: ${bundle.url}`);
      }
    }));
  }
  for (const bundleName of new Set(Object.values(index.files || {}))) {
    if (!index.bundles?.[bundleName]) throw new Error(`bundles.json names unknown bundle "${bundleName}" in its files map`);
  }
  return release;
}

export async function checkMirror(base) {
  if (!base) throw new Error("usage: mirror check <mirror dir | release dir | release URL>");
  if (/^https?:\/\//.test(base)) {
    const releaseUrl = new URL(`${base.replace(/\/$/, "")}/release.json`);
    const response = await fetch(releaseUrl, { signal: AbortSignal.timeout(30000), redirect: "manual" });
    if (!response.ok) throw new Error(`release.json returned HTTP ${response.status}`);
    const release = validateReleaseShape(await response.json());
    const workerUrl = new URL(safeAssetPath(release.files[release.engines.pdftex.worker].url, "pdfTeX worker"), releaseUrl);
    if (workerUrl.origin !== releaseUrl.origin || !workerUrl.pathname.startsWith(releaseUrl.pathname.slice(0, releaseUrl.pathname.lastIndexOf("/") + 1))) {
      throw new Error("pdfTeX worker URL escapes the release directory");
    }
    const workerResponse = await fetch(workerUrl, { signal: AbortSignal.timeout(30000), redirect: "manual" });
    if (!workerResponse.ok) throw new Error(`pdfTeX worker returned HTTP ${workerResponse.status}`);
    const worker = Buffer.from(await workerResponse.arrayBuffer());
    const workerRecord = release.files[release.engines.pdftex.worker];
    if (worker.length !== workerRecord.size || sha256(worker) !== workerRecord.sha256) {
      throw new Error("pdfTeX worker payload does not match release.json");
    }
    console.log(`LaTeX manifest and pdfTeX worker verified: ${base} (${release.id}, format ${release.format})`);
  } else {
    const root = resolve(base);
    const single = await readFile(resolve(root, "release.json")).then(() => true, () => false);
    const directories = single
      ? [root]
      : (await readdir(root, { withFileTypes: true })).filter((entry) => entry.isDirectory() && ID.test(entry.name)).map((entry) => resolve(root, entry.name));
    if (!directories.length) throw new Error("no release directories (<sha256>/) found");
    for (const directory of directories) {
      let release;
      try {
        release = await checkDirectory(directory);
      } catch (error) {
        throw new Error(`${basename(directory)}: ${error.message}`);
      }
      console.log(`LaTeX release ready: ${directory} (${release.id}, format ${release.format})`);
    }
  }
}

// === S3 publisher ===

const HASH = /(?:^|[./_-])[a-f0-9]{64}(?=$|[./_-])/i;
const COMPRESSIBLE = new Set([".css", ".data", ".fmt", ".html", ".js", ".json", ".mjs", ".svg", ".tar", ".txt", ".wasm", ".xml"]);
const MIME = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"], [".js", "text/javascript; charset=utf-8"],
  [".gz", "application/gzip"], [".json", "application/json"], [".mjs", "text/javascript; charset=utf-8"],
  [".otf", "font/otf"], [".pdf", "application/pdf"], [".svg", "image/svg+xml"],
  [".tar", "application/x-tar"], [".ttf", "font/ttf"], [".txt", "text/plain; charset=utf-8"],
  [".wasm", "application/wasm"], [".woff", "font/woff"], [".woff2", "font/woff2"], [".xml", "application/xml"],
]);
const SHA256 = /^[a-f0-9]{64}$/;
const AWS_OPERATIONS = new Set(["head-object", "put-bucket-cors", "s3 sync"]);
const AWS_HINTS = new Map([
  ["AccessDenied", "Check that the mirror credentials have permission for this bucket and prefix."],
  ["InvalidAccessKeyId", "Check that the configured AWS access key is valid for this mirror."],
  ["SignatureDoesNotMatch", "Check that the S3 access key ID and secret match, and verify the endpoint and region."],
  ["NoSuchBucket", "Check that the configured mirror bucket exists at this endpoint."],
  ["InvalidRequest", "Check the mirror bucket and endpoint configuration."],
  ["NotImplemented", "Check that the mirror endpoint supports this S3 operation."],
]);

export function awsFailureMessage(error, args) {
  const name = args?.[0] === "s3" ? `s3 ${args[1]}` : args?.[1];
  const operation = AWS_OPERATIONS.has(name) ? name : null;
  const exitCode = Number.isInteger(error?.status) && error.status >= 0
    ? String(error.status)
    : (Number.isInteger(error?.code) && error.code >= 0 ? String(error.code)
      : typeof error?.code === "string" && /^\d+$/.test(error.code) ? error.code : null);
  const context = [operation && ` ${operation}`, exitCode && ` (exit code ${exitCode})`].filter(Boolean).join("");
  const stderr = Buffer.isBuffer(error?.stderr) ? error.stderr.toString("utf8") : String(error?.stderr || "");
  const serviceError = stderr.match(/An error occurred \(([A-Za-z0-9]{1,80})\) when calling the [A-Za-z0-9]+ operation:/);
  if (serviceError) {
    const code = serviceError[1];
    const hint = AWS_HINTS.get(code);
    return `AWS CLI operation failed${context}: ${code}${hint ? `. ${hint}` : ""}`;
  }
  if (/Could not connect to the endpoint URL/i.test(stderr)) {
    return `AWS CLI operation failed${context}: could not connect to the configured endpoint. Check endpoint and network access.`;
  }
  if (/SSL validation failed/i.test(stderr)) {
    return `AWS CLI operation failed${context}: SSL validation failed. Check the endpoint certificate and local trust configuration.`;
  }
  return `AWS CLI operation failed${context}`;
}

function skipRelativePath(path) {
  const parts = path.split(sep);
  const name = parts.at(-1).toLowerCase();
  return ["_headers", ".assetsignore"].includes(name) || name.endsWith(".br") || name.endsWith(".metadata") || name.endsWith(".cfmeta") ||
    ["cf-cache-status", "cf-metadata.json", "cloudflare-metadata.json"].includes(name) ||
    parts.some((part) => part.toLowerCase() === ".cloudflare");
}

export function metadataFor(key) {
  const name = key.split("/").at(-1).toLowerCase();
  const extension = name.includes(".") ? `.${name.split(".").at(-1)}` : "";
  const contentType = MIME.get(extension) || "application/octet-stream";
  const cacheControl = /(^|\/)(licen[cs]e|copying|notice)(\.|\/|$)/i.test(key) || /(^|\/)licenses\//i.test(key)
      ? "public, max-age=86400" : HASH.test(key)
        ? "public, max-age=31536000, immutable" : "public, max-age=3600";
  const gzip = COMPRESSIBLE.has(extension);
  return { contentType, cacheControl, contentEncoding: gzip ? "gzip" : undefined };
}

async function listFiles(root, directory = root, found = []) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const absolute = join(directory, entry.name);
    const rel = relative(root, absolute);
    if (skipRelativePath(rel)) continue;
    if (entry.isSymbolicLink()) throw new Error(`mirror contains a symbolic link: ${rel}`);
    if (entry.isDirectory()) await listFiles(root, absolute, found);
    else if (entry.isFile()) found.push({ absolute, relative: rel.split(sep).join("/") });
  }
  return found;
}

export function publisherConfiguration(env) {
  for (const key of ["S3_ENDPOINT", "S3_REGION", "S3_BUCKET", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"]) {
    if (!env[key]) throw new Error(`missing required environment variable ${key}`);
  }
  let endpoint;
  try { endpoint = new URL(env.S3_ENDPOINT); } catch { throw new Error("S3_ENDPOINT must be a valid HTTPS URL"); }
  if (endpoint.protocol !== "https:") throw new Error("S3_ENDPOINT must use HTTPS");
  if (endpoint.username || endpoint.password || endpoint.search || endpoint.hash) {
    throw new Error("S3_ENDPOINT must not contain credentials, a query, or a fragment");
  }
  if (!/^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/i.test(env.S3_BUCKET)) throw new Error("S3_BUCKET is not a valid bucket name");
  return { endpoint: endpoint.href.replace(/\/$/, ""), region: env.S3_REGION, bucket: env.S3_BUCKET };
}

async function digestFile(path) {
  const hash = createHash("sha256");
  let size = 0;
  for await (const chunk of createReadStream(path)) {
    size += chunk.length;
    hash.update(chunk);
  }
  return { size, sha256: hash.digest("hex") };
}

function safeRelativePath(root, base, url, label) {
  const pieces = [...base.split("/").filter((part) => part && part !== "."), ...relativePathParts(url, label, "URL")];
  const absolute = resolve(root, ...pieces);
  const rel = relative(root, absolute);
  if (!rel || rel === ".." || rel.startsWith(`..${sep}`)) throw new Error(`${label} escapes the mirror directory`);
  return rel.split(sep).join("/");
}

function expectedReference(expected, relativePath, size, sha256, label) {
  if (!Number.isSafeInteger(size) || size < 0 || !SHA256.test(sha256 || "")) {
    throw new Error(`${label} has invalid size or SHA-256`);
  }
  const actual = expected.get(relativePath);
  if (!actual) throw new Error(`${label} references a file that will not be published: ${relativePath}`);
  if (actual.size !== size || actual.sha256 !== sha256) {
    throw new Error(`${label} integrity mismatch: ${relativePath}`);
  }
}

async function validateMirrorFiles(root, prefix, files) {
  const expected = new Map();
  let totalBytes = 0;
  for (const file of files) {
    const record = await digestFile(file.absolute);
    const bundleHash = file.relative.match(/(?:^|\/)bundles\/b\/([a-f0-9]{64})\/[^/]+\.tar$/)?.[1];
    if (prefix === "latex" && bundleHash && record.sha256 !== bundleHash) {
      throw new Error(`LaTeX bundle hash directory does not match raw bytes: ${file.relative}`);
    }
    expected.set(file.relative, record);
    totalBytes += record.size;
  }

  if (prefix === "wasm") {
    // Every module is content-addressed: <sha256>/<name>.wasm, the directory
    // being the hash of the bytes, so a URL can never name different bytes.
    for (const file of files) {
      const match = file.relative.match(/^([a-f0-9]{64})\/[a-z][a-z0-9-]*\.wasm$/);
      if (!match) throw new Error(`wasm mirror file must be <sha256>/<name>.wasm: ${file.relative}`);
      if (expected.get(file.relative).sha256 !== match[1]) {
        throw new Error(`WASM hash directory does not match raw bytes: ${file.relative}`);
      }
    }
  } else {
    await validateLatexReleases(root, files, expected);
  }
  return { expected, count: files.length, bytes: totalBytes };
}

/// Every string anywhere in a parsed JSON value, so a release's own
/// inventory (files, licences, notices) can vouch for the files it names.
function jsonStrings(value, found = []) {
  if (typeof value === "string") found.push(value);
  else if (Array.isArray(value)) for (const item of value) jsonStrings(item, found);
  else if (value && typeof value === "object") for (const item of Object.values(value)) jsonStrings(item, found);
  return found;
}

/// A LaTeX mirror is a set of immutable release directories, `<id>/`, where
/// `id` is the SHA-256 of `<id>/MANIFEST.json`. Nothing sits at the top level
/// but those directories, and every path inside a release is relative to it.
async function validateLatexReleases(root, files, expected) {
  const ids = new Set();
  for (const file of files) {
    const id = file.relative.split("/")[0];
    if (!file.relative.includes("/") || !SHA256.test(id)) {
      throw new Error(`LaTeX mirror file must live in a <sha256>/ release directory: ${file.relative}`);
    }
    ids.add(id);
  }
  if (!ids.size) throw new Error("LaTeX mirror has no releases");
  for (const id of [...ids].sort()) {
    const label = `LaTeX release ${id.slice(0, 12)}`;
    const manifestRecord = expected.get(`${id}/MANIFEST.json`);
    if (!manifestRecord) throw new Error(`${label} is missing MANIFEST.json`);
    if (manifestRecord.sha256 !== id) throw new Error(`${label} directory does not match the SHA-256 of MANIFEST.json`);
    if (!expected.get(`${id}/release.json`)) throw new Error(`${label} is missing release.json`);
    let release;
    try { release = JSON.parse((await readFile(join(root, id, "release.json"))).toString("utf8")); } catch { throw new Error(`${label} release.json is invalid JSON`); }
    validateReleaseShape(release);
    if (release.id !== id) throw new Error(`${label} release.json id does not match its directory`);
    const releaseRoot = join(root, id);
    const referenced = new Set([`${id}/MANIFEST.json`, `${id}/release.json`]);
    const reference = (rel, size, sha256, what) => {
      expectedReference(expected, rel, size, sha256, what);
      referenced.add(rel);
    };
    for (const [name, record] of Object.entries(release.files)) {
      const rel = safeRelativePath(releaseRoot, "", record?.url, `${label} file ${name}`);
      reference(`${id}/${rel}`, record?.size, record?.sha256, `${label} file ${name}`);
    }
    const bundleMeta = release.bundles;
    if (!bundleMeta || !Number.isSafeInteger(bundleMeta.count) || bundleMeta.count < 0 || !SHA256.test(bundleMeta.sha256 || "")) {
      throw new Error(`${label} has invalid bundles metadata`);
    }
    const indexRel = safeRelativePath(releaseRoot, "", bundleMeta.index, `${label} bundles index`);
    const indexKey = `${id}/${indexRel}`;
    const indexRecord = expected.get(indexKey);
    if (!indexRecord) throw new Error(`${label} bundles index will not be published: ${indexRel}`);
    const indexBytes = await readFile(join(root, indexKey));
    const indexHash = createHash("sha256").update(indexBytes).digest("hex");
    if (indexHash !== bundleMeta.sha256 || indexHash !== indexRecord.sha256) {
      throw new Error(`${label} bundles index SHA-256 mismatch: ${indexRel}`);
    }
    referenced.add(indexKey);
    let index;
    try { index = JSON.parse(indexBytes.toString("utf8")); } catch { throw new Error(`${label} bundles index is invalid JSON`); }
    if (!index?.bundles || typeof index.bundles !== "object" || Array.isArray(index.bundles) ||
        Object.keys(index.bundles).length !== bundleMeta.count) {
      throw new Error(`${label} bundles index count does not match release.json`);
    }
    const indexDir = dirname(indexRel).split(sep).join("/");
    for (const [name, bundle] of Object.entries(index.bundles)) {
      const rel = safeRelativePath(releaseRoot, indexDir === "." ? "" : indexDir, bundle?.url, `${label} bundle ${name}`);
      reference(`${id}/${rel}`, bundle?.size, bundle?.sha256, `${label} bundle ${name}`);
    }
    // No orphans: every other file must be named by release.json or
    // MANIFEST.json, either as a file or as a directory ending in "/".
    const named = jsonStrings(release);
    try { jsonStrings(JSON.parse((await readFile(join(root, id, "MANIFEST.json"))).toString("utf8")), named); } catch { /* the id check above is the integrity gate */ }
    const names = new Set(named.map((value) => value.replace(/^\.\//, "")));
    const directories = [...names].filter((value) => value.endsWith("/"));
    for (const key of expected.keys()) {
      if (!key.startsWith(`${id}/`) || referenced.has(key)) continue;
      const rel = key.slice(id.length + 1);
      if (!names.has(rel) && !directories.some((directory) => rel.startsWith(directory))) {
        throw new Error(`${label} contains a file no manifest names: ${rel}`);
      }
    }
  }
}

export async function preflightMirror({ dir, prefix }) {
  if (!/^(wasm|latex)$/.test(prefix || "")) throw new Error("prefix must be exactly wasm or latex");
  const root = resolve(dir);
  const info = await stat(root).catch(() => null);
  if (!info?.isDirectory()) throw new Error("--dir must name an existing directory");
  const files = await listFiles(root);
  if (!files.length) throw new Error("mirror directory has no publishable files");
  const result = await validateMirrorFiles(root, prefix, files);
  return { count: result.count, bytes: result.bytes };
}

async function aws(args, env) {
  try {
    const { stdout } = await runFile("aws", args, { env, maxBuffer: 1024 * 1024 });
    return stdout;
  } catch (error) {
    if (error.code === "ENOENT") throw new Error("AWS CLI v2 was not found; install awscli2 (or use `nix shell nixpkgs#awscli2`)" );
    // Keep diagnostics useful without echoing raw AWS stderr, endpoint details,
    // request details, or credentials.
    throw new Error(awsFailureMessage(error, args));
  }
}

async function stageSource(source, destination, expected, compress) {
  const hash = createHash("sha256");
  let size = 0;
  const measure = new Transform({
    transform(chunk, encoding, callback) {
      size += chunk.length;
      hash.update(chunk);
      callback(null, chunk);
    },
  });
  await mkdir(dirname(destination), { recursive: true });
  await pipeline(createReadStream(source), measure, ...(compress ? [createGzip({ level: 6 })] : []), createWriteStream(destination));
  if (size !== expected.size || hash.digest("hex") !== expected.sha256) {
    await rm(destination, { force: true });
    throw new Error(`mirror file changed after preflight: ${source}`);
  }
}

const size = (bytes) => bytes >= 2 ** 30 ? `${(bytes / 2 ** 30).toFixed(1)} GB` : `${(bytes / 2 ** 20).toFixed(1)} MB`;
const duration = (seconds) => seconds >= 3600 ? `${Math.floor(seconds / 3600)}h${Math.floor(seconds % 3600 / 60)}m`
  : seconds >= 60 ? `${Math.floor(seconds / 60)}m${Math.floor(seconds % 60)}s` : `${Math.floor(seconds)}s`;

const SYNC_CONCURRENCY = 32;
const STAGE_CONCURRENCY = 8;
const SPOT_CHECKS = 5;

/// Files sharing one set of object metadata upload together in one `aws s3 sync`.
/// release.json files form their own last groups: a release becomes visible
/// only once its release.json exists.
function groupFiles(prefix, files) {
  const groups = new Map();
  for (const file of files) {
    const metadata = metadataFor(`${prefix}/${file.relative}`);
    const last = file.relative.endsWith("/release.json");
    const id = JSON.stringify([last, metadata.contentType, metadata.contentEncoding || null, metadata.cacheControl]);
    if (!groups.has(id)) groups.set(id, { id, last, metadata, files: [] });
    groups.get(id).files.push(file);
  }
  return [...groups.values()].sort((a, b) => Number(a.last) - Number(b.last) || a.id.localeCompare(b.id));
}

async function runPool(items, limit, work) {
  let next = 0;
  let failed = false;
  let firstError = null;
  // Every worker stops at the first failure and is awaited before the caller
  // cleans up the staging directory it may still be writing into.
  await Promise.allSettled(Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length && !failed) {
      try {
        await work(items[next++]);
      } catch (error) {
        if (!failed) firstError = error;
        failed = true;
      }
    }
  }));
  if (firstError) throw firstError;
}

/// Read back a few objects' headers, cheaply: the CLI already checksummed each
/// upload, so this only catches a sync that silently stored the wrong thing.
async function spotCheck(prefix, groups, target, env) {
  const picks = new Map();
  const staged = groups.flatMap((group) => group.files.map((file) => ({ file, group })));
  const largest = staged.reduce((best, item) => (item.file.stagedSize > (best?.file.stagedSize ?? -1) ? item : best), null);
  const release = staged.find((item) => item.file.relative.endsWith("/release.json"));
  const stride = Math.ceil(staged.length / SPOT_CHECKS);
  for (const item of [largest, release, ...staged.filter((_, index) => index % stride === 0)]) {
    if (item && picks.size < SPOT_CHECKS) picks.set(item.file.relative, item);
  }
  for (const { file, group } of picks.values()) {
    const key = `${prefix}/${file.relative}`;
    const stdout = await aws(["s3api", "head-object", "--endpoint-url", target.endpoint, "--region", target.region,
      "--bucket", target.bucket, "--key", key, "--output", "json", "--no-cli-pager"], env);
    let head;
    try { head = JSON.parse(stdout); } catch { throw new Error(`AWS returned invalid object metadata for ${key}`); }
    const { metadata } = group;
    if (head.ContentLength !== file.stagedSize) throw new Error(`uploaded object size mismatch: ${key}`);
    if (head.ContentType !== metadata.contentType || (head.ContentEncoding || undefined) !== metadata.contentEncoding ||
        head.CacheControl !== metadata.cacheControl) {
      throw new Error(`uploaded object metadata mismatch: ${key}`);
    }
  }
}

export async function publish({ dir, prefix, dryRun = false, configureCors = false, env = process.env, output = console.log }) {
  if (!/^(wasm|latex)$/.test(prefix || "")) throw new Error("prefix must be exactly wasm or latex");
  const root = resolve(dir);
  const info = await stat(root).catch(() => null);
  if (!info?.isDirectory()) throw new Error("--dir must name an existing directory");
  const files = (await listFiles(root)).sort((a, b) => {
    // A release becomes visible only once its release.json exists, so it goes last.
    const rank = (file) => file.relative.endsWith("/release.json") ? 1 : 0;
    return rank(a) - rank(b) || a.relative.localeCompare(b.relative);
  });
  if (!files.length) throw new Error("mirror directory has no publishable files");
  const target = dryRun ? null : publisherConfiguration(env);
  const preflight = dryRun ? null : await validateMirrorFiles(root, prefix, files);
  if (configureCors && dryRun) output("CORS: would allow GET and HEAD from any origin (explicit --configure-cors)");
  if (dryRun) output("Integrity preflight skipped in dry run.");
  let staging;
  try {
    if (dryRun) {
      for (const file of files) {
        const key = `${prefix}/${file.relative}`;
        const metadata = metadataFor(key);
        output(`${key}\t${metadata.contentType}\t${metadata.contentEncoding || "identity"}\t${metadata.cacheControl}`);
      }
    } else {
      const started = Date.now();
      staging = await mkdtemp(join(tmpdir(), "librepaper-mirror-"));
      const groups = groupFiles(prefix, files);
      // Each group is staged in its own subtree, gzipped where its metadata says so,
      // so one sync of that subtree uploads exactly the group with one set of headers.
      // Compressing gigabytes takes minutes, so say how far along it is: a
      // redrawn line on a terminal, a line every ten seconds otherwise.
      const tty = process.stderr.isTTY;
      let staged = 0;
      let stagedBytes = 0;
      let shown = 0;
      const showStaging = (final) => {
        const now = Date.now();
        if (!final && now - shown < (tty ? 200 : 10_000)) return;
        shown = now;
        const elapsed = (now - started) / 1000;
        const eta = stagedBytes ? duration(elapsed * (preflight.bytes - stagedBytes) / stagedBytes) : "?";
        const line = `${prefix}: preparing ${staged}/${files.length} files, ${size(stagedBytes)}/${size(preflight.bytes)}, ` +
          (final ? `done in ${duration(elapsed)}` : `ETA ${eta}`);
        process.stderr.write(tty ? `\r${line}\x1b[K${final ? "\n" : ""}` : `${line}\n`);
      };
      await runPool(groups.flatMap((group, index) => group.files.map((file) => ({ group, index, file }))), STAGE_CONCURRENCY,
        async ({ group, index, file }) => {
          const body = join(staging, "groups", String(index), ...file.relative.split("/"));
          const expected = preflight.expected.get(file.relative);
          await stageSource(file.absolute, body, expected, group.metadata.contentEncoding === "gzip");
          file.stagedSize = (await stat(body)).size;
          staged += 1;
          stagedBytes += expected.size;
          showStaging(false);
        });
      showStaging(true);
      const config = join(staging, "aws-config");
      await writeFile(config, `[default]\ns3 =\n  max_concurrent_requests = ${SYNC_CONCURRENCY}\n  max_queue_size = 10000\n`);
      const syncEnv = { ...env, AWS_CONFIG_FILE: config };
      delete syncEnv.AWS_PROFILE;
      for (const [index, group] of groups.entries()) {
        const { metadata } = group;
        const bytes = group.files.reduce((total, file) => total + file.stagedSize, 0);
        process.stderr.write(`${prefix}: group ${index + 1}/${groups.length} ${metadata.contentType} ${metadata.contentEncoding || "identity"}, ` +
          `${group.files.length} files, ${size(bytes)}${group.last ? " (release.json, last)" : ""} ...\n`);
        // Append-only, so never --delete. --size-only because staged mtimes are always new; every
        // key is content-addressed, so an existing key of the same size is the same object.
        const args = ["s3", "sync", join(staging, "groups", String(index)), `s3://${target.bucket}/${prefix}/`,
          "--endpoint-url", target.endpoint, "--region", target.region, "--size-only",
          "--content-type", metadata.contentType, "--cache-control", metadata.cacheControl, "--acl", "public-read",
          "--no-progress", "--only-show-errors"];
        if (metadata.contentEncoding) args.push("--content-encoding", metadata.contentEncoding);
        await aws(args, syncEnv);
      }
      await spotCheck(prefix, groups, target, env);
      process.stderr.write(`${prefix}: ${files.length} files in ${groups.length} groups, ${size(preflight.bytes)} raw, done in ${duration((Date.now() - started) / 1000)}\n`);
    }
    if (configureCors && !dryRun) {
      const cors = JSON.stringify({ CORSRules: [{ AllowedOrigins: ["*"], AllowedMethods: ["GET", "HEAD"], AllowedHeaders: ["*"], MaxAgeSeconds: 3600 }] });
      await aws(["s3api", "put-bucket-cors", "--endpoint-url", target.endpoint, "--region", target.region,
        "--bucket", target.bucket, "--cors-configuration", cors], env);
    }
    const uploaded = dryRun ? 0 : files.length;
    output(dryRun ? `Dry run: ${files.length} files; no objects changed.` : `Published ${uploaded} objects to s3://${target.bucket}/${prefix}/.`);
    return { count: files.length, uploaded, dryRun };
  } finally {
    if (staging) await rm(staging, { recursive: true, force: true });
  }
}

// === Staging and credential resolution ===

export async function stageWasmMirror(directory = join(fileURLToPath(new URL("../..", import.meta.url)), "web/wasm"), lockPath = join(fileURLToPath(new URL("../..", import.meta.url)), "assets.lock")) {
  const rows = (await readFile(lockPath, "utf8"))
    .split("\n")
    .map((line) => line.replace(/#.*$/, "").trim())
    .filter(Boolean)
    .map((line) => line.split(/\s+/))
    // The latex row pins a mirror release directory, not a wasm module.
    .filter(([module]) => module !== "latex");
  if (!rows.length) throw new Error("assets.lock has no modules");
  const staged = await mkdtemp(join(tmpdir(), "librepaper-wasm-mirror-"));
  try {
    for (const [module, , , sha256, ...extra] of rows) {
      if (extra.length || !/^[a-f0-9]{64}$/.test(sha256 || "")) throw new Error(`${module || "lock entry"}: expected module repo tag sha256`);
      let bytes;
      try {
        bytes = await readFile(join(directory, module));
      } catch {
        throw new Error(`${module} not found in ${directory}; run tools/assets/pins fetch first`);
      }
      if (createHash("sha256").update(bytes).digest("hex") !== sha256) {
        throw new Error(`${module} in ${directory} does not match assets.lock; run tools/assets/pins fetch first`);
      }
      await mkdir(join(staged, sha256), { recursive: true });
      await copyFile(join(directory, module), join(staged, sha256, module));
    }
  } catch (error) {
    await rm(staged, { recursive: true, force: true });
    throw error;
  }
  return staged;
}

export function mappedEnvironment(env) {
  const bucketSource = env.S3_BUCKET || env.OVH_S3_BUCKET;
  const bucket = bucketSource || bucketFromArn(env.OVH_S3_ARN);
  const mapped = {
    ...env,
    S3_ENDPOINT: env.S3_ENDPOINT || env.OVH_S3_ENDPOINT,
    S3_REGION: env.S3_REGION || env.OVH_S3_REGION,
    S3_BUCKET: bucket,
    AWS_ACCESS_KEY_ID: env.AWS_ACCESS_KEY_ID || env.OVH_S3_USER,
    AWS_SECRET_ACCESS_KEY: env.AWS_SECRET_ACCESS_KEY || env.OVH_S3_SECRET,
  };
  for (const [key, value] of Object.entries({
    S3_ENDPOINT: mapped.S3_ENDPOINT,
    S3_REGION: mapped.S3_REGION,
    S3_BUCKET: mapped.S3_BUCKET,
    AWS_ACCESS_KEY_ID: mapped.AWS_ACCESS_KEY_ID,
    AWS_SECRET_ACCESS_KEY: mapped.AWS_SECRET_ACCESS_KEY,
  })) {
    if (value !== undefined && value !== null && typeof value !== "string") throw new Error(`${key} must be a string`);
  }
  return mapped;
}

export function selectedOvhFields(secrets) {
  const selected = {};
  for (const key of ["OVH_S3_ENDPOINT", "OVH_S3_USER", "OVH_S3_SECRET", "OVH_S3_ARN", "OVH_S3_REGION", "OVH_S3_BUCKET"]) {
    if (secrets[key] !== undefined && secrets[key] !== null) selected[key] = secrets[key];
  }
  return selected;
}

export function bucketFromArn(arn) {
  if (!arn) return undefined;
  const match = /^arn:aws:s3:::([a-z0-9][a-z0-9.-]{1,61}[a-z0-9])$/.exec(arn);
  if (!match) throw new Error("OVH_S3_ARN must be an exact bucket ARN (or set S3_BUCKET)");
  return match[1];
}

const publisherEnvironmentKeys = ["S3_ENDPOINT", "S3_REGION", "S3_BUCKET", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"];

// Deploy-assets resolves these once before its S3 probes. Standalone pushes
// still load SOPS when the caller has not supplied a complete canonical config.
export async function publisherEnvironment(env, loadSecrets = loadSecretsFromSops) {
  const complete = publisherEnvironmentKeys.every((key) => env[key]);
  const candidate = complete ? env : { ...env, ...selectedOvhFields(await loadSecrets()) };
  return mappedEnvironment(candidate);
}

async function loadSecretsFromSops(keysFile) {
  try {
    const { stdout } = await runFile("sops", ["--decrypt", "--output-type", "json", keysFile], {
      maxBuffer: 1024 * 1024,
    });
    const secrets = JSON.parse(stdout);
    if (!secrets || typeof secrets !== "object" || Array.isArray(secrets)) throw new Error("invalid key file");
    return secrets;
  } catch {
    throw new Error("SOPS could not load deploy keys; check SOPS availability and deploy key configuration");
  }
}

// === CLI Entry Point ===

const invoked = process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url));

if (invoked) {
  const command = process.argv[2];

  if (!command || command === "help" || command === "-h" || command === "--help") {
    console.log("Usage: mirror <command> [args]\n");
    console.log("Commands:");
    console.log("  check <dir|url>    Validate LaTeX mirror directory or URL");
    console.log("  publish            Check and publish wasm and LaTeX mirrors to S3");
    console.log("  credentials        Resolve S3 credentials from SOPS secrets (stdin)");
    process.exit(command ? 0 : 2);
  }

  (async () => {
    try {
      switch (command) {
        case "check": {
          const target = process.argv[3];
          if (!target) throw new Error("usage: mirror check <dir | url>");
          await checkMirror(target);
          break;
        }

        case "publish": {
          const keys = process.env.KEYS || "tools/deploy/keys.yaml";
          const root = fileURLToPath(new URL("../..", import.meta.url));
          const wasmDirectory = process.env.WASM_DIR || join(root, "web/wasm");
          const wasmLock = process.env.WASM_LOCK || join(root, "assets.lock");
          const mirrorPath = process.env.MIRROR || "../wasm-latex/mirror";

          if (![undefined, "", "0", "1"].includes(process.env.MIRRORS_DRY_RUN)) {
            throw new Error("MIRRORS_DRY_RUN must be 0 or 1");
          }
          const dryRun = process.env.MIRRORS_DRY_RUN === "1";

          const staged = await stageWasmMirror(wasmDirectory, wasmLock);
          try {
            const mirrors = [
              { dir: staged, prefix: "wasm" },
              { dir: mirrorPath, prefix: "latex" },
            ];

            // Validate both trees before either publisher makes a remote call
            let env = process.env;
            if (!dryRun) {
              env = await publisherEnvironment(process.env, () => loadSecretsFromSops(keys));
            }

            for (const mirror of mirrors) {
              try {
                statSync(mirror.dir);
              } catch {
                throw new Error(`${mirror.prefix} mirror not found at ${mirror.dir}; run tools/assets/mirror build first`);
              }
              console.log(`Checking ${mirror.prefix} mirror...`);
              const result = await preflightMirror(mirror);
              console.log(`Validated ${mirror.prefix}: ${result.count} files, ${result.bytes} bytes.`);
            }

            if (!dryRun) {
              for (const mirror of mirrors) {
                await publish({ ...mirror, dryRun: false, env });
              }
            } else {
              console.log("Dry run complete; no credentials loaded or objects changed.");
            }
          } finally {
            await rm(staged, { recursive: true, force: true });
          }
          break;
        }

        case "credentials": {
          // Read secrets from stdin, resolve with publisherEnvironment and publisherConfiguration
          let input = "";
          process.stdin.setEncoding("utf8");
          for await (const chunk of process.stdin) input += chunk;

          const secrets = JSON.parse(input);
          if (!secrets || typeof secrets !== "object" || Array.isArray(secrets)) {
            throw new Error("invalid secrets JSON");
          }

          const env = await publisherEnvironment(process.env, async () => secrets);
          publisherConfiguration(env);

          const keys = ["S3_ENDPOINT", "S3_REGION", "S3_BUCKET", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"];
          const values = keys.map((key) => env[key]);

          if (values.some((value) => typeof value !== "string" || !value || /[\x00-\x1f]/.test(value))) {
            process.exit(1);
          }

          process.stdout.write(values.join("\n"));
          break;
        }

        default:
          console.error(`unknown command: ${command}`);
          process.exit(2);
      }
    } catch (error) {
      console.error(`mirror: ${error.message}`);
      process.exitCode = 1;
    }
  })();
}
