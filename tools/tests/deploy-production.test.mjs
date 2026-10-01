import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, chmodSync, statSync, rmSync, existsSync, readdirSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'tools/deploy-production');

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
  grafanaAuthStatus = 200,
  pgUpHealthy = true,
} = {}) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'librepaper-deploy-test-'));
  const bin = path.join(root, 'bin');
  const remote = path.join(root, 'remote');
  mkdirSync(bin);
  mkdirSync(remote);
  const adminPasswordFile = path.join(root, 'admin-password');
  const exporterPasswordFile = path.join(root, 'exporter-password');
  const grafanaCountFile = path.join(root, 'grafana-count');
  writeFileSync(adminPasswordFile, adminPassword);
  writeFileSync(exporterPasswordFile, exporterPassword);

  mockCommand(bin, 'make', 'exit 0');
  mockCommand(bin, 'dig', 'printf "192.0.2.7\\n"');
  mockCommand(bin, 'sops', `
for arg do
  case "$arg" in
    *PRODUCTION_ACME_EMAIL*) printf 'admin@example.org\\n'; exit ;;
    *PRODUCTION_POSTGRES_PASSWORD*) printf 'pg-secret\\n'; exit ;;
    *PRODUCTION_POSTGRES_EXPORTER_PASSWORD*) cat "$EXPORTER_PASSWORD_FILE"; exit ;;
    *PRODUCTION_GITHUB_CLIENT_ID*) printf 'github-id\\n'; exit ;;
    *PRODUCTION_GITHUB_CLIENT_SECRET*) printf 'github-secret\\n'; exit ;;
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
case "$*" in *admin-secret*|*exporter-secret*|*pg-secret*|*github-secret*) exit 91 ;; esac
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
    script=$(cat)
    case "$script" in *pg_up*) ;; *) exit 93 ;; esac
    printf '%s\\n---LIBREPAPER-METRICS---\\n%s\\n---LIBREPAPER-METRICS---\\n%s\\n' \\
      '{"data":{"activeTargets":[{"health":"up","labels":{"job":"librepaper"}},{"health":"up","labels":{"job":"node"}},{"health":"up","labels":{"job":"postgres"}}]}}' \\
      '{"data":{"result":[{"values":[[1,"1"],[2,"1"]]}]}}' \\
      '${pgUpHealthy ? '{"data":{"result":[{"value":[3,"1"]}]}}' : '{"data":{"result":[]}}'}'
    ;;
  *) : ;;
esac`);
mockCommand(bin, 'curl', `
set -eu
out= headers= format= url= config=
for arg do case "$arg" in *admin-secret*|*exporter-secret*|*pg-secret*|*github-secret*) exit 92 ;; esac; done
while [ "$#" -gt 0 ]; do
  case "$1" in
    --config) config="$2"; shift 2 ;;
    --connect-timeout|--max-time) shift 2 ;;
    -o) out="$2"; shift 2 ;;
    -D) headers="$2"; shift 2 ;;
    -w) format="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
if [ -n "$config" ]; then
  [ "$(stat -c %a "$config")" = 600 ]
  grep -q 'user = "admin:admin-secret"' "$config"
fi
code=200 body=''
case "$url" in
  */admin/monitoring/api/search*)
    if [ -n "$config" ]; then
      count=$(cat "$GRAFANA_COUNT_FILE" 2>/dev/null || printf '0')
      count=$((count + 1))
      printf '%s' "$count" > "$GRAFANA_COUNT_FILE"
      if [ "$count" -le "$GRAFANA_TRANSIENT_FAILURES" ]; then code=502
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
  https://docs.librepaper.org/) code=200 ;;
  https://librepaper.com) printf 'HTTP/2 301\\r\\nlocation: https://librepaper.org/\\r\\n\\r\\n'; exit 0 ;;
esac
if [ -n "$out" ] && [ "$out" != /dev/null ]; then printf '%s' "$body" > "$out"; fi
  if [ -n "$format" ]; then printf '%s' "$code"; fi`);
  mockCommand(bin, 'sleep', 'exit 0');
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
      HOST: 'ubuntu@test-host',
      ADMIN_PASSWORD_FILE: adminPasswordFile,
      EXPORTER_PASSWORD_FILE: exporterPasswordFile,
      GRAFANA_COUNT_FILE: grafanaCountFile,
      GRAFANA_TRANSIENT_FAILURES: String(grafanaTransientFailures),
      GRAFANA_AUTH_STATUS: String(grafanaAuthStatus),
    },
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

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
    assert.equal(readFileSync(path.join(f.root, 'grafana-count'), 'utf8'), '3');
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
    assert.match(result.stderr, /Prometheus targets, LibrePaper sample history, or snapshot freshness are unhealthy/);
  } finally {
    f.cleanup();
  }
});
