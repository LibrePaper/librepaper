#!/usr/bin/env node

// Load deployment credentials through SOPS, validate both prepared mirrors,
// then publish them with the shared integrity-checking publisher.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { preflightMirror, publish, publisherConfiguration } from "./publish-mirror.mjs";

const runFile = promisify(execFile);
const keys = process.env.KEYS || "deploy/keys.yaml";
const mirrors = [
  { dir: process.env.TYPST_MIRROR || "../wasm-typst/mirror", prefix: "typst" },
  { dir: process.env.MIRROR || "../wasm-latex/mirror", prefix: "latex" },
];

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
  // Validate both trees before either publisher can make a remote call.
  for (const mirror of mirrors) {
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
