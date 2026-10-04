import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, chmodSync, statSync, rmSync, existsSync, readdirSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'tools/deploy-production');

function mockCommand(dir, name, contents) {
  const file = path.join(dir, name);
  writeFileSync(file, `#!/bin/sh\n${contents}\n`);
  chmodSync(file, 0o755);
}

function sha384(text) {
  return createHash('sha384').update(text).digest('hex');
}

const defaultMigration = 'CREATE TABLE a (id integer);';

function fixture({
  remoteMigrations = 'absent',
  remoteSchema = 'CREATE TABLE a (id integer);',
  squashSchema = remoteSchema,
  migrations = { '0001_catalog.sql': defaultMigration },
  corruptTransfer = false,
  adminPassword = 'admin-secret',
  exporterPassword = 'exporter-secret',
  grafanaTransientFailures = 0,
  grafanaPersistentStatus = 0,
  grafanaAuthStatus = 200,
  pgUpHealthy = true,
  prometheusTransientFailures = 0,
  realSleep = false,
  googleClientId = 'google-id',
  googleClientSecret = 'google-secret',
  googleClientIdPresent = true,
  googleClientSecretPresent = true,
  googleHttpStatus = '302',
  googleLocation = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=test',
  sopsFailureKey = '',
} = {}) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'librepaper-deploy-test-'));
  const bin = path.join(root, 'bin');
  const remote = path.join(root, 'remote');
  const temp = path.join(root, 'tmp');
  mkdirSync(bin);
  mkdirSync(remote);
  mkdirSync(temp);
  const adminPasswordFile = path.join(root, 'admin-password');
  const exporterPasswordFile = path.join(root, 'exporter-password');
  const grafanaCountFile = path.join(root, 'grafana-count');
  const prometheusCountFile = path.join(root, 'prometheus-count');
  const makeCalledFile = path.join(root, 'make-called');
  const oauthCallsFile = path.join(root, 'oauth-calls');
  const curlCallsFile = path.join(root, 'curl-calls');
  const migrationsDir = path.join(root, 'migrations');
  const remoteMigrationsFile = path.join(root, 'remote-migrations');
  const remoteSchemaFile = path.join(root, 'remote-schema');
  const squashSchemaFile = path.join(root, 'squash-schema');
  const pinSqlFile = path.join(root, 'pin-sql');
  const composeUpFile = path.join(root, 'compose-up');
  const backupFile = path.join(root, 'backup');
  const orderFile = path.join(root, 'order');
  mkdirSync(migrationsDir);
  for (const [name, contents] of Object.entries(migrations)) writeFileSync(path.join(migrationsDir, name), contents);
  writeFileSync(remoteMigrationsFile, `${remoteMigrations}\n`);
  writeFileSync(remoteSchemaFile, `${remoteSchema}\n`);
  writeFileSync(squashSchemaFile, `${squashSchema}\n`);
  writeFileSync(adminPasswordFile, adminPassword);
  writeFileSync(exporterPasswordFile, exporterPassword);

  mockCommand(bin, 'make', 'printf called >> "$MAKE_CALLED_FILE"; exit 0');
  mockCommand(bin, 'dig', 'printf "192.0.2.7\\n"');
  mockCommand(bin, 'sops', `
for arg do
  case "$arg" in
    *PRODUCTION_ACME_EMAIL*) printf 'admin@example.org\\n'; exit ;;
    *PRODUCTION_POSTGRES_PASSWORD*) printf 'pg-secret\\n'; exit ;;
    *PRODUCTION_POSTGRES_EXPORTER_PASSWORD*) cat "$EXPORTER_PASSWORD_FILE"; exit ;;
    *PRODUCTION_GITHUB_CLIENT_ID*) printf 'github-id\\n'; exit ;;
    *PRODUCTION_GITHUB_CLIENT_SECRET*) printf 'github-secret\\n'; exit ;;
    *PRODUCTION_GOOGLE_CLIENT_ID*)
      if [ "$SOPS_FAILURE_KEY" = PRODUCTION_GOOGLE_CLIENT_ID ]; then printf 'private SOPS diagnostic\\n' >&2; exit 1; fi
      [ "$GOOGLE_CLIENT_ID_PRESENT" = 1 ] || exit 0
      printf '%s' "$GOOGLE_CLIENT_ID"
      exit
      ;;
    *PRODUCTION_GOOGLE_CLIENT_SECRET*)
      if [ "$SOPS_FAILURE_KEY" = PRODUCTION_GOOGLE_CLIENT_SECRET ]; then printf 'private SOPS diagnostic\\n' >&2; exit 1; fi
      [ "$GOOGLE_CLIENT_SECRET_PRESENT" = 1 ] || exit 0
      printf '%s' "$GOOGLE_CLIENT_SECRET"
      exit
      ;;
    *PRODUCTION_ADMIN_PASSWORD*) cat "$ADMIN_PASSWORD_FILE"; exit ;;
  esac
done
exit 1`);
mockCommand(bin, 'rsync', `
set -eu
previous= last=
for arg do previous="$last"; last="$arg"; done
source="$previous"
destination="$last"
case "$destination" in
  *:librepaper/librepaper.tmp)
    mkdir -p "$REMOTE_ROOT"
    cp "$source" "$REMOTE_ROOT/librepaper.tmp"
    ${corruptTransfer ? "printf 'corrupt\\n' >> \"$REMOTE_ROOT/librepaper.tmp\"" : ':'}
    ;;
  *:librepaper/site/)
    mkdir -p "$REMOTE_ROOT/site"
    ;;
  *:librepaper/)
    mkdir -p "$REMOTE_ROOT"
    ;;
esac`);
mockCommand(bin, 'ssh', `
set -eu
command="$2"
case "$*" in *admin-secret*|*exporter-secret*|*pg-secret*|*github-secret*|*google-id*|*google-secret*) exit 91 ;; esac
case "$command" in
  *'.env.tmp'*)
    cat > "$REMOTE_ROOT/.env.tmp"
    chmod 600 "$REMOTE_ROOT/.env.tmp"
    mv "$REMOTE_ROOT/.env.tmp" "$REMOTE_ROOT/.env"
    ;;
  *'compose.override.yaml.tmp'*)
    cat > "$REMOTE_ROOT/compose.override.yaml.tmp"
    mv "$REMOTE_ROOT/compose.override.yaml.tmp" "$REMOTE_ROOT/compose.override.yaml"
    ;;
  *'Caddyfile'*) cat >> "$REMOTE_ROOT/Caddyfile" ;;
  *'postgres sh -s'*) cat > "$REMOTE_ROOT/monitoring-user.sh" ;;
  *'sha256sum librepaper.tmp'*) sha256sum "$REMOTE_ROOT/librepaper.tmp" | cut -d ' ' -f1 ;;
  *'chmod 755 librepaper.tmp'*) mv "$REMOTE_ROOT/librepaper.tmp" "$REMOTE_ROOT/librepaper" ;;
  *'prometheus sh -s'*)
    count=$(cat "$PROMETHEUS_COUNT_FILE" 2>/dev/null || printf '0')
    count=$((count + 1))
    printf '%s' "$count" > "$PROMETHEUS_COUNT_FILE"
    if [ "$count" -le "$PROMETHEUS_TRANSIENT_FAILURES" ]; then exit 1; fi
    script=$(cat)
    case "$script" in *pg_up*) ;; *) exit 93 ;; esac
    printf '%s\\n---LIBREPAPER-METRICS---\\n%s\\n---LIBREPAPER-METRICS---\\n%s\\n' \\
      '{"data":{"activeTargets":[{"health":"up","labels":{"job":"librepaper"}},{"health":"up","labels":{"job":"node"}},{"health":"up","labels":{"job":"postgres"}}]}}' \\
      '{"data":{"result":[{"values":[[1,"1"],[2,"1"]]}]}}' \\
      '${pgUpHealthy ? '{"data":{"result":[{"value":[3,"1"]}]}}' : '{"data":{"result":[]}}'}'
    ;;
  *'docker compose up -d --build'*)
    printf called >> "$COMPOSE_UP_FILE"
    printf 'up\\n' >> "$ORDER_FILE"
    ;;
  *'pg_dump'*' -d librepaper_squash'*)
    printf -- '-- comment\\nSET x = y;\\n'
    cat "$SQUASH_SCHEMA_FILE"
    ;;
  *'pg_dump'*)
    printf -- '-- comment\\nSET x = y;\\n'
    cat "$REMOTE_SCHEMA_FILE"
    ;;
  *'psql'*' -d postgres'*) cat >/dev/null; exit 0 ;;
  *'psql'*' -d librepaper_squash'*) cat >/dev/null ;;
  *'psql'*)
    sql=$(cat)
    case "$sql" in
      *to_regclass*)
        if [ "$(cat "$REMOTE_MIGRATIONS_FILE")" = absent ]; then printf 'absent\\n'; else tr ':' '\\t' < "$REMOTE_MIGRATIONS_FILE"; fi
        ;;
    esac
    case "$sql" in
      *'DELETE FROM _sqlx_migrations'*)
        printf '%s\\n' "$sql" > "$PIN_SQL_FILE"
        printf 'pin\\n' >> "$ORDER_FILE"
        ;;
    esac
    ;;
  *'migrations-squash.log'*) cat >> "$REMOTE_ROOT/migrations-squash.log" ;;
  *'admin backup'*) printf called >> "$BACKUP_FILE" ;;
  *) : ;;
esac`);
mockCommand(bin, 'curl', `
set -eu
out= headers= format= url= config= followed=no connect_timeout= max_time=
for arg do case "$arg" in *admin-secret*|*exporter-secret*|*pg-secret*|*github-secret*|*google-id*|*google-secret*) exit 92 ;; esac; done
while [ "$#" -gt 0 ]; do
  case "$1" in
    --config) config="$2"; shift 2 ;;
    --connect-timeout) connect_timeout="$2"; shift 2 ;;
    --max-time) max_time="$2"; shift 2 ;;
    -o) out="$2"; shift 2 ;;
    -D) headers="$2"; shift 2 ;;
    -w) format="$2"; shift 2 ;;
    -L|--location) followed=yes; shift ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
printf '%s\t%s\t%s\n' "$url" "$connect_timeout" "$max_time" >> "$CURL_CALLS_FILE"
if [ -n "$config" ]; then
  [ "$(stat -c %a "$config")" = 600 ]
  grep -q 'user = "admin:admin-secret"' "$config"
fi
code=200 body='' location= provider=
case "$url" in
  */admin/monitoring/api/search*)
    if [ -n "$config" ]; then
      count=$(cat "$GRAFANA_COUNT_FILE" 2>/dev/null || printf '0')
      count=$((count + 1))
      printf '%s' "$count" > "$GRAFANA_COUNT_FILE"
      if [ "$GRAFANA_PERSISTENT_STATUS" != 0 ]; then code="$GRAFANA_PERSISTENT_STATUS"
      elif [ "$count" -le "$GRAFANA_TRANSIENT_FAILURES" ]; then code=502
      elif [ "$GRAFANA_AUTH_STATUS" != 200 ]; then code="$GRAFANA_AUTH_STATUS"
      else body='[{"type":"dash-db","uid":"monitoring","title":"Production"}]'; fi
    else code=401; fi
    ;;
  */admin/monitoring/api/dashboards/uid/*) body='{"dashboard":{"panels":[{"type":"stat","title":"Healthy"}]}}' ;;
  */admin/monitoring/d/*)
    code=302
    [ -z "$headers" ] || printf 'HTTP/1.1 302 Found\\r\\nLocation: https://app.librepaper.org/admin/monitoring/login\\r\\n\\r\\n' > "$headers"
    ;;
  https://app.librepaper.org/api/status) code=403 ;;
  https://app.librepaper.org/metrics|https://app.librepaper.org/api/v1/*) code=404 ;;
  https://app.librepaper.org/auth/login/github)
    code=302
    location=https://github.com/login/oauth/authorize?client_id=test
    provider=github
    ;;
  https://app.librepaper.org/auth/login/google)
    code="$GOOGLE_HTTP_STATUS"
    location="$GOOGLE_LOCATION"
    provider=google
    ;;
  https://docs.librepaper.org/) code=200 ;;
  https://librepaper.com) printf 'HTTP/2 301\\r\\nlocation: https://librepaper.org/\\r\\n\\r\\n'; exit 0 ;;
esac
if [ -n "$out" ] && [ "$out" != /dev/null ]; then printf '%s' "$body" > "$out"; fi
if [ -n "$provider" ]; then printf '%s\\t%s\\n' "$provider" "$followed" >> "$OAUTH_CALLS_FILE"; fi
if [ -n "$format" ]; then
  case "$format" in
    *'%{redirect_url}'*) printf '%s\\t%s' "$code" "$location" ;;
    *) printf '%s' "$code" ;;
  esac
fi`);
  mockCommand(bin, 'sleep', `
if [ "$REAL_SLEEP" = 1 ]; then
  exec "$NODE_EXECUTABLE" -e 'setTimeout(() => process.exit(0), Number(process.argv[1]) * 1000)' "$1"
fi
exit 0`);
  const executable = path.join(root, 'librepaper');
  writeFileSync(executable, '#!/bin/sh\nexit 0\n');
  chmodSync(executable, 0o755);

  return {
    root,
    remote,
    executable,
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      REMOTE_ROOT: remote,
      MIGRATIONS_DIR: migrationsDir,
      REMOTE_MIGRATIONS_FILE: remoteMigrationsFile,
      REMOTE_SCHEMA_FILE: remoteSchemaFile,
      SQUASH_SCHEMA_FILE: squashSchemaFile,
      PIN_SQL_FILE: pinSqlFile,
      COMPOSE_UP_FILE: composeUpFile,
      BACKUP_FILE: backupFile,
      ORDER_FILE: orderFile,
      HOST: 'ubuntu@test-host',
      TMPDIR: temp,
      NODE_EXECUTABLE: process.execPath,
      REAL_SLEEP: realSleep ? '1' : '0',
      ADMIN_PASSWORD_FILE: adminPasswordFile,
      EXPORTER_PASSWORD_FILE: exporterPasswordFile,
      GRAFANA_COUNT_FILE: grafanaCountFile,
      PROMETHEUS_COUNT_FILE: prometheusCountFile,
      GRAFANA_TRANSIENT_FAILURES: String(grafanaTransientFailures),
      GRAFANA_PERSISTENT_STATUS: String(grafanaPersistentStatus),
      GRAFANA_AUTH_STATUS: String(grafanaAuthStatus),
      PROMETHEUS_TRANSIENT_FAILURES: String(prometheusTransientFailures),
      MAKE_CALLED_FILE: makeCalledFile,
      OAUTH_CALLS_FILE: oauthCallsFile,
      CURL_CALLS_FILE: curlCallsFile,
      GOOGLE_CLIENT_ID: String(googleClientId ?? ''),
      GOOGLE_CLIENT_SECRET: String(googleClientSecret ?? ''),
      GOOGLE_CLIENT_ID_PRESENT: googleClientIdPresent ? '1' : '0',
      GOOGLE_CLIENT_SECRET_PRESENT: googleClientSecretPresent ? '1' : '0',
      GOOGLE_HTTP_STATUS: String(googleHttpStatus),
      GOOGLE_LOCATION: googleLocation,
      SOPS_FAILURE_KEY: sopsFailureKey,
    },
    makeCalledFile,
    oauthCallsFile,
    pinSqlFile,
    composeUpFile,
    backupFile,
    orderFile,
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

