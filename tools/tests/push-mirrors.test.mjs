import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import test from "node:test";
import { bucketFromArn, mappedEnvironment } from "../push-mirrors.mjs";

const script = new URL("../push-mirrors.mjs", import.meta.url).pathname;
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function makeMirrors(root, typstName = "typst mirror", latexName = "latex mirror") {
  const typst = join(root, typstName);
  const latex = join(root, latexName);
  const wasm = Buffer.from("valid typst wasm");
  await mkdir(join(typst, hash(wasm)), { recursive: true });
  await writeFile(join(typst, hash(wasm), "typst.wasm"), wasm);
  const asset = Buffer.from("latex engine");
  await mkdir(latex, { recursive: true });
  await writeFile(join(latex, "engine.js"), asset);
  await writeFile(join(latex, "manifest.json"), JSON.stringify({
    format: 1,
    releases: { release: { files: { engine: { url: "engine.js", size: asset.length, sha256: hash(asset) } },
      bundles: { index: "bundles.json", count: 1, sha256: hash(Buffer.from('{"bundles":{"core":{"url":"bundle.tar","size":6,"sha256":"' + hash(Buffer.from("bundle")) + '"}}}')) } } },
  }));
  const bundle = Buffer.from("bundle");
  const index = Buffer.from(JSON.stringify({ bundles: { core: { url: "bundle.tar", size: bundle.length, sha256: hash(bundle) } } }));
  // Refresh manifest metadata after serializing the index.
  const manifest = JSON.parse(await readFile(join(latex, "manifest.json"), "utf8"));
  manifest.releases.release.bundles.sha256 = hash(index);
  await writeFile(join(latex, "manifest.json"), JSON.stringify(manifest));
  await writeFile(join(latex, "bundles.json"), index);
  await writeFile(join(latex, "bundle.tar"), bundle);
  return { typst, latex };
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

test("dry run hashes both overridden mirrors and skips SOPS, even without credentials", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-dryrun-"));
  const bin = join(root, "bin");
  const paths = await makeMirrors(root, "typst 'mirror'", "latex mirror");
  await mkdir(bin);
  const sops = join(bin, "sops");
  await writeFile(sops, `#!${process.execPath}\nrequire('node:fs').writeFileSync(process.env.SOPS_CALLED, 'yes');\n`);
  await chmod(sops, 0o755);
  try {
    const output = execFileSync(process.execPath, [script], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, MIRRORS_DRY_RUN: "1", TYPST_MIRROR: paths.typst, MIRROR: paths.latex,
      SOPS_CALLED: join(root, "sops-called") } });
    assert.match(output, /Validated typst: 1 files/);
    assert.match(output, /Validated latex: 4 files/);
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
  await writeFile(join(paths.latex, "manifest.json"), "invalid manifest");
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
      PATH: `${bin}:${process.env.PATH}`, TYPST_MIRROR: paths.typst, MIRROR: paths.latex, AWS_CALLED: calls } }); }
    catch (caught) { error = caught; }
    assert.ok(error);
    assert.match(error.stderr, /LaTeX manifest\.json is invalid JSON/);
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
const fs=require('node:fs'); const a=process.argv.slice(2); const at=k=>a.indexOf(k); const val=k=>a[at(k)+1];
const storePath=process.env.AWS_STORE; let store={}; try{store=JSON.parse(fs.readFileSync(storePath,'utf8'))}catch{}
const cmd=a.slice(0,2).join(' '); const key=val('--key');
if(cmd==='s3api put-object') store[key]={body:fs.readFileSync(val('--body')).toString('base64'), ContentType:val('--content-type'),ContentEncoding:val('--content-encoding')||undefined,CacheControl:val('--cache-control')};
if(cmd==='s3api get-object'){const obj=store[key]; fs.writeFileSync(a[at('--key')+2],Buffer.from(obj.body,'base64')); process.stdout.write(JSON.stringify({ContentType:obj.ContentType,ContentEncoding:obj.ContentEncoding,CacheControl:obj.CacheControl}));}
fs.writeFileSync(storePath,JSON.stringify(store));
fs.appendFileSync(process.env.AWS_LOG,JSON.stringify({cmd,key,endpoint:val('--endpoint-url'),region:val('--region'),bucket:val('--bucket'),id:process.env.AWS_ACCESS_KEY_ID,secret:process.env.AWS_SECRET_ACCESS_KEY})+'\\n');
`);
  await chmod(sops, 0o755);
  await chmod(aws, 0o755);
  try {
    execFileSync(process.execPath, [script], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, TYPST_MIRROR: paths.typst, MIRROR: paths.latex, AWS_LOG: log, AWS_STORE: store,
      S3_BUCKET: "explicit-bucket", AWS_ACCESS_KEY_ID: "override-id", AWS_SECRET_ACCESS_KEY: "override-secret" } });
    const calls = (await readFile(log, "utf8")).trim().split("\n").map(JSON.parse);
    const puts = calls.filter((call) => call.cmd === "s3api put-object");
    assert.ok(puts.some((call) => call.key.startsWith("typst/")));
    assert.ok(puts.some((call) => call.key.startsWith("latex/")));
    assert.ok(calls.every((call) => call.endpoint === "https://ovh.example.invalid" && call.region === "bhs" &&
      call.bucket === "explicit-bucket" && call.id === "override-id" && call.secret === "override-secret"));
  } finally { await rm(root, { recursive: true, force: true }); }
});
