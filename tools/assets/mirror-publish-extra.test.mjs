import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { publish } from './mirror.mjs';

const secret = 'do-not-print-this-secret';
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

// A minimal release directory `<id>/` that passes LaTeX preflight: id is the
// SHA-256 of MANIFEST.json, release.json is format 2 with paths relative to the
// directory, and the bundles index is empty. `assets` are extra listed files.
async function writeLatexRelease(source, assets = {}, label = 'release') {
  const manifest = Buffer.from(`{"label":"${label}"}\n`);
  const id = sha256(manifest);
  const dir = join(source, id);
  const index = Buffer.from('{"bundles":{}}\n');
  await mkdir(join(dir, 'bundles'), { recursive: true });
  await writeFile(join(dir, 'MANIFEST.json'), manifest);
  await writeFile(join(dir, 'bundles', 'bundles.json'), index);
  const files = { 'bundles.json': { url: 'bundles/bundles.json', size: index.length, sha256: sha256(index) } };
  const worker = Buffer.from('pdftex worker');
  await writeFile(join(dir, 'worker.js'), worker);
  files['worker.js'] = { url: 'worker.js', size: worker.length, sha256: sha256(worker) };
  for (const [name, bytes] of Object.entries(assets)) {
    await mkdir(join(dir, name, '..'), { recursive: true });
    await writeFile(join(dir, name), bytes);
    files[name] = { url: name, size: bytes.length, sha256: sha256(bytes) };
  }
  await writeFile(join(dir, 'release.json'), JSON.stringify({
    format: 2, id, engines: { pdftex: { worker: 'worker.js', files: ['worker.js'] } }, files,
    bundles: { index: 'bundles/bundles.json', count: 0, sha256: sha256(index) },
  }));
  return { id, dir };
}

// === Additional publish-mirror tests (from original publish-mirror.test.mjs) ===

// Fake aws program used by multiple tests
const fakeAwsProgram = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const { brotliDecompressSync, gunzipSync } = require('node:zlib');
const args = process.argv.slice(2);
const value = (key) => { const index = args.indexOf(key); return index < 0 ? undefined : args[index + 1]; };
const command = args.slice(0, 2).join(' ');
const storePath = process.env.AWS_STORE;
let store = {}; try { store = JSON.parse(fs.readFileSync(storePath, 'utf8')); } catch {}
const log = (entry) => fs.appendFileSync(process.env.AWS_LOG, JSON.stringify(entry) + '\\n');
const walk = (directory, base = '') => fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
  entry.isDirectory() ? walk(path.join(directory, entry.name), base + entry.name + '/') : [base + entry.name]);