function runProduction(f, command, version = 'v0.0.9') {
  const args = command === 'deploy-local' ? ['deploy-local', f.executable]
    : command === 'squash-migrations' ? [command]
      : [command, version];
  return spawnSync(deploy, args, { cwd: repo, env: f.env, encoding: 'utf8' });
}

test('deploy writes Google OAuth credentials to .env and keeps them out of output', () => {
  const f = fixture();
  try {
    const result = runProduction(f, 'deploy');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /8 keys decrypted/);
    assert.match(result.stdout, /15 settings/);
    const envFile = readFileSync(path.join(f.remote, '.env'), 'utf8');
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_ID=google-id$/m);
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_SECRET=google-secret$/m);
    assert.equal(envFile.trimEnd().split('\n').length, 15);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /google-id|google-secret/);
  } finally {
    f.cleanup();
  }
});

test('deploy-local writes Google OAuth credentials to .env and keeps them out of output', () => {
  const f = fixture();
  try {
    const result = runProduction(f, 'deploy-local');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /8 keys decrypted/);
    assert.match(result.stdout, /15 settings/);
    const envFile = readFileSync(path.join(f.remote, '.env'), 'utf8');
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_ID=google-id$/m);
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_SECRET=google-secret$/m);
    assert.equal(envFile.trimEnd().split('\n').length, 15);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /google-id|google-secret/);
  } finally {
    f.cleanup();
  }
});

