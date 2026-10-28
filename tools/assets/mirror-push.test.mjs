import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { mappedEnvironment, metadataFor, preflightMirror, publisherEnvironment, selectedMirrorFields, stagePinnedMirrors } from './mirror.mjs';
import { PINNED_FILES } from './pins-lock.mjs';

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

// === push-mirrors tests ===


async function makeMirrors(root, assetsName = 'assets dir', latexName = 'latex mirror') {
  const assets = join(root, assetsName);
  const latex = join(root, latexName);
  const module = Buffer.from('valid markdown wasm');
  const font = Buffer.from('valid font');
  await mkdir(join(assets, 'wasm'), { recursive: true });
  await mkdir(join(assets, 'fonts'), { recursive: true });
  const rows = [];
  for (const [name, { repo, prefix }] of PINNED_FILES) {
    const bytes = prefix === 'wasm' ? module : font;
    await writeFile(join(assets, prefix, name), bytes);
    rows.push(`${name} ${repo} v0.1.1 ${sha256(bytes)}`);
  }
  const lock = join(root, 'assets.lock');
  await writeFile(lock, `# test lock\n${rows.join('\n')}\nlatex wasm-latex v0.1.0 ${'a'.repeat(64)}\n`);
  // One release directory: <id>/ where id is the SHA-256 of MANIFEST.json.
  const manifest = Buffer.from('{"label":"push-mirrors"}\n');
  const id = sha256(manifest);
  const releaseDir = join(latex, id);
  const asset = Buffer.from('latex engine');
  const worker = Buffer.from('pdftex worker');
  const bundle = Buffer.from('bundle');
  const index = Buffer.from(JSON.stringify({ bundles: { core: { url: 'bundle.tar', size: bundle.length, sha256: sha256(bundle) } } }));
  await mkdir(join(releaseDir, 'bundles'), { recursive: true });
  await writeFile(join(releaseDir, 'MANIFEST.json'), manifest);
  await writeFile(join(releaseDir, 'engine.js'), asset);
  await writeFile(join(releaseDir, 'worker.js'), worker);
  await writeFile(join(releaseDir, 'bundles', 'bundles.json'), index);
  await writeFile(join(releaseDir, 'bundles', 'bundle.tar'), bundle);
  await writeFile(join(releaseDir, 'release.json'), JSON.stringify({
    format: 2, id, engines: { pdftex: { worker: 'worker.js', files: ['worker.js'] } },
    files: {
      engine: { url: 'engine.js', size: asset.length, sha256: sha256(asset) },
      'worker.js': { url: 'worker.js', size: worker.length, sha256: sha256(worker) },
      'bundles.json': { url: 'bundles/bundles.json', size: index.length, sha256: sha256(index) },
    },
    bundles: { index: 'bundles/bundles.json', count: 1, sha256: sha256(index) },
  }));
  return { assets, lock, latex, release: releaseDir, moduleHash: sha256(module), fontHash: sha256(font) };
}

test('Mirror variables map safely and canonical overrides win', () => {
  assert.deepEqual(mappedEnvironment({ LIBREPAPER_MIRROR_S3_ENDPOINT: 'https://mirror.invalid', LIBREPAPER_MIRROR_S3_REGION: 'bhs',
    LIBREPAPER_MIRROR_S3_ACCESS_KEY_ID: 'publisher', LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY: 'secret', LIBREPAPER_MIRROR_S3_BUCKET: 'asset-bucket' }), {
    LIBREPAPER_MIRROR_S3_ENDPOINT: 'https://mirror.invalid', LIBREPAPER_MIRROR_S3_REGION: 'bhs', LIBREPAPER_MIRROR_S3_ACCESS_KEY_ID: 'publisher', LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY: 'secret',
    LIBREPAPER_MIRROR_S3_BUCKET: 'asset-bucket', S3_ENDPOINT: 'https://mirror.invalid', S3_REGION: 'bhs',
    S3_BUCKET: 'asset-bucket', AWS_ACCESS_KEY_ID: 'publisher', AWS_SECRET_ACCESS_KEY: 'secret',
  });
  assert.equal(mappedEnvironment({ S3_ENDPOINT: 'https://override.invalid', S3_REGION: 'override', S3_BUCKET: 'chosen',
    AWS_ACCESS_KEY_ID: 'id', AWS_SECRET_ACCESS_KEY: 'secret', LIBREPAPER_MIRROR_S3_ENDPOINT: 'https://ignored.invalid',
    LIBREPAPER_MIRROR_S3_BUCKET: 'ignored-bucket' }).S3_BUCKET, 'chosen');
});

test('null SOPS placeholders are absent and canonical overrides precede effective-value validation', () => {
  const selected = selectedMirrorFields({ LIBREPAPER_MIRROR_S3_ENDPOINT: null, LIBREPAPER_MIRROR_S3_BUCKET: null, LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY: 17 });
  assert.deepEqual(selected, { LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY: 17 });
  assert.equal(mappedEnvironment({ ...selected, S3_ENDPOINT: 'https://canonical.invalid', S3_BUCKET: 'canonical-bucket',
    S3_REGION: 'region', AWS_ACCESS_KEY_ID: 'id', AWS_SECRET_ACCESS_KEY: 'secret' }).S3_BUCKET, 'canonical-bucket');
  assert.throws(() => mappedEnvironment({ ...selected, LIBREPAPER_MIRROR_S3_ENDPOINT: 17 }), /S3_ENDPOINT must be a string/);
});

