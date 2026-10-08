import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test, { after } from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'deploy');
const composeAvailable = spawnSync('docker', ['compose', 'version'], { encoding: 'utf8' }).status === 0;
const skip = !composeAvailable && 'Docker Compose is unavailable';

const coreServices = ['backup', 'caddy', 'librepaper', 'postgres'];
const imagePattern = /^ghcr\.io\/librepaper\/librepaper(-backup)?:v\d+\.\d+\.\d+$/;

// Operator state and image build inputs never belong to a config check.
const notCopied = new Set([
  'compose.override.yaml', 'site', 'local.d', 'Dockerfile', 'backup',
]);

const env = { ...process.env, COMPOSE_DISABLE_ENV_FILE: '1' };
for (const name of ['COMPOSE_PROFILES', 'COMPOSE_FILE', 'COMPOSE_PROJECT_NAME', 'COMPOSE_ENV_FILES']) {
  delete env[name];
}

// tools/release/version sets both; a release must not ship a kit that pins another version.
test('the kit pins the application and backup images to the workspace version', () => {
  const version = readFileSync(path.join(repo, 'Cargo.toml'), 'utf8').match(/^version = "([^"]+)"/m)[1];
  const pins = [...readFileSync(path.join(deploy, 'compose.yaml'), 'utf8').matchAll(/ghcr\.io\/librepaper\/librepaper(?:-backup)?:(v\S+)/g)].map((match) => match[1]);
  assert.equal(pins.length, 2);
  assert.deepEqual(pins, [`v${version}`, `v${version}`], 'run tools/release/version to update the kit pins');
});

// docs/host/simple.md shows the kit's librepaper.toml in full; keep the copy exact.
test('the simple deployment page shows the kit librepaper.toml as shipped', () => {
  const page = readFileSync(path.join(repo, 'docs/host/simple.md'), 'utf8');
  const shown = page.match(/### librepaper\.toml\n[\s\S]*?```toml\n([\s\S]*?)\n```/)[1];
  assert.equal(`${shown}\n`, readFileSync(path.join(deploy, 'librepaper.toml'), 'utf8'), 'copy deploy/librepaper.toml into docs/host/simple.md');
});

let project;
// A copy of the kit in which files exist as in the repo.
function projectDir() {
  if (project) return project;
  project = mkdtempSync(path.join(os.tmpdir(), 'librepaper-compose-test-'));
  cpSync(deploy, project, { recursive: true, filter: (source) => !notCopied.has(path.basename(source)) });
  return project;
}

after(() => {
  if (project) rmSync(project, { recursive: true, force: true });
});

function composeConfig({ managed = false } = {}) {
  const dir = projectDir();
  const files = ['compose.yaml', ...(managed ? ['compose.managed-db.yaml'] : [])];
  const result = spawnSync('docker', [
    'compose', '--project-directory', dir,
    ...files.flatMap((file) => ['-f', path.join(dir, file)]),
    'config', '--format', 'json',
  ], { cwd: dir, env, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return JSON.parse(result.stdout);
}

const names = (object) => Object.keys(object ?? {}).sort();
const isEmpty = (value) => value === undefined || value === null || names(value).length === 0;

function assertNoSecrets(config) {
  for (const [name, service] of Object.entries(config.services)) {
    assert.ok(isEmpty(service.secrets), `${name} must not use a secret`);
  }
  assert.ok(isEmpty(config.secrets), 'no top-level secrets are expected');
}

test('the kit resolves and starts only the four core services', { skip }, () => {
  const config = composeConfig();
  assert.deepEqual(names(config.services), coreServices);
  assert.equal(config.name, 'librepaper');
  const { postgres } = config.services;
  assert.equal(postgres.network_mode, 'none');
  assert.ok(isEmpty(postgres.networks), 'postgres has no networks');
  assertNoSecrets(config);
  // librepaper networks are exactly edge and default
  const librepaper = config.services.librepaper;
  assert.deepEqual(names(librepaper.networks), ['default', 'edge']);
  // No ports on librepaper
  assert.ok(isEmpty(librepaper.ports), 'librepaper has no ports; Caddy is the reverse proxy');
  assert.deepEqual(names(config.networks), ['default', 'edge']);
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
  assertNoSecrets(config);
});
