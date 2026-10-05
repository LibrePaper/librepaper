import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

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
else if (args[0] === 'tools/assets/push-mirrors.mjs') {
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
process.stdout.write(JSON.stringify({ OVH_S3_ENDPOINT: 'https://api.from-sops.invalid', OVH_S3_REGION: 'region-from-sops',
  OVH_S3_USER: 'key-from-sops', OVH_S3_SECRET: 'secret-from-sops', OVH_S3_ARN: 'arn:aws:s3:::bucket-from-sops' }));
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
