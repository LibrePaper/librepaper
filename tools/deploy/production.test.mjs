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

// What the kit sync delivers to the host: the previous release's tags, which
// the helper must pin to the deployed version.
const KIT_COMPOSE = [
  'services:',
  '  librepaper:',
  '    image: ghcr.io/librepaper/librepaper:v0.0.20',
  '  backup:',
  '    image: ghcr.io/librepaper/librepaper-backup:v0.0.20',
  '  caddy:',
  '    image: caddy:2.11.4-alpine',
  '',
].join('\n');

// Paths the archive never contains and a sync must neither send nor delete.
const OPERATOR_PATHS = [
  '/librepaper.toml*',
  '/resticprofile.toml',
  '/compose.override.yaml*',
  '/caddy/Caddyfile*',
  '/caddy/local.d',
  '/site',
];
// The image build context stays in the repository.
const BUILD_ONLY_PATHS = ['/Dockerfile', '/backup'];

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
  caddyReloadFailure = false,
  googleClientId = 'google-id',
  googleClientSecret = 'google-secret',
  googleClientIdPresent = true,
  googleClientSecretPresent = true,
  googleHttpStatus = '302',
  googleLocation = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=test',
  sopsFailureKey = '',
  configCheckFailure = false,
  caddyCheckFailure = false,
  composeYaml = KIT_COMPOSE,
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
  const composeUpFile = path.join(root, 'compose-up');
  const eventsFile = path.join(root, 'events');
  const caddyTimeoutFile = path.join(root, 'caddy-timeout');
  const kitComposeFile = path.join(root, 'kit-compose.yaml');
  const checkedConfigFile = path.join(root, 'checked-librepaper.toml');
  const runningConfigFile = path.join(root, 'running-librepaper.toml');
  const realTimeout = process.env.PATH.split(path.delimiter)
    .map((directory) => path.join(directory, 'timeout'))
    .find(existsSync);
  writeFileSync(adminPasswordFile, adminPassword);
  const escapedAdminPassword = adminPassword.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
  writeFileSync(expectedAuthFile, `user = "admin:${escapedAdminPassword}"\n`, { mode: 0o600 });
  writeFileSync(kitComposeFile, composeYaml);
  writeFileSync(runningConfigFile, 'previous container config\n');

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
  // rsync only records its arguments and delivers the kit's compose.yaml. A
  // test that needs the real filter semantics replays the recorded arguments
  // against a local tree.
  mockCommand(bin, 'rsync', `
set -eu
printf 'rsync' >> "$EVENTS_FILE"
for arg do printf '\\t%s' "$arg" >> "$EVENTS_FILE"; done
printf '\\n' >> "$EVENTS_FILE"
last=
for arg do last="$arg"; done
destination="$last"
case "$destination" in
  *:librepaper/site/)
    mkdir -p "$REMOTE_ROOT/site"
    ;;
  *:librepaper/)
    mkdir -p "$REMOTE_ROOT"
    cp "$KIT_COMPOSE_FILE" "$REMOTE_ROOT/compose.yaml"
    ;;
  *) echo "unexpected rsync destination: $destination" >&2; exit 99 ;;
esac`);
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
  'compose pull') ;;
  'compose up -d --wait --wait-timeout 180 --remove-orphans')
    cp -f librepaper.toml "$RUNNING_CONFIG_FILE"
    printf called >> "$COMPOSE_UP_FILE"
    ;;
  'compose up -d --force-recreate --no-deps --wait --wait-timeout 180 librepaper backup')
    cp -f librepaper.toml "$RUNNING_CONFIG_FILE"
    ;;
  'compose run --rm --no-deps -v ./librepaper.toml.candidate:/etc/librepaper/librepaper.toml:ro librepaper admin config check --config /etc/librepaper/librepaper.toml')
    [ -f librepaper.toml.candidate ] || exit 96
    cp -f librepaper.toml.candidate "$CHECKED_CONFIG_FILE"
    [ "$CONFIG_CHECK_FAILURE" = 0 ]
    ;;
  'compose run --rm --no-deps caddy caddy validate --config /etc/caddy/Caddyfile.candidate --adapter caddyfile')
    [ -f caddy/Caddyfile.candidate ] || exit 96
    [ "$CADDY_CHECK_FAILURE" = 0 ]
    ;;
  'compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile')
    [ "$CADDY_RELOAD_FAILURE" = 0 ]
    ;;
  'compose logs --tail 50') ;;
  *) echo "unexpected docker command: $*" >&2; exit 99 ;;
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
      KIT_COMPOSE_FILE: kitComposeFile,
      CHECKED_CONFIG_FILE: checkedConfigFile,
      RUNNING_CONFIG_FILE: runningConfigFile,
      COMPOSE_UP_FILE: composeUpFile,
      CADDY_TIMEOUT_FILE: caddyTimeoutFile,
      REAL_TIMEOUT: realTimeout,
      CADDY_RELOAD_FAILURE: caddyReloadFailure ? '1' : '0',
      CONFIG_CHECK_FAILURE: configCheckFailure ? '1' : '0',
      CADDY_CHECK_FAILURE: caddyCheckFailure ? '1' : '0',
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
    composeUpFile,
    caddyTimeoutFile,
    checkedConfigFile,
    runningConfigFile,
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

