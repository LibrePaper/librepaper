import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'deploy');
const compose = path.join(deploy, 'compose.yaml');
const docker = spawnSync('docker', ['compose', 'version'], { encoding: 'utf8' });
const composeAvailable = docker.status === 0;

function withComposeConfig(run) {
  const temp = mkdtempSync(path.join(os.tmpdir(), 'librepaper-compose-test-'));
  const envFile = path.join(temp, 'empty.env');
  writeFileSync(envFile, '');
  try {
    run((password) => {
      const env = { ...process.env, COMPOSE_DISABLE_ENV_FILE: '1' };
      delete env.GRAFANA_ADMIN_PASSWORD;
      if (password !== undefined) env.GRAFANA_ADMIN_PASSWORD = password;
      return spawnSync('docker', [
        'compose', '--project-directory', deploy, '--env-file', envFile,
        '-f', compose, 'config', '--quiet',
      ], { cwd: repo, env, encoding: 'utf8' });
    });
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}

test('Compose rejects a missing or empty Grafana admin password with guidance', {
  skip: !composeAvailable && 'Docker Compose is unavailable',
}, () => withComposeConfig((config) => {
  for (const password of [undefined, '']) {
    const result = config(password);
    assert.notEqual(result.status, 0, 'Compose accepted a missing or empty password');
    assert.match(`${result.stdout}${result.stderr}`, /GRAFANA_ADMIN_PASSWORD/);
    assert.match(`${result.stdout}${result.stderr}`, /Set GRAFANA_ADMIN_PASSWORD in deploy\/\.env/);
  }
}));

test('Compose accepts a supplied Grafana admin password', {
  skip: !composeAvailable && 'Docker Compose is unavailable',
}, () => withComposeConfig((config) => {
  const result = config('compose-test-secret');
  assert.equal(result.status, 0, result.stderr || result.stdout);
}));
