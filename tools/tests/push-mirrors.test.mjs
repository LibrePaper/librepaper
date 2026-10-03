import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { bucketFromArn, mappedEnvironment, publisherEnvironment, selectedOvhFields, stageWasmMirror } from "../push-mirrors.mjs";

const script = fileURLToPath(new URL("../push-mirrors.mjs", import.meta.url));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function makeMirrors(root, wasmName = "wasm dir", latexName = "latex mirror") {
  const wasm = join(root, wasmName);
  const latex = join(root, latexName);
  const module = Buffer.from("valid markdown wasm");
  await mkdir(wasm, { recursive: true });
  await writeFile(join(wasm, "markdown.wasm"), module);
  const lock = join(root, "assets.lock");
  await writeFile(lock, `# test lock\nmarkdown.wasm wasm-markdown v0.1.1 ${hash(module)}\nlatex wasm-latex v0.1.0 ${"a".repeat(64)}\n`);
  // One release directory: <id>/ where id is the SHA-256 of MANIFEST.json.
  const manifest = Buffer.from('{"label":"push-mirrors"}\n');
  const id = hash(manifest);
  const release = join(latex, id);
  const asset = Buffer.from("latex engine");
  const bundle = Buffer.from("bundle");
  const index = Buffer.from(JSON.stringify({ bundles: { core: { url: "bundle.tar", size: bundle.length, sha256: hash(bundle) } } }));
  await mkdir(join(release, "bundles"), { recursive: true });
  await writeFile(join(release, "MANIFEST.json"), manifest);
  await writeFile(join(release, "engine.js"), asset);
  await writeFile(join(release, "bundles", "bundles.json"), index);
  await writeFile(join(release, "bundles", "bundle.tar"), bundle);
  await writeFile(join(release, "release.json"), JSON.stringify({
    format: 2, id,
    files: {
      engine: { url: "engine.js", size: asset.length, sha256: hash(asset) },
      "bundles.json": { url: "bundles/bundles.json", size: index.length, sha256: hash(index) },
    },
    bundles: { index: "bundles/bundles.json", count: 1, sha256: hash(index) },
  }));
  return { wasm, lock, latex, release, moduleHash: hash(module) };
}

test("OVH variables map safely, canonical overrides win, and only an exact bucket ARN is derived", () => {
  assert.deepEqual(mappedEnvironment({ OVH_S3_ENDPOINT: "https://ovh.invalid", OVH_S3_REGION: "bhs",
    OVH_S3_USER: "publisher", OVH_S3_SECRET: "secret", OVH_S3_ARN: "arn:aws:s3:::asset-bucket" }), {
    OVH_S3_ENDPOINT: "https://ovh.invalid", OVH_S3_REGION: "bhs", OVH_S3_USER: "publisher", OVH_S3_SECRET: "secret",
    OVH_S3_ARN: "arn:aws:s3:::asset-bucket", S3_ENDPOINT: "https://ovh.invalid", S3_REGION: "bhs",
    S3_BUCKET: "asset-bucket", AWS_ACCESS_KEY_ID: "publisher", AWS_SECRET_ACCESS_KEY: "secret",
  });
  assert.equal(mappedEnvironment({ S3_ENDPOINT: "https://override.invalid", S3_REGION: "override", S3_BUCKET: "chosen",
    AWS_ACCESS_KEY_ID: "id", AWS_SECRET_ACCESS_KEY: "secret", OVH_S3_ENDPOINT: "https://ignored.invalid",
    OVH_S3_ARN: "not-an-arn" }).S3_BUCKET, "chosen");
  assert.throws(() => bucketFromArn("arn:aws:s3:::bucket/object"), /exact bucket ARN/);
});

test("null SOPS placeholders are absent and canonical overrides precede effective-value validation", () => {
  const selected = selectedOvhFields({ OVH_S3_ENDPOINT: null, OVH_S3_BUCKET: null, OVH_S3_SECRET: 17 });
  assert.deepEqual(selected, { OVH_S3_SECRET: 17 });
  assert.equal(mappedEnvironment({ ...selected, S3_ENDPOINT: "https://canonical.invalid", S3_BUCKET: "canonical-bucket",
    S3_REGION: "region", AWS_ACCESS_KEY_ID: "id", AWS_SECRET_ACCESS_KEY: "secret" }).S3_BUCKET, "canonical-bucket");
  assert.throws(() => mappedEnvironment({ ...selected, OVH_S3_ENDPOINT: 17 }), /S3_ENDPOINT must be a string/);
});