test('invalid or undecryptable Google credentials stop both deployment paths before build or remote mutation', () => {
  const badCredentials = [
    { name: 'missing Google client ID', options: { googleClientIdPresent: false }, key: 'PRODUCTION_GOOGLE_CLIENT_ID' },
    { name: 'null Google client secret', options: { googleClientSecret: 'null' }, key: 'PRODUCTION_GOOGLE_CLIENT_SECRET' },
    { name: 'empty Google client ID', options: { googleClientId: '' }, key: 'PRODUCTION_GOOGLE_CLIENT_ID' },
    { name: 'whitespace-only Google client secret', options: { googleClientSecret: '  \t' }, key: 'PRODUCTION_GOOGLE_CLIENT_SECRET' },
    { name: 'Google client secret decryption failure', options: { sopsFailureKey: 'PRODUCTION_GOOGLE_CLIENT_SECRET' }, key: 'PRODUCTION_GOOGLE_CLIENT_SECRET' },
  ];
  for (const command of ['deploy', 'deploy-local']) {
    for (const scenario of badCredentials) {
      const f = fixture(scenario.options);
      try {
        const result = runProduction(f, command);
        assert.notEqual(result.status, 0, `${command}: ${scenario.name} unexpectedly succeeded`);
        assert.match(result.stderr, new RegExp(scenario.key));
        assert.doesNotMatch(`${result.stdout}${result.stderr}`, /google-id|google-secret|private SOPS diagnostic/);
        assert.equal(existsSync(f.makeCalledFile), false, `${command}: ran make for ${scenario.name}`);
        assert.deepEqual(readdirSync(f.remote), [], `${command}: mutated remote for ${scenario.name}`);
      } finally {
        f.cleanup();
      }
    }
  }
});

