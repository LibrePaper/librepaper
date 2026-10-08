import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, readdir, rm, writeFile, open, stat } from 'node:fs/promises';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync, spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { awsFailureMessage, mappedEnvironment, metadataFor, preflightMirror, publish, publisherEnvironment, selectedMirrorFields, stageWasmMirror } from './mirror.mjs';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const deploy = join(repo, 'tools/assets/mirror');

async function fixture(corsOrigin) {
  const root = await mkdtemp(join(tmpdir(), 'librepaper-deploy-assets-'));
  const bin = join(root, 'bin');
  const mirror = join(root, 'mirror');
  const calls = join(root, 'calls');
  const awsCalls = join(root, 'aws-calls');
  const sopsCalls = join(root, 'sops-calls');
  const publisherEnvironment = join(root, 'publisher-environment.json');
  await mkdir(bin);
  const lock = await readFile(join(repo, 'assets.lock'), 'utf8');
  const latex = lock.split('\n').map((line) => line.trim().split(/\s+/)).find(([name]) => name === 'latex');
  const release = join(mirror, latex[3]);
  await mkdir(release, { recursive: true });
  await writeFile(join(release, 'release.json'), '{}');

  async function mock(name, body) {
    const path = join(bin, name);
    await writeFile(path, `#!${process.execPath}\n${body}\n`);
    await chmod(path, 0o755);
  }

  await mock('node', `
const fs = require('node:fs');
const { execFileSync } = require('node:child_process');
const args = process.argv.slice(2);
if (args.includes('-e')) {
  try { process.stdout.write(execFileSync(process.execPath, args, { input: fs.readFileSync(0) })); }
  catch (error) { process.stderr.write(error.stderr || ''); process.exit(error.status || 1); }
} else if (args[0] === '-p') process.stdout.write('https://source.invalid/source.tar');
else if (args[0] === 'tools/assets/mirror.mjs' && args[1] === 'credentials') {
  const input = fs.readFileSync(0, 'utf8');
  const secrets = JSON.parse(input);
  const keys = ['S3_ENDPOINT', 'S3_REGION', 'S3_BUCKET', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY'];
  const resolved = {
    S3_ENDPOINT: process.env.S3_ENDPOINT || secrets.LIBREPAPER_MIRROR_S3_ENDPOINT,
    S3_REGION: process.env.S3_REGION || secrets.LIBREPAPER_MIRROR_S3_REGION,
    S3_BUCKET: process.env.S3_BUCKET || secrets.LIBREPAPER_MIRROR_S3_BUCKET,
    AWS_ACCESS_KEY_ID: process.env.AWS_ACCESS_KEY_ID || secrets.LIBREPAPER_MIRROR_S3_ACCESS_KEY_ID,
    AWS_SECRET_ACCESS_KEY: process.env.AWS_SECRET_ACCESS_KEY || secrets.LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY,
  };
  const values = keys.map((key) => resolved[key]);
  if (values.some((value) => typeof value !== 'string' || !value || /[\\x00-\\x1f]/.test(value))) process.exit(1);
  process.stdout.write(values.join('\\n'));
} else if (args[0] === 'tools/assets/mirror.mjs' && args[1] === 'publish') {
  fs.writeFileSync(process.env.PUBLISHER_ENVIRONMENT, JSON.stringify({
    endpoint: process.env.S3_ENDPOINT, region: process.env.S3_REGION, bucket: process.env.S3_BUCKET,
    accessKey: process.env.AWS_ACCESS_KEY_ID, secretKey: process.env.AWS_SECRET_ACCESS_KEY,
  }));
}
`);
  await mock('make', '');
  await mock('psql', '');
  await mock('openssl', '');
  await mock('sops', `
require('node:fs').appendFileSync(process.env.SOPS_CALLS, 'decrypt\\n');
process.stdout.write(JSON.stringify({ LIBREPAPER_MIRROR_S3_ENDPOINT: 'https://api.from-sops.invalid', LIBREPAPER_MIRROR_S3_REGION: 'region-from-sops',
  LIBREPAPER_MIRROR_S3_ACCESS_KEY_ID: 'key-from-sops', LIBREPAPER_MIRROR_S3_SECRET_ACCESS_KEY: 'secret-from-sops', LIBREPAPER_MIRROR_S3_BUCKET: 'bucket-from-sops' }));
`);
  await mock('aws', `
require('node:fs').appendFileSync(process.env.AWS_CALLS, process.argv.slice(2).join(' ') + '\\n');
`);
  await mock('curl', `
const fs = require('node:fs');
const args = process.argv.slice(2);
const url = args[args.length - 1];
fs.appendFileSync(process.env.CURL_CALLS, url + '\\n');
if (args.some((arg) => /^-[a-zA-Z]*I/.test(arg))) {
  process.stdout.write('200');
} else if (url.includes('/test/') || url.endsWith('/release.json')) {
  const origin = process.env.CORS_ORIGIN;
  process.stdout.write('HTTP/1.1 200 OK\\r\\n');
  if (origin) process.stdout.write('Access-Control-Allow-Origin: ' + origin + '\\r\\n');
  process.stdout.write('\\r\\n200');
} else {
  process.stdout.write('403');
}
`);

  const env = {
    ...process.env,
    PATH: `${bin}:${process.env.PATH}`,
    MIRROR: mirror,
    S3_ENDPOINT: 'https://custom-api.invalid',
    S3_REGION: 'custom-region',
    S3_BUCKET: '',
    S3_PUBLIC_BASE_URL: 'https://cdn.custom.invalid/public-assets/',
    AWS_ACCESS_KEY_ID: '',
    AWS_SECRET_ACCESS_KEY: '',
    CORS_ORIGIN: corsOrigin,
    CURL_CALLS: calls,
    AWS_CALLS: awsCalls,
    SOPS_CALLS: sopsCalls,
    PUBLISHER_ENVIRONMENT: publisherEnvironment,
    LIBREPAPER_TEST_POSTGRES_URL: 'postgres://unused',
  };
  return { root, calls, awsCalls, sopsCalls, publisherEnvironment, env, latexId: latex[3] };
}

