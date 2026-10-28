import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, readdir, rm, writeFile, open, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { awsFailureMessage, metadataFor, preflightMirror, publish } from './mirror.mjs';

// === S3 publisher ===


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
    contentType: 'application/wasm', contentEncoding: 'br', cacheControl: 'public, max-age=31536000, immutable',
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
  assert.equal(metadataFor('latex/engine.data').contentEncoding, 'br');
  assert.deepEqual(metadataFor('latex/receipt.pdf.gz'), {
    contentType: 'application/gzip', contentEncoding: undefined, cacheControl: 'public, max-age=3600',
  });
});

test('dry run lists large files without staging or compression', async () => {
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
    assert.ok(lines.some((line) => /application\/wasm\tbr\tpublic, max-age=3600/.test(line)));
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

test('wasm preflight accepts only <sha256>/<pinned file> files whose directory is their hash', async () => {
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
    await assert.rejects(preflightMirror({ dir: root, prefix: 'wasm' }), /must be <sha256>\/<pinned file>/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
