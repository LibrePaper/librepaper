import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, chmodSync, statSync, rmSync, existsSync, readdirSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const deploy = path.join(repo, 'tools/deploy/production');
const productionToml = readFileSync(path.join(repo, 'tools/deploy/production.toml'), 'utf8');
const productionResticProfile = readFileSync(path.join(repo, 'tools/deploy/production-resticprofile.toml'), 'utf8');
const sshHost = 'ubuntu@test-host';

// The values the sops mock returns for each placeholder.
const mockCredentials = {
  LIBREPAPER_GITHUB_CLIENT_ID: 'github-id',
  LIBREPAPER_GITHUB_CLIENT_SECRET: 'github-secret',
  LIBREPAPER_GOOGLE_CLIENT_ID: 'google-id',
  LIBREPAPER_GOOGLE_CLIENT_SECRET: 'google-secret',
  LIBREPAPER_ADMIN_PASSWORD: 'admin-secret',
  LIBREPAPER_OBJECTS_S3_ACCESS_KEY_ID: 'objects-key-id',
  LIBREPAPER_OBJECTS_S3_SECRET_ACCESS_KEY: 'objects-secret',
  LIBREPAPER_RESTIC_PASSWORD: 'restic-password',
  LIBREPAPER_BACKUPS_S3_ACCESS_KEY_ID: 'backup-key-id',
  LIBREPAPER_BACKUPS_S3_SECRET_ACCESS_KEY: 'backup-secret',
  LIBREPAPER_HEALTHCHECK_BACKUP_URL: 'https://hc-ping.example/backup-uuid',
  LIBREPAPER_HEALTHCHECK_CHECK_URL: 'https://hc-ping.example/check-uuid',
};

// Settings that are not secret: they may appear in commands and output.
const mockSettings = {
  LIBREPAPER_OBJECTS_S3_ENDPOINT: 'https://objects.example',
  LIBREPAPER_OBJECTS_S3_REGION: 'objects-region',
  LIBREPAPER_OBJECTS_S3_BUCKET: 'objects-bucket',
  LIBREPAPER_BACKUPS_S3_ENDPOINT: 'https://backups.example',
  LIBREPAPER_BACKUPS_S3_REGION: 'backups-region',
  LIBREPAPER_BACKUPS_S3_BUCKET: 'backups-bucket',
  LIBREPAPER_MIRROR_S3_REGION: 'mirror-region',
  LIBREPAPER_MIRROR_S3_BUCKET: 'mirror-bucket',
};

