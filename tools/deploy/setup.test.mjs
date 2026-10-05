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
  const dockerLog = path.join(root, 'docker.log');
  const composeConfig = path.join(root, 'compose-config.json');
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, MOCK_DOCKER_LOG: dockerLog, MOCK_COMPOSE_CONFIG: composeConfig };
  const run = (args, moreEnv = {}, input, selectedSecrets = secrets, selectedState = state, selectedEnv = envFile) => spawnSync('python3', [setup, ...args, '--secrets-dir', selectedSecrets, '--state-file', selectedState, '--env-file', selectedEnv], {
    cwd: path.join(repo, 'deploy'), env: { ...env, ...moreEnv }, input, encoding: 'utf8',
  });
  return { root, bin, docker, secrets, state, envFile, dockerLog, composeConfig, env, run };
}

function resolvedComposeConfig(secretsDir, configPath, resticprofilePath, { wrongPostgresSecret = false } = {}) {
  const secretFiles = {
    'postgres-bootstrap-password': 'postgres_bootstrap_password',
    'database-owner-password': 'database_owner_password',
    'database-app-password': 'database_app_password',
    'database-backup-password': 'database_backup_password',
    'database-metrics-password': 'database_metrics_password',
    'database-owner-url': 'database_owner_url',
    'database-app-url': 'database_app_url',
    'database-backup-url': 'database_backup_url',
  };
  const secrets = Object.fromEntries(Object.entries(secretFiles).map(([name, file]) => [
    name, { file: path.join(wrongPostgresSecret && name === 'postgres-bootstrap-password' ? path.join(secretsDir, 'wrong') : secretsDir, file) },
  ]));
  const secretMounts = (names) => names.map((source) => ({ source, target: secretFiles[source] || source }));
  const configMount = { type: 'bind', source: configPath, target: '/etc/librepaper/librepaper.toml' };
  return {
    secrets,
    services: {
      postgres: { secrets: secretMounts(Object.keys(secretFiles).slice(0, 5)) },
      migrate: { secrets: [{ source: 'database-owner-url', target: 'database_url' }], volumes: [configMount] },
      librepaper: { secrets: [{ source: 'database-app-url', target: 'database_url' }], volumes: [configMount] },
      backup: {
        secrets: [{ source: 'database-backup-url', target: 'database_url' }],
        volumes: [configMount, { type: 'bind', source: resticprofilePath, target: '/etc/resticprofile/resticprofile.toml' }],
      },
    },
  };
}