if (command === 's3 sync') {
  const source = args[2];
  const destination = args[3].replace(/^s3:\\/\\/[^/]+\\//, '');
  const config = fs.readFileSync(process.env.AWS_CONFIG_FILE, 'utf8');
  for (const relative of walk(source).sort()) {
    const key = destination + relative;
    const encoded = fs.readFileSync(path.join(source, relative));
    if (args.includes('--size-only') && store[key] && Buffer.from(store[key].encoded, 'base64').length === encoded.length) continue;
    const encodingArg = value('--content-encoding');
    const decoded = encodingArg === 'gzip' ? gunzipSync(encoded) : encodingArg === 'br' ? brotliDecompressSync(encoded) : encoded;
    store[key] = { encoded: encoded.toString('base64'), decoded: decoded.toString('base64'),
      contentType: value('--content-type'), contentEncoding: value('--content-encoding') || undefined,
      cacheControl: value('--cache-control'), acl: value('--acl') };
    log({ command, key, body: decoded.toString('base64'), bodyPath: path.join(source, relative),
      contentType: store[key].contentType, contentEncoding: store[key].contentEncoding || null,
      cacheControl: store[key].cacheControl, acl: store[key].acl, deleting: args.includes('--delete'),
      concurrency: /max_concurrent_requests = (\\d+)/.exec(config)?.[1] });
  }
  fs.writeFileSync(storePath, JSON.stringify(store));
  log({ command: 's3 sync done', destination, deleting: args.includes('--delete') });
} else if (command === 's3api head-object') {
  const key = value('--key');
  const object = store[key];
  if (!object) process.exit(4);
  const response = { ContentLength: Buffer.from(object.encoded, 'base64').length, ContentType: object.contentType, CacheControl: object.cacheControl };
  if (object.contentEncoding) response.ContentEncoding = object.contentEncoding;
  const tamper = key === process.env.AWS_TAMPER_KEY ? process.env.AWS_TAMPER_MODE : '';
  if (tamper === 'size') response.ContentLength += 1;
  if (tamper === 'metadata') response.ContentType = 'application/octet-stream';
  process.stdout.write(JSON.stringify(response));
  log({ command, key });
} else {
  log({ command, key: value('--key') || null });
}
`;

test("successful S3 publishing compresses payloads, skips transport files, and uploads release.json last", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-success-"));
  const source = join(root, "mirror");
  const bin = join(root, "bin");
  const log = join(root, "aws.jsonl");
  await mkdir(source);
  await mkdir(bin);
  const { id, dir } = await writeLatexRelease(source, {
    "readme.txt": Buffer.from("payload text\n"),
    "empty.txt": Buffer.alloc(0),
    "engine.data": Buffer.from([0, 1, 2, 255]),
    "engine.fmt": Buffer.from([0, 255, 1]),
    "archive.tar": Buffer.from("tar bytes"),
    "icudt.dat.gz": Buffer.from([31, 139, 8, 0, 1]),
    "engine.wasm": Buffer.from("compiled wasm"),
  });
  await mkdir(join(dir, "nested"));
  await mkdir(join(dir, ".cloudflare"));
  await writeFile(join(source, "_headers"), "ignored");
  await writeFile(join(source, ".assetsignore"), "ignored");
  await writeFile(join(dir, "nested", "old.js.br"), "ignored");
  await writeFile(join(dir, "nested", "x.metadata"), "ignored");
  await writeFile(join(dir, "nested", "x.cfmeta"), "ignored");
  await writeFile(join(dir, ".cloudflare", "meta.json"), "ignored");

  const fakeAws = join(bin, "aws");
  await writeFile(fakeAws, fakeAwsProgram);
  await chmod(fakeAws, 0o755);
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, AWS_LOG: log, AWS_STORE: join(root, "aws-store.json"),
    S3_ENDPOINT: "https://s3.example.invalid", S3_REGION: "region-1", S3_BUCKET: "dedicated-assets",
    AWS_ACCESS_KEY_ID: "access", AWS_SECRET_ACCESS_KEY: secret };
  const records = async () => (await readFile(log, "utf8")).trim().split("\n").filter(Boolean).map(JSON.parse);
  try {
    const result = await publish({ dir: source, prefix: "latex", env, output: () => {} });
    assert.equal(result.count, 11);
    assert.equal(result.uploaded, 11);
    const entries = await records();
    assert.ok(entries.every((entry) => ["s3 sync", "s3 sync done", "s3api head-object"].includes(entry.command)));
    const puts = entries.filter((entry) => entry.command === "s3 sync");
    assert.ok(puts.every((entry) => !entry.deleting), "the mirror is append-only: never --delete");
    assert.ok(entries.filter((entry) => entry.command === "s3 sync done").every((entry) => entry.destination === "latex/"));
    assert.ok(puts.every((entry) => entry.concurrency === "32"));
    const keys = puts.map((entry) => entry.key);
    assert.equal(keys.at(-1), `latex/${id}/release.json`);
    for (const skipped of ["latex/_headers", "latex/.assetsignore", `latex/${id}/nested/old.js.br`, `latex/${id}/nested/x.metadata`, `latex/${id}/nested/x.cfmeta`, `latex/${id}/.cloudflare/meta.json`]) {
      assert.ok(!keys.includes(skipped), `${skipped} should be skipped`);
    }
    const byKey = new Map(puts.map((entry) => [entry.key, entry]));
    const at = (name) => byKey.get(`latex/${id}/${name}`);
    assert.equal(Buffer.from(at("readme.txt").body, "base64").toString(), "payload text\n");
    assert.equal(at("readme.txt").contentType, "text/plain; charset=utf-8");
    assert.equal(at("readme.txt").contentEncoding, "gzip");
    assert.equal(Buffer.from(at("empty.txt").body, "base64").length, 0);
    assert.equal(Buffer.from(at("engine.data").body, "base64").toString("hex"), "000102ff");
    assert.equal(at("engine.fmt").contentType, "application/octet-stream");
    assert.equal(at("engine.fmt").contentEncoding, "gzip");
    assert.equal(at("engine.data").contentEncoding, "br");
    assert.equal(at("engine.wasm").contentEncoding, "br");
    assert.equal(at("archive.tar").contentType, "application/x-tar");
    assert.equal(Buffer.from(at("icudt.dat.gz").body, "base64").toString("hex"), "1f8b080001");
    assert.equal(at("icudt.dat.gz").contentType, "application/gzip");
    assert.equal(at("icudt.dat.gz").contentEncoding, null);
    assert.ok(puts.every((entry) => entry.cacheControl === "public, max-age=31536000, immutable"));
    assert.equal(Buffer.from(at("engine.wasm").body, "base64").toString(), "compiled wasm");
    assert.ok(puts.every((entry) => entry.acl === "public-read"));
    const releaseIndex = entries.findIndex((entry) => entry.key === `latex/${id}/release.json`);
    assert.ok(releaseIndex > 0);
    assert.ok(entries.slice(releaseIndex + 1).every((entry) => entry.command !== "s3 sync"));
    assert.ok(entries.slice(0, releaseIndex).filter((entry) => entry.command === "s3 sync").length === puts.length - 1);
    const heads = entries.filter((entry) => entry.command === "s3api head-object");
    assert.ok(heads.length > 0 && heads.length <= 5);
    assert.ok(heads.some((entry) => entry.key === `latex/${id}/release.json`));
    assert.ok(entries.findIndex((entry) => entry.command === "s3api head-object") > releaseIndex);
    assert.equal((await readFile(join(dir, "icudt.dat.gz"))).toString("hex"), "1f8b080001");
    for (const entry of entries.filter((row) => row.bodyPath)) {
      await assert.rejects(stat(entry.bodyPath), { code: "ENOENT" });
    }

    await rm(log);
    await publish({ dir: source, prefix: "latex", configureCors: true, env, output: () => {} });
    const corsEntries = await records();
    assert.ok(!corsEntries.some((entry) => entry.command === "s3 sync"));
    assert.equal(corsEntries.at(-1).command, "s3api put-bucket-cors");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a spot check that finds the wrong size or metadata fails the publish", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-spot-check-"));
  const source = join(root, "mirror");
  const bin = join(root, "bin");
  const log = join(root, "aws.jsonl");
  const store = join(root, "aws-store.json");
  await mkdir(source);
  await mkdir(bin);
  const { id } = await writeLatexRelease(source, { "engine.wasm": Buffer.from("verified wasm release") });
  const tamperedKey = `latex/${id}/release.json`;
  const fakeAws = join(bin, "aws");
  await writeFile(fakeAws, fakeAwsProgram);
  await chmod(fakeAws, 0o755);
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, AWS_LOG: log, AWS_STORE: store,
    AWS_TAMPER_KEY: tamperedKey, S3_ENDPOINT: "https://s3.example.invalid", S3_REGION: "region-1",
    S3_BUCKET: "dedicated-assets", AWS_ACCESS_KEY_ID: "access", AWS_SECRET_ACCESS_KEY: secret };
  try {
    for (const [mode, error] of [["size", /uploaded object size mismatch/], ["metadata", /metadata mismatch/]]) {
      await assert.rejects(publish({ dir: source, prefix: "latex", env: { ...env, AWS_TAMPER_MODE: mode }, output: () => {} }), error);
    }
    const entries = (await readFile(log, "utf8")).trim().split("\n").map(JSON.parse);
    assert.ok(entries.some((entry) => entry.command === "s3api head-object" && entry.key === tamperedKey));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("AWS upload failures exit unsuccessfully without claiming publication or printing credentials", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-failure-"));
  const source = join(root, "mirror");
  const bin = join(root, "bin");
  await mkdir(source);
  await mkdir(bin);
  await writeLatexRelease(source, { "engine.wasm": Buffer.from("verified before upload") });
  const fakeAws = join(bin, "aws");
  await writeFile(fakeAws, "#!/bin/sh\nprintf '%s\\n' \"$AWS_SECRET_ACCESS_KEY\" >&2\nexit 17\n");
  await chmod(fakeAws, 0o755);
  try {
    await assert.rejects(publish({ dir: source, prefix: "latex", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`,
      S3_ENDPOINT: "https://s3.example.invalid", S3_REGION: "region-1", S3_BUCKET: "dedicated-assets",
      AWS_ACCESS_KEY_ID: "access", AWS_SECRET_ACCESS_KEY: secret }, output: () => {} }),
      /AWS CLI operation failed/);
    // Verify no credential text in error
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