test('Mirror bucket name must be a valid S3 bucket name', () => {
  assert.throws(() => mappedEnvironment({ LIBREPAPER_MIRROR_S3_BUCKET: 'invalid/bucket' }), /valid bucket name/);
  assert.throws(() => mappedEnvironment({ LIBREPAPER_MIRROR_S3_BUCKET: 'x' }), /valid bucket name/);
  assert.throws(() => mappedEnvironment({ LIBREPAPER_MIRROR_S3_BUCKET: 'Upper-Case' }), /valid bucket name/);
  assert.doesNotThrow(() => mappedEnvironment({ LIBREPAPER_MIRROR_S3_BUCKET: 'a-valid-bucket-123' }));
});

test('complete canonical publisher settings skip SOPS while incomplete settings retain fallback mapping', async () => {
  let loads = 0;
  const canonical = {
    S3_ENDPOINT: 'https://canonical.invalid', S3_REGION: 'canonical-region', S3_BUCKET: 'canonical-bucket',
    AWS_ACCESS_KEY_ID: 'canonical-id', AWS_SECRET_ACCESS_KEY: 'canonical-secret',
  };
  const resolved = await publisherEnvironment(canonical, async () => {
    loads += 1;
    return {};
  });
  assert.equal(loads, 0);
  assert.equal(resolved.S3_ENDPOINT, 'https://canonical.invalid');
  assert.equal(resolved.S3_BUCKET, 'canonical-bucket');

  const partial = await publisherEnvironment({ S3_ENDPOINT: 'https://override.invalid' }, async () => {
    loads += 1;
    return {
      LIBREPAPER_MIRROR_S3_ENDPOINT: 'https://from-sops.invalid', LIBREPAPER_MIRROR_S3_REGION: 'region-from-sops',
      LIBREPAPER_MIRROR_S3_BUCKET: 'sops-bucket', LIBREPAPER_MIRROR_S3_ACCESS_KEY_ID: 'sops-id', LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY: 'sops-secret',
    };
  });
  assert.equal(loads, 1);
  assert.equal(partial.S3_ENDPOINT, 'https://override.invalid');
  assert.equal(partial.S3_REGION, 'region-from-sops');
  assert.equal(partial.S3_BUCKET, 'sops-bucket');
  assert.equal(partial.AWS_ACCESS_KEY_ID, 'sops-id');
});

