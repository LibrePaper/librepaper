import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test, { after } from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'deploy');
const composeAvailable = spawnSync('docker', ['compose', 'version'], { encoding: 'utf8' }).status === 0;
const skip = !composeAvailable && 'Docker Compose is unavailable';

const coreServices = ['backup', 'caddy', 'librepaper', 'postgres'];
const monitoringServices = ['grafana', 'node-exporter', 'postgres-exporter', 'prometheus'];
const grafanaSecret = 'grafana-admin-password';
const imagePattern = /^ghcr\.io\/librepaper\/librepaper(-backup)?:v\d+\.\d+\.\d+$/;

// Operator state and image build inputs never belong to a config check.
const notCopied = new Set([
  '.env', 'compose.override.yaml', 'site', 'local.d', 'Dockerfile', 'backup', 'grafana_admin_password',
]);

const env = { ...process.env, COMPOSE_DISABLE_ENV_FILE: '1' };
for (const name of ['COMPOSE_PROFILES', 'COMPOSE_FILE', 'COMPOSE_PROJECT_NAME', 'COMPOSE_ENV_FILES']) {
  delete env[name];
}

let project;
// A copy of the kit in which the one Compose secret file exists and is empty,
// so that `docker compose config` resolves it without touching the checkout.
function projectDir() {
  if (project) return project;
  project = mkdtempSync(path.join(os.tmpdir(), 'librepaper-compose-test-'));
  cpSync(deploy, project, { recursive: true, filter: (source) => !notCopied.has(path.basename(source)) });
  mkdirSync(path.join(project, 'monitoring'), { recursive: true });
  writeFileSync(path.join(project, 'monitoring', 'grafana_admin_password'), '');
  return project;
}

after(() => {
  if (project) rmSync(project, { recursive: true, force: true });
});

function composeConfig({ managed = false, monitoring = false } = {}) {
  const dir = projectDir();
  const files = ['compose.yaml', ...(managed ? ['compose.managed-db.yaml'] : [])];
  const result = spawnSync('docker', [
    'compose', '--project-directory', dir,
    ...files.flatMap((file) => ['-f', path.join(dir, file)]),
    ...(monitoring ? ['--profile', 'monitoring'] : []),
    'config', '--format', 'json',
  ], { cwd: dir, env, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return JSON.parse(result.stdout);
}

const names = (object) => Object.keys(object ?? {}).sort();
const isEmpty = (value) => value === undefined || value === null || names(value).length === 0;
const inMonitoring = (service) => (service.profiles ?? []).includes('monitoring');

function assertNoSecretsOutsideMonitoring(config) {
  for (const [name, service] of Object.entries(config.services)) {
    if (inMonitoring(service)) continue;
    assert.ok(isEmpty(service.secrets), `${name} is outside the monitoring profile and must not use a secret`);
  }
  for (const secret of names(config.secrets)) {
    assert.equal(secret, grafanaSecret, `unexpected top-level secret ${secret}`);
  }
}

test('the kit resolves with no profile and starts only the four core services', { skip }, () => {
  const config = composeConfig();
  assert.deepEqual(names(config.services), coreServices);
  assert.equal(config.name, 'librepaper');
  assert.equal(config.services.migrate, undefined, 'the server migrates at startup; there is no migrate service');
  const { postgres } = config.services;
  assert.equal(postgres.network_mode, 'none');
  assert.ok(isEmpty(postgres.networks), 'postgres has no networks');
  assertNoSecretsOutsideMonitoring(config);
});

test('the monitoring profile adds its four services and the one Grafana secret', { skip }, () => {
  const config = composeConfig({ monitoring: true });
  assert.deepEqual(names(config.services), [...coreServices, ...monitoringServices].sort());
  for (const name of monitoringServices) {
    assert.ok(inMonitoring(config.services[name]), `${name} belongs to the monitoring profile`);
  }
  assert.deepEqual(names(config.secrets), [grafanaSecret]);
  assert.ok(
    (config.services.grafana.secrets ?? []).some((secret) => secret.source === grafanaSecret),
    'grafana reads the admin password from the Compose secret',
  );
  assertNoSecretsOutsideMonitoring(config);
  assert.equal(config.services.postgres.network_mode, 'none');
});

test('both published image tags are literals, equal and versioned', { skip }, () => {
  const { services } = composeConfig();
  const images = [services.librepaper.image, services.backup.image];
  for (const image of images) {
    assert.match(image, imagePattern);
  }
  const [appTag, backupTag] = images.map((image) => image.split(':').pop());
  assert.equal(appTag, backupTag, 'the app and backup images carry the same tag');
});

test('compose.managed-db.yaml removes the local database and the start-up dependency on it', { skip }, () => {
  const config = composeConfig({ managed: true });
  assert.equal(config.services.postgres, undefined, 'no postgres service');
  assert.deepEqual(names(config.services), ['backup', 'caddy', 'librepaper']);
  for (const name of ['librepaper', 'backup']) {
    assert.ok(isEmpty(config.services[name].depends_on), `${name} has no depends_on`);
  }
  assertNoSecretsOutsideMonitoring(config);
});

test('compose.managed-db.yaml also removes the database exporter when monitoring is on', { skip }, () => {
  const config = composeConfig({ managed: true, monitoring: true });
  assert.equal(config.services.postgres, undefined);
  assert.equal(config.services['postgres-exporter'], undefined, 'the exporter has no socket to read');
  assert.deepEqual(names(config.secrets), [grafanaSecret]);
});