test('verify checks both OAuth providers with unfollowed matching authorization redirects', () => {
  const f = fixture();
  try {
    const result = spawnSync(deploy, ['verify'], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /github OAuth redirect answers/i);
    assert.match(result.stdout, /google OAuth redirect answers/i);
    const calls = readFileSync(f.oauthCallsFile, 'utf8').trim().split('\n');
    assert.deepEqual(calls, ['github\tno', 'google\tno']);
  } finally {
    f.cleanup();
  }
});

test('verify bounds every HTTP request and passes the health probe its remaining deadline', () => {
  const f = fixture();
  try {
    const result = spawnSync(deploy, ['verify'], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const requests = readFileSync(path.join(f.root, 'curl-calls'), 'utf8').trim().split('\n')
      .map((line) => line.split('\t'));
    assert.ok(requests.length > 0);
    for (const [url, connectTimeout, maxTime] of requests) {
      assert.ok(Number(connectTimeout) > 0, `${url} is missing a connect timeout`);
      assert.ok(Number(maxTime) > 0, `${url} is missing a total timeout`);
    }
    const health = requests.find(([url]) => url === 'https://app.librepaper.org/health');
    assert.ok(health, 'health probe was not recorded');
    assert.ok(Number(health[1]) > 0 && Number(health[1]) <= 5, `unexpected health connect timeout ${health[1]}`);
    assert.ok(Number(health[2]) > 0 && Number(health[2]) <= 120, `unexpected health total timeout ${health[2]}`);
  } finally {
    f.cleanup();
  }
});

test('verify rejects Google 200, 404, and wrong-provider redirects without revealing the URL', () => {
  for (const scenario of [
    { name: '200 without redirect', googleHttpStatus: '200', googleLocation: '' },
    { name: '404 without Location', googleHttpStatus: '404', googleLocation: '' },
    { name: 'redirect to GitHub', googleHttpStatus: '302', googleLocation: 'https://github.com/login/oauth/authorize?client_id=test' },
  ]) {
    const f = fixture(scenario);
    try {
      const result = spawnSync(deploy, ['verify'], { cwd: repo, env: f.env, encoding: 'utf8' });
      assert.notEqual(result.status, 0, `${scenario.name} unexpectedly passed`);
      assert.match(result.stderr, /google OAuth/i);
      assert.doesNotMatch(`${result.stdout}${result.stderr}`, /accounts\.google\.com|github\.com/);
    } finally {
      f.cleanup();
    }
  }
});

test('deploy-local verifies the copied binary before replacing it and keeps secrets out of output', () => {
  const f = fixture();
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /verified SHA-256/);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /admin-secret|exporter-secret|pg-secret|github-secret/);
    const envFile = readFileSync(path.join(f.remote, '.env'), 'utf8');
    assert.match(envFile, /LIBREPAPER_ADMIN_PASSWORD=admin-secret/);
    assert.match(envFile, /POSTGRES_EXPORTER_PASSWORD=exporter-secret/);
    assert.equal(statSync(path.join(f.remote, '.env')).mode & 0o777, 0o600);
    assert.match(readFileSync(path.join(f.remote, 'compose.override.yaml'), 'utf8'), /Dockerfile.local/);
    assert.equal(existsSync(path.join(f.remote, 'librepaper')), true);
  } finally {
    f.cleanup();
  }
});