// Fills each @NAME@ the way the deploy does: values escaped for a TOML basic string.
function renderExpected(text, values) {
  return text.replace(/@(LIBREPAPER_[A-Z0-9_]+)@/g, (match, name) => {
    assert.ok(name in values, `no mock value for ${name}`);
    return values[name].replace(/\\/g, '\\\\').replace(/"/g, '\\"');
  });
}
const expectedConfig = renderExpected(productionToml, { ...mockCredentials, ...mockSettings });
const expectedResticProfile = renderExpected(productionResticProfile, { ...mockCredentials, ...mockSettings });

function mockCommand(dir, name, contents) {
  const file = path.join(dir, name);
  writeFileSync(file, `#!/bin/sh\n${contents}\n`);
  chmodSync(file, 0o755);
}

// The host is a directory: ssh runs each remote command with a shell in
// root/home, so `librepaper/...` is root/home/librepaper (f.remote). Files,
// modes and renames are real. docker and rsync are mocked, and every call is
// appended to one events file, tab separated:
// kind, then the arguments.
function fixture({
  adminPassword = 'admin-secret',
  adminSeries = 12,
  adminDnsMissing = false,
  googleClientId = 'google-id',
  googleClientSecret = 'google-secret',
  googleClientIdPresent = true,
  googleClientSecretPresent = true,
  googleHttpStatus = '302',
  googleLocation = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=test',
  sopsFailureKey = '',
  manageFailure = false,
} = {}) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'librepaper-deploy-test-'));
  const bin = path.join(root, 'bin');
  const home = path.join(root, 'home');
  const remote = path.join(home, 'librepaper');
  const temp = path.join(root, 'tmp');
  mkdirSync(bin);
  mkdirSync(remote, { recursive: true });
  mkdirSync(temp);
  const adminPasswordFile = path.join(root, 'admin-password');
  const expectedAuthFile = path.join(root, 'expected-curl-auth');
  const makeCalledFile = path.join(root, 'make-called');
  const oauthCallsFile = path.join(root, 'oauth-calls');
  const curlCallsFile = path.join(root, 'curl-calls');
  const eventsFile = path.join(root, 'events');
  writeFileSync(adminPasswordFile, adminPassword);
  const escapedAdminPassword = adminPassword.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
  writeFileSync(expectedAuthFile, `user = "admin:${escapedAdminPassword}"\n`, { mode: 0o600 });

  mockCommand(bin, 'make', 'printf "make\\n" >> "$EVENTS_FILE"; printf called >> "$MAKE_CALLED_FILE"; exit 0');
  mockCommand(bin, 'dig', `
query="$2"
if [ "$ADMIN_DNS_MISSING" = 1 ] && [ "$query" = admin.librepaper.org ]; then
  exit 0
fi
printf "192.0.2.7\\n"
`);
  mockCommand(bin, 'sops', `
for arg do
  case "$arg" in
    *LIBREPAPER_GITHUB_CLIENT_ID*) printf 'github-id\\n'; exit ;;
    *LIBREPAPER_GITHUB_CLIENT_SECRET*) printf 'github-secret\\n'; exit ;;
    *LIBREPAPER_GOOGLE_CLIENT_ID*)
      if [ "$SOPS_FAILURE_KEY" = LIBREPAPER_GOOGLE_CLIENT_ID ]; then printf 'private SOPS diagnostic\\n' >&2; exit 1; fi
      [ "$GOOGLE_CLIENT_ID_PRESENT" = 1 ] || exit 0
      printf '%s' "$GOOGLE_CLIENT_ID"
      exit
      ;;
    *LIBREPAPER_GOOGLE_CLIENT_SECRET*)
      if [ "$SOPS_FAILURE_KEY" = LIBREPAPER_GOOGLE_CLIENT_SECRET ]; then printf 'private SOPS diagnostic\\n' >&2; exit 1; fi
      [ "$GOOGLE_CLIENT_SECRET_PRESENT" = 1 ] || exit 0
      printf '%s' "$GOOGLE_CLIENT_SECRET"
      exit
      ;;
    *LIBREPAPER_ADMIN_PASSWORD*) cat "$ADMIN_PASSWORD_FILE"; exit ;;
    *LIBREPAPER_RESTIC_PASSWORD*) printf 'restic-password\\n'; exit ;;
    *LIBREPAPER_HEALTHCHECK_BACKUP_URL*) printf 'https://hc-ping.example/backup-uuid\\n'; exit ;;
    *LIBREPAPER_HEALTHCHECK_CHECK_URL*) printf 'https://hc-ping.example/check-uuid\\n'; exit ;;
    *LIBREPAPER_OBJECTS_S3_ENDPOINT*) printf 'https://objects.example\\n'; exit ;;
    *LIBREPAPER_OBJECTS_S3_REGION*) printf 'objects-region\\n'; exit ;;
    *LIBREPAPER_OBJECTS_S3_BUCKET*) printf 'objects-bucket\\n'; exit ;;
    *LIBREPAPER_BACKUPS_S3_ENDPOINT*) printf 'https://backups.example\\n'; exit ;;
    *LIBREPAPER_BACKUPS_S3_REGION*) printf 'backups-region\\n'; exit ;;
    *LIBREPAPER_BACKUPS_S3_BUCKET*) printf 'backups-bucket\\n'; exit ;;
    *LIBREPAPER_MIRROR_S3_REGION*) printf 'mirror-region\\n'; exit ;;
    *LIBREPAPER_MIRROR_S3_BUCKET*) printf 'mirror-bucket\\n'; exit ;;
    *LIBREPAPER_BACKUPS_S3_ACCESS_KEY_ID*) printf 'backup-key-id\\n'; exit ;;
    *LIBREPAPER_BACKUPS_S3_SECRET_ACCESS_KEY*) printf 'backup-secret\\n'; exit ;;
    *LIBREPAPER_OBJECTS_S3_ACCESS_KEY_ID*) printf 'objects-key-id\\n'; exit ;;
    *LIBREPAPER_OBJECTS_S3_SECRET_ACCESS_KEY*) printf 'objects-secret\\n'; exit ;;
  esac
done
exit 1`);
  // rsync only records its arguments; the deploy syncs just the landing site.
  mockCommand(bin, 'rsync', `
set -eu
printf 'rsync' >> "$EVENTS_FILE"
for arg do printf '\\t%s' "$arg" >> "$EVENTS_FILE"; done
printf '\\n' >> "$EVENTS_FILE"
last=
for arg do last="$arg"; done
case "$last" in
  *:librepaper/site/) mkdir -p "$REMOTE_ROOT/site" ;;
  *) echo "unexpected rsync destination: $last" >&2; exit 99 ;;
esac`);
  // The release tarball is not real: tar prints a fake manage that records its
  // arguments and fails when the fixture says so.
  mockCommand(bin, 'tar', `
set -eu
printf 'tar\\t%s\\n' "$*" >> "$EVENTS_FILE"
cat > /dev/null
cat <<'MANAGE'
#!/bin/sh
printf 'manage\\t%s\\n' "$*" >> "$EVENTS_FILE"
[ "$MANAGE_FAILURE" = 0 ]
MANAGE`);
  // A credential in ssh arguments fails the call. Every other remote command
  // really runs, in the fake home, so stdin, modes and renames are exercised.
  mockCommand(bin, 'ssh', `
set -eu
while [ "$#" -gt 0 ]; do
  case "$1" in
    -*) shift ;;
    *) break ;;
  esac
done
command="$2"
case "$*" in *admin-secret*|*github-id*|*github-secret*|*google-id*|*google-secret*|*restic-password*|*backup-key-id*|*backup-secret*|*objects-key-id*|*objects-secret*) exit 91 ;; esac
printf 'ssh\\t%s\\n' "$command" >> "$EVENTS_FILE"
cd "$FAKE_HOME"
exec bash -c "$command"`);
  // The calls run from the host's librepaper directory, as after
  // `cd librepaper &&`; image prune runs from the home directory.
  mockCommand(bin, 'docker', `
set -eu
printf 'docker\\t%s\\n' "$*" >> "$EVENTS_FILE"
case "$*" in
  'image prune -f') ;;
  'compose logs --tail 50') ;;
  *) echo "unexpected docker command: $*" >&2; exit 99 ;;
esac`);
  mockCommand(bin, 'curl', `
set -eu
out= format= url= config= followed=no connect_timeout= max_time=
for arg do case "$arg" in *admin-secret*|*github-secret*|*google-id*|*google-secret*) exit 92 ;; esac; done
while [ "$#" -gt 0 ]; do
  case "$1" in
    --config) config="$2"; shift 2 ;;
    --connect-timeout) connect_timeout="$2"; shift 2 ;;
    --max-time) max_time="$2"; shift 2 ;;
    -o) out="$2"; shift 2 ;;
    -w) format="$2"; shift 2 ;;
    -L|--location) followed=yes; shift ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
printf '%s\t%s\t%s\n' "$url" "$connect_timeout" "$max_time" >> "$CURL_CALLS_FILE"
case "$url" in
  */librepaper-deploy.tar.gz) printf 'curl\\t%s\\n' "$url" >> "$EVENTS_FILE"; printf 'tarball'; exit 0 ;;
esac
if [ -n "$config" ]; then
  [ "$(stat -c %a "$config")" = 600 ]
  grep -Fxq -f "$EXPECTED_AUTH_FILE" "$config"
fi
code=200 body='' location= provider=
case "$url" in
  https://admin.librepaper.org/data?points=*)
    if [ -n "$config" ]; then
      [ "$(stat -c %a "$config")" = 600 ]
      grep -Fxq -f "$EXPECTED_AUTH_FILE" "$config"
      series=
      for i in $(seq 1 "$ADMIN_SERIES"); do
        entry='{"name":"series'"$i"'","label":"Series '"$i"'","unit":"count","values":[0.0]}'
        if [ -z "$series" ]; then series="$entry"; else series="$series,$entry"; fi
      done
      body='{"from":1791300000,"to":1791386400,"step":144.0,"series":['"$series"']}'
    else code=401; fi
    ;;
  https://app.librepaper.org/api/status) code=403 ;;
  https://app.librepaper.org/metrics|https://app.librepaper.org/data|https://app.librepaper.org/graphs) code=404 ;;
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
  mockCommand(bin, 'sleep', 'exit 0');

  return {
    root,
    home,
    remote,
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      HOST: sshHost,
      FAKE_HOME: home,
      REMOTE_ROOT: remote,
      EVENTS_FILE: eventsFile,
      MANAGE_FAILURE: manageFailure ? '1' : '0',
      ADMIN_DNS_MISSING: adminDnsMissing ? '1' : '0',
      TMPDIR: temp,
      ADMIN_PASSWORD_FILE: adminPasswordFile,
      EXPECTED_AUTH_FILE: expectedAuthFile,
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
      ADMIN_SERIES: String(adminSeries),
    },
    eventsFile,
    makeCalledFile,
    oauthCallsFile,
    curlCallsFile,
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

function run(f, ...args) {
  return spawnSync(deploy, args, { cwd: repo, env: f.env, encoding: 'utf8' });
}

// One entry per mock call, in order: [kind, ...arguments].
function events(f) {
  if (!existsSync(f.eventsFile)) return [];
  return readFileSync(f.eventsFile, 'utf8').split('\n').filter(Boolean).map((line) => line.split('\t'));
}

function dockerCalls(f) {
  return events(f).filter(([kind]) => kind === 'docker').map(([, call]) => call);
}

function indexOfEvent(list, [kind, pattern]) {
  return list.findIndex(([k, ...rest]) => k === kind && pattern.test(rest.join('\t')));
}

function assertInOrder(list, steps) {
  let previous = -1;
  for (const step of steps) {
    const at = indexOfEvent(list, step);
    assert.notEqual(at, -1, `missing ${step[0]} ${step[1]}`);
    assert.ok(at > previous, `${step[0]} ${step[1]} ran out of order`);
    previous = at;
  }
}

function walk(dir, base = dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    return entry.isDirectory() ? walk(full, base) : [path.relative(base, full)];
  });
}

function writeTree(dir, files) {
  for (const [name, contents] of Object.entries(files)) {
    const file = path.join(dir, name);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, contents);
  }
}

function read(...parts) {
  return readFileSync(path.join(...parts), 'utf8');
}

test('deploy renders the credentials into the candidates only, never in argv or output', () => {
  const adminPassword = 'unsafe$#admin-secret with "quotes" and back\\slash';
  const f = fixture({ adminPassword });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /20 values decrypted from keys.yaml/);

    const values = { ...mockCredentials, ...mockSettings, LIBREPAPER_ADMIN_PASSWORD: adminPassword };
    const config = read(f.remote, 'librepaper.toml.candidate');
    assert.equal(config, renderExpected(productionToml, values));
    assert.ok(config.includes('password = "unsafe$#admin-secret with \\"quotes\\" and back\\\\slash"\n'), 'the admin password is escaped for TOML');
    assert.equal(read(f.remote, 'resticprofile.toml.candidate'), renderExpected(productionResticProfile, values));

    // The values live in the two candidates and nowhere else on the host.
    const secretValues = Object.values({ ...mockCredentials, LIBREPAPER_ADMIN_PASSWORD: adminPassword });
    for (const file of walk(f.remote)) {
      if (file.endsWith('.candidate')) continue;
      const text = read(f.remote, file);
      for (const value of secretValues) assert.ok(!text.includes(value), `${file} contains a credential`);
    }
    // Nor in any command line, nor in what the helper prints.
    const visible = `${result.stdout}${result.stderr}${readFileSync(f.eventsFile, 'utf8')}`;
    for (const value of secretValues) assert.ok(!visible.includes(value), 'a credential reached argv or output');
  } finally {
    f.cleanup();
  }
});

test('a control character in a key stops the deploy before any remote call, naming the key but not the value', () => {
  const f = fixture({ googleClientSecret: 'google\tsecret' });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /LIBREPAPER_GOOGLE_CLIENT_SECRET in tools\/deploy\/keys\.yaml contains a control character/);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /google\tsecret/);
    assert.equal(existsSync(f.makeCalledFile), false, 'ran make');
    assert.deepEqual(readdirSync(f.remote), []);
    assert.deepEqual(events(f), [], 'called the host');
  } finally {
    f.cleanup();
  }
});

test('deploy writes compose.override.yaml with only the site mount', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(read(f.remote, 'compose.override.yaml'), [
      'services:',
      '  caddy:',
      '    volumes:',
      '      - ./site:/srv/site:ro',
      '',
    ].join('\n'));
    assert.equal(statSync(path.join(f.remote, 'compose.override.yaml')).mode & 0o777, 0o644, 'compose.override.yaml');
    assert.deepEqual(walk(f.remote).filter((file) => file.endsWith('.tmp')), []);
  } finally {
    f.cleanup();
  }
});

test('deploy fetches manage, uploads the candidates, then runs the upgrade, then prunes and verifies', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /Done: v0\.0\.24 is live/);
    assertInOrder(events(f), [
      ['make', /^/],
      ['ssh', /\.manage\.tmp/],
      ['curl', /releases\/download\/v0\.0\.24\/librepaper-deploy\.tar\.gz$/],
      ['tar', /^xzO librepaper\/manage$/],
      ['ssh', /librepaper\.toml\.candidate\.tmp/],
      ['ssh', /resticprofile\.toml\.candidate\.tmp/],
      ['rsync', /librepaper\/site\/$/],
      ['ssh', /compose\.override\.yaml\.tmp/],
      ['ssh', /librepaper-org\.caddy\.tmp/],
      ['ssh', /^cd librepaper && \.\/manage upgrade v0\.0\.24$/],
      ['manage', /^upgrade v0\.0\.24$/],
      ['docker', /^image prune -f$/],
      ['docker', /^compose logs --tail 50$/],
    ]);
    assert.equal(statSync(path.join(f.remote, 'manage')).mode & 0o777, 0o755);
    assert.equal(existsSync(path.join(f.remote, '.manage.tmp')), false);
    // The deploy no longer touches the kit, the installed configs or the stack.
    for (const name of ['compose.yaml', 'librepaper.toml', 'resticprofile.toml', 'caddy/Caddyfile']) {
      assert.equal(existsSync(path.join(f.remote, name)), false, name);
    }
    for (const call of dockerCalls(f)) assert.doesNotMatch(call, /^compose (pull|up|stop|down|restart|exec|run)\b/, call);
  } finally {
    f.cleanup();
  }
});

test('the candidates land on the host with mode 444 and the rendered content', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(read(f.remote, 'librepaper.toml.candidate'), expectedConfig);
    assert.equal(read(f.remote, 'resticprofile.toml.candidate'), expectedResticProfile);
    assert.equal(statSync(path.join(f.remote, 'librepaper.toml.candidate')).mode & 0o777, 0o444);
    assert.equal(statSync(path.join(f.remote, 'resticprofile.toml.candidate')).mode & 0o777, 0o444);
    assert.deepEqual(walk(f.remote).filter((file) => file.endsWith('.tmp')), []);
  } finally {
    f.cleanup();
  }
});

test('the site and redirect blocks go to caddy/local.d/librepaper-org.caddy, once, however often it deploys', () => {
  const f = fixture();
  try {
    writeTree(f.remote, { 'caddy/local.d/other.caddy': 'another app\n' });
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const result = run(f, 'deploy', 'v0.0.24');
      assert.equal(result.status, 0, result.stderr || result.stdout);
    }
    const file = path.join(f.remote, 'caddy/local.d/librepaper-org.caddy');
    assert.equal(read(file), [
      'librepaper.org {',
      '\tencode zstd gzip',
      '\troot * /srv/site',
      '\tfile_server',
      '}',
      '',
      'www.librepaper.org, librepaper.com, www.librepaper.com {',
      '\tredir https://librepaper.org{uri} permanent',
      '}',
      '',
    ].join('\n'));
    assert.equal(statSync(file).mode & 0o777, 0o644);
    assert.equal(read(f.remote, 'caddy/local.d/other.caddy'), 'another app\n');
    assert.equal(existsSync(path.join(f.remote, 'caddy/Caddyfile')), false);
  } finally {
    f.cleanup();
  }
});

test('a failing manage upgrade stops the deploy before verify and says what to do', () => {
  const f = fixture({ manageFailure: true });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /upgrade to v0\.0\.24 failed; if validation failed nothing was switched; run \.\/manage status/);
    assert.ok(indexOfEvent(events(f), ['manage', /^upgrade v0\.0\.24$/]) !== -1);
    assert.deepEqual(dockerCalls(f), [], 'pruned or verified after the failure');
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /Done:|App readiness/);
  } finally {
    f.cleanup();
  }
});

test('invalid or undecryptable Google credentials stop the deploy before the site build or any remote change', () => {
  const badCredentials = [
    { name: 'missing Google client ID', options: { googleClientIdPresent: false }, key: 'LIBREPAPER_GOOGLE_CLIENT_ID' },
    { name: 'null Google client secret', options: { googleClientSecret: 'null' }, key: 'LIBREPAPER_GOOGLE_CLIENT_SECRET' },
    { name: 'empty Google client ID', options: { googleClientId: '' }, key: 'LIBREPAPER_GOOGLE_CLIENT_ID' },
    { name: 'whitespace-only Google client secret', options: { googleClientSecret: '  \t' }, key: 'LIBREPAPER_GOOGLE_CLIENT_SECRET' },
    { name: 'Google client secret decryption failure', options: { sopsFailureKey: 'LIBREPAPER_GOOGLE_CLIENT_SECRET' }, key: 'LIBREPAPER_GOOGLE_CLIENT_SECRET' },
  ];
  for (const scenario of badCredentials) {
    const f = fixture(scenario.options);
    try {
      const result = run(f, 'deploy', 'v0.0.24');
      assert.notEqual(result.status, 0, `${scenario.name} unexpectedly succeeded`);
      assert.match(result.stderr, new RegExp(scenario.key));
      assert.doesNotMatch(`${result.stdout}${result.stderr}`, /google-id|google-secret|private SOPS diagnostic/);
      assert.equal(existsSync(f.makeCalledFile), false, `ran make for ${scenario.name}`);
      assert.deepEqual(readdirSync(f.remote), [], `mutated the remote for ${scenario.name}`);
      assert.deepEqual(events(f), [], `called the host for ${scenario.name}`);
    } finally {
      f.cleanup();
    }
  }
});

test('deploy refuses a release below the v0.0.24 floor, and a non-canonical tag, before writing remotely', () => {
  for (const [version, message] of [
    ['v0.0.8', /requires v0\.0\.24 or later.*squashed migration history/],
    ['v0.0.21', /requires v0\.0\.24 or later.*squashed migration history/],
    ['v0.0.23', /requires v0\.0\.24 or later.*squashed migration history/],
    ['0.0.22', /canonical tag/],
    ['v0.0.022', /canonical tag/],
  ]) {
    const f = fixture();
    try {
      const result = run(f, 'deploy', version);
      assert.notEqual(result.status, 0, `${version} unexpectedly deployed`);
      assert.match(result.stderr, message, version);
      assert.deepEqual(readdirSync(f.remote), [], version);
      assert.deepEqual(events(f), [], version);
    } finally {
      f.cleanup();
    }
  }
});

test('deploy fails at DNS check when admin.librepaper.org is missing, mentions the admin name, and makes no remote change', () => {
  const f = fixture({ adminDnsMissing: true });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /admin\.librepaper\.org.*does not resolve/);
    assert.deepEqual(readdirSync(f.remote), []);
    assert.deepEqual(events(f), []);
  } finally {
    f.cleanup();
  }
});

test('verify checks both OAuth providers with unfollowed matching authorization redirects', () => {
  const f = fixture();
  try {
    const result = run(f, 'verify');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /github OAuth redirect answers/i);
    assert.match(result.stdout, /google OAuth redirect answers/i);
    const calls = read(f.oauthCallsFile).trim().split('\n');
    assert.deepEqual(calls, ['github\tno', 'google\tno']);
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
      const result = run(f, 'verify');
      assert.notEqual(result.status, 0, `${scenario.name} unexpectedly passed`);
      assert.match(result.stderr, /google OAuth/i);
      assert.doesNotMatch(`${result.stdout}${result.stderr}`, /accounts\.google\.com|github\.com/);
    } finally {
      f.cleanup();
    }
  }
});

test('verify bounds every HTTP request and passes the readiness probe its remaining deadline', () => {
  const f = fixture();
  try {
    const result = run(f, 'verify');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const requests = read(f.curlCallsFile).trim().split('\n').map((line) => line.split('\t'));
    assert.ok(requests.length > 0);
    for (const [url, connectTimeout, maxTime] of requests) {
      assert.ok(Number(connectTimeout) > 0, `${url} is missing a connect timeout`);
      assert.ok(Number(maxTime) > 0, `${url} is missing a total timeout`);
    }
    const readiness = requests.find(([url]) => url === 'https://app.librepaper.org/ready');
    assert.ok(readiness, 'readiness probe was not recorded');
    assert.ok(Number(readiness[1]) > 0 && Number(readiness[1]) <= 5, `unexpected readiness connect timeout ${readiness[1]}`);
    assert.ok(Number(readiness[2]) > 0 && Number(readiness[2]) <= 120, `unexpected readiness total timeout ${readiness[2]}`);
  } finally {
    f.cleanup();
  }
});

test('verify checks the operator URLs listed in caddy/local.d/check-urls', () => {
  const f = fixture();
  try {
    writeTree(f.remote, { 'caddy/local.d/check-urls': '# the other app\n\nhttps://other.example/health\n' });
    const result = run(f, 'verify');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /https:\/\/other\.example\/health answers/);
    const urls = read(f.curlCallsFile).trim().split('\n').map((line) => line.split('\t')[0]);
    assert.ok(urls.includes('https://other.example/health'));
  } finally {
    f.cleanup();
  }
});

test('verify checks admin graphs returns twelve series', () => {
  const f = fixture();
  try {
    const result = run(f, 'verify');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /authenticated endpoint returns twelve series/);
    assert.match(result.stdout, /unauthenticated endpoint is 401/);
    assert.match(result.stdout, /app origin does not serve graphs or data/);
  } finally {
    f.cleanup();
  }
});

test('verify fails when admin graphs returns fewer than twelve series', () => {
  const f = fixture({ adminSeries: 11 });
  try {
    const result = run(f, 'verify');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /admin graphs endpoint returned 11 series, expected 12/);
  } finally {
    f.cleanup();
  }
});
