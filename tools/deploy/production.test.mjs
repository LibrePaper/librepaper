import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, chmodSync, statSync, rmSync, existsSync, readdirSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'tools/deploy/production');

function mockCommand(dir, name, contents) {
  const file = path.join(dir, name);
  writeFileSync(file, `#!/bin/sh\n${contents}\n`);
  chmodSync(file, 0o755);
}

function fixture({
  corruptTransfer = false,
  adminPassword = 'admin-secret',
  exporterPassword = 'exporter-secret',
  grafanaTransientFailures = 0,
  grafanaPersistentStatus = 0,
  grafanaAuthStatus = 200,
  pgUpHealthy = true,
  prometheusTransientFailures = 0,
  caddyReloadFailure = false,
  realSleep = false,
  googleClientId = 'google-id',
  googleClientSecret = 'google-secret',
  googleClientIdPresent = true,
  googleClientSecretPresent = true,
  googleHttpStatus = '302',
  googleLocation = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=test',
  sopsFailureKey = '',
  configCheckFailure = false,
  backupCheckFailure = false,
  localKitConfig = '',
  backupConfig = '',
  runningConfig = 'previous container config\n',
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
  const composeUpFile = path.join(root, 'compose-up');
  const orderFile = path.join(root, 'order');
  const caddyTimeoutFile = path.join(root, 'caddy-timeout');
  const realTimeout = process.env.PATH.split(path.delimiter)
    .map((directory) => path.join(directory, 'timeout'))
    .find(existsSync);
  const kitSourceFile = path.join(root, 'kit-source');
  const localKitConfigFile = path.join(root, 'local-kit-librepaper.toml');
  const runningConfigFile = path.join(root, 'running-librepaper.toml');
  const backupConfigFile = path.join(remote, 'resticprofile.toml');
  writeFileSync(adminPasswordFile, adminPassword);
  writeFileSync(exporterPasswordFile, exporterPassword);
  if (localKitConfig) writeFileSync(localKitConfigFile, localKitConfig);
  if (backupConfig) writeFileSync(backupConfigFile, backupConfig);
  writeFileSync(runningConfigFile, runningConfig);

  mockCommand(bin, 'make', 'printf called >> "$MAKE_CALLED_FILE"; exit 0');
  mockCommand(bin, 'dig', 'printf "192.0.2.7\\n"');
  mockCommand(bin, 'sops', `
for arg do
  case "$arg" in
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
exclude_config=no
exclude_backup_config=no
ignore_existing=no
for arg do
  case "$arg" in
    --exclude=/librepaper.toml) exclude_config=yes ;;
    --exclude=/resticprofile.toml) exclude_backup_config=yes ;;
    --ignore-existing) ignore_existing=yes ;;
  esac
done
case "$destination" in
  *:librepaper/librepaper.candidate.tmp)
    mkdir -p "$REMOTE_ROOT"
    cp "$source" "$REMOTE_ROOT/librepaper.candidate.tmp"
    ${corruptTransfer ? "printf 'corrupt\\n' >> \"$REMOTE_ROOT/librepaper.candidate.tmp\"" : ':'}
    ;;
  *:librepaper/site/)
    mkdir -p "$REMOTE_ROOT/site"
    ;;
  *:librepaper/)
    mkdir -p "$REMOTE_ROOT"
    printf '%s' "$source" > "$KIT_SOURCE_FILE"
    if [ -n "$LOCAL_KIT_CONFIG_FILE" ] && [ -f "$LOCAL_KIT_CONFIG_FILE" ] && [ "$exclude_config" = no ]; then
      cp "$LOCAL_KIT_CONFIG_FILE" "$REMOTE_ROOT/librepaper.toml"
    fi
    [ "$exclude_backup_config" = yes ] || exit 94
    ;;
  *:librepaper/resticprofile.toml)
    [ "$ignore_existing" = yes ] || exit 95
    [ -e "$REMOTE_ROOT/resticprofile.toml" ] || cp "$source" "$REMOTE_ROOT/resticprofile.toml"
    ;;
esac`);
mockCommand(bin, 'ssh', `
set -eu
command="$2"
case "$*" in *admin-secret*|*github-secret*|*google-id*|*google-secret*) exit 91 ;; esac
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
  *'librepaper.toml.candidate.tmp'*)
    cat > "$REMOTE_ROOT/librepaper.toml.candidate.tmp"
    chmod 644 "$REMOTE_ROOT/librepaper.toml.candidate.tmp"
    mv -f "$REMOTE_ROOT/librepaper.toml.candidate.tmp" "$REMOTE_ROOT/librepaper.toml.candidate"
    ;;
  *'mv -f librepaper.toml.candidate librepaper.toml'*)
    mv -f "$REMOTE_ROOT/librepaper.toml.candidate" "$REMOTE_ROOT/librepaper.toml"
    printf 'config-install\\n' >> "$ORDER_FILE"
    ;;
  'cd librepaper && docker compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile')
    printf 'caddy-reload\\n' >> "$ORDER_FILE"
    [ "$CADDY_RELOAD_FAILURE" = 0 ]
    ;;
  'cat >> librepaper/caddy/Caddyfile') mkdir -p "$REMOTE_ROOT/caddy"; cat >> "$REMOTE_ROOT/caddy/Caddyfile" ;;
  *'psql -U librepaper -d librepaper -v ON_ERROR_STOP=1 -q'*) cat > "$REMOTE_ROOT/metrics-role.sql" ;;
  *'sha256sum librepaper.candidate.tmp'*) sha256sum "$REMOTE_ROOT/librepaper.candidate.tmp" | cut -d ' ' -f1 ;;
  *'chmod 755 librepaper.candidate.tmp'*) chmod 755 "$REMOTE_ROOT/librepaper.candidate.tmp"; mv "$REMOTE_ROOT/librepaper.candidate.tmp" "$REMOTE_ROOT/librepaper.candidate" ;;
  *'mv -f librepaper.candidate librepaper'*) mv -f "$REMOTE_ROOT/librepaper.candidate" "$REMOTE_ROOT/librepaper" ;;
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
  *'docker compose up -d --force-recreate --no-deps --wait --wait-timeout 180 librepaper'*)
    cp "$REMOTE_ROOT/librepaper.toml" "$RUNNING_CONFIG_FILE"
    printf 'app-recreate\\n' >> "$ORDER_FILE"
    ;;
  *'docker compose stop backup'*) printf 'backup-stop\\n' >> "$ORDER_FILE" ;;
  *'docker compose up -d --force-recreate --no-deps backup'*) printf 'backup-recreate\\n' >> "$ORDER_FILE" ;;
  *'docker compose up -d --wait --wait-timeout 180'*)
    printf called >> "$COMPOSE_UP_FILE"
    printf 'stack-up\\n' >> "$ORDER_FILE"
    ;;
  *'docker compose build librepaper backup'*) printf 'image-build\\n' >> "$ORDER_FILE" ;;
  *'docker compose run --rm --no-deps librepaper admin config check'*)
    printf 'config-check\\n' >> "$ORDER_FILE"
    [ "$CONFIG_CHECK_FAILURE" = 0 ]
    ;;
  *'docker compose run --rm --no-deps -e LIBREPAPER_BACKUP_VALIDATE_ONLY=1 backup'*)
    [ -f "$REMOTE_ROOT/resticprofile.toml" ] || exit 96
    cp "$REMOTE_ROOT/resticprofile.toml" "$BACKUP_CONFIG_COPY_FILE"
    printf 'backup-check\\n' >> "$ORDER_FILE"
    [ "$BACKUP_CHECK_FAILURE" = 0 ]
    ;;
  *) : ;;
esac`);
  mockCommand(bin, 'timeout', `
set -eu
duration="$1"
shift
case "$*" in
  *'caddy reload'*)
    [ "$duration" = 30 ] && [ "$#" -eq 3 ] && [ "$1" = ssh ] && [ "$2" = "$HOST" ] &&
      [ "$3" = 'cd librepaper && docker compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile' ] || exit 97
    printf '%s\\n' "$duration" > "$CADDY_TIMEOUT_FILE"
    ;;
esac
exec "$REAL_TIMEOUT" "$duration" "$@"
`);
mockCommand(bin, 'curl', `
set -eu
out= headers= format= url= config= followed=no connect_timeout= max_time=
for arg do case "$arg" in *admin-secret*|*github-secret*|*google-id*|*google-secret*) exit 92 ;; esac; done
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
      COMPOSE_UP_FILE: composeUpFile,
      KIT_SOURCE_FILE: kitSourceFile,
      ORDER_FILE: orderFile,
      CADDY_TIMEOUT_FILE: caddyTimeoutFile,
      REAL_TIMEOUT: realTimeout,
      CADDY_RELOAD_FAILURE: caddyReloadFailure ? '1' : '0',
      LOCAL_KIT_CONFIG_FILE: localKitConfig ? localKitConfigFile : '',
      RUNNING_CONFIG_FILE: runningConfigFile,
      BACKUP_CONFIG_COPY_FILE: path.join(root, 'backup-config-at-preflight'),
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
      CONFIG_CHECK_FAILURE: configCheckFailure ? '1' : '0',
      BACKUP_CHECK_FAILURE: backupCheckFailure ? '1' : '0',
    },
    makeCalledFile,
    oauthCallsFile,
    composeUpFile,
    kitSourceFile,
    orderFile,
    runningConfigFile,
    backupConfigFile,
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

function runProduction(f, command, version = 'v0.0.9') {
  const args = command === 'deploy-local' ? ['deploy-local', f.executable]
    : [command, version];
  return spawnSync(deploy, args, { cwd: repo, env: f.env, encoding: 'utf8' });
}

test('deploy writes Google OAuth credentials to .env and keeps them out of output', () => {
  const f = fixture();
  try {
    const result = runProduction(f, 'deploy');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /5 keys decrypted/);
    assert.match(result.stdout, /staged librepaper.toml/);
    assert.equal(readFileSync(f.kitSourceFile, 'utf8'), 'deploy/');
    const envFile = readFileSync(path.join(f.remote, '.env'), 'utf8');
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_ID=google-id$/m);
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_SECRET=google-secret$/m);
    assert.equal(envFile.trimEnd().split('\n').length, 7);
    const productionConfig = readFileSync(path.join(f.remote, 'librepaper.toml'), 'utf8');
    const shippedBackupConfig = readFileSync(path.join(repo, 'deploy', 'resticprofile.toml'), 'utf8');
    assert.equal(readFileSync(f.backupConfigFile, 'utf8'), shippedBackupConfig);
    assert.equal(readFileSync(path.join(f.root, 'backup-config-at-preflight'), 'utf8'), shippedBackupConfig);
    const override = readFileSync(path.join(f.remote, 'compose.override.yaml'), 'utf8');
    assert.match(override, /\.\/site:\/srv\/site:ro/);
    assert.doesNotMatch(override, /SOURCE: local|librepaper\.candidate/);
    assert.match(readFileSync(path.join(f.remote, 'caddy', 'Caddyfile'), 'utf8'), /librepaper\.org/);
    assert.equal(existsSync(path.join(f.remote, 'Caddyfile')), false);
    assert.match(productionConfig, /\[origins\]\napp = "https:\/\/app\.librepaper\.org"/);
    assert.match(productionConfig, /database_url = \{ env = "LIBREPAPER_DATABASE_URL" \}/);
    const order = readFileSync(f.orderFile, 'utf8');
    assert.ok(order.indexOf('image-build') < order.indexOf('config-check'));
    assert.ok(order.indexOf('config-check') < order.indexOf('backup-check'));
    assert.ok(order.indexOf('backup-check') < order.indexOf('backup-stop'));
    assert.ok(order.indexOf('backup-stop') < order.indexOf('config-install'));
    assert.ok(order.indexOf('config-install') < order.indexOf('backup-recreate'));
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /google-id|google-secret/);
  } finally {
    f.cleanup();
  }
});

test('deploy-local stages Google OAuth credentials and the candidate binary for rebuilds', () => {
  const f = fixture();
  try {
    const result = runProduction(f, 'deploy-local');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /5 keys decrypted/);
    assert.match(result.stdout, /staged librepaper.toml/);
    const envFile = readFileSync(path.join(f.remote, '.env'), 'utf8');
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_ID=google-id$/m);
    assert.match(envFile, /^LIBREPAPER_GOOGLE_CLIENT_SECRET=google-secret$/m);
    assert.equal(envFile.trimEnd().split('\n').length, 7);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /google-id|google-secret/);
    assert.equal(existsSync(path.join(f.remote, 'librepaper.candidate')), true);
    const override = readFileSync(path.join(f.remote, 'compose.override.yaml'), 'utf8');
    assert.match(override, /\.\/site:\/srv\/site:ro/);
    assert.match(override, /SOURCE: local/);
    assert.match(override, /librepaper\.candidate/);
  } finally {
    f.cleanup();
  }
});

for (const command of ['deploy', 'deploy-local']) {
  test(`${command} replaces the running app config even when the image is unchanged`, () => {
    const f = fixture({
      localKitConfig: 'operator-local config that must not be deployed\n',
      backupConfig: '[global]\nscheduler = "operator scheduler"\n',
      runningConfig: 'previously mounted config\n',
    });
    try {
      writeFileSync(path.join(f.remote, 'librepaper.toml'), 'previously installed config\n');
      const result = runProduction(f, command);
      assert.equal(result.status, 0, result.stderr || result.stdout);
      const installed = readFileSync(path.join(f.remote, 'librepaper.toml'), 'utf8');
      const backupConfig = '[global]\nscheduler = "operator scheduler"\n';
      assert.equal(readFileSync(f.backupConfigFile, 'utf8'), backupConfig);
      assert.equal(readFileSync(path.join(f.root, 'backup-config-at-preflight'), 'utf8'), backupConfig);
      assert.doesNotMatch(installed, /operator-local config/);
      assert.equal(readFileSync(f.runningConfigFile, 'utf8'), installed);
      assert.deepEqual(readFileSync(f.orderFile, 'utf8').trim().split('\n'), [
        'image-build', 'config-check', 'backup-check', 'backup-stop', 'config-install',
        'app-recreate', 'backup-recreate', 'stack-up', 'caddy-reload',
      ]);
      assert.equal(readFileSync(path.join(f.root, 'caddy-timeout'), 'utf8').trim(), '30');
    } finally {
      f.cleanup();
    }
  });
}

for (const command of ['deploy', 'deploy-local']) {
  test(`${command} reports Caddy reload failure after the stack starts`, () => {
    const f = fixture({ caddyReloadFailure: true });
    try {
      const result = runProduction(f, command);
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /Caddy configuration reload failed/);
      assert.deepEqual(readFileSync(f.orderFile, 'utf8').trim().split('\n').slice(-2), [
        'stack-up', 'caddy-reload',
      ]);
      assert.doesNotMatch(`${result.stdout}${result.stderr}`, /Done:/);
    } finally {
      f.cleanup();
    }
  });
}

test('a binary without TOML config support leaves the installed config and running app in place', () => {
  const f = fixture({ configCheckFailure: true, localKitConfig: 'operator-local config\n' });
  try {
    writeFileSync(path.join(f.remote, 'librepaper.toml'), 'previous production config\n');
    const result = runProduction(f, 'deploy');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /candidate binary cannot load librepaper\.toml/);
    assert.equal(readFileSync(path.join(f.remote, 'librepaper.toml'), 'utf8'), 'previous production config\n');
    assert.equal(readFileSync(f.runningConfigFile, 'utf8'), 'previous container config\n');
    assert.equal(existsSync(f.composeUpFile), false);
    assert.doesNotMatch(readFileSync(f.orderFile, 'utf8'), /caddy-reload/);
  } finally {
    f.cleanup();
  }
});

test('an invalid backup candidate leaves the installed config and running services in place', () => {
  const f = fixture({ backupCheckFailure: true, localKitConfig: 'operator-local config\n' });
  try {
    writeFileSync(path.join(f.remote, 'librepaper.toml'), 'previous production config\n');
    const result = runProduction(f, 'deploy');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /candidate backup image cannot validate the configured profile/);
    assert.equal(readFileSync(path.join(f.remote, 'librepaper.toml'), 'utf8'), 'previous production config\n');
    assert.equal(readFileSync(f.runningConfigFile, 'utf8'), 'previous container config\n');
    const order = readFileSync(f.orderFile, 'utf8');
    assert.match(order, /image-build[\s\S]*config-check[\s\S]*backup-check/);
    assert.doesNotMatch(order, /backup-stop|config-install|app-recreate|backup-recreate|stack-up/);
  } finally {
    f.cleanup();
  }
});

test('an incompatible local candidate leaves the installed executable untouched', () => {
  const f = fixture({ configCheckFailure: true, localKitConfig: 'operator-local config\n' });
  try {
    writeFileSync(path.join(f.remote, 'librepaper'), 'previous production executable\n');
    writeFileSync(path.join(f.remote, 'librepaper.toml'), 'previous production config\n');
    const result = runProduction(f, 'deploy-local');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /candidate binary cannot load librepaper\.toml/);
    assert.equal(readFileSync(path.join(f.remote, 'librepaper'), 'utf8'), 'previous production executable\n');
    assert.equal(readFileSync(path.join(f.remote, 'librepaper.toml'), 'utf8'), 'previous production config\n');
    assert.equal(readFileSync(f.runningConfigFile, 'utf8'), 'previous container config\n');
    assert.equal(existsSync(f.composeUpFile), false);
    assert.doesNotMatch(readFileSync(f.orderFile, 'utf8'), /caddy-reload/);
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

test('deploy-local verifies and retains the copied candidate binary and keeps secrets out of output', () => {
  const f = fixture();
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /verified SHA-256/);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /admin-secret|github-secret/);
    const envFile = readFileSync(path.join(f.remote, '.env'), 'utf8');
    assert.match(envFile, /GRAFANA_ADMIN_PASSWORD=admin-secret/);
    assert.match(envFile, /^COMPOSE_FILE=compose.yaml:compose.override.yaml$/m);
    assert.doesNotMatch(envFile, /^COMPOSE_PROFILES=/m);
    assert.doesNotMatch(envFile, /POSTGRES_EXPORTER_PASSWORD/);
    assert.equal(statSync(path.join(f.remote, '.env')).mode & 0o777, 0o600);
    const override = readFileSync(path.join(f.remote, 'compose.override.yaml'), 'utf8');
    assert.match(override, /SOURCE: local/);
    assert.match(override, /librepaper.candidate/);
    assert.equal(existsSync(path.join(f.remote, 'librepaper.candidate')), true);
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
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /admin-secret|github-secret/);
  } finally {
    f.cleanup();
  }
});

test('deploy-local rejects unsafe .env passwords before any remote write', () => {
  const secrets = { adminPassword: 'unsafe$#secret' };
  const f = fixture(secrets);
  try {
    const result = spawnSync(deploy, ['deploy-local', f.executable], { cwd: repo, env: f.env, encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /production passwords must use only ASCII letters/);
    assert.deepEqual(readdirSync(f.remote), []);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /unsafe\$#secret/);
  } finally {
    f.cleanup();
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
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /admin-secret|github-secret/);
  } finally {
    f.cleanup();
  }
});
