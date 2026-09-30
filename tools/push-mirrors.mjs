#!/usr/bin/env node

// Load deployment credentials through SOPS, validate both prepared mirrors,
// then publish them with the shared integrity-checking publisher. The LaTeX
// mirror is prepared in ../wasm-latex; the wasm mirror is staged here from
// web/wasm and assets.lock as <sha256>/<module>.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { statSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { preflightMirror, publish, publisherConfiguration } from "./publish-mirror.mjs";

const runFile = promisify(execFile);
const keys = process.env.KEYS || "deploy/keys.yaml";
const root = fileURLToPath(new URL("..", import.meta.url));
const wasmDirectory = process.env.WASM_DIR || join(root, "web/wasm");
const wasmLock = process.env.WASM_LOCK || join(root, "assets.lock");

// Copy every wasm module in assets.lock from wasmDirectory into a temporary
// tree laid out as <sha256>/<module>, refusing any file whose bytes do not
// hash to the lock. Returns the tree; the caller removes it.
export async function stageWasmMirror(directory = wasmDirectory, lockPath = wasmLock) {
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
        throw new Error(`${module} not found in ${directory}; run tools/pins fetch first`);
      }
      if (createHash("sha256").update(bytes).digest("hex") !== sha256) {
        throw new Error(`${module} in ${directory} does not match assets.lock; run tools/pins fetch first`);
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

async function run() {
  if (![undefined, "", "0", "1"].includes(process.env.MIRRORS_DRY_RUN)) {
    throw new Error("MIRRORS_DRY_RUN must be 0 or 1");
  }
  const dryRun = process.env.MIRRORS_DRY_RUN === "1";
  if (dryRun) {
    await publishPrepared(process.env, { dryRun: true });
    return;
  }

  let secrets;
  try {
    const { stdout } = await runFile("sops", ["--decrypt", "--output-type", "json", keys], {
      maxBuffer: 1024 * 1024,
    });
    secrets = JSON.parse(stdout);
    if (!secrets || typeof secrets !== "object" || Array.isArray(secrets)) throw new Error("invalid key file");
  } catch {
    throw new Error("SOPS could not load deploy keys; check SOPS availability and deploy key configuration");
  }
  const selected = selectedOvhFields(secrets);
  await publishPrepared({ ...process.env, ...selected });
}

async function publishPrepared(rawEnv, { dryRun = false } = {}) {
  const env = dryRun ? rawEnv : mappedEnvironment(rawEnv);
  if (!dryRun) publisherConfiguration(env);
  const staged = await stageWasmMirror();
  try {
    await publishMirrors([
      { dir: staged, prefix: "wasm" },
      { dir: process.env.MIRROR || "../wasm-latex/mirror", prefix: "latex" },
    ], env, dryRun);
  } finally {
    await rm(staged, { recursive: true, force: true });
  }
}

async function publishMirrors(mirrors, env, dryRun) {
  // Validate both trees before either publisher can make a remote call.
  for (const mirror of mirrors) {
    try {
      statSync(mirror.dir);
    } catch {
      throw new Error(`${mirror.prefix} mirror not found at ${mirror.dir}; run deploy/assets build first`);
    }
    console.log(`Checking ${mirror.prefix} mirror...`);
    const result = await preflightMirror(mirror);
    console.log(`Validated ${mirror.prefix}: ${result.count} files, ${result.bytes} bytes.`);
  }
  if (dryRun) {
    console.log("Dry run complete; no credentials loaded or objects changed.");
    return;
  }
  for (const mirror of mirrors) {
    await publish({ ...mirror, dryRun, env });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))) {
  run().catch((error) => {
    // Only print known-safe validation messages; never surface SOPS diagnostics.
    console.error(`mirrors-push: ${error.message}`);
    process.exitCode = 1;
  });
}