test("a null OVH bucket allows the exact bucket ARN fallback", () => {
  const selected = selectedOvhFields({ OVH_S3_BUCKET: null, OVH_S3_ARN: "arn:aws:s3:::asset-bucket" });
  assert.equal(mappedEnvironment(selected).S3_BUCKET, "asset-bucket");
});

test("complete canonical publisher settings skip SOPS while incomplete settings retain fallback mapping", async () => {
  let loads = 0;
  const canonical = {
    S3_ENDPOINT: "https://canonical.invalid", S3_REGION: "canonical-region", S3_BUCKET: "canonical-bucket",
    AWS_ACCESS_KEY_ID: "canonical-id", AWS_SECRET_ACCESS_KEY: "canonical-secret",
  };
  const resolved = await publisherEnvironment(canonical, async () => {
    loads += 1;
    return {};
  });
  assert.equal(loads, 0);
  assert.equal(resolved.S3_ENDPOINT, "https://canonical.invalid");
  assert.equal(resolved.S3_BUCKET, "canonical-bucket");

  const partial = await publisherEnvironment({ S3_ENDPOINT: "https://override.invalid" }, async () => {
    loads += 1;
    return {
      OVH_S3_ENDPOINT: "https://from-sops.invalid", OVH_S3_REGION: "region-from-sops",
      OVH_S3_ARN: "arn:aws:s3:::sops-bucket", OVH_S3_USER: "sops-id", OVH_S3_SECRET: "sops-secret",
    };
  });
  assert.equal(loads, 1);
  assert.equal(partial.S3_ENDPOINT, "https://override.invalid");
  assert.equal(partial.S3_REGION, "region-from-sops");
  assert.equal(partial.S3_BUCKET, "sops-bucket");
  assert.equal(partial.AWS_ACCESS_KEY_ID, "sops-id");
});

