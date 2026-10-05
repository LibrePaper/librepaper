import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const setup = path.join(repo, 'deploy/setup');
const grafanaGuard = path.join(repo, 'deploy/monitoring/grafana-entrypoint.sh');

function fixture() {
  const root = mkdtempSync(path.join(os.tmpdir(), 'librepaper-setup-test-'));
  const bin = path.join(root, 'bin');
  mkdirSync(bin);
  const docker = path.join(bin, 'docker');
  writeFileSync(docker, '#!/bin/sh\nif [ "$1" = volume ] && [ "$2" = inspect ]; then echo "no such volume" >&2; exit 1; fi\nexit 0\n');
  chmodSync(docker, 0o755);
  const secrets = path.join(root, 'secrets');
  const state = path.join(root, 'state.json');
  const envFile = path.join(root, 'compose.env');
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}` };
  const run = (args, moreEnv = {}, input, selectedSecrets = secrets) => spawnSync('python3', [setup, ...args, '--secrets-dir', selectedSecrets, '--state-file', state, '--env-file', envFile], {
    cwd: path.join(repo, 'deploy'), env: { ...env, ...moreEnv }, input, encoding: 'utf8',
  });
  return { root, bin, docker, secrets, state, envFile, env, run };
}

function setSecret(f, name, value, secretsDir = f.secrets) {
  return f.run(['set-secret', name], {}, `${value}\n`, secretsDir);
}

function seedGithub(f, secretsDir = f.secrets) {
  assert.equal(setSecret(f, 'github_client_id', 'demo-client-id', secretsDir).status, 0);
  assert.equal(setSecret(f, 'github_client_secret', 'demo-client-secret', secretsDir).status, 0);
}

test('setup supports init, filling empty OAuth placeholders, and stable reruns', () => {
  const f = fixture();
  try {
    const first = f.run(['init', '--database', 'local', '--project', 'setup-test']);
    assert.notEqual(first.status, 0);
    assert.match(first.stderr, /github_client_id/);
    const appPasswordPath = path.join(f.secrets, 'database_app_password');
    const appPassword = readFileSync(appPasswordPath, 'utf8');
    const ownerPasswordPath = path.join(f.secrets, 'database_owner_password');
    const ownerPassword = readFileSync(ownerPasswordPath, 'utf8');
    chmodSync(appPasswordPath, 0o644);
    writeFileSync(appPasswordPath, '');
    chmodSync(appPasswordPath, 0o444);
    assert.equal(setSecret(f, 'github_client_id', 'demo-client-id').status, 0);
    assert.equal(setSecret(f, 'github_client_secret', 'demo-client-secret').status, 0);
    const partial = f.run(['init', '--database', 'local', '--project', 'setup-test']);
    assert.notEqual(partial.status, 0);
    assert.match(partial.stderr, /existing database credential files are empty.*restore.*reconcile/);
    assert.equal(readFileSync(appPasswordPath, 'utf8'), '');
    assert.equal(readFileSync(ownerPasswordPath, 'utf8'), ownerPassword);
    chmodSync(appPasswordPath, 0o644);
    writeFileSync(appPasswordPath, appPassword);
    chmodSync(appPasswordPath, 0o444);
    const initialized = f.run(['init', '--database', 'local', '--project', 'setup-test', '--local-build', '--production-overlay', '--version', 'local-0123456789ab']);
    assert.equal(initialized.status, 0, initialized.stderr || initialized.stdout);
    const originalPassword = readFileSync(path.join(f.secrets, 'database_app_password'), 'utf8');
    const rerun = f.run(['init', '--database', 'local', '--project', 'setup-test']);
    assert.equal(rerun.status, 0, rerun.stderr || rerun.stdout);
    assert.equal(readFileSync(path.join(f.secrets, 'database_app_password'), 'utf8'), originalPassword);
    const selection = readFileSync(f.envFile, 'utf8');
    assert.match(selection, /LIBREPAPER_VERSION=local-0123456789ab/);
    assert.match(selection, /COMPOSE_FILE=compose\.yaml:compose\.local-binary\.yaml:compose\.production\.yaml/);
    assert.equal(statSync(f.secrets).mode & 0o777, 0o700);
    assert.equal(statSync(path.join(f.secrets, 'database_app_password')).mode & 0o777, 0o444);
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('set-secret uses the persisted custom secrets directory when no directory flag is supplied', () => {
  const f = fixture();
  const customSecrets = path.join(f.root, 'custom-secrets');
  try {
    seedGithub(f, customSecrets);
    const initialized = f.run(['init', '--database', 'local', '--project', 'custom-secrets-test'], {}, undefined, customSecrets);
    assert.equal(initialized.status, 0, initialized.stderr || initialized.stdout);
    const result = spawnSync('python3', [setup, 'set-secret', 'google_client_id', '--state-file', f.state, '--env-file', f.envFile], {
      cwd: path.join(repo, 'deploy'), env: f.env, input: 'google-client-id\n', encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(readFileSync(path.join(customSecrets, 'google_client_id'), 'utf8').trim(), 'google-client-id');
    assert.equal(existsSync(path.join(f.secrets, 'google_client_id')), false);
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('monitoring initialization rejects missing or empty Grafana password and accepts a supplied value', () => {
  for (const grafanaPassword of ['', '   ', 'dashboard-test-password']) {
    const f = fixture();
    try {
      seedGithub(f);
      const result = f.run(['init', '--database', 'local', '--monitoring'], { GRAFANA_ADMIN_PASSWORD: grafanaPassword });
      if (grafanaPassword.trim()) {
        assert.equal(result.status, 0, result.stderr || result.stdout);
        assert.equal(readFileSync(path.join(f.secrets, 'grafana_admin_password'), 'utf8').trim(), grafanaPassword);
        assert.doesNotMatch(`${result.stdout}${result.stderr}`, /dashboard-test-password/);
      } else {
        assert.notEqual(result.status, 0);
        assert.match(result.stderr, /grafana_admin_password|GRAFANA_ADMIN_PASSWORD/);
      }
    } finally {
      rmSync(f.root, { recursive: true, force: true });
    }
  }
});

test('external database setup does not require a metrics URL while monitoring is disabled', () => {
  const f = fixture();
  try {
    seedGithub(f);
    for (const [name, url] of [
      ['database_owner_url', 'postgresql://owner:owner-pass@db.example:5432/paper?sslmode=verify-full'],
      ['database_app_url', 'postgresql://app:app-pass@db.example:5432/paper?sslmode=verify-full'],
      ['database_backup_url', 'postgresql://backup:backup-pass@db.example:5432/paper?sslmode=verify-full'],
    ]) {
      assert.equal(setSecret(f, name, url).status, 0);
    }
    const result = f.run(['init', '--database', 'external', '--project', 'external-test']);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(readFileSync(path.join(f.secrets, 'database_metrics_url'), 'utf8').trim(), '');
    assert.equal(readFileSync(path.join(f.secrets, 'database_metrics_password'), 'utf8').trim(), '');
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('legacy upgrade moves old Compose credentials into files and removes them from the selection env', () => {
  const f = fixture();
  try {
    writeFileSync(f.docker, '#!/bin/sh\ncase "$*" in *" ps --status running --services "*) printf "postgres\\n";; esac\nexit 0\n');
    chmodSync(f.docker, 0o755);
    writeFileSync(f.envFile, [
      'COMPOSE_FILE=compose.yaml:compose.override.yaml',
      'COMPOSE_PROJECT_NAME=legacy-test',
      'LIBREPAPER_VERSION=v0.0.21',
      'LIBREPAPER_GITHUB_CLIENT_ID=old-client-id',
      'LIBREPAPER_GITHUB_CLIENT_SECRET=old-client-secret',
      'LIBREPAPER_GOOGLE_CLIENT_ID=old-google-id',
      'LIBREPAPER_GOOGLE_CLIENT_SECRET=old-google-secret',
      'GRAFANA_ADMIN_PASSWORD=old-grafana-password',
      'POSTGRES_USER=librepaper',
      'POSTGRES_PASSWORD=old-bootstrap-password',
      '',
    ].join('\n'));
    const result = f.run(['upgrade', '--yes', '--project', 'legacy-test', '--version', 'v0.0.21', '--production-overlay']);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /old-client|old-google|old-grafana|old-bootstrap/);
    assert.equal(readFileSync(path.join(f.secrets, 'github_client_id'), 'utf8').trim(), 'old-client-id');
    assert.equal(readFileSync(path.join(f.secrets, 'github_client_secret'), 'utf8').trim(), 'old-client-secret');
    assert.equal(readFileSync(path.join(f.secrets, 'google_client_id'), 'utf8').trim(), 'old-google-id');
    assert.equal(readFileSync(path.join(f.secrets, 'google_client_secret'), 'utf8').trim(), 'old-google-secret');
    assert.equal(readFileSync(path.join(f.secrets, 'grafana_admin_password'), 'utf8').trim(), 'old-grafana-password');
    const selection = readFileSync(f.envFile, 'utf8');
    assert.match(selection, /COMPOSE_FILE=compose\.yaml:compose\.monitoring\.yaml:compose\.production\.yaml/);
    assert.doesNotMatch(selection, /LIBREPAPER_GITHUB_CLIENT_SECRET|GRAFANA_ADMIN_PASSWORD|POSTGRES_PASSWORD/);
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('config validation checks only the selected provider file references', () => {
  const f = fixture();
  const config = path.join(f.root, 'google-only.toml');
  try {
    writeFileSync(config, '[server]\nmigrate = false\n[storage]\ndatabase_url = { file = "/run/secrets/database_url" }\n[auth.google]\nclient_id = { file = "/run/secrets/google_client_id" }\nclient_secret = { file = "/run/secrets/google_client_secret" }\n');
    writeFileSync(f.envFile, `LIBREPAPER_CONFIG_FILE=${config}\n`);
    assert.equal(setSecret(f, 'google_client_id', 'google-client-id').status, 0);
    assert.equal(setSecret(f, 'google_client_secret', 'google-client-secret').status, 0);
    const result = f.run(['init', '--database', 'local', '--project', 'google-test']);
    assert.equal(result.status, 0, result.stderr || result.stdout);
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('Google placeholders remain empty when the selected config omits the provider', () => {
  const f = fixture();
  try {
    seedGithub(f);
    const config = path.join(repo, 'deploy/librepaper.toml');
    const result = f.run(['init', '--database', 'local', '--project', 'google-omitted-test', '--config-file', config]);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(readFileSync(path.join(f.secrets, 'google_client_id'), 'utf8').trim(), '');
    assert.equal(readFileSync(path.join(f.secrets, 'google_client_secret'), 'utf8').trim(), '');
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('config validation rejects empty credentials referenced by an enabled Google provider', () => {
  const f = fixture();
  const config = path.join(f.root, 'google-enabled.toml');
  try {
    seedGithub(f);
    writeFileSync(config, '[server]\nmigrate = false\n[storage]\ndatabase_url = { file = "/run/secrets/database_url" }\n[auth.github]\nclient_id = { file = "/run/secrets/github_client_id" }\nclient_secret = { file = "/run/secrets/github_client_secret" }\n[auth.google]\nclient_id = { file = "/run/secrets/google_client_id" }\nclient_secret = { file = "/run/secrets/google_client_secret" }\n');
    const result = f.run(['init', '--database', 'local', '--project', 'google-enabled-test', '--config-file', config]);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /google\.client_id requires a nonempty secret file/);
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('Grafana wrapper refuses an empty password file and execs upstream only for nonempty input', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'librepaper-grafana-test-'));
  try {
    const secret = path.join(root, 'password');
    const marker = path.join(root, 'ran');
    const upstream = path.join(root, 'upstream.sh');
    writeFileSync(upstream, `#!/bin/sh\nprintf ran > '${marker}'\n`);
    chmodSync(upstream, 0o755);
    writeFileSync(secret, '  \n');
    const run = () => spawnSync('sh', [grafanaGuard], {
      env: { ...process.env, GF_SECURITY_ADMIN_PASSWORD__FILE: secret, LIBREPAPER_GRAFANA_RUN_SCRIPT: upstream },
      encoding: 'utf8',
    });
    const empty = run();
    assert.notEqual(empty.status, 0);
    assert.match(empty.stderr, /missing or empty/);
    assert.equal(existsSync(marker), false);
    writeFileSync(secret, 'test-secret\n');
    const valid = run();
    assert.equal(valid.status, 0, valid.stderr || valid.stdout);
    assert.equal(readFileSync(marker, 'utf8'), 'ran');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