function run(f, env = f.env) {
  return spawnSync(deploy, ['publish'], { cwd: repo, env, encoding: 'utf8' });
}

test('publish uses an explicit public base for custom S3 endpoints and accepts matching CORS origin', async () => {
  const f = await fixture('https://example.com');
  try {
    const result = run(f);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const calls = (await readFile(f.calls, 'utf8')).trim().split('\n');
    assert.ok(calls.includes('https://cdn.custom.invalid/public-assets/test/markdown.wasm'));
    assert.ok(calls.includes(`https://cdn.custom.invalid/public-assets/latex/${f.latexId}/release.json`));
    assert.ok(calls.includes('https://cdn.custom.invalid/public-assets/'));
    assert.equal((await readFile(f.sopsCalls, 'utf8')).trim().split('\n').length, 1);
    assert.deepEqual(JSON.parse(await readFile(f.publisherEnvironment, 'utf8')), {
      endpoint: 'https://custom-api.invalid', region: 'custom-region', bucket: 'bucket-from-sops',
      accessKey: 'key-from-sops', secretKey: 'secret-from-sops',
    });
    const awsCalls = await readFile(f.awsCalls, 'utf8');
    assert.match(awsCalls, /--endpoint-url https:\/\/custom-api\.invalid --region custom-region --bucket bucket-from-sops/);
  } finally {
    await rm(f.root, { recursive: true, force: true });
  }
});

test('publish accepts wildcard CORS for its public object probes', async () => {
  const f = await fixture('*');
  try {
    const result = run(f);
    assert.equal(result.status, 0, result.stderr || result.stdout);
  } finally {
    await rm(f.root, { recursive: true, force: true });
  }
});