test("dry run stages the wasm mirror, hashes both mirrors and skips SOPS, even without credentials", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-dryrun-"));
  const bin = join(root, "bin");
  const paths = await makeMirrors(root, "wasm 'dir'", "latex mirror");
  await mkdir(bin);
  const sops = join(bin, "sops");
  await writeFile(sops, `#!${process.execPath}\nrequire('node:fs').writeFileSync(process.env.SOPS_CALLED, 'yes');\n`);
  await chmod(sops, 0o755);
  try {
    const output = execFileSync(process.execPath, [script], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, MIRRORS_DRY_RUN: "1", WASM_DIR: paths.wasm, WASM_LOCK: paths.lock, MIRROR: paths.latex,
      SOPS_CALLED: join(root, "sops-called") } });
    assert.match(output, /Validated wasm: 1 files/);
    assert.match(output, /Validated latex: 5 files/);
    await assert.rejects(readFile(join(root, "sops-called")));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("SOPS failure diagnostics are suppressed", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-sops-error-"));
  const bin = join(root, "bin");
  await mkdir(bin);
  const sops = join(bin, "sops");
  await writeFile(sops, "#!/bin/sh\necho private-test-secret >&2\nexit 9\n");
  await chmod(sops, 0o755);
  try {
    let error;
    try { execFileSync(process.execPath, [script], { encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}` } }); }
    catch (caught) { error = caught; }
    assert.ok(error);
    assert.match(error.stderr, /SOPS could not load deploy keys/);
    assert.doesNotMatch(error.stderr, /private-test-secret/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("both mirrors are preflighted before any AWS upload", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-preflight-"));
  const bin = join(root, "bin");
  const paths = await makeMirrors(root);
  await writeFile(join(paths.release, "release.json"), "invalid release");
  await mkdir(bin);
  const sops = join(bin, "sops");
  const aws = join(bin, "aws");
  const calls = join(root, "aws-called");
  await writeFile(sops, `#!${process.execPath}\nprocess.stdout.write(JSON.stringify({ OVH_S3_ENDPOINT:'https://s3.example.invalid', OVH_S3_REGION:'bhs', OVH_S3_USER:'id', OVH_S3_SECRET:'secret', OVH_S3_ARN:'arn:aws:s3:::asset-bucket' }));\n`);
  await writeFile(aws, `#!${process.execPath}\nrequire('node:fs').appendFileSync(process.env.AWS_CALLED, 'call\\n');\n`);
  await chmod(sops, 0o755);
  await chmod(aws, 0o755);
  try {
    let error;
    try { execFileSync(process.execPath, [script], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, WASM_DIR: paths.wasm, WASM_LOCK: paths.lock, MIRROR: paths.latex, AWS_CALLED: calls } }); }
    catch (caught) { error = caught; }
    assert.ok(error);
    assert.match(error.stderr, /release\.json is invalid JSON/);
    await assert.rejects(readFile(calls));
    assert.doesNotMatch(error.stderr, /secret/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("SOPS key mappings reach the shared publisher for both prefixes", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-publish-"));
  const bin = join(root, "bin");
  const paths = await makeMirrors(root);
  const log = join(root, "aws.jsonl");
  const store = join(root, "objects.json");
  await mkdir(bin);
  const sops = join(bin, "sops");
  const aws = join(bin, "aws");
  await writeFile(sops, `#!${process.execPath}\nprocess.stdout.write(JSON.stringify({ OVH_S3_ENDPOINT:'https://ovh.example.invalid', OVH_S3_REGION:'bhs', OVH_S3_USER:'mapped-id', OVH_S3_SECRET:'mapped-secret', OVH_S3_ARN:'arn:aws:s3:::mapped-bucket', unrelated:{ignored:true} }));\n`);
  await writeFile(aws, `#!${process.execPath}
const fs=require('node:fs'); const path=require('node:path'); const a=process.argv.slice(2); const at=k=>a.indexOf(k); const val=k=>at(k)<0?undefined:a[at(k)+1];
const storePath=process.env.AWS_STORE; let store={}; try{store=JSON.parse(fs.readFileSync(storePath,'utf8'))}catch{}
const cmd=a.slice(0,2).join(' ');
const walk=(d,b='')=>fs.readdirSync(d,{withFileTypes:true}).flatMap(e=>e.isDirectory()?walk(path.join(d,e.name),b+e.name+'/'):[b+e.name]);
const log=(key)=>fs.appendFileSync(process.env.AWS_LOG,JSON.stringify({cmd,key,endpoint:val('--endpoint-url'),region:val('--region'),bucket:val('--bucket')||a[3].split('/')[2],id:process.env.AWS_ACCESS_KEY_ID,secret:process.env.AWS_SECRET_ACCESS_KEY,deleting:a.includes('--delete')})+'\\n');
if(cmd==='s3 sync'){
  const destination=a[3].replace(/^s3:\\/\\/[^/]+\\//,'');
  for(const rel of walk(a[2]).sort()){
    const body=fs.readFileSync(path.join(a[2],rel));
    store[destination+rel]={length:body.length,ContentType:val('--content-type'),ContentEncoding:val('--content-encoding')||undefined,CacheControl:val('--cache-control')};
    log(destination+rel);
  }
}
if(cmd==='s3api head-object'){const obj=store[val('--key')]; process.stdout.write(JSON.stringify({ContentLength:obj.length,ContentType:obj.ContentType,ContentEncoding:obj.ContentEncoding,CacheControl:obj.CacheControl})); log(val('--key'));}
fs.writeFileSync(storePath,JSON.stringify(store));
`);
  await chmod(sops, 0o755);
  await chmod(aws, 0o755);
  try {
    execFileSync(process.execPath, [script], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, WASM_DIR: paths.wasm, WASM_LOCK: paths.lock, MIRROR: paths.latex, AWS_LOG: log, AWS_STORE: store,
      S3_BUCKET: "explicit-bucket", AWS_ACCESS_KEY_ID: "override-id", AWS_SECRET_ACCESS_KEY: "override-secret" } });
    const calls = (await readFile(log, "utf8")).trim().split("\n").map(JSON.parse);
    const puts = calls.filter((call) => call.cmd === "s3 sync");
    assert.ok(calls.every((call) => !call.deleting));
    assert.ok(puts.some((call) => call.key === `wasm/${paths.moduleHash}/markdown.wasm`));
    assert.ok(puts.some((call) => call.key.startsWith("latex/")));
    assert.ok(calls.every((call) => call.endpoint === "https://ovh.example.invalid" && call.region === "bhs" &&
      call.bucket === "explicit-bucket" && call.id === "override-id" && call.secret === "override-secret"));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("staging refuses a module whose bytes do not match assets.lock", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-stage-"));
  const paths = await makeMirrors(root);
  try {
    const staged = await stageWasmMirror(paths.wasm, paths.lock);
    try {
      assert.equal(await readFile(join(staged, paths.moduleHash, "markdown.wasm"), "utf8"), "valid markdown wasm");
    } finally { await rm(staged, { recursive: true, force: true }); }
    await writeFile(join(paths.wasm, "markdown.wasm"), "tampered");
    await assert.rejects(stageWasmMirror(paths.wasm, paths.lock), /does not match assets\.lock/);
    await rm(join(paths.wasm, "markdown.wasm"));
    await assert.rejects(stageWasmMirror(paths.wasm, paths.lock), /run tools\/pins fetch first/);
  } finally { await rm(root, { recursive: true, force: true }); }
});
