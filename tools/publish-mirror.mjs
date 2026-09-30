#!/usr/bin/env node

// Publish a prepared browser asset mirror to an S3-compatible bucket. AWS CLI
// supplies SigV4 signing, so credentials stay in its environment and never in
// command arguments or this tool's reports.
import { execFile } from "node:child_process";
import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, mkdtemp, readFile, readdir, rm, stat } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { Transform, Writable } from "node:stream";
import { createGunzip, createGzip } from "node:zlib";
import { pipeline } from "node:stream/promises";

const runFile = promisify(execFile);
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
const AWS_OPERATIONS = new Set(["put-object", "get-object", "put-bucket-cors"]);
const AWS_HINTS = new Map([
  ["AccessDenied", "Check that the mirror credentials have permission for this bucket and prefix."],
  ["InvalidAccessKeyId", "Check that the configured AWS access key is valid for this mirror."],
  ["SignatureDoesNotMatch", "Check that the S3 access key ID and secret match, and verify the endpoint and region."],
  ["NoSuchBucket", "Check that the configured mirror bucket exists at this endpoint."],
  ["InvalidRequest", "Check the mirror bucket and endpoint configuration."],
  ["NotImplemented", "Check that the mirror endpoint supports this S3 operation."],
]);

export function awsFailureMessage(error, args) {
  const operation = AWS_OPERATIONS.has(args?.[1]) ? args[1] : null;
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

export function parseArgs(args) {
  const options = { cors: false, dryRun: false };
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--configure-cors") options.cors = true;
    else if (["--dir", "--prefix"].includes(arg)) {
      const value = args[++i];
      if (!value || value.startsWith("--")) throw new Error(`${arg} requires a value`);
      options[arg.slice(2)] = value;
    } else throw new Error(`unknown option: ${arg}`);
  }
  if (!options.dir) throw new Error("--dir <mirror> is required");
  if (!options.prefix) throw new Error("--prefix wasm|latex is required");
  if (!/^(wasm|latex)$/.test(options.prefix)) throw new Error("prefix must be exactly wasm or latex");
  return options;
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
  if (typeof url !== "string" || !url || url.trim() !== url ||
      url.startsWith("/") || /[\\%?#:\0]/.test(url)) {
    throw new Error(`${label} has an unsafe URL`);
  }
  const pieces = [...base.split("/").filter((part) => part && part !== "."), ...url.split("/")];
  if (pieces.some((part) => !part || part === "." || part === "..")) {
    throw new Error(`${label} has an unsafe URL`);
  }
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
    if (release?.format !== 2) throw new Error(`${label} release.json must use format 2`);
    if (release.id !== id) throw new Error(`${label} release.json id does not match its directory`);
    if (!release.files || typeof release.files !== "object" || Array.isArray(release.files)) {
      throw new Error(`${label} has no files map`);
    }
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
  await pipeline(createReadStream(source), measure, ...(compress ? [createGzip({ level: 9 })] : []), createWriteStream(destination));
  if (size !== expected.size || hash.digest("hex") !== expected.sha256) {
    await rm(destination, { force: true });
    throw new Error(`mirror file changed after preflight: ${source}`);
  }
}

async function verifyDownloaded(path, response, metadata, expected, key) {
  if (!response || response.ContentType !== metadata.contentType ||
      (response.ContentEncoding || undefined) !== metadata.contentEncoding ||
      response.CacheControl !== metadata.cacheControl) {
    throw new Error(`uploaded object metadata mismatch: ${key}`);
  }
  const hash = createHash("sha256");
  let size = 0;
  const sink = new Writable({
    write(chunk, encoding, callback) {
      size += chunk.length;
      hash.update(chunk);
      callback();
    },
  });
  const streams = [createReadStream(path)];
  if (metadata.contentEncoding === "gzip") streams.push(createGunzip());
  streams.push(sink);
  await pipeline(...streams);
  if (size !== expected.size || hash.digest("hex") !== expected.sha256) {
    throw new Error(`uploaded object bytes mismatch: ${key}`);
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
  let uploaded = 0;
  try {
    for (const [index, file] of files.entries()) {
      const key = `${prefix}/${file.relative}`;
      const metadata = metadataFor(key);
      if (dryRun) {
        output(`${key}\t${metadata.contentType}\t${metadata.contentEncoding || "identity"}\t${metadata.cacheControl}`);
      } else {
        staging ||= await mkdtemp(join(tmpdir(), "librepaper-mirror-"));
        const body = join(staging, "upload", ...file.relative.split("/"));
        await stageSource(file.absolute, body, preflight.expected.get(file.relative), metadata.contentEncoding === "gzip");
        const args = ["s3api", "put-object", "--endpoint-url", target.endpoint, "--region", target.region,
          "--bucket", target.bucket, "--key", key, "--body", body,
          "--content-type", metadata.contentType, "--cache-control", metadata.cacheControl,
          "--acl", "public-read"];
        if (metadata.contentEncoding) args.push("--content-encoding", metadata.contentEncoding);
        await aws(args, env);
        await rm(body, { force: true });
        const downloadPath = join(staging, "readback", `${index}.object`);
        await mkdir(dirname(downloadPath), { recursive: true });
        try {
          const stdout = await aws(["s3api", "get-object", "--endpoint-url", target.endpoint, "--region", target.region,
            "--bucket", target.bucket, "--key", key, downloadPath, "--output", "json", "--no-cli-pager"], env);
          let response;
          try { response = JSON.parse(stdout); } catch { throw new Error(`AWS returned invalid object metadata for ${key}`); }
          await verifyDownloaded(downloadPath, response, metadata, preflight.expected.get(file.relative), key);
        } finally {
          await rm(downloadPath, { force: true });
        }
        uploaded += 1;
      }
    }
    if (configureCors && !dryRun) {
      const cors = JSON.stringify({ CORSRules: [{ AllowedOrigins: ["*"], AllowedMethods: ["GET", "HEAD"], AllowedHeaders: ["*"], MaxAgeSeconds: 3600 }] });
      await aws(["s3api", "put-bucket-cors", "--endpoint-url", target.endpoint, "--region", target.region,
        "--bucket", target.bucket, "--cors-configuration", cors], env);
    }
    output(dryRun ? `Dry run: ${files.length} files; no objects changed.` : `Published ${uploaded} objects to s3://${target.bucket}/${prefix}/.`);
    return { count: files.length, uploaded, dryRun };
  } finally {
    if (staging) await rm(staging, { recursive: true, force: true });
  }
}

const invoked = process.argv[1] && resolve(process.argv[1]) === resolve(new URL(import.meta.url).pathname);
if (invoked) {
  try {
    const options = parseArgs(process.argv.slice(2));
    await publish({ ...options, configureCors: options.cors });
  } catch (error) {
    console.error(`publish-mirror: ${error.message}`);
    process.exitCode = 1;
  }
}