test('staging refuses a file whose bytes do not match assets.lock', async () => {
  const root = await mkdtemp(join(tmpdir(), 'push-mirrors-stage-'));
  const paths = await makeMirrors(root);
  try {
    const staged = await stagePinnedMirrors(paths.assets, paths.lock);
    try {
      assert.deepEqual(staged.mirrors.map(({ prefix }) => prefix).sort(), ['fonts', 'wasm']);
      const wasm = staged.mirrors.find(({ prefix }) => prefix === 'wasm');
      const fonts = staged.mirrors.find(({ prefix }) => prefix === 'fonts');
      assert.equal(await readFile(join(wasm.dir, paths.moduleHash, 'markdown.wasm'), 'utf8'), 'valid markdown wasm');
      assert.equal(await readFile(join(fonts.dir, paths.fontHash, 'fira-code.woff2'), 'utf8'), 'valid font');
      assert.equal((await preflightMirror(fonts)).count, 14);
    } finally { await staged.cleanup(); }
    await writeFile(join(paths.assets, 'wasm', 'markdown.wasm'), 'tampered');
    await assert.rejects(stagePinnedMirrors(paths.assets, paths.lock), /does not match assets\.lock/);
    await rm(join(paths.assets, 'wasm', 'markdown.wasm'));
    await assert.rejects(stagePinnedMirrors(paths.assets, paths.lock), /run node tools\/assets\/pins\.mjs fetch first/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('staging refuses a font that is missing or tampered', async () => {
  const root = await mkdtemp(join(tmpdir(), 'push-mirrors-stage-fonts-'));
  const paths = await makeMirrors(root);
  try {
    await writeFile(join(paths.assets, 'fonts', 'fira-code.woff2'), 'tampered');
    await assert.rejects(stagePinnedMirrors(paths.assets, paths.lock), /fira-code\.woff2 .* does not match assets\.lock/);
    await rm(join(paths.assets, 'fonts', 'fira-code.woff2'));
    await assert.rejects(stagePinnedMirrors(paths.assets, paths.lock), /fira-code\.woff2 not found/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('font preflight rejects unknown names, strays and hash mismatches', async () => {
  const root = await mkdtemp(join(tmpdir(), 'publish-mirror-fonts-layout-'));
  const bytes = Buffer.from('font bytes');
  const hash = sha256(bytes);
  await mkdir(join(root, hash));
  await writeFile(join(root, hash, 'fira-code.woff2'), bytes);
  try {
    assert.equal((await preflightMirror({ dir: root, prefix: 'fonts' })).count, 1);
    await writeFile(join(root, hash, 'unknown.woff2'), bytes);
    await assert.rejects(preflightMirror({ dir: root, prefix: 'fonts' }), /must be <sha256>\/<pinned file>: .*unknown\.woff2/);
    await rm(join(root, hash, 'unknown.woff2'));
    await assert.rejects(preflightMirror({ dir: root, prefix: 'wasm' }), /must be <sha256>\/<pinned file>/);
    await writeFile(join(root, hash, 'fira-code.woff2'), 'other bytes');
    await assert.rejects(preflightMirror({ dir: root, prefix: 'fonts' }), /hash directory does not match/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('font keys are served as immutable uncompressed woff2', () => {
  assert.deepEqual(metadataFor('fonts/' + 'a'.repeat(64) + '/fira-code.woff2'), {
    contentType: 'font/woff2', contentEncoding: undefined, cacheControl: 'public, max-age=31536000, immutable',
  });
});

// === Additional push-mirrors tests (from original push-mirrors.test.mjs) ===

test("dry run stages the pinned mirrors, hashes both mirrors and skips SOPS, even without credentials", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-dryrun-"));
  const bin = join(root, "bin");
  const paths = await makeMirrors(root, "assets 'dir'", "latex mirror");
  await mkdir(bin);
  const sops = join(bin, "sops");
  await writeFile(sops, `#!${process.execPath}\nrequire('node:fs').writeFileSync(process.env.SOPS_CALLED, 'yes');\n`);
  await chmod(sops, 0o755);
  try {
    const output = execFileSync(process.execPath, ['tools/assets/mirror.mjs', 'publish'], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, MIRRORS_DRY_RUN: "1", ASSETS_DIR: paths.assets, ASSETS_LOCK: paths.lock, MIRROR: paths.latex,
      SOPS_CALLED: join(root, "sops-called") } });
    assert.match(output, /Validated wasm: 6 files/);
    assert.match(output, /Validated fonts: 14 files/);
    assert.match(output, /Validated latex: 6 files/);
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
    try { execFileSync(process.execPath, ['tools/assets/mirror.mjs', 'publish'], { encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}` } }); }
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
  await writeFile(sops, `#!${process.execPath}\nprocess.stdout.write(JSON.stringify({ LIBREPAPER_MIRROR_S3_ENDPOINT:'https://s3.example.invalid', LIBREPAPER_MIRROR_S3_REGION:'bhs', LIBREPAPER_MIRROR_S3_ACCESS_KEY_ID:'id', LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY:'secret', LIBREPAPER_MIRROR_S3_BUCKET:'asset-bucket' }));\n`);
  await writeFile(aws, `#!${process.execPath}\nrequire('node:fs').appendFileSync(process.env.AWS_CALLED, 'call\\n');\n`);
  await chmod(sops, 0o755);
  await chmod(aws, 0o755);
  try {
    let error;
    try { execFileSync(process.execPath, ['tools/assets/mirror.mjs', 'publish'], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, ASSETS_DIR: paths.assets, ASSETS_LOCK: paths.lock, MIRROR: paths.latex, AWS_CALLED: calls } }); }
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
  await writeFile(sops, `#!${process.execPath}\nprocess.stdout.write(JSON.stringify({ LIBREPAPER_MIRROR_S3_ENDPOINT:'https://ovh.example.invalid', LIBREPAPER_MIRROR_S3_REGION:'bhs', LIBREPAPER_MIRROR_S3_ACCESS_KEY_ID:'mapped-id', LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY:'mapped-secret', LIBREPAPER_MIRROR_S3_BUCKET:'mapped-bucket', unrelated:{ignored:true} }));\n`);
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
    execFileSync(process.execPath, ['tools/assets/mirror.mjs', 'publish'], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, ASSETS_DIR: paths.assets, ASSETS_LOCK: paths.lock, MIRROR: paths.latex, AWS_LOG: log, AWS_STORE: store,
      S3_BUCKET: "explicit-bucket", AWS_ACCESS_KEY_ID: "override-id", AWS_SECRET_ACCESS_KEY: "override-secret" } });
    const calls = (await readFile(log, "utf8")).trim().split("\n").map(JSON.parse);
    const puts = calls.filter((call) => call.cmd === "s3 sync");
    assert.ok(calls.every((call) => !call.deleting));
    assert.ok(puts.some((call) => call.key === `wasm/${paths.moduleHash}/markdown.wasm`));
    assert.ok(puts.some((call) => call.key === `fonts/${paths.fontHash}/fira-code.woff2`));
    assert.ok(puts.some((call) => call.key.startsWith("latex/")));
    assert.ok(calls.every((call) => call.endpoint === "https://ovh.example.invalid" && call.region === "bhs" &&
      call.bucket === "explicit-bucket" && call.id === "override-id" && call.secret === "override-secret"));
  } finally { await rm(root, { recursive: true, force: true }); }
});