test('deploy-local refuses to replace the existing binary when transfer checksum differs', () => {
  const f = fixture({ corruptTransfer: true });
  try {
    writeFileSync(path.join(f.remote, 'librepaper'), 'previous binary');
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /remote binary checksum mismatch/);
    assert.equal(readFileSync(path.join(f.remote, 'librepaper'), 'utf8'), 'previous binary');
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /admin-secret|exporter-secret|pg-secret|github-secret/);
  } finally {
    f.cleanup();
  }
});

test('deploy-local rejects unsafe .env passwords before any remote write', () => {
  for (const secrets of [
    { adminPassword: 'unsafe$#secret' },
    { exporterPassword: 'line one\nline two' },
  ]) {
    const f = fixture(secrets);
    try {
      const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /production passwords must use only ASCII letters/);
      assert.deepEqual(readdirSync(f.remote), []);
      assert.doesNotMatch(`${result.stdout}${result.stderr}`, /unsafe\$#secret|line one/);
    } finally {
      f.cleanup();
    }
  }
});

test('deploy-local retries transient Grafana startup errors within the bounded startup window', () => {
  const f = fixture({ grafanaTransientFailures: 1 });
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /waiting for Grafana to finish starting/);
    assert.equal(readFileSync(path.join(f.root, 'grafana-count'), 'utf8'), '2');
  } finally {
    f.cleanup();
  }
});