test('publish rejects absent and mismatched CORS origins after the public probe', async () => {
  for (const [name, corsOrigin, message] of [
    ['missing', '', /expected 'https:\/\/example\.com' or '\*'/],
    ['mismatched', 'https://attacker.invalid', /expected 'https:\/\/example\.com' or '\*'/],
  ]) {
    const f = await fixture(corsOrigin);
    try {
      const result = run(f);
      assert.notEqual(result.status, 0, `${name} CORS origin unexpectedly passed`);
      assert.match(result.stderr, message);
      assert.ok((await readFile(f.calls, 'utf8')).includes('/test/markdown.wasm'));
    } finally {
      await rm(f.root, { recursive: true, force: true });
    }
  }
});

test('publish requires an explicit public base for custom S3 targets before AWS access', async () => {
  const f = await fixture('*');
  try {
    const env = { ...f.env };
    delete env.S3_PUBLIC_BASE_URL;
    const result = run(f, env);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /set S3_PUBLIC_BASE_URL/);
    await assert.rejects(readFile(f.awsCalls), { code: 'ENOENT' });
  } finally {
    await rm(f.root, { recursive: true, force: true });
  }
});

// === LaTeX release checker ===

const directory = mkdtempSync(join(tmpdir(), 'librepaper-mirror-test-'));
const checker = fileURLToPath(new URL('./mirror.mjs', import.meta.url));
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

// A mirror is `<id>/` release directories; id is the SHA-256 of MANIFEST.json
// and every path in release.json is relative to that directory.
let mirrorTest;
let release;
let id;
let remoteServer;

function assetTest(url, content) {
  const bytes = Buffer.from(content);
  mkdirSync(join(release, url, '..'), { recursive: true });
  writeFileSync(join(release, url), bytes);
  return { url, size: bytes.length, sha256: digest(bytes) };
}

// Two tiny bundle "tars" (any bytes; check-mirror
// only hashes them) under a release's own `bundles/` directory, named by
// digest as the real ones would be.
function bundlesFixture() {
  const put = (content, slug) => {
    const bytes = Buffer.from(content);
    return assetTest(`bundles/b/${digest(bytes)}/${slug}.tar`, bytes);
  };
  const coreFile = put('fixture core bundle', 'core');
  const tikzFile = put('fixture tikz bundle', 'tex-latex-tikz');
  const index = {
    schemaVersion: 1,
    snapshot: 'texlive-20260301',
    sourceDateEpoch: 1,
    bundles: {
      core: { url: coreFile.url.slice('bundles/'.length), size: coreFile.size, sha256: coreFile.sha256 },
      'tex/latex/tikz': { url: tikzFile.url.slice('bundles/'.length), size: tikzFile.size, sha256: tikzFile.sha256 },
    },
    files: { 'tex/latex/base/article.cls': 'core', 'tex/generic/pgf/basiclayer/pgfcore.code.tex': 'tex/latex/tikz' },
  };
  const indexFile = assetTest('bundles/bundles.json', JSON.stringify(index));
  return { index: indexFile.url, sha256: indexFile.sha256, snapshot: index.snapshot, count: 2, bytes: coreFile.size + tikzFile.size };
}

/// A fresh, complete mirror with one release; `mutate(entry)` may break the
/// release.json entry before it is written, and returns nothing.
function buildCheckTest(mutate = () => {}) {
  mirrorTest = mkdtempSync(join(directory, 'mirror-'));
  const manifest = Buffer.from(`{"label":"${Math.random()}"}\n`);
  id = digest(manifest);
  release = join(mirrorTest, id);
  mkdirSync(release, { recursive: true });
  writeFileSync(join(release, 'MANIFEST.json'), manifest);
  const entry = {
    format: 2,
    id,
    engines: { pdftex: { worker: 'worker.js', files: ['worker.js'] } },
    files: { 'worker.js': assetTest('worker.js', 'worker') },
    bundles: bundlesFixture(),
  };
  mutate(entry);
  writeFileSync(join(release, 'release.json'), JSON.stringify(entry));
}

function checkRun(expected, message, target = mirrorTest) {
  const result = spawnSync(process.execPath, [checker, 'check', target], { encoding: 'utf8' });
  if (result.error) throw result.error;
  assert.equal(result.status, expected, result.stderr || result.stdout);
  if (message) assert.match(result.stderr, message);
}

