import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, open, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import test from "node:test";
import { awsFailureMessage, metadataFor, parseArgs, preflightMirror, publish } from "../publish-mirror.mjs";

const script = new URL("../publish-mirror.mjs", import.meta.url).pathname;
const secret = "do-not-print-this-secret";
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

// A minimal release directory `<id>/` that passes LaTeX preflight: id is the
// SHA-256 of MANIFEST.json, release.json is format 2 with paths relative to the
// directory, and the bundles index is empty. `assets` are extra listed files.
async function writeLatexRelease(source, assets = {}, label = "release") {
  const manifest = Buffer.from(`{"label":"${label}"}\n`);
  const id = sha256(manifest);
  const dir = join(source, id);
  const index = Buffer.from('{"bundles":{}}\n');
  await mkdir(join(dir, "bundles"), { recursive: true });
  await writeFile(join(dir, "MANIFEST.json"), manifest);
  await writeFile(join(dir, "bundles", "bundles.json"), index);
  const files = { "bundles.json": { url: "bundles/bundles.json", size: index.length, sha256: sha256(index) } };
  for (const [name, bytes] of Object.entries(assets)) {
    await mkdir(join(dir, name, ".."), { recursive: true });
    await writeFile(join(dir, name), bytes);
    files[name] = { url: name, size: bytes.length, sha256: sha256(bytes) };
  }
  await writeFile(join(dir, "release.json"), JSON.stringify({
    format: 2, id, engines: {}, files,
    bundles: { index: "bundles/bundles.json", count: 0, sha256: sha256(index) },
  }));
  return { id, dir };
}

test("AWS failure diagnostics expose safe service codes and actionable hints only", () => {
  const serviceMessage = awsFailureMessage({
    status: 254,
    stderr: "An error occurred (AccessDenied) when calling the PutObject operation: secret=do-not-print-this-secret https://private.example/request",
  }, ["s3", "sync", "/staging", "s3://bucket/latex/"]);
  assert.match(serviceMessage, /s3 sync/);
  assert.match(serviceMessage, /exit code 254/);
  assert.match(serviceMessage, /AccessDenied/);
  assert.match(serviceMessage, /permission/);
  assert.doesNotMatch(serviceMessage, /do-not-print-this-secret|private\.example|request/);

  const unknownMessage = awsFailureMessage({
    code: "private-code-do-not-print",
    stderr: "arbitrary secret=do-not-print-this-secret endpoint https://private.example",
  }, ["s3api", "head-object"]);
  assert.equal(unknownMessage, "AWS CLI operation failed head-object");
  assert.doesNotMatch(unknownMessage, /private-code|secret|private\.example/);
  const signatureMessage = awsFailureMessage({ code: 254,
    stderr: "An error occurred (SignatureDoesNotMatch) when calling the HeadObject operation: private detail",
  }, ["s3api", "head-object"]);
  assert.match(signatureMessage, /exit code 254/);
  assert.match(signatureMessage, /SignatureDoesNotMatch/);
  assert.match(signatureMessage, /S3 access key ID and secret match/);
  assert.doesNotMatch(signatureMessage, /private detail/);
});

// A fake `aws`: `s3 sync` copies the staged subtree into a JSON store the way the
// real CLI would (keys under the destination prefix, given metadata, skip when a
// key of the same size exists with --size-only) and logs one record per file it
// stores; `s3api head-object` answers from the store.
const fakeAwsProgram = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const { gunzipSync } = require('node:zlib');
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
    const decoded = value('--content-encoding') === 'gzip' ? gunzipSync(encoded) : encoded;
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

test("publisher validates prefixes and selects explicit browser metadata", () => {
  assert.throws(() => parseArgs(["--dir", ".", "--prefix", "../wasm"]), /prefix must be exactly/);
  assert.deepEqual(metadataFor("wasm/" + "a".repeat(64) + "/markdown.wasm"), {
    contentType: "application/wasm", contentEncoding: "gzip", cacheControl: "public, max-age=31536000, immutable",
  });
  const releaseId = "b".repeat(64);
  assert.deepEqual(metadataFor(`latex/${releaseId}/release.json`), {
    contentType: "application/json", contentEncoding: "gzip", cacheControl: "public, max-age=31536000, immutable",
  });
  assert.equal(metadataFor(`latex/${releaseId}/bundles/bundles.json`).cacheControl, "public, max-age=31536000, immutable");
  assert.equal(metadataFor("latex/licenses/OFL.txt").cacheControl, "public, max-age=86400");
  assert.equal(metadataFor("latex/fonts/example.otf").contentEncoding, undefined);
  assert.equal(metadataFor("latex/engine.fmt").contentType, "application/octet-stream");
  assert.equal(metadataFor("latex/engine.fmt").contentEncoding, "gzip");
  assert.equal(metadataFor("latex/archive.tar").contentEncoding, "gzip");
  assert.equal(metadataFor("latex/engine.data").contentEncoding, "gzip");
  assert.deepEqual(metadataFor("latex/receipt.pdf.gz"), {
    contentType: "application/gzip", contentEncoding: undefined, cacheControl: "public, max-age=3600",
  });
  assert.throws(() => parseArgs(["--dir", ".", "--prefix", "wasm", "--bogus"]), /unknown option/);
});