// Calls that touch the running stack: none may happen before the candidates pass.
const STACK_CALL = /^compose (pull|up|stop|down|restart|exec)\b|^image /;

function stackCalls(f) {
  return dockerCalls(f).filter((call) => STACK_CALL.test(call));
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

test('deploy renders the credentials into the host config files only, never in argv or output', () => {
  const adminPassword = 'unsafe$#admin-secret with "quotes" and back\\slash';
  const f = fixture({ adminPassword });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /20 values decrypted from keys.yaml/);

    const values = { ...mockCredentials, ...mockSettings, LIBREPAPER_ADMIN_PASSWORD: adminPassword };
    const config = read(f.remote, 'librepaper.toml');
    assert.equal(config, renderExpected(productionToml, values));
    assert.ok(config.includes('password = "unsafe$#admin-secret with \\"quotes\\" and back\\\\slash"\n'), 'the admin password is escaped for TOML');
    assert.equal(statSync(path.join(f.remote, 'librepaper.toml')).mode & 0o777, 0o444, 'librepaper.toml');
    const profile = read(f.remote, 'resticprofile.toml');
    assert.equal(profile, renderExpected(productionResticProfile, values));
    assert.equal(statSync(path.join(f.remote, 'resticprofile.toml')).mode & 0o777, 0o444, 'resticprofile.toml');

    // The values live in librepaper.toml and resticprofile.toml, and nowhere else on the host.
    const secretValues = Object.values({ ...mockCredentials, LIBREPAPER_ADMIN_PASSWORD: adminPassword });
    for (const file of walk(f.remote)) {
      if (file === 'librepaper.toml' || file === 'resticprofile.toml') continue;
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

test('deploy installs the checked candidates, then pulls and starts the stack in order, stopping nothing by hand', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /Done: v0\.0\.24 is live/);
    assertInOrder(events(f), [
      ['make', /^/],
      ['rsync', /--delete/],
      ['ssh', /^chmod 700 librepaper$/],
      ['ssh', /sed -i -E/],
      ['ssh', /\.resticprofile\.toml\.tmp/],
      ['ssh', /compose\.override\.yaml\.tmp/],
      ['ssh', /librepaper\.toml\.candidate\.tmp/],
      ['ssh', /Caddyfile\.candidate\.tmp/],
      ['docker', /admin config check/],
      ['docker', /caddy validate/],
      ['ssh', /mv -f librepaper\.toml\.candidate librepaper\.toml/],
      ['ssh', /mv -f caddy\/Caddyfile\.candidate caddy\/Caddyfile/],
      ['docker', /^compose pull$/],
      ['docker', /^compose up -d --wait --wait-timeout 180 --remove-orphans$/],
      ['docker', /^compose up -d --force-recreate --no-deps --wait --wait-timeout 180 /],
      ['docker', /caddy reload/],
      ['docker', /^image prune -f$/],
      ['docker', /^compose logs --tail 50$/],
    ]);
    assert.equal(read(f.remote, 'librepaper.toml'), expectedConfig);
    assert.equal(read(f.remote, 'resticprofile.toml'), expectedResticProfile);
    assert.equal(read(f.runningConfigFile), expectedConfig);
    assert.match(read(f.remote, 'caddy/Caddyfile'), /librepaper\.org/);
    assert.equal(existsSync(path.join(f.remote, 'librepaper.toml.candidate')), false);
    assert.equal(existsSync(path.join(f.remote, 'caddy/Caddyfile.candidate')), false);
    assert.equal(read(f.root, 'caddy-timeout').trim(), '30');
    // The one forced recreate names exactly the services that bind-mount a file the deploy replaces.
    const recreates = dockerCalls(f).filter((call) => call.includes('--force-recreate'));
    assert.equal(recreates.length, 1);
    assert.ok(recreates[0].includes('--no-deps'));
    assert.deepEqual(
      recreates[0].split(' ').slice(2).filter((arg) => !arg.startsWith('-') && !/^\d+$/.test(arg)),
      ['librepaper', 'backup'],
    );
    // The server migrates at startup: no setup, no migrate, no manual stop, no volume or system cleanup.
    for (const call of dockerCalls(f)) {
      assert.doesNotMatch(call, /^compose (stop|down|restart|rm|kill)\b|volume|system|migrate/, call);
    }
    for (const [kind, text] of events(f)) {
      if (kind === 'ssh') assert.doesNotMatch(text, /setup|migrate/, text);
    }
  } finally {
    f.cleanup();
  }
});