test('deploy-local fails immediately for rejected Grafana credentials', () => {
  const f = fixture({ grafanaAuthStatus: 401 });
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /authenticated Grafana dashboard API returned HTTP 401/);
    assert.equal(readFileSync(path.join(f.root, 'grafana-count'), 'utf8'), '1');
  } finally {
    f.cleanup();
  }
});

test('deploy-local refuses monitoring readiness without PostgreSQL exporter health', () => {
  const f = fixture({ pgUpHealthy: false });
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Prometheus APIs, targets, LibrePaper sample history, snapshot, or pg_up are unhealthy/);
  } finally {
    f.cleanup();
  }
});

test('deploy refuses an uninstrumented v0.0.8 release before writing remotely', () => {
  const f = fixture();
  try {
    const result = spawnSync(deploy, ['deploy', 'v0.0.8'], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /v0.0.8 lacks metrics.*Use deploy-local/);
    assert.deepEqual(readdirSync(f.remote), []);
  } finally {
    f.cleanup();
  }
});

test('deploy-local retries a temporary Prometheus API failure and continues when it recovers', () => {
  const f = fixture({ prometheusTransientFailures: 1 });
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /waiting for Prometheus APIs/);
    assert.equal(readFileSync(path.join(f.root, 'prometheus-count'), 'utf8'), '2');
  } finally {
    f.cleanup();
  }
});

test('persistent Grafana API failures time out and remove credential files', {
  skip: process.env.LIBREPAPER_TEST_SLOW_DEPLOY !== '1',
}, () => {
  const f = fixture({ grafanaPersistentStatus: 503, realSleep: true });
  const started = Date.now();
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], {
      cwd: repo,
      env: f.env,
      encoding: 'utf8',
      timeout: 70_000,
    });
    const elapsed = (Date.now() - started) / 1000;
    assert.equal(result.error, undefined, result.error?.message);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /authenticated Grafana dashboard API returned HTTP 503/);
    assert.ok(elapsed >= 59 && elapsed <= 65, `expected ~60 seconds, got ${elapsed}`);
    assert.deepEqual(readdirSync(path.join(f.root, 'tmp')), []);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /admin-secret|exporter-secret|pg-secret|github-secret/);
  } finally {
    f.cleanup();
  }
});