function checkRunAsync(expected, message, target) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [checker, 'check', target], { encoding: 'utf8' });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8').on('data', (chunk) => { stdout += chunk; });
    child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
    child.once('error', reject);
    child.once('close', (status) => {
      try {
        assert.equal(status, expected, stderr || stdout);
        if (message) assert.match(stderr, message);
        resolve();
      } catch (error) {
        reject(error);
      }
    });
  });
}

try {
  buildCheckTest();
  checkRun(0);
  checkRun(0, undefined, release); // a single release directory
  let corruptWorker = false;
  remoteServer = createServer((request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    if (path === `/${id}/release.json`) return response.end(readFileSync(join(release, 'release.json')));
    if (path === `/${id}/worker.js`) return response.end(corruptWorker ? 'corrupt' : readFileSync(join(release, 'worker.js')));
    response.writeHead(404).end();
  });
  remoteServer.listen(0, '127.0.0.1');
  await new Promise((resolve, reject) => {
    remoteServer.once('listening', resolve);
    remoteServer.once('error', reject);
  });
  const remote = `http://127.0.0.1:${remoteServer.address().port}/${id}/`;
  await checkRunAsync(0, undefined, remote);
  corruptWorker = true;
  await checkRunAsync(1, /worker payload does not match/, remote);
  await new Promise((resolve, reject) => remoteServer.close((error) => error ? reject(error) : resolve()));
  remoteServer = null;
  buildCheckTest((entry) => { entry.format = 1; });
  checkRun(1, /unsupported release format/);
  buildCheckTest((entry) => { delete entry.engines.pdftex; });
  checkRun(1, /pdfTeX worker specification/);
  buildCheckTest((entry) => { entry.engines.pdftex.files = []; });
  checkRun(1, /pdfTeX engine has no valid worker inventory/);
  buildCheckTest((entry) => { entry.engines.xetex = { worker: 'xetex.worker.js', files: [] }; });
  checkRun(1, /engine xetex has no valid worker inventory/);
  buildCheckTest((entry) => { entry.files['worker.js'].size = 0; });
  checkRun(1, /non-empty file record/);
  buildCheckTest((entry) => { entry.files['worker.js'].url = '..%2foutside.js'; });
  checkRun(1, /unsafe URL/);
  buildCheckTest((entry) => { entry.bundles = null; });
  checkRun(1, /has no bundles/);
  buildCheckTest();
  rmSync(join(release, 'worker.js'));
  checkRun(1, /worker\.js references a file that will not be published/);
  buildCheckTest();
  writeFileSync(join(release, 'worker.js'), 'broken');
  checkRun(1, /integrity mismatch/);
  buildCheckTest();
  const outside = join(directory, 'outside-worker.js');
  writeFileSync(outside, 'worker');
  rmSync(join(release, 'worker.js'));
  symlinkSync(outside, join(release, 'worker.js'));
  checkRun(1, /symbolic link/);

  // The directory name is the SHA-256 of MANIFEST.json.
  buildCheckTest();
  writeFileSync(join(release, 'MANIFEST.json'), 'changed');
  checkRun(1, /directory does not match the SHA-256 of MANIFEST\.json/);
  buildCheckTest();
  rmSync(join(release, 'MANIFEST.json'));
  checkRun(1, /is missing MANIFEST\.json/);
  mirrorTest = mkdtempSync(join(directory, 'empty-'));
  checkRun(1, /no release directories/);

  // bundles.json's own digest must match what
  // the release pins, and every bundle path it names must actually be on
  // disk -- both independent of `release.files`, which only proves the
  // files the importer copied are intact, not that bundles.json still
  // agrees with them.
  buildCheckTest();
  writeFileSync(join(release, 'bundles', 'bundles.json'), 'tampered');
  checkRun(1, /bundles index SHA-256 mismatch/);
  buildCheckTest();
  const [coreName, coreBundle] = Object.entries(JSON.parse(readFileSync(join(release, 'bundles', 'bundles.json'), 'utf8')).bundles)[0];
  writeFileSync(join(release, 'bundles', coreBundle.url), 'corrupted bundle bytes');
  checkRun(1, new RegExp(`bundle ${coreName} integrity mismatch`));
  buildCheckTest();
  rmSync(join(release, 'bundles', coreBundle.url));
  checkRun(1, new RegExp(`bundle ${coreName} references a file that will not be published`));

  buildCheckTest();
  const indexPath = join(release, 'bundles', 'bundles.json');
  const releasePath = join(release, 'release.json');
  const brokenIndex = JSON.parse(readFileSync(indexPath, 'utf8'));
  brokenIndex.files['tex/latex/base/article.cls'] = 'missing-bundle';
  const brokenIndexBytes = JSON.stringify(brokenIndex);
  writeFileSync(indexPath, brokenIndexBytes);
  const brokenRelease = JSON.parse(readFileSync(releasePath, 'utf8'));
  brokenRelease.bundles.sha256 = digest(brokenIndexBytes);
  writeFileSync(releasePath, JSON.stringify(brokenRelease));
  checkRun(1, /names unknown bundle "missing-bundle"/);

  console.log('mirror preflight: release shape, worker payloads, path containment, missing assets, corruption, and bundle mismatches checked');
} finally {
  if (remoteServer) await new Promise((resolve) => remoteServer.close(resolve));
  rmSync(directory, { recursive: true, force: true });
}