function installUpgradeDockerMock(f, config) {
  writeFileSync(f.composeConfig, JSON.stringify(config));
  writeFileSync(f.docker, [
    '#!/bin/sh',
    'set -eu',
    'printf "ARGS=%s|COMPOSE_PROJECT_NAME=%s|COMPOSE_FILE=%s|SECRETS=%s|CONFIG=%s|RESTIC=%s|VERSION=%s\\n" "$*" "${COMPOSE_PROJECT_NAME:-}" "${COMPOSE_FILE:-}" "${LIBREPAPER_SECRETS_DIR:-}" "${LIBREPAPER_CONFIG_FILE:-}" "${LIBREPAPER_RESTICPROFILE_CONFIG_FILE:-}" "${LIBREPAPER_VERSION:-}" >> "$MOCK_DOCKER_LOG"',
    'case "$*" in',
    '  info) : ;;',
    '  "volume inspect "*) : ;;',
    '  *" config --format json"*) cat "$MOCK_COMPOSE_CONFIG" ;;',
    '  *" ps --status running --services"*) printf "postgres\\n" ;;',
    '  *) : ;;',
    'esac',
    '',
  ].join('\n'), { mode: 0o755 });
  chmodSync(f.docker, 0o755);
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
    assert.match(partial.stderr, /existing database credential files are empty.*restore.*reconcile/i);
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
  const customSecrets = path.join(f.root, 'legacy-secrets');
  const customEnv = path.join(f.root, 'legacy.env');
  const activeConfig = path.join(f.root, 'active-legacy.toml');
  const candidateConfig = path.join(f.root, 'candidate.toml');
  const resticConfig = path.join(f.root, 'custom-resticprofile.toml');
  try {
    writeFileSync(activeConfig, '# legacy config mounted before deployment upgrade\n');
    writeFileSync(resticConfig, '# selected restic profile\n');
    writeFileSync(candidateConfig, '[server]\nmigrate = false\n[storage]\ndatabase_url = { file = "/run/secrets/database_url" }\n[auth.github]\nclient_id = { file = "/run/secrets/github_client_id" }\nclient_secret = { file = "/run/secrets/github_client_secret" }\n');
    writeFileSync(customEnv, [
      'COMPOSE_FILE=compose.yaml:compose.override.yaml',
      'COMPOSE_PROJECT_NAME=legacy-test',
      'LIBREPAPER_VERSION=v0.0.21',
      `LIBREPAPER_CONFIG_FILE=${activeConfig}`,
      `LIBREPAPER_RESTICPROFILE_CONFIG_FILE=${resticConfig}`,
      'LIBREPAPER_GITHUB_CLIENT_ID=old#client-id',
      'LIBREPAPER_GITHUB_CLIENT_SECRET=old-client-secret',
      'LIBREPAPER_GOOGLE_CLIENT_ID=old-google-id',
      'LIBREPAPER_GOOGLE_CLIENT_SECRET=old-google-secret',
      'GRAFANA_ADMIN_PASSWORD=old-grafana password',
      'POSTGRES_USER=librepaper',
      'POSTGRES_PASSWORD=old-bootstrap-password',
      '',
    ].join('\n'));
    installUpgradeDockerMock(f, resolvedComposeConfig(customSecrets, activeConfig, resticConfig));
    const conflictingEnvironment = {
      COMPOSE_PROJECT_NAME: 'wrong-project',
      COMPOSE_FILE: 'wrong-compose.yaml',
      LIBREPAPER_SECRETS_DIR: '/wrong/secrets',
      LIBREPAPER_CONFIG_FILE: '/wrong/config.toml',
      LIBREPAPER_RESTICPROFILE_CONFIG_FILE: '/wrong/restic.toml',
      LIBREPAPER_VERSION: 'bad-inherited-version',
    };
    const result = f.run(['upgrade', '--yes', '--project', 'selected-project', '--version', 'v0.0.22', '--production-overlay', '--config-file', candidateConfig], conflictingEnvironment, undefined, customSecrets, f.state, customEnv);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /old-client|old-google|old-grafana|old-bootstrap/);
    assert.equal(readFileSync(path.join(customSecrets, 'github_client_id'), 'utf8').trim(), 'old#client-id');
    assert.equal(readFileSync(path.join(customSecrets, 'github_client_secret'), 'utf8').trim(), 'old-client-secret');
    assert.equal(readFileSync(path.join(customSecrets, 'google_client_id'), 'utf8').trim(), 'old-google-id');
    assert.equal(readFileSync(path.join(customSecrets, 'google_client_secret'), 'utf8').trim(), 'old-google-secret');
    assert.equal(readFileSync(path.join(customSecrets, 'grafana_admin_password'), 'utf8').trim(), 'old-grafana password');
    const selection = readFileSync(customEnv, 'utf8');
    assert.match(selection, /COMPOSE_FILE=compose\.yaml:compose\.monitoring\.yaml:compose\.production\.yaml/);
    assert.match(selection, /COMPOSE_PROJECT_NAME=selected-project/);
    assert.match(selection, /LIBREPAPER_VERSION=v0\.0\.22/);
    assert.match(selection, new RegExp(`LIBREPAPER_CONFIG_FILE=${activeConfig.replaceAll('/', '\\/')}`));
    assert.doesNotMatch(selection, /LIBREPAPER_GITHUB_CLIENT_SECRET|GRAFANA_ADMIN_PASSWORD|POSTGRES_PASSWORD/);
    const composeCalls = readFileSync(f.dockerLog, 'utf8').split('\n').filter((line) => line.startsWith('ARGS=compose '));
    assert.ok(composeCalls.length >= 7, 'preflight and every lifecycle operation invoke Compose');
    for (const line of composeCalls) {
      assert.match(line, new RegExp(`--env-file ${customEnv.replaceAll('/', '\\/')}`));
      assert.match(line, /--project-name selected-project/);
      assert.match(line, /COMPOSE_PROJECT_NAME=selected-project/);
      assert.match(line, /COMPOSE_FILE=compose\.yaml:compose\.monitoring\.yaml:compose\.production\.yaml/);
      assert.equal(line.includes(`SECRETS=${customSecrets}|`), true);
      assert.equal(line.includes(`CONFIG=${activeConfig}|`), true);
      assert.equal(line.includes(`RESTIC=${resticConfig}|`), true);
      assert.match(line, /VERSION=v0\.0\.22/);
    }
  } finally {
    rmSync(f.root, { recursive: true, force: true });
  }
});