test('deploy pins the tag into compose.yaml before it pulls, and a line the sed leaves alone stops it', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(read(f.remote, 'compose.yaml'), KIT_COMPOSE.replaceAll('v0.0.20', 'v0.0.24'));
    assertInOrder(events(f), [['ssh', /sed -i -E/], ['docker', /^compose pull$/]]);
  } finally {
    f.cleanup();
  }

  for (const [name, composeYaml] of [
    ['a backup image on latest', KIT_COMPOSE.replace('librepaper-backup:v0.0.20', 'librepaper-backup:latest')],
    ['an interpolated tag', KIT_COMPOSE.replace('librepaper:v0.0.20', 'librepaper:${LIBREPAPER_VERSION:-v0.0.20}')],
    ['no image lines at all', 'services: {}\n'],
  ]) {
    const unpinned = fixture({ composeYaml });
    try {
      const result = run(unpinned, 'deploy', 'v0.0.24');
      assert.notEqual(result.status, 0, `${name} unexpectedly deployed`);
      assert.match(result.stderr, /does not pin both images to v0\.0\.24/, name);
      assert.deepEqual(dockerCalls(unpinned), [], `${name} reached docker`);
      assert.equal(existsSync(path.join(unpinned.remote, 'librepaper.toml')), false, `${name} installed a config`);
    } finally {
      unpinned.cleanup();
    }
  }
});

test('a rejected candidate config leaves the installed config and the running stack untouched', () => {
  const f = fixture({ configCheckFailure: true });
  try {
    writeTree(f.remote, {
      'librepaper.toml': 'previous production config\n',
      'caddy/Caddyfile': 'previous Caddyfile\n',
    });
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /candidate binary cannot load librepaper\.toml/);
    assert.equal(read(f.checkedConfigFile), expectedConfig, 'the candidate that was checked is the rendered production.toml');
    assert.equal(read(f.remote, 'librepaper.toml'), 'previous production config\n');
    assert.equal(read(f.remote, 'caddy/Caddyfile'), 'previous Caddyfile\n');
    assert.equal(read(f.runningConfigFile), 'previous container config\n');
    assert.equal(existsSync(f.composeUpFile), false);
    assert.deepEqual(stackCalls(f), []);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /Done:/);
  } finally {
    f.cleanup();
  }
});

test('an invalid Caddyfile candidate is rejected before the installed files change, a pull or an up', () => {
  const f = fixture({ caddyCheckFailure: true });
  try {
    writeTree(f.remote, {
      'librepaper.toml': 'previous production config\n',
      'caddy/Caddyfile': 'previous Caddyfile\n',
    });
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /candidate Caddy configuration is invalid/);
    assertInOrder(events(f), [['docker', /admin config check/], ['docker', /caddy validate/]]);
    assert.equal(read(f.remote, 'librepaper.toml'), 'previous production config\n');
    assert.equal(read(f.remote, 'caddy/Caddyfile'), 'previous Caddyfile\n');
    assert.equal(existsSync(path.join(f.remote, 'caddy/Caddyfile.candidate')), true);
    assert.equal(read(f.runningConfigFile), 'previous container config\n');
    assert.deepEqual(stackCalls(f), []);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /Done:/);
  } finally {
    f.cleanup();
  }
});