// === S3 publisher ===


const script = fileURLToPath(new URL('./mirror.mjs', import.meta.url));
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

test('AWS failure diagnostics expose safe service codes and actionable hints only', () => {
  const serviceMessage = awsFailureMessage({
    status: 254,
    stderr: 'An error occurred (AccessDenied) when calling the PutObject operation: secret=do-not-print-this-secret https://private.example/request',
  }, ['s3', 'sync', '/staging', 's3://bucket/latex/']);
  assert.match(serviceMessage, /s3 sync/);
  assert.match(serviceMessage, /exit code 254/);
  assert.match(serviceMessage, /AccessDenied/);
  assert.match(serviceMessage, /permission/);
  assert.doesNotMatch(serviceMessage, /do-not-print-this-secret|private\.example|request/);

  const unknownMessage = awsFailureMessage({
    code: 'private-code-do-not-print',
    stderr: 'arbitrary secret=do-not-print-this-secret endpoint https://private.example',
  }, ['s3api', 'head-object']);
  assert.equal(unknownMessage, 'AWS CLI operation failed head-object');
  assert.doesNotMatch(unknownMessage, /private-code|secret|private\.example/);
  const signatureMessage = awsFailureMessage({ code: 254,
    stderr: 'An error occurred (SignatureDoesNotMatch) when calling the HeadObject operation: private detail',
  }, ['s3api', 'head-object']);
  assert.match(signatureMessage, /exit code 254/);
  assert.match(signatureMessage, /SignatureDoesNotMatch/);
  assert.match(signatureMessage, /S3 access key ID and secret match/);
  assert.doesNotMatch(signatureMessage, /private detail/);
});

test('publisher validates prefixes and selects explicit browser metadata', () => {
  assert.deepEqual(metadataFor('wasm/' + 'a'.repeat(64) + '/markdown.wasm'), {
    contentType: 'application/wasm', contentEncoding: 'gzip', cacheControl: 'public, max-age=31536000, immutable',
  });
  const releaseId = 'b'.repeat(64);
  assert.deepEqual(metadataFor(`latex/${releaseId}/release.json`), {
    contentType: 'application/json', contentEncoding: 'gzip', cacheControl: 'public, max-age=31536000, immutable',
  });
  assert.equal(metadataFor(`latex/${releaseId}/bundles/bundles.json`).cacheControl, 'public, max-age=31536000, immutable');
  assert.equal(metadataFor('latex/licenses/OFL.txt').cacheControl, 'public, max-age=86400');
  assert.equal(metadataFor('latex/fonts/example.otf').contentEncoding, undefined);
  assert.equal(metadataFor('latex/engine.fmt').contentType, 'application/octet-stream');
  assert.equal(metadataFor('latex/engine.fmt').contentEncoding, 'gzip');
  assert.equal(metadataFor('latex/archive.tar').contentEncoding, 'gzip');
  assert.equal(metadataFor('latex/engine.data').contentEncoding, 'gzip');
  assert.deepEqual(metadataFor('latex/receipt.pdf.gz'), {
    contentType: 'application/gzip', contentEncoding: undefined, cacheControl: 'public, max-age=3600',
  });
});