const migrationsAB = { '0001_a.sql': 'CREATE TABLE a (id integer);', '0002_b.sql': 'CREATE TABLE b (id integer);' };
const staleHistory = `1:${'cd'.repeat(48)}\n2:${'ef'.repeat(48)}\n3:${'01'.repeat(48)}`;

test('deploy proceeds without touching _sqlx_migrations when every applied migration matches the tree', () => {
  const f = fixture({
    migrations: migrationsAB,
    remoteMigrations: `1:${sha384(migrationsAB['0001_a.sql'])}\n2:${sha384(migrationsAB['0002_b.sql'])}`,
  });
  try {
    const result = runProduction(f, 'deploy');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /2 applied migrations match the tree/);
    assert.equal(existsSync(f.composeUpFile), true);
    assert.equal(existsSync(f.pinSqlFile), false);
  } finally {
    f.cleanup();
  }
});

test('deploy proceeds on a fresh database', () => {
  const f = fixture();
  try {
    const result = runProduction(f, 'deploy');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /fresh database/);
    assert.equal(existsSync(f.pinSqlFile), false);
  } finally {
    f.cleanup();
  }
});

test('deploy stops before starting containers when an applied migration has no matching file and the tree is not squashed', () => {
  const f = fixture({
    migrations: migrationsAB,
    remoteMigrations: `1:${sha384(migrationsAB['0001_a.sql'])}\n2:${sha384(migrationsAB['0002_b.sql'])}\n3:${'ab'.repeat(48)}`,
  });
  try {
    const result = runProduction(f, 'deploy');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /version 3/);
    assert.equal(existsSync(f.composeUpFile), false);
    assert.equal(existsSync(f.pinSqlFile), false);
  } finally {
    f.cleanup();
  }
});

test("deploy re-pins _sqlx_migrations when the tree holds a single squashed file that reproduces production's schema", () => {
  const f = fixture({ remoteMigrations: staleHistory });
  try {
    const result = runProduction(f, 'deploy');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const pin = readFileSync(f.pinSqlFile, 'utf8');
    assert.match(pin, /DELETE FROM _sqlx_migrations/);
    assert.match(pin, /VALUES \(1, 'catalog'/);
    assert.ok(pin.includes(sha384(defaultMigration)));
    assert.equal(existsSync(f.composeUpFile), true);
    assert.equal(existsSync(f.backupFile), true);
    assert.deepEqual(readFileSync(f.orderFile, 'utf8').trim().split('\n'), ['pin', 'up']);
  } finally {
    f.cleanup();
  }
});

test("deploy refuses to re-pin when the squashed file does not reproduce production's schema", () => {
  const f = fixture({ remoteMigrations: staleHistory, squashSchema: 'CREATE TABLE other (id integer);' });
  try {
    const result = runProduction(f, 'deploy');
    assert.notEqual(result.status, 0);
    const output = `${result.stdout}${result.stderr}`;
    assert.match(output, /^--- production$/m);
    assert.match(output, /^\+\+\+ squashed$/m);
    assert.equal(existsSync(f.pinSqlFile), false);
    assert.equal(existsSync(f.composeUpFile), false);
  } finally {
    f.cleanup();
  }
});

test('squash-migrations subcommand refuses when production already matches the single file', () => {
  const f = fixture({ remoteMigrations: `1:${sha384(defaultMigration)}` });
  try {
    const result = runProduction(f, 'squash-migrations');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /nothing to do/);
    assert.equal(existsSync(f.pinSqlFile), false);
  } finally {
    f.cleanup();
  }
});

test('squash-migrations subcommand pins and logs', () => {
  const f = fixture({ remoteMigrations: staleHistory });
  try {
    const result = runProduction(f, 'squash-migrations');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(existsSync(f.pinSqlFile), true);
    assert.match(readFileSync(path.join(f.remote, 'migrations-squash.log'), 'utf8'), /-> 1/);
  } finally {
    f.cleanup();
  }
});
