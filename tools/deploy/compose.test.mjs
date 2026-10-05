import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'deploy');
const docker = spawnSync('docker', ['compose', 'version'], { encoding: 'utf8' });
const composeAvailable = docker.status === 0;
const sources = [
  'postgres_bootstrap_password', 'database_owner_password', 'database_app_password',
  'database_backup_password', 'database_metrics_password', 'database_owner_url',
  'database_app_url', 'database_backup_url', 'database_metrics_url', 'github_client_id',
  'github_client_secret', 'google_client_id', 'google_client_secret', 'restic_password',
  'aws_credentials', 'restic_known_hosts', 'grafana_admin_password', 'database_metrics_uri',
  'database_metrics_user',
];

function withComposeConfig(files, run, { format = false, extraEnv = {}, fromEnv = false } = {}) {
  const temp = mkdtempSync(path.join(os.tmpdir(), 'librepaper-compose-test-'));
  const secrets = path.join(temp, 'secrets');
  mkdirSync(secrets, { mode: 0o700 });
  for (const name of sources) {
    writeFileSync(path.join(secrets, name), `${name}-test\n`, { mode: 0o600 });
    chmodSync(path.join(secrets, name), 0o444);
  }
  const envFile = path.join(temp, 'empty.env');
  writeFileSync(envFile, '');
  try {
    const env = {
      ...process.env,
      COMPOSE_DISABLE_ENV_FILE: '1',
      LIBREPAPER_SECRETS_DIR: secrets,
      ...(fromEnv ? { COMPOSE_FILE: files.join(':') } : {}),
      ...extraEnv,
    };
    delete env.GRAFANA_ADMIN_PASSWORD;
    run(spawnSync('docker', [
      'compose', '--project-directory', deploy, '--env-file', envFile,
      ...(fromEnv ? [] : files.flatMap((file) => ['-f', path.join(deploy, file)])),
      ...(format ? ['config', '--format', 'json'] : ['config', '--quiet']),
    ], { cwd: fromEnv ? deploy : repo, env, encoding: 'utf8' }));
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}

test('base Compose config resolves file-backed deployment secrets', {
  skip: !composeAvailable && 'Docker Compose is unavailable',
}, () => withComposeConfig(['compose.yaml'], (result) => {
  assert.equal(result.status, 0, result.stderr || result.stdout);
}));

test('optional monitoring Compose config resolves the explicit Grafana password secret', {
  skip: !composeAvailable && 'Docker Compose is unavailable',
}, () => withComposeConfig(['compose.yaml', 'compose.monitoring.yaml'], (result) => {
  assert.equal(result.status, 0, result.stderr || result.stdout);
}));

test('production local-binary selection builds app, migration, and backup from the staged musl binary', {
  skip: !composeAvailable && 'Docker Compose is unavailable',
}, () => withComposeConfig(['compose.yaml', 'compose.local-binary.yaml', 'compose.production.yaml'], (result) => {
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const config = JSON.parse(result.stdout);
  for (const service of ['librepaper', 'migrate']) {
    assert.equal(config.services[service].image, 'librepaper:local-0123456789ab');
    assert.equal(config.services[service].build.args.SOURCE, 'local');
    assert.equal(config.services[service].build.args.LIBREPAPER_BINARY, 'librepaper.candidate');
  }
  assert.equal(config.services.backup.image, 'librepaper-backup:local-0123456789ab');
  assert.equal(config.services.backup.build.args.LIBREPAPER_BINARY, 'librepaper.candidate');
  assert.ok(config.services.caddy.volumes.some((volume) => volume.target === '/srv/site'));
}, { format: true, fromEnv: true, extraEnv: { LIBREPAPER_VERSION: 'local-0123456789ab' } }));
