#!/usr/bin/env node

// Publish a prepared browser asset mirror to an S3-compatible bucket. AWS CLI
// supplies SigV4 signing, so credentials stay in its environment and never in
// command arguments or this tool's reports.
import { execFile } from "node:child_process";
import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { createGzip } from "node:zlib";
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
const MANIFESTS = new Set(["manifest.json", "bundles.json", "index.html", "index.json"]);

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
  if (!options.prefix) throw new Error("--prefix typst|latex is required");
  if (!/^(typst|latex)$/.test(options.prefix)) throw new Error("prefix must be exactly typst or latex");
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
  const cacheControl = name === "manifest.json" ? "no-store" : name === "bundles.json" ? "no-cache" :
    /(^|\/)(licen[cs]e|copying|notice)(\.|\/|$)/i.test(key) || /(^|\/)licenses\//i.test(key)
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

function configuration(env) {
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

async function aws(args, env) {
  try {
    await runFile("aws", args, { env, maxBuffer: 1024 * 1024 });
  } catch (error) {
    if (error.code === "ENOENT") throw new Error("AWS CLI v2 was not found; install awscli2 (or use `nix shell nixpkgs#awscli2`)" );
    // AWS diagnostic text can include endpoint or request details. Keep the
    // report useful without ever echoing environment values or credentials.
    throw new Error(`AWS CLI operation failed${error.code ? ` (${error.code})` : ""}`);
  }
}

export async function publish({ dir, prefix, dryRun = false, configureCors = false, env = process.env, output = console.log }) {
  if (!/^(typst|latex)$/.test(prefix || "")) throw new Error("prefix must be exactly typst or latex");
  const root = resolve(dir);
  const info = await stat(root).catch(() => null);
  if (!info?.isDirectory()) throw new Error("--dir must name an existing directory");
  const files = (await listFiles(root)).sort((a, b) => {
    const rank = (file) => file.relative === "manifest.json" ? 2 :
      MANIFESTS.has(file.relative.split("/").at(-1).toLowerCase()) ? 1 : 0;
    return rank(a) - rank(b) || a.relative.localeCompare(b.relative);
  });
  if (!files.length) throw new Error("mirror directory has no publishable files");
  const target = dryRun ? null : configuration(env);
  if (configureCors && dryRun) output("CORS: would allow GET and HEAD from any origin (explicit --configure-cors)");
  let staging;
  let uploaded = 0;
  try {
    for (const file of files) {
      const key = `${prefix}/${file.relative}`;
      const metadata = metadataFor(key);
      let body = file.absolute;
      if (dryRun) {
        output(`${key}\t${metadata.contentType}\t${metadata.contentEncoding || "identity"}\t${metadata.cacheControl}`);
      } else {
        if (metadata.contentEncoding) {
          staging ||= await mkdtemp(join(tmpdir(), "librepaper-mirror-"));
          body = join(staging, file.relative);
          await mkdir(dirname(body), { recursive: true });
          await pipeline(createReadStream(file.absolute), createGzip({ level: 9 }), createWriteStream(body));
        }
        const args = ["s3api", "put-object", "--endpoint-url", target.endpoint, "--region", target.region,
          "--bucket", target.bucket, "--key", key, "--body", body,
          "--content-type", metadata.contentType, "--cache-control", metadata.cacheControl];
        if (metadata.contentEncoding) args.push("--content-encoding", metadata.contentEncoding);
        await aws(args, env);
        if (metadata.contentEncoding) await rm(body, { force: true });
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