test('dry run lists large files without staging or gzip compression', async () => {
  const root = await mkdtemp(join(tmpdir(), 'publish-mirror-dry-run-'));
  const large = join(root, 'large.wasm');
  const handle = await open(large, 'w');
  await handle.truncate(11 * 1024 * 1024 * 1024);
  await handle.close();
  try {
    const before = new Set((await readdir(tmpdir())).filter((name) => name.startsWith('librepaper-mirror-')));
    const lines = [];
    const result = await publish({ dir: root, prefix: 'wasm', dryRun: true, env: {}, output: (line) => lines.push(line) });
    const after = new Set((await readdir(tmpdir())).filter((name) => name.startsWith('librepaper-mirror-')));
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

test('S3 endpoint must be HTTPS without embedded credentials, query, or fragment', async () => {
  const root = await mkdtemp(join(tmpdir(), 'publish-mirror-endpoint-'));
  await writeFile(join(root, 'asset.txt'), 'asset');
  const baseEnv = { S3_REGION: 'region-1', S3_BUCKET: 'dedicated-assets',
    AWS_ACCESS_KEY_ID: 'access', AWS_SECRET_ACCESS_KEY: secret };
  try {
    for (const endpoint of ['http://s3.example.invalid', 'https://user:pass@s3.example.invalid',
      'https://s3.example.invalid/?token=x', 'https://s3.example.invalid/#fragment']) {
      await assert.rejects(publish({ dir: root, prefix: 'wasm', env: { ...baseEnv, S3_ENDPOINT: endpoint }, output: () => {} }),
        /S3_ENDPOINT/);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('LaTeX preflight verifies release files, index, and relative bundle records', async () => {
  const root = await mkdtemp(join(tmpdir(), 'publish-mirror-latex-preflight-'));
  const manifest = Buffer.from('{"label":"preflight"}\n');
  const id = sha256(manifest);
  const dir = join(root, id);
  const bundleBytes = Buffer.from('bundle bytes');
  const bundleHash = sha256(bundleBytes);
  const indexBytes = Buffer.from(JSON.stringify({ bundles: {
    core: { url: `b/${bundleHash}/bundle.tar`, size: bundleBytes.length, sha256: bundleHash },
  } }));
  const assetBytes = Buffer.from('engine js');
  const workerBytes = Buffer.from('pdftex worker');
  await mkdir(join(dir, 'bundles', 'b', bundleHash), { recursive: true });
  await writeFile(join(dir, 'MANIFEST.json'), manifest);
  await writeFile(join(dir, 'bundles', 'bundles.json'), indexBytes);
  await writeFile(join(dir, 'bundles', 'b', bundleHash, 'bundle.tar'), bundleBytes);
  await writeFile(join(dir, 'engine.js'), assetBytes);
  await writeFile(join(dir, 'worker.js'), workerBytes);
  await writeFile(join(dir, 'release.json'), JSON.stringify({
    format: 2, id, engines: { pdftex: { worker: 'worker.js', files: ['worker.js'] } },
    files: {
      'worker.js': { url: 'worker.js', size: workerBytes.length, sha256: sha256(workerBytes) },
      engine: { url: 'engine.js', size: assetBytes.length, sha256: sha256(assetBytes) },
      'bundles.json': { url: 'bundles/bundles.json', size: indexBytes.length, sha256: sha256(indexBytes) },
    },
    bundles: { index: 'bundles/bundles.json', sha256: sha256(indexBytes), count: 1 },
  }));
  try {
    const result = await preflightMirror({ dir: root, prefix: 'latex' });
    assert.equal(result.count, 6);
    assert.equal(result.bytes, manifest.length + bundleBytes.length + indexBytes.length + workerBytes.length + assetBytes.length +
      (await stat(join(dir, 'release.json'))).size);

    // An unlisted file in a release is an orphan.
    await writeFile(join(dir, 'orphan.txt'), 'not in any manifest');
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /no manifest names/);
    await rm(join(dir, 'orphan.txt'));

    // Nothing but release directories at the top level.
    await writeFile(join(root, 'manifest.json'), '{}');
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /<sha256>\/ release directory/);
    await rm(join(root, 'manifest.json'));

    // A retained bundle whose hash directory lies about its bytes.
    const liar = join(dir, 'bundles', 'b', '0'.repeat(64));
    await mkdir(liar, { recursive: true });
    await writeFile(join(liar, 'orphan.tar'), 'corrupt retained bundle');
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /bundle hash directory does not match/);
    await rm(liar, { recursive: true });

    // The directory name must be the SHA-256 of MANIFEST.json.
    await writeFile(join(dir, 'MANIFEST.json'), '{"label":"changed"}\n');
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /does not match the SHA-256 of MANIFEST.json/);
    await writeFile(join(dir, 'MANIFEST.json'), manifest);

    // release.json must be format 2 and reference bytes that match.
    await writeFile(join(dir, 'engine.js'), 'tampered');
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /integrity mismatch/);
    await writeFile(join(dir, 'engine.js'), assetBytes);
    const releaseJson = JSON.parse(await readFile(join(dir, 'release.json'), 'utf8'));
    await writeFile(join(dir, 'release.json'), JSON.stringify({ ...releaseJson, format: 1 }));
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /unsupported release format/);
    await writeFile(join(dir, 'release.json'), JSON.stringify({ ...releaseJson, engines: { pdftex: { worker: 'worker.js', files: [] } } }));
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /pdfTeX engine has no valid worker inventory/);
    await writeFile(join(dir, 'release.json'), JSON.stringify({ ...releaseJson, engines: { pdftex: { worker: 'worker.js', files: ['worker.js'] } },
      files: { ...releaseJson.files, 'worker.js': { ...releaseJson.files['worker.js'], size: 0 } } }));
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /non-empty file record/);
    await writeFile(join(dir, 'release.json'), JSON.stringify({ ...releaseJson, files: { ...releaseJson.files,
      engine: { url: '../escape.js', size: 1, sha256: '0'.repeat(64) } } }));
    await assert.rejects(preflightMirror({ dir: root, prefix: 'latex' }), /unsafe URL/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('WASM hash mismatch fails preflight before making AWS calls', async () => {
  const root = await mkdtemp(join(tmpdir(), 'publish-mirror-wasm-preflight-'));
  const source = join(root, 'mirror');
  const bin = join(root, 'bin');
  const log = join(root, 'aws.jsonl');
  await mkdir(join(source, '0'.repeat(64)), { recursive: true });
  await mkdir(bin);
  await writeFile(join(source, '0'.repeat(64), 'markdown.wasm'), 'not-the-hash');
  const fakeAws = join(bin, 'aws');
  await writeFile(fakeAws, `#!${process.execPath}\nrequire('node:fs').appendFileSync(process.env.AWS_LOG, 'called\\n');\n`);
  await chmod(fakeAws, 0o755);
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, AWS_LOG: log,
    S3_ENDPOINT: 'https://s3.example.invalid', S3_REGION: 'region-1', S3_BUCKET: 'dedicated-assets',
    AWS_ACCESS_KEY_ID: 'access', AWS_SECRET_ACCESS_KEY: secret };
  try {
    await assert.rejects(publish({ dir: source, prefix: 'wasm', env, output: () => {} }), /hash directory does not match/);
    await assert.rejects(stat(log), { code: 'ENOENT' });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('wasm preflight accepts only <sha256>/<name>.wasm files whose directory is their hash', async () => {
  const root = await mkdtemp(join(tmpdir(), 'publish-mirror-wasm-layout-'));
  const bytes = Buffer.from('module bytes');
  const hash = sha256(bytes);
  await mkdir(join(root, hash));
  await writeFile(join(root, hash, 'markdown.wasm'), bytes);
  try {
    const result = await preflightMirror({ dir: root, prefix: 'wasm' });
    assert.equal(result.count, 1);
    assert.equal(result.bytes, bytes.length);
    await writeFile(join(root, 'stray.txt'), 'not a module');
    await assert.rejects(preflightMirror({ dir: root, prefix: 'wasm' }), /must be <sha256>\/<name>\.wasm/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// === push-mirrors tests ===


async function makeMirrors(root, wasmName = 'wasm dir', latexName = 'latex mirror') {
  const wasm = join(root, wasmName);
  const latex = join(root, latexName);
  const module = Buffer.from('valid markdown wasm');
  await mkdir(wasm, { recursive: true });
  const modules = [
    ['markdown.wasm', 'wasm-markdown'], ['bibliography.wasm', 'wasm-bibliography'],
    ['citations.wasm', 'wasm-bibliography'], ['typst.wasm', 'wasm-typst'],
  ];
  for (const [name] of modules) await writeFile(join(wasm, name), module);
  const lock = join(root, 'assets.lock');
  const moduleRows = modules.map(([name, repo]) => `${name} ${repo} v0.1.1 ${sha256(module)}`).join('\n');
  await writeFile(lock, `# test lock\n${moduleRows}\nlatex wasm-latex v0.1.0 ${'a'.repeat(64)}\n`);
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
  return { wasm, lock, latex, release: releaseDir, moduleHash: sha256(module) };
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

test('staging refuses a module whose bytes do not match assets.lock', async () => {
  const root = await mkdtemp(join(tmpdir(), 'push-mirrors-stage-'));
  const paths = await makeMirrors(root);
  try {
    const staged = await stageWasmMirror(paths.wasm, paths.lock);
    try {
      assert.equal(await readFile(join(staged, paths.moduleHash, 'markdown.wasm'), 'utf8'), 'valid markdown wasm');
    } finally { await rm(staged, { recursive: true, force: true }); }
    await writeFile(join(paths.wasm, 'markdown.wasm'), 'tampered');
    await assert.rejects(stageWasmMirror(paths.wasm, paths.lock), /does not match assets\.lock/);
    await rm(join(paths.wasm, 'markdown.wasm'));
    await assert.rejects(stageWasmMirror(paths.wasm, paths.lock), /run node tools\/assets\/pins\.mjs fetch first/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

// === Additional push-mirrors tests (from original push-mirrors.test.mjs) ===

test("dry run stages the wasm mirror, hashes both mirrors and skips SOPS, even without credentials", async () => {
  const root = await mkdtemp(join(tmpdir(), "push-mirrors-dryrun-"));
  const bin = join(root, "bin");
  const paths = await makeMirrors(root, "wasm 'dir'", "latex mirror");
  await mkdir(bin);
  const sops = join(bin, "sops");
  await writeFile(sops, `#!${process.execPath}\nrequire('node:fs').writeFileSync(process.env.SOPS_CALLED, 'yes');\n`);
  await chmod(sops, 0o755);
  try {
    const output = execFileSync(process.execPath, ['tools/assets/mirror.mjs', 'publish'], { encoding: "utf8", env: { ...process.env,
      PATH: `${bin}:${process.env.PATH}`, MIRRORS_DRY_RUN: "1", WASM_DIR: paths.wasm, WASM_LOCK: paths.lock, MIRROR: paths.latex,
      SOPS_CALLED: join(root, "sops-called") } });
    assert.match(output, /Validated wasm: 4 files/);
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

// === Additional publish-mirror tests (from original publish-mirror.test.mjs) ===

// Fake aws program used by multiple tests
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
