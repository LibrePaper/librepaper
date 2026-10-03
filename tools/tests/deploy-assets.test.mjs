import assert from 'node:assert/strict';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const deploy = join(repo, 'tools/deploy-assets');

async function fixture(corsOrigin) {
  const root = await mkdtemp(join(tmpdir(), 'librepaper-deploy-assets-'));
  const bin = join(root, 'bin');
  const mirror = join(root, 'mirror');
  const calls = join(root, 'calls');
  const awsCalls = join(root, 'aws-calls');
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
const args = process.argv.slice(2);
if (args[0] === '-p') process.stdout.write('https://source.invalid/source.tar');
`);
  await mock('make', '');
  await mock('sops', `
const arg = process.argv.slice(2).join(' ');
if (arg.includes('OVH_S3_ENDPOINT')) process.stdout.write('https://api.custom.invalid');
else if (arg.includes('OVH_S3_REGION')) process.stdout.write('custom-region');
else if (arg.includes('OVH_S3_USER')) process.stdout.write('test-access-key');
else if (arg.includes('OVH_S3_SECRET')) process.stdout.write('test-secret');
else if (arg.includes('OVH_S3_ARN')) process.stdout.write('arn:aws:s3:::asset-bucket');
else process.exit(2);
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
    S3_PUBLIC_BASE_URL: 'https://cdn.custom.invalid/public-assets/',
    CORS_ORIGIN: corsOrigin,
    CURL_CALLS: calls,
    AWS_CALLS: awsCalls,
    LIBREPAPER_TEST_POSTGRES_URL: 'postgres://unused',
  };
  return { root, calls, awsCalls, env, latexId: latex[3] };
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