test('legacy secret import rejects ambiguous dotenv syntax before writing or changing the old env file', () => {
  for (const value of ['"quoted-secret"', 'escaped\\secret', 'plain-secret # inline comment', '${SECRET_VALUE}']) {
    const f = fixture();
    try {
      const legacyEnv = [
        'COMPOSE_PROJECT_NAME=legacy-syntax-test',
        'LIBREPAPER_GITHUB_CLIENT_ID=plain#literal',
        `LIBREPAPER_GITHUB_CLIENT_SECRET=${value}`,
        '',
      ].join('\n');
      writeFileSync(f.envFile, legacyEnv);
      installUpgradeDockerMock(f, {});
      const result = f.run(['upgrade', '--yes', '--project', 'legacy-syntax-test']);
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /LIBREPAPER_GITHUB_CLIENT_SECRET.*effective value/);
      assert.doesNotMatch(`${result.stdout}${result.stderr}`, /quoted-secret|escaped\\secret|inline comment|SECRET_VALUE/);
      assert.equal(readFileSync(f.envFile, 'utf8'), legacyEnv);
      assert.equal(existsSync(f.secrets), false, 'a valid earlier key is not partially imported');
      const calls = existsSync(f.dockerLog) ? readFileSync(f.dockerLog, 'utf8') : '';
      assert.doesNotMatch(calls, / stop | exec | up /);
      if (value === '"quoted-secret"') {
        const repaired = setSecret(f, 'github_client_secret', 'effective-secret');
        assert.equal(repaired.status, 0, repaired.stderr || repaired.stdout);
        assert.equal(readFileSync(path.join(f.secrets, 'github_client_secret'), 'utf8').trim(), 'effective-secret');
        assert.equal(readFileSync(f.envFile, 'utf8'), legacyEnv, 'explicit file secret repair leaves the legacy env intact');
      }
    } finally {
      rmSync(f.root, { recursive: true, force: true });
    }
  }
});

test('legacy upgrade rejects invalid version and interpolation values before invoking Docker', () => {
  for (const [key, value, argument] of [
    ['LIBREPAPER_VERSION', 'not-a-release', ['--version', 'not-a-release']],
    ['LIBREPAPER_CONFIG_FILE', './candidate$unsafe.toml', []],
  ]) {
    const f = fixture();
    try {
      installUpgradeDockerMock(f, {});
      writeFileSync(f.envFile, `${key}=${value}\n`);
      const result = f.run(['upgrade', '--yes', ...argument]);
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, key === 'LIBREPAPER_VERSION' ? /LIBREPAPER_VERSION/ : /Compose interpolation/);
      assert.equal(existsSync(f.dockerLog), false, 'invalid setup values fail before any Docker operation');
    } finally {
      rmSync(f.root, { recursive: true, force: true });
    }
  }
});

test('legacy upgrade rejects wrong resolved credential mounts before stop or database exec', () => {
  const f = fixture();
  try {
    writeFileSync(f.envFile, [
      'COMPOSE_PROJECT_NAME=preflight-test',
      'LIBREPAPER_GITHUB_CLIENT_ID=legacy-id',
      'LIBREPAPER_GITHUB_CLIENT_SECRET=legacy-secret',
      '',
    ].join('\n'));
    const selectionBefore = readFileSync(f.envFile, 'utf8');
    installUpgradeDockerMock(f, resolvedComposeConfig(f.secrets, path.join(repo, 'deploy/librepaper.toml'), path.join(repo, 'deploy/resticprofile.toml'), { wrongPostgresSecret: true }));
    const result = f.run(['upgrade', '--yes', '--project', 'preflight-test']);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /wrong or missing credential file/);
    assert.equal(readFileSync(f.envFile, 'utf8'), selectionBefore, 'failed preflight leaves the legacy Compose env file untouched');
    const calls = readFileSync(f.dockerLog, 'utf8');
    assert.match(calls, / config --format json/);
    assert.doesNotMatch(calls, / ps --status| stop | exec | up /);
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