test("dry run lists large files without staging or gzip compression", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-dry-run-"));
  const large = join(root, "large.wasm");
  const handle = await open(large, "w");
  await handle.truncate(11 * 1024 * 1024 * 1024);
  await handle.close();
  try {
    const before = new Set((await readdir(tmpdir())).filter((name) => name.startsWith("librepaper-mirror-")));
    const lines = [];
    const result = await publish({ dir: root, prefix: "wasm", dryRun: true, env: {}, output: (line) => lines.push(line) });
    const after = new Set((await readdir(tmpdir())).filter((name) => name.startsWith("librepaper-mirror-")));
    assert.equal(result.count, 1);
    assert.equal(result.uploaded, 0);
    assert.equal(result.dryRun, true);
    assert.deepEqual(after, before);
    assert.ok(lines.some((line) => /application\/wasm\tgzip\tpublic, max-age=3600/.test(line)));
    assert.equal((await stat(large)).size, 11 * 1024 * 1024 * 1024);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("S3 endpoint must be HTTPS without embedded credentials, query, or fragment", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-endpoint-"));
  await writeFile(join(root, "asset.txt"), "asset");
  const baseEnv = { S3_REGION: "region-1", S3_BUCKET: "dedicated-assets",
    AWS_ACCESS_KEY_ID: "access", AWS_SECRET_ACCESS_KEY: secret };
  try {
    for (const endpoint of ["http://s3.example.invalid", "https://user:pass@s3.example.invalid",
      "https://s3.example.invalid/?token=x", "https://s3.example.invalid/#fragment"]) {
      await assert.rejects(publish({ dir: root, prefix: "wasm", env: { ...baseEnv, S3_ENDPOINT: endpoint }, output: () => {} }),
        /S3_ENDPOINT/);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("LaTeX preflight verifies release files, index, and relative bundle records", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-latex-preflight-"));
  const manifest = Buffer.from('{"label":"preflight"}\n');
  const id = sha256(manifest);
  const dir = join(root, id);
  const bundleBytes = Buffer.from("bundle bytes");
  const bundleHash = sha256(bundleBytes);
  const indexBytes = Buffer.from(JSON.stringify({ bundles: {
    core: { url: `b/${bundleHash}/bundle.tar`, size: bundleBytes.length, sha256: bundleHash },
  } }));
  const assetBytes = Buffer.from("engine js");
  await mkdir(join(dir, "bundles", "b", bundleHash), { recursive: true });
  await writeFile(join(dir, "MANIFEST.json"), manifest);
  await writeFile(join(dir, "bundles", "bundles.json"), indexBytes);
  await writeFile(join(dir, "bundles", "b", bundleHash, "bundle.tar"), bundleBytes);
  await writeFile(join(dir, "engine.js"), assetBytes);
  await writeFile(join(dir, "release.json"), JSON.stringify({
    format: 2, id,
    files: {
      engine: { url: "engine.js", size: assetBytes.length, sha256: sha256(assetBytes) },
      "bundles.json": { url: "bundles/bundles.json", size: indexBytes.length, sha256: sha256(indexBytes) },
    },
    bundles: { index: "bundles/bundles.json", sha256: sha256(indexBytes), count: 1 },
  }));
  try {
    const result = await preflightMirror({ dir: root, prefix: "latex" });
    assert.equal(result.count, 5);
    assert.equal(result.bytes, manifest.length + bundleBytes.length + indexBytes.length + assetBytes.length +
      (await stat(join(dir, "release.json"))).size);

    // An unlisted file in a release is an orphan.
    await writeFile(join(dir, "orphan.txt"), "not in any manifest");
    await assert.rejects(preflightMirror({ dir: root, prefix: "latex" }), /no manifest names/);
    await rm(join(dir, "orphan.txt"));

    // Nothing but release directories at the top level.
    await writeFile(join(root, "manifest.json"), "{}");
    await assert.rejects(preflightMirror({ dir: root, prefix: "latex" }), /<sha256>\/ release directory/);
    await rm(join(root, "manifest.json"));

    // A retained bundle whose hash directory lies about its bytes.
    const liar = join(dir, "bundles", "b", "0".repeat(64));
    await mkdir(liar, { recursive: true });
    await writeFile(join(liar, "orphan.tar"), "corrupt retained bundle");
    await assert.rejects(preflightMirror({ dir: root, prefix: "latex" }), /bundle hash directory does not match/);
    await rm(liar, { recursive: true });

    // The directory name must be the SHA-256 of MANIFEST.json.
    await writeFile(join(dir, "MANIFEST.json"), '{"label":"changed"}\n');
    await assert.rejects(preflightMirror({ dir: root, prefix: "latex" }), /does not match the SHA-256 of MANIFEST.json/);
    await writeFile(join(dir, "MANIFEST.json"), manifest);

    // release.json must be format 2 and reference bytes that match.
    await writeFile(join(dir, "engine.js"), "tampered");
    await assert.rejects(preflightMirror({ dir: root, prefix: "latex" }), /integrity mismatch/);
    await writeFile(join(dir, "engine.js"), assetBytes);
    const release = JSON.parse(await readFile(join(dir, "release.json"), "utf8"));
    await writeFile(join(dir, "release.json"), JSON.stringify({ ...release, format: 1 }));
    await assert.rejects(preflightMirror({ dir: root, prefix: "latex" }), /must use format 2/);
    await writeFile(join(dir, "release.json"), JSON.stringify({ ...release, files: { engine: { url: "../escape.js", size: 1, sha256: "0".repeat(64) } } }));
    await assert.rejects(preflightMirror({ dir: root, prefix: "latex" }), /unsafe URL/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("WASM hash mismatch fails preflight before making AWS calls", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-wasm-preflight-"));
  const source = join(root, "mirror");
  const bin = join(root, "bin");
  const log = join(root, "aws.jsonl");
  await mkdir(join(source, "0".repeat(64)), { recursive: true });
  await mkdir(bin);
  await writeFile(join(source, "0".repeat(64), "markdown.wasm"), "not-the-hash");
  const fakeAws = join(bin, "aws");
  await writeFile(fakeAws, `#!${process.execPath}\nrequire('node:fs').appendFileSync(process.env.AWS_LOG, 'called\\n');\n`);
  await chmod(fakeAws, 0o755);
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, AWS_LOG: log,
    S3_ENDPOINT: "https://s3.example.invalid", S3_REGION: "region-1", S3_BUCKET: "dedicated-assets",
    AWS_ACCESS_KEY_ID: "access", AWS_SECRET_ACCESS_KEY: secret };
  try {
    await assert.rejects(publish({ dir: source, prefix: "wasm", env, output: () => {} }), /hash directory does not match/);
    await assert.rejects(stat(log), { code: "ENOENT" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("successful S3 publishing gzips payloads, skips transport files, and uploads release.json last", async () => {
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
    assert.equal(result.count, 10);
    assert.equal(result.uploaded, 10);
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
    assert.equal(at("archive.tar").contentType, "application/x-tar");
    assert.equal(Buffer.from(at("icudt.dat.gz").body, "base64").toString("hex"), "1f8b080001");
    assert.equal(at("icudt.dat.gz").contentType, "application/gzip");
    assert.equal(at("icudt.dat.gz").contentEncoding, null);
    // Everything under a release directory is immutable: no special cases.
    assert.ok(puts.every((entry) => entry.cacheControl === "public, max-age=31536000, immutable"));
    assert.equal(Buffer.from(at("engine.wasm").body, "base64").toString(), "compiled wasm");
    assert.ok(puts.every((entry) => entry.acl === "public-read"));
    // A single sync per metadata group, with release.json in a final sync of its own.
    const releaseIndex = entries.findIndex((entry) => entry.key === `latex/${id}/release.json`);
    assert.ok(releaseIndex > 0);
    assert.ok(entries.slice(releaseIndex + 1).every((entry) => entry.command !== "s3 sync"));
    assert.ok(entries.slice(0, releaseIndex).filter((entry) => entry.command === "s3 sync").length === puts.length - 1);
    // Spot checks read headers only after every sync, including release.json's.
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
    // Every key already exists with the same size, so the second sync stores nothing and deletes nothing.
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
    let stderr = "";
    try {
      execFileSync(process.execPath, [script, "--dir", source, "--prefix", "latex"], {
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, S3_ENDPOINT: "https://s3.example.invalid",
          S3_REGION: "region-1", S3_BUCKET: "dedicated-assets", AWS_ACCESS_KEY_ID: "access",
          AWS_SECRET_ACCESS_KEY: secret },
        encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
      });
      assert.fail("publisher should fail when AWS CLI fails");
    } catch (error) {
      assert.equal(error.status, 1);
      stderr = error.stderr;
    }
    assert.match(stderr, /AWS CLI operation failed/);
    assert.doesNotMatch(stderr, /Published/);
    assert.doesNotMatch(stderr, new RegExp(secret));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("wasm preflight accepts only <sha256>/<name>.wasm files whose directory is their hash", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-wasm-layout-"));
  const bytes = Buffer.from("module bytes");
  const hash = sha256(bytes);
  await mkdir(join(root, hash));
  await writeFile(join(root, hash, "markdown.wasm"), bytes);
  try {
    const result = await preflightMirror({ dir: root, prefix: "wasm" });
    assert.equal(result.count, 1);
    assert.equal(result.bytes, bytes.length);
    await writeFile(join(root, "stray.txt"), "not a module");
    await assert.rejects(preflightMirror({ dir: root, prefix: "wasm" }), /must be <sha256>\/<name>\.wasm/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