test('a failed Caddy reload is reported after the stack starts', () => {
  const f = fixture({ caddyReloadFailure: true });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Caddy configuration reload failed/);
    assert.deepEqual(dockerCalls(f).slice(-2).map((call) => call.split(' ').slice(0, 3).join(' ')), [
      'compose up -d',
      'compose exec -T',
    ]);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /Done:/);
  } finally {
    f.cleanup();
  }
});

test('repeated deploys render Caddy from its template without duplicating site and redirect blocks', () => {
  const f = fixture();
  try {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const result = run(f, 'deploy', 'v0.0.24');
      assert.equal(result.status, 0, result.stderr || result.stdout);
    }
    const caddy = read(f.remote, 'caddy/Caddyfile');
    assert.equal((caddy.match(/librepaper\.org \{/g) ?? []).length, 1);
    assert.equal((caddy.match(/www\.librepaper\.org, librepaper\.com/g) ?? []).length, 1);
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

function kitSyncArguments(f) {
  const calls = events(f).filter(([kind]) => kind === 'rsync').map(([, ...args]) => args);
  const kit = calls.find((args) => args.at(-1) === `${sshHost}:librepaper/`);
  assert.ok(kit, 'the kit sync was not recorded');
  return { calls, kit };
}

test('the kit sync deletes dropped kit files and neither sends nor removes the operator paths', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const { kit } = kitSyncArguments(f);
    assert.equal(kit.at(-2), 'deploy/');
    assert.ok(kit.includes('--delete'));
    const excluded = kit.filter((arg) => arg.startsWith('--exclude=')).map((arg) => arg.slice('--exclude='.length));
    assert.deepEqual(excluded.sort(), [...OPERATOR_PATHS, ...BUILD_ONLY_PATHS].sort());
    const protectedPaths = kit.filter((arg) => arg.startsWith('--filter=')).map((arg) => arg.slice('--filter='.length));
    assert.deepEqual(protectedPaths.sort(), OPERATOR_PATHS.map((p) => `P ${p}`).sort());
    assert.ok(!events(f).some(([kind, ...args]) => kind === 'rsync' && args.at(-1) === `${sshHost}:librepaper/resticprofile.toml`), 'resticprofile.toml comes from tools/deploy/production-resticprofile.toml, not the kit');
    assertInOrder(events(f), [['rsync', /--delete/], ['ssh', /^chmod 700 librepaper$/]]);
    assert.equal(statSync(f.remote).mode & 0o777, 0o700);
  } finally {
    f.cleanup();
  }
});

const realRsync = spawnSync('rsync', ['--version'], { encoding: 'utf8' });

test('the recorded kit sync arguments keep operator files and drop deleted kit files under real rsync', {
  skip: realRsync.status !== 0 ? 'rsync is not installed' : false,
}, () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const { kit } = kitSyncArguments(f);
    const source = path.join(f.root, 'kit-source');
    const target = path.join(f.root, 'kit-target');
    writeTree(source, {
      'compose.yaml': 'kit compose\n',
      'librepaper.toml': 'kit sample config\n',
      'resticprofile.toml': 'kit sample backup\n',
      'caddy/Caddyfile': 'kit Caddyfile\n',
      'postgres/init.sql': 'kit sql\n',
      Dockerfile: 'FROM scratch\n',
      'backup/entrypoint.sh': 'build context\n',
    });
    const operatorFiles = {
      'librepaper.toml': 'operator config\n',
      'librepaper.toml.candidate': 'operator candidate\n',
      'resticprofile.toml': 'operator backup\n',
      'compose.override.yaml': 'operator override\n',
      'caddy/Caddyfile': 'operator Caddyfile\n',
      'caddy/Caddyfile.candidate': 'operator Caddyfile candidate\n',
      'caddy/local.d/other.caddy': 'another app\n',
      'site/index.html': 'the site\n',
    };
    writeTree(target, {
      ...operatorFiles,
      'compose.yaml': 'old compose, longer so rsync cannot mistake it for the kit file\n',
      'old-kit-file': 'dropped from the kit\n',
    });
    const sync = spawnSync('rsync', [...kit.slice(0, -2), `${source}/`, `${target}/`], { encoding: 'utf8' });
    assert.equal(sync.status, 0, sync.stderr);
    assert.deepEqual(walk(target).sort(), [
      ...Object.keys(operatorFiles),
      'compose.yaml',
      'postgres/init.sql',
    ].sort());
    for (const [file, contents] of Object.entries(operatorFiles)) assert.equal(read(target, file), contents, file);
    assert.equal(read(target, 'compose.yaml'), 'kit compose\n');
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
