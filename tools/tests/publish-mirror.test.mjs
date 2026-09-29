import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, open, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import test from "node:test";
import { metadataFor, parseArgs, publish } from "../publish-mirror.mjs";

const script = new URL("../publish-mirror.mjs", import.meta.url).pathname;
const secret = "do-not-print-this-secret";

test("publisher validates prefixes and selects explicit browser metadata", () => {
  assert.throws(() => parseArgs(["--dir", ".", "--prefix", "../typst"]), /prefix must be exactly/);
  assert.deepEqual(metadataFor("typst/objects/" + "a".repeat(64) + ".wasm"), {
    contentType: "application/wasm", contentEncoding: "gzip", cacheControl: "public, max-age=31536000, immutable",
  });
  assert.deepEqual(metadataFor("latex/manifest.json"), {
    contentType: "application/json", contentEncoding: "gzip", cacheControl: "no-store",
  });
  assert.equal(metadataFor("latex/bundles.json").cacheControl, "no-cache");
  assert.equal(metadataFor("typst/licenses/OFL.txt").cacheControl, "public, max-age=86400");
  assert.equal(metadataFor("typst/fonts/example.otf").contentEncoding, undefined);
  assert.equal(metadataFor("typst/cache/engine/bundles.json").cacheControl, "no-cache");
  assert.equal(metadataFor("typst/engine.fmt").contentType, "application/octet-stream");
  assert.equal(metadataFor("typst/engine.fmt").contentEncoding, "gzip");
  assert.equal(metadataFor("typst/archive.tar").contentEncoding, "gzip");
  assert.equal(metadataFor("typst/engine.data").contentEncoding, "gzip");
  assert.deepEqual(metadataFor("latex/receipt.pdf.gz"), {
    contentType: "application/gzip", contentEncoding: undefined, cacheControl: "public, max-age=3600",
  });
  assert.throws(() => parseArgs(["--dir", ".", "--prefix", "typst", "--bogus"]), /unknown option/);
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
    const result = await publish({ dir: root, prefix: "typst", dryRun: true, env: {}, output: (line) => lines.push(line) });
    const after = new Set((await readdir(tmpdir())).filter((name) => name.startsWith("librepaper-mirror-")));
    assert.equal(result.count, 1);
    assert.equal(result.uploaded, 0);
    assert.equal(result.dryRun, true);
    assert.deepEqual(after, before);
    assert.match(lines[0], /application\/wasm\tgzip\tpublic, max-age=3600/);
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
      await assert.rejects(publish({ dir: root, prefix: "typst", env: { ...baseEnv, S3_ENDPOINT: endpoint }, output: () => {} }),
        /S3_ENDPOINT/);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("successful S3 publishing gzips payloads, skips transport files, and uploads indexes last", async () => {
  const root = await mkdtemp(join(tmpdir(), "publish-mirror-success-"));
  const source = join(root, "mirror");
  const bin = join(root, "bin");
  const log = join(root, "aws.jsonl");
  await mkdir(source);
  await mkdir(join(source, "engine"));
  await mkdir(join(source, ".cloudflare"));
  await mkdir(join(source, "nested"));
  await mkdir(bin);
  await writeFile(join(source, "readme.txt"), "payload text\n");
  await writeFile(join(source, "empty.txt"), "");
  await writeFile(join(source, "engine.data"), Buffer.from([0, 1, 2, 255]));
  await writeFile(join(source, "engine.fmt"), Buffer.from([0, 255, 1]));
  await writeFile(join(source, "archive.tar"), "tar bytes");
  await writeFile(join(source, "icudt.dat.gz"), Buffer.from([31, 139, 8, 0, 1]));
  await writeFile(join(source, "engine", "bundles.json"), '{"bundle":"hash"}\n');
  await mkdir(join(source, "zz"));
  await writeFile(join(source, "zz", "index.json"), '{"index":true}\n');
  await writeFile(join(source, "index.html"), "<h1>index</h1>");
  await writeFile(join(source, "manifest.json"), '{"version":1}\n');
  await writeFile(join(source, "_headers"), "ignored");
  await writeFile(join(source, ".assetsignore"), "ignored");
  await writeFile(join(source, "nested", "old.js.br"), "ignored");
  await writeFile(join(source, "nested", "x.metadata"), "ignored");
  await writeFile(join(source, "nested", "x.cfmeta"), "ignored");
  await writeFile(join(source, ".cloudflare", "meta.json"), "ignored");

  const fakeAws = join(bin, "aws");
  await writeFile(fakeAws, `#!${process.execPath}\nconst fs = require('node:fs');\nconst { gunzipSync } = require('node:zlib');\nconst args = process.argv.slice(2);\nconst value = (key) => { const index = args.indexOf(key); return index < 0 ? undefined : args[index + 1]; };\nconst entry = { command: args.slice(0, 2).join(' '), key: value('--key') || null };\nif (entry.command === 's3api put-object') { const bodyPath = value('--body'); const encoded = fs.readFileSync(bodyPath); if (bodyPath.includes('/librepaper-mirror-')) entry.bodyPath = bodyPath; entry.body = (value('--content-encoding') === 'gzip' ? gunzipSync(encoded) : encoded).toString('base64'); entry.contentType = value('--content-type'); entry.contentEncoding = value('--content-encoding') || null; entry.cacheControl = value('--cache-control'); }\nfs.appendFileSync(process.env.AWS_LOG, JSON.stringify(entry) + '\\n');\n`);
  await chmod(fakeAws, 0o755);
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, AWS_LOG: log,
    S3_ENDPOINT: "https://s3.example.invalid", S3_REGION: "region-1", S3_BUCKET: "dedicated-assets",
    AWS_ACCESS_KEY_ID: "access", AWS_SECRET_ACCESS_KEY: secret };
  const records = async () => (await readFile(log, "utf8")).trim().split("\n").filter(Boolean).map(JSON.parse);
  try {
    const result = await publish({ dir: source, prefix: "typst", env, output: () => {} });
    assert.equal(result.count, 10);
    assert.equal(result.uploaded, 10);
    const entries = await records();
    assert.ok(entries.every((entry) => entry.command === "s3api put-object"));
    const keys = entries.map((entry) => entry.key);
    assert.equal(keys.at(-1), "typst/manifest.json");
    const firstIndex = keys.findIndex((key) => /(?:^|\/)(?:bundles\.json|index\.html|index\.json|manifest\.json)$/.test(key));
    assert.ok(firstIndex > 0);
    assert.ok(keys.slice(firstIndex).every((key) => /(?:^|\/)(?:bundles\.json|index\.html|index\.json|manifest\.json)$/.test(key)));
    assert.deepEqual(keys.slice(-4), ["typst/engine/bundles.json", "typst/index.html", "typst/zz/index.json", "typst/manifest.json"]);
    for (const skipped of ["typst/_headers", "typst/.assetsignore", "typst/nested/old.js.br", "typst/nested/x.metadata", "typst/nested/x.cfmeta", "typst/.cloudflare/meta.json"]) {
      assert.ok(!keys.includes(skipped), `${skipped} should be skipped`);
    }
    const byKey = new Map(entries.map((entry) => [entry.key, entry]));
    assert.equal(Buffer.from(byKey.get("typst/readme.txt").body, "base64").toString(), "payload text\n");
    assert.equal(byKey.get("typst/readme.txt").contentType, "text/plain; charset=utf-8");
    assert.equal(byKey.get("typst/readme.txt").contentEncoding, "gzip");
    assert.equal(Buffer.from(byKey.get("typst/empty.txt").body, "base64").length, 0);
    assert.equal(Buffer.from(byKey.get("typst/engine.data").body, "base64").toString("hex"), "000102ff");
    assert.equal(byKey.get("typst/engine.fmt").contentType, "application/octet-stream");
    assert.equal(byKey.get("typst/engine.fmt").contentEncoding, "gzip");
    assert.equal(byKey.get("typst/archive.tar").contentType, "application/x-tar");
    assert.equal(Buffer.from(byKey.get("typst/icudt.dat.gz").body, "base64").toString("hex"), "1f8b080001");
    assert.equal(byKey.get("typst/icudt.dat.gz").contentType, "application/gzip");
    assert.equal(byKey.get("typst/icudt.dat.gz").contentEncoding, null);
    assert.equal(byKey.get("typst/manifest.json").cacheControl, "no-store");
    assert.equal(byKey.get("typst/engine/bundles.json").cacheControl, "no-cache");
    assert.equal((await readFile(join(source, "icudt.dat.gz"))).toString("hex"), "1f8b080001");
    for (const entry of entries.filter((row) => row.bodyPath)) {
      await assert.rejects(stat(entry.bodyPath), { code: "ENOENT" });
    }

    await rm(log);
    await publish({ dir: source, prefix: "typst", configureCors: true, env, output: () => {} });
    const corsEntries = await records();
    assert.equal(corsEntries.at(-1).command, "s3api put-bucket-cors");
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
  await writeFile(join(source, "manifest.json"), "{}\n");
  const fakeAws = join(bin, "aws");
  await writeFile(fakeAws, "#!/bin/sh\nprintf '%s\\n' \"$AWS_SECRET_ACCESS_KEY\" >&2\nexit 17\n");
  await chmod(fakeAws, 0o755);
  try {
    let stderr = "";
    try {
      execFileSync(process.execPath, [script, "--dir", source, "--prefix", "typst"], {
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
