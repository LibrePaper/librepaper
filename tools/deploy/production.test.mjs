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
  LIBREPAPER_OBJECTS_S3_ACCESS_KEY_ID: 'objects-key-id',
  LIBREPAPER_OBJECTS_S3_SECRET_ACCESS_KEY: 'objects-secret',
  LIBREPAPER_RESTIC_PASSWORD: 'restic-password',
  LIBREPAPER_BACKUPS_S3_ACCESS_KEY_ID: 'backup-key-id',
  LIBREPAPER_BACKUPS_S3_SECRET_ACCESS_KEY: 'backup-secret',
  LIBREPAPER_REPLICA_READONLY_S3_ACCESS_KEY_ID: 'replica-key-id',
  LIBREPAPER_REPLICA_READONLY_S3_SECRET_ACCESS_KEY: 'replica-secret',
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
  LIBREPAPER_ADMIN_GITHUB_IDS: '987057',
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
// The server's public address, and the DNS records the dig mock serves: one
// "name TYPE address" line each. Every deployed name resolves to it.
const productionAddress = '15.235.24.221';
const deployNames = ['librepaper.org', 'app.librepaper.org', 'docs.librepaper.org', 'admin.librepaper.org', 'www.librepaper.org', 'librepaper.com', 'www.librepaper.com'];
function defaultDnsRecords() {
  return deployNames.map((name) => `${name} A ${productionAddress}`);
}

const defaultSshdT = [
  'passwordauthentication no',
  'kbdinteractiveauthentication no',
  'pubkeyauthentication yes',
  'permitrootlogin prohibit-password',
];
const defaultListeners = [
  'tcp LISTEN 0 4096 0.0.0.0:22 0.0.0.0:*',
  'tcp LISTEN 0 4096 0.0.0.0:443 0.0.0.0:*',
  'tcp LISTEN 0 4096 [::]:443 [::]:*',
  'tcp LISTEN 0 4096 0.0.0.0:80 0.0.0.0:*',
  'tcp LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:*',
  'udp UNCONN 0 0 [::]:41641 [::]:*',
  'udp UNCONN 0 0 203.0.113.10%ens3:68 0.0.0.0:*',
  'tcp LISTEN 0 4096 [fd7a:115c:a1e0::1]%tailscale0:34572 [::]:*',
];

function fixture({
  dnsRecords = null,
  githubClientSecret = 'github-secret',
  adminRootStatus = '302',
  adminRootLocation = 'https://github.com/login/oauth/authorize?client_id=x&redirect_uri=https%3A%2F%2Fadmin.librepaper.org%2Fauth%2Fcallback&scope=&state=y',
  adminDnsMissing = false,
  googleClientId = 'google-id',
  googleClientSecret = 'google-secret',
  googleClientIdPresent = true,
  googleClientSecretPresent = true,
  googleHttpStatus = '302',
  googleLocation = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=test',
  sopsFailureKey = '',
  manageFailure = false,
  attestationFailure = false,
  hostAddress = productionAddress,
  sshdT = defaultSshdT,
  listeners = defaultListeners,
  unattended = 'enabled',
  diskUsed = 15,
  publicAdminStatus = 403,
  replicaWritable = false,
  replicaVersionsDenied = false,
  resticNewest = new Date(Date.now() - 10 * 60 * 1000).toISOString(),
  sopsAgeKeyCmd = '',
  drillVersion = 'v0.0.49',
  drillSnapshotTime = new Date(Date.now() - 20 * 60 * 1000).toISOString().slice(0, 19).replace('T', ' '),
  drillCopyCounts = '284 1374 1601 14 1 9 1616',
  drillProdCounts = '285 1375 1601 14 1 9 1616',
  drillCopyFiles = 1616,
} = {}) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'librepaper-deploy-test-'));
  const bin = path.join(root, 'bin');
  const home = path.join(root, 'home');
  const remote = path.join(home, 'librepaper');
  const temp = path.join(root, 'tmp');
  mkdirSync(bin);
  mkdirSync(remote, { recursive: true });
  chmodSync(remote, 0o700);
  mkdirSync(temp);
  const makeCalledFile = path.join(root, 'make-called');
  const oauthCallsFile = path.join(root, 'oauth-calls');
  const curlCallsFile = path.join(root, 'curl-calls');
  const eventsFile = path.join(root, 'events');
  const dnsFile = path.join(root, 'dns');
  const zone = dnsRecords ?? defaultDnsRecords().filter((line) => !(adminDnsMissing && line.startsWith('admin.librepaper.org ')));
  writeFileSync(dnsFile, zone.map((line) => `${line}\n`).join(''));

  const sshdFile = path.join(root, 'sshd-t');
  const ssFile = path.join(root, 'ss');
  writeFileSync(sshdFile, sshdT.map((line) => `${line}\n`).join(''));
  writeFileSync(ssFile, listeners.map((line) => `${line}\n`).join(''));

  mockCommand(bin, 'make', 'printf "make\\n" >> "$EVENTS_FILE"; printf called >> "$MAKE_CALLED_FILE"; exit 0');
  mockCommand(bin, 'dig', `
name=""
type=A
for arg do
  case "$arg" in
    +*) ;;
    A | AAAA) type="$arg" ;;
    *) name="$arg" ;;
  esac
done
awk -v name="$name" -v type="$type" '$1 == name && $2 == type { print $3 }' "$DNS_ZONE"
`);
  // The fake host's addresses, as `ip -o addr show scope global` prints them.
  mockCommand(bin, 'ip', `printf '2: ens3    inet %s/32 scope global ens3\\n' "$HOST_ADDRESS"`);
  mockCommand(bin, 'sops', `
for arg do
  case "$arg" in
    *LIBREPAPER_GITHUB_CLIENT_ID*) printf 'github-id\\n'; exit ;;
    *LIBREPAPER_GITHUB_CLIENT_SECRET*) printf '%s' "$GITHUB_CLIENT_SECRET"; exit ;;
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
    *LIBREPAPER_ADMIN_GITHUB_IDS*) printf '987057\\n'; exit ;;
    *LIBREPAPER_BACKUPS_S3_ACCESS_KEY_ID*) printf 'backup-key-id\\n'; exit ;;
    *LIBREPAPER_BACKUPS_S3_SECRET_ACCESS_KEY*) printf 'backup-secret\\n'; exit ;;
    *LIBREPAPER_REPLICA_READONLY_S3_ACCESS_KEY_ID*) printf 'replica-key-id\\n'; exit ;;
    *LIBREPAPER_REPLICA_READONLY_S3_SECRET_ACCESS_KEY*) printf 'replica-secret\\n'; exit ;;
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
has_xzf=0
dir=
prev=
for arg do
  case "$arg" in -xzf) has_xzf=1 ;; esac
  if [ "$prev" = -C ]; then dir="$arg"; fi
  prev="$arg"
done
if [ "$has_xzf" = 1 ] && [ -n "$dir" ]; then
  # The drill's kit: a stub manage that records its calls and answers the drill's two commands.
  mkdir -p "$dir/librepaper"
  cat > "$dir/librepaper/manage" <<'DRILLMANAGE'
#!/bin/sh
printf 'drill-manage\\t%s\\n' "$*" >> "$EVENTS_FILE"
case "$1 $2" in
  'backup list')
    printf 'ID        Time                 Host        Tags        Paths                    Size\\n'
    printf '450fafe1  %s  librepaper  librepaper  /librepaper-backup.tar  111.607 MiB\\n' "$DRILL_SNAPSHOT_TIME"
    printf -- '--------\\n'
    ;;
  'backup restore') exit "$MANAGE_FAILURE" ;;
esac
DRILLMANAGE
  chmod 755 "$dir/librepaper/manage"
  exit 0
fi
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
case "$*" in *github-id*|*github-secret*|*google-id*|*google-secret*|*restic-password*|*backup-key-id*|*backup-secret*|*objects-key-id*|*objects-secret*) exit 91 ;; esac
printf 'ssh\\t%s\\n' "$command" >> "$EVENTS_FILE"
cd "$FAKE_HOME"
exec bash -c "$command"`);
  // The calls run from the host's librepaper directory, as after
  // `cd librepaper &&`; image prune runs from the home directory.
  mockCommand(bin, 'docker', `
set -eu
printf 'docker\\t%s\\n' "$*" >> "$EVENTS_FILE"
if [ -n "\${COMPOSE_PROJECT_NAME:-}" ]; then printf '%s\\n' "$COMPOSE_PROJECT_NAME" >> "$DRILL_PROJECT_FILE"; fi
case "$*" in
  'compose exec -T postgres psql '*)
    # The copy runs from its own directory; production's query runs from the fake home.
    case "$(pwd)" in
      "$FAKE_HOME"/*) printf '%s\\n' "$DRILL_PROD_COUNTS" ;;
      *) printf '%s\\n' "$DRILL_COPY_COUNTS" ;;
    esac
    exit 0
    ;;
  'compose exec -T librepaper sh -c '*find*) printf '%s\\n' "$DRILL_COPY_FILES"; exit 0 ;;
  'compose exec -T librepaper wget '*) echo ready; exit 0 ;;
  'compose down -v'|'compose run '*) exit 0 ;;
  'image prune -f') ;;
  'compose logs --tail 50') ;;
  'compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile') ;;
  *) echo "unexpected docker command: $*" >&2; exit 99 ;;
esac`);
  // gh downloads the kit into --dir and records each call. Verification fails
  // when the fixture says so.
  mockCommand(bin, 'gh', `
set -eu
printf 'gh\\t%s\\n' "$*" >> "$EVENTS_FILE"
case "$1 $2" in
  'release download')
    dir=
    while [ "$#" -gt 0 ]; do
      case "$1" in --dir) dir="$2"; shift 2 ;; *) shift ;; esac
    done
    printf 'kit' > "$dir/librepaper-deploy.tar.gz"
    ;;
  'attestation verify') [ "$ATTESTATION_FAILURE" = 0 ] ;;
  *) echo "unexpected gh command: $*" >&2; exit 99 ;;
esac`);
  mockCommand(bin, 'curl', `
set -eu
out= format= url= followed=no connect_timeout= max_time= resolved=
for arg do case "$arg" in *github-secret*|*google-id*|*google-secret*) exit 92 ;; esac; done
while [ "$#" -gt 0 ]; do
  case "$1" in
    --connect-timeout) connect_timeout="$2"; shift 2 ;;
    --max-time) max_time="$2"; shift 2 ;;
    -o) out="$2"; shift 2 ;;
    -w) format="$2"; shift 2 ;;
    -L|--location) followed=yes; shift ;;
    --resolve) resolved="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
printf '%s\t%s\t%s\n' "$url" "$connect_timeout" "$max_time" >> "$CURL_CALLS_FILE"
code=200 body='' location= provider=
case "$url" in
  https://admin.librepaper.org/) code="$ADMIN_ROOT_STATUS"; location="$ADMIN_ROOT_LOCATION" ;;
  https://admin.librepaper.org/data?points=*) code=401 ;;
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
if [ -n "$resolved" ] && [ "$url" = https://admin.librepaper.org/ ]; then code="$PUBLIC_ADMIN_STATUS"; fi
if [ -n "$out" ] && [ "$out" != /dev/null ]; then printf '%s' "$body" > "$out"; fi
if [ -n "$provider" ]; then printf '%s\\t%s\\n' "$provider" "$followed" >> "$OAUTH_CALLS_FILE"; fi
if [ -n "$format" ]; then
  case "$format" in
    *'%{redirect_url}'*)
      if [ "$url" = https://admin.librepaper.org/ ]; then printf '%s %s' "$code" "$location"; else printf '%s\\t%s' "$code" "$location"; fi
      ;;
    *) printf '%s' "$code" ;;
  esac
fi`);
  mockCommand(bin, 'sleep', 'exit 0');
  mockCommand(bin, 'sudo', `if [ "$*" = '-n sshd -T' ]; then cat "$SSHD_T_FILE"; else exit 1; fi`);
  mockCommand(bin, 'ss', 'cat "$SS_FILE"');
  mockCommand(bin, 'systemctl', 'echo "$UNATTENDED_STATE"');
  mockCommand(bin, 'df', `echo 'Use%'
echo "$DISK_USED%"`);
  mockCommand(bin, 'aws', `
printf 'aws\\t%s\\n' "$*" >> "$EVENTS_FILE"
case "$*" in
  *' cp '*)
    if [ "$REPLICA_WRITABLE" = 1 ]; then exit 0; fi
    echo 'upload failed: An error occurred (AccessDenied) when calling the PutObject operation: Access Denied' >&2
    exit 1
    ;;
  *list-object-versions*)
    if [ "$REPLICA_VERSIONS_DENIED" = 1 ]; then
      echo 'An error occurred (AccessDenied) when calling the ListObjectVersions operation: Access Denied' >&2
      exit 254
    fi
    ;;
esac
exit 0`);
  mockCommand(bin, 'restic', `
printf 'restic\\t%s\\n' "$*" >> "$EVENTS_FILE"
case "$1" in
  snapshots) printf '[{"time":"%s"}]\\n' "$RESTIC_NEWEST" ;;
  check) exit 0 ;;
esac`);

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
      ATTESTATION_FAILURE: attestationFailure ? '1' : '0',
      HOST_ADDRESS: hostAddress,
      DNS_ZONE: dnsFile,
      ADDRESSES: productionAddress,
      TMPDIR: temp,
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
      SOPS_AGE_KEY_CMD: sopsAgeKeyCmd,
      GITHUB_CLIENT_SECRET: String(githubClientSecret),
      ADMIN_ROOT_STATUS: String(adminRootStatus),
      ADMIN_ROOT_LOCATION: adminRootLocation,
      SSHD_T_FILE: sshdFile,
      SS_FILE: ssFile,
      UNATTENDED_STATE: String(unattended),
      DISK_USED: String(diskUsed),
      PUBLIC_ADMIN_STATUS: String(publicAdminStatus),
      REPLICA_WRITABLE: replicaWritable ? '1' : '0',
      REPLICA_VERSIONS_DENIED: replicaVersionsDenied ? '1' : '0',
      RESTIC_NEWEST: resticNewest,
      DRILL_VERSION: drillVersion,
      DRILL_SNAPSHOT_TIME: drillSnapshotTime,
      DRILL_COPY_COUNTS: drillCopyCounts,
      DRILL_PROD_COUNTS: drillProdCounts,
      DRILL_COPY_FILES: String(drillCopyFiles),
      DRILL_PROJECT_FILE: path.join(root, 'drill-projects'),
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
  const githubSecret = 'unsafe$#github-secret with "quotes" and back\\slash';
  const f = fixture({ githubClientSecret: githubSecret });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /20 values decrypted from keys.yaml/);

    const values = { ...mockCredentials, ...mockSettings, LIBREPAPER_GITHUB_CLIENT_SECRET: githubSecret };
    const config = read(f.remote, 'librepaper.toml.candidate');
    assert.equal(config, renderExpected(productionToml, values));
    assert.ok(config.includes('client_secret = "unsafe$#github-secret with \\"quotes\\" and back\\\\slash"\n'), 'the GitHub client secret is escaped for TOML');
    assert.equal(read(f.remote, 'resticprofile.toml.candidate'), renderExpected(productionResticProfile, values));

    // The values live in the two candidates and nowhere else on the host.
    const secretValues = Object.values({ ...mockCredentials, LIBREPAPER_GITHUB_CLIENT_SECRET: githubSecret });
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

test('deploy writes compose.override.yaml with the site mount and journald logs', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(read(f.remote, 'compose.override.yaml'), [
      'services:',
      '  caddy:',
      '    volumes:',
      '      - ./site:/srv/site:ro',
      '  librepaper:',
      '    logging:',
      '      driver: journald',
      '      options:',
      '        tag: "{{.Name}}"',
      '  backup:',
      '    logging:',
      '      driver: journald',
      '      options:',
      '        tag: "{{.Name}}"',
      '',
    ].join('\n'));
    assert.equal(statSync(path.join(f.remote, 'compose.override.yaml')).mode & 0o777, 0o644, 'compose.override.yaml');
    assert.deepEqual(walk(f.remote).filter((file) => file.endsWith('.tmp')), []);
  } finally {
    f.cleanup();
  }
});

test('deploy verifies the kit on the laptop, installs manage from it, upgrades from it, then prunes and verifies', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /Done: v0\.0\.24 is live/);
    assertInOrder(events(f), [
      ['make', /^/],
      ['gh', /^release download v0\.0\.24 --repo LibrePaper\/librepaper --pattern librepaper-deploy\.tar\.gz --dir /],
      ['gh', /^attestation verify \S+ --repo LibrePaper\/librepaper --signer-workflow LibrePaper\/librepaper\/\.github\/workflows\/publish-docker\.yml$/],
      ['ssh', /^umask 077; cd librepaper && cat > \.kit\.tar\.gz\.tmp && mv -f \.kit\.tar\.gz\.tmp \.kit\.tar\.gz$/],
      ['tar', /^xzOf \.kit\.tar\.gz librepaper\/manage$/],
      ['ssh', /librepaper\.toml\.candidate\.tmp/],
      ['ssh', /resticprofile\.toml\.candidate\.tmp/],
      ['rsync', /librepaper\/site\/$/],
      ['ssh', /compose\.override\.yaml\.tmp/],
      ['ssh', /librepaper-org\.caddy\.tmp/],
      ['ssh', /^cd librepaper && LIBREPAPER_KIT=\.kit\.tar\.gz \.\/manage upgrade v0\.0\.24$/],
      ['manage', /^upgrade v0\.0\.24$/],
      ['ssh', /^rm -f librepaper\/\.kit\.tar\.gz && docker image prune -f$/],
      ['docker', /^compose logs --tail 50$/],
    ]);
    assert.equal(statSync(path.join(f.remote, 'manage')).mode & 0o777, 0o755);
    assert.equal(existsSync(path.join(f.remote, '.manage.tmp')), false);
    assert.equal(existsSync(path.join(f.remote, '.kit.tar.gz')), false, 'the kit is removed from the host after the upgrade');
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
      '\theader {',
      '\t\tStrict-Transport-Security "max-age=31536000; includeSubDomains"',
      '\t\tX-Content-Type-Options nosniff',
      '\t\tReferrer-Policy strict-origin-when-cross-origin',
      '\t\tContent-Security-Policy "frame-ancestors \'none\'; base-uri \'none\'; object-src \'none\'; form-action \'self\'"',
      '\t}',
      '\tredir /install.html /desktop-app.html permanent',
      '\tredir /local-app.html /desktop-app.html permanent',
      '\troot * /srv/site',
      '\tfile_server',
      '}',
      '',
      'www.librepaper.org, librepaper.com, www.librepaper.com {',
      '\theader Strict-Transport-Security "max-age=31536000; includeSubDomains"',
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

test('site installs the Caddy site block and reloads Caddy without touching the release', () => {
  const f = fixture();
  try {
    const result = run(f, 'site');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const file = path.join(f.remote, 'caddy/local.d/librepaper-org.caddy');
    assert.equal(read(file), [
      'librepaper.org {',
      '\tencode zstd gzip',
      '\theader {',
      '\t\tStrict-Transport-Security "max-age=31536000; includeSubDomains"',
      '\t\tX-Content-Type-Options nosniff',
      '\t\tReferrer-Policy strict-origin-when-cross-origin',
      '\t\tContent-Security-Policy "frame-ancestors \'none\'; base-uri \'none\'; object-src \'none\'; form-action \'self\'"',
      '\t}',
      '\tredir /install.html /desktop-app.html permanent',
      '\tredir /local-app.html /desktop-app.html permanent',
      '\troot * /srv/site',
      '\tfile_server',
      '}',
      '',
      'www.librepaper.org, librepaper.com, www.librepaper.com {',
      '\theader Strict-Transport-Security "max-age=31536000; includeSubDomains"',
      '\tredir https://librepaper.org{uri} permanent',
      '}',
      '',
    ].join('\n'));
    assertInOrder(events(f), [
      ['ssh', /librepaper-org\.caddy\.tmp/],
      ['ssh', /caddy reload --config \/etc\/caddy\/Caddyfile/],
    ]);
    assert.deepEqual(dockerCalls(f), ['compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile'], 'site pruned or touched the release');
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

test('a kit without a valid release-workflow attestation stops the deploy before any host call', () => {
  const f = fixture({ attestationFailure: true });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /the kit for v0\.0\.24 has no valid attestation from the release workflow and nothing was changed/);
    assert.ok(indexOfEvent(events(f), ['gh', /^attestation verify /]) !== -1, 'the kit was not verified');
    assert.deepEqual(events(f).filter(([kind]) => ['ssh', 'rsync', 'docker'].includes(kind)), [], 'called the host');
    assert.deepEqual(readdirSync(f.remote), [], 'mutated the remote');
    assert.deepEqual(readdirSync(path.join(f.root, 'tmp')), [], 'left the downloaded kit behind');
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /Done:|Upgrade:/);
  } finally {
    f.cleanup();
  }
});

test('deploy refuses an ssh host that owns none of ADDRESSES, before writing remotely', () => {
  const f = fixture({ hostAddress: '192.0.2.9' });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /ubuntu@test-host has none of ADDRESSES \(15\.235\.24\.221\)/);
    assert.deepEqual(readdirSync(f.remote), [], 'mutated the remote');
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /Done:|Upgrade:/);
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
    assert.match(result.stderr, /admin\.librepaper\.org has no A record/);
    assert.deepEqual(readdirSync(f.remote), []);
    assert.deepEqual(events(f), []);
  } finally {
    f.cleanup();
  }
});

test('deploy passes the DNS check when every name resolves only to ADDRESSES', () => {
  const f = fixture();
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /names -> 15\.235\.24\.221/);
  } finally {
    f.cleanup();
  }
});

test('deploy fails at DNS check when a name has an extra A record outside ADDRESSES, before any remote call', () => {
  const f = fixture({ dnsRecords: [...defaultDnsRecords(), 'app.librepaper.org A 192.0.2.7'] });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /app\.librepaper\.org resolves to 192\.0\.2\.7, which is not in ADDRESSES/);
    assert.deepEqual(readdirSync(f.remote), []);
    assert.deepEqual(events(f), []);
  } finally {
    f.cleanup();
  }
});

test('deploy fails at DNS check when a name has a stray AAAA record, before any remote call', () => {
  const f = fixture({ dnsRecords: [...defaultDnsRecords(), 'librepaper.org AAAA 2001:db8::7'] });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /librepaper\.org resolves to 2001:db8::7, which is not in ADDRESSES/);
    assert.deepEqual(readdirSync(f.remote), []);
    assert.deepEqual(events(f), []);
  } finally {
    f.cleanup();
  }
});

test('deploy fails at DNS check when a name has no A record, before any remote call', () => {
  const f = fixture({ dnsRecords: defaultDnsRecords().filter((line) => !line.startsWith('docs.librepaper.org ')) });
  try {
    const result = run(f, 'deploy', 'v0.0.24');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /docs\.librepaper\.org has no A record/);
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

test('verify checks the admin console sends visitors to GitHub sign-in and its data endpoint is 401 without a session', () => {
  const f = fixture();
  try {
    const result = run(f, 'verify');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /a visitor is sent to GitHub sign-in/);
    assert.match(result.stdout, /unauthenticated data endpoint is 401/);
    assert.match(result.stdout, /app origin does not serve graphs or data/);
  } finally {
    f.cleanup();
  }
});

test('verify fails when the admin console does not redirect to GitHub sign-in for its own callback', () => {
  for (const scenario of [
    { name: 'answers 200', adminRootStatus: '200', adminRootLocation: '' },
    { name: 'redirects to another callback', adminRootStatus: '302', adminRootLocation: 'https://github.com/login/oauth/authorize?client_id=x&redirect_uri=https%3A%2F%2Fother.example%2Fauth%2Fcallback' },
  ]) {
    const f = fixture(scenario);
    try {
      const result = run(f, 'verify');
      assert.notEqual(result.status, 0, `${scenario.name} unexpectedly passed`);
      assert.match(result.stderr, /admin console answered HTTP/, scenario.name);
    } finally {
      f.cleanup();
    }
  }
});

test('check-host passes on a host with only the expected listeners and settings', () => {
  const f = fixture();
  try {
    const result = run(f, 'check-host');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /only SSH on IPv4/);
    assert.match(result.stdout, /answers 403 off the tailnet/);
  } finally {
    f.cleanup();
  }
});

test('check-host fails on SSH over IPv6 or a stray published port', () => {
  const f = fixture({
    listeners: [...defaultListeners, 'tcp LISTEN 0 4096 [::]:22 [::]:*', 'tcp LISTEN 0 4096 0.0.0.0:8000 0.0.0.0:*'],
  });
  try {
    const result = run(f, 'check-host');
    assert.notEqual(result.status, 0);
    const output = `${result.stdout}${result.stderr}`;
    assert.match(output, /unexpected listeners: .*\[::\]:22/);
    assert.match(output, /0\.0\.0\.0:8000/);
  } finally {
    f.cleanup();
  }
});

test('check-host fails when password login is on or the kit directory is open', () => {
  const f = fixture({
    sshdT: ['passwordauthentication yes', 'kbdinteractiveauthentication no', 'pubkeyauthentication yes', 'permitrootlogin prohibit-password'],
  });
  try {
    chmodSync(f.remote, 0o755);
    const result = run(f, 'check-host');
    assert.notEqual(result.status, 0);
    const output = `${result.stdout}${result.stderr}`;
    assert.match(output, /expected passwordauthentication no/);
    assert.match(output, /is 755, expected 700/);
  } finally {
    f.cleanup();
  }
});

test('check-host fails when the admin console answers off the tailnet', () => {
  const f = fixture({ publicAdminStatus: 302 });
  try {
    const result = run(f, 'check-host');
    assert.notEqual(result.status, 0);
    assert.match(`${result.stdout}${result.stderr}`, /answers 302 off the tailnet/);
  } finally {
    f.cleanup();
  }
});

test('check-replica passes when the key reads but cannot write', () => {
  const f = fixture();
  try {
    const result = run(f, 'check-replica');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /write refused/);
    assert.match(result.stdout, /old versions listed/);
    assert.match(result.stdout, /minutes old/);
    assert.match(result.stdout, /check passed/);
    assert.ok(indexOfEvent(events(f), ['restic', /^check --no-lock$/]) !== -1, 'restic check did not run');
  } finally {
    f.cleanup();
  }
});

test('check-replica fails when the read-only key can write', () => {
  const f = fixture({ replicaWritable: true });
  try {
    const result = run(f, 'check-replica');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /wrote to librepaper-backups-replica-0001/);
  } finally {
    f.cleanup();
  }
});

test('check-replica fails when the key cannot list old versions', () => {
  const f = fixture({ replicaVersionsDenied: true });
  try {
    const result = run(f, 'check-replica');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /s3:ListBucketVersions/);
  } finally {
    f.cleanup();
  }
});

test('check-replica fails when the newest snapshot is old', () => {
  const f = fixture({ resticNewest: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString() });
  try {
    const result = run(f, 'check-replica');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /minutes old: replication may have stopped/);
  } finally {
    f.cleanup();
  }
});

test('check-replica never puts a credential in a command argument', () => {
  const f = fixture();
  try {
    const result = run(f, 'check-replica');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const calls = events(f).filter(([kind]) => kind === 'aws' || kind === 'restic');
    assert.ok(calls.length > 0, 'no aws or restic calls were recorded');
    for (const call of calls) {
      for (const secret of ['replica-key-id', 'replica-secret', 'restic-password']) {
        assert.ok(!call.join('\t').includes(secret), `${call[0]} carries a credential`);
      }
    }
  } finally {
    f.cleanup();
  }
});

test('check-replica unlocks the age identity once and hands it only to sops', () => {
  const f = fixture();
  const counter = path.join(f.root, 'unlocks');
  const sopsSeen = path.join(f.root, 'sops-age');
  // The real SOPS_AGE_KEY_CMD asks for a passphrase each time it runs.
  f.env.SOPS_AGE_KEY_CMD = `printf x >> ${counter}; printf 'AGE-SECRET-KEY-1TEST'`;
  const sops = path.join(f.root, 'bin', 'sops');
  writeFileSync(sops, readFileSync(sops, 'utf8').replace('#!/bin/sh\n', `#!/bin/sh\nprintf '%s|%s\\n' "\${SOPS_AGE_KEY:-}" "\${SOPS_AGE_KEY_CMD:-}" >> ${sopsSeen}\n`));
  try {
    const result = run(f, 'check-replica');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(read(counter), 'x', 'the identity was unlocked more than once');
    const seen = read(sopsSeen).trim().split('\n');
    assert.ok(seen.length > 1);
    for (const line of seen) assert.equal(line, 'AGE-SECRET-KEY-1TEST|', 'sops did not get the unlocked identity alone');
    for (const call of events(f).filter(([kind]) => kind === 'aws' || kind === 'restic')) {
      assert.ok(!call.join('\t').includes('AGE-SECRET-KEY'), `${call[0]} saw the identity`);
    }
  } finally {
    f.cleanup();
  }
});

// The production host as backup-drill sees it: a manage that prints the release
// version, and the restic profile the drill copies. Only drill tests write these,
// because the other tests expect the remote directory to stay empty.
function drillHost(f) {
  writeFileSync(path.join(f.remote, 'manage'), '#!/bin/sh\nif [ "$1" = version ]; then echo "$DRILL_VERSION"; fi\n');
  chmodSync(path.join(f.remote, 'manage'), 0o755);
  writeFileSync(path.join(f.remote, 'resticprofile.toml'), '[resticprofile]\nrepository = "s3:example/restic"\n[resticprofile.env]\nRESTIC_PASSWORD = "restic-password"\n');
}

function drillCopies(f) {
  return readdirSync(f.env.TMPDIR).filter((name) => name.startsWith('librepaper-drill.'));
}

test('backup-drill restores the newest snapshot into a separate project and removes it', () => {
  const f = fixture();
  try {
    drillHost(f);
    const result = run(f, 'backup-drill');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /PASS: snapshot 450fafe1 restores/);
    assert.ok(indexOfEvent(events(f), ['drill-manage', /^backup restore 450fafe1$/]) !== -1, 'the snapshot was not restored');
    const projects = read(f.env.DRILL_PROJECT_FILE).trim().split('\n');
    assert.ok(projects.length > 0, 'no compose project was recorded');
    for (const project of projects) assert.match(project, /^librepaper-drill-\d+$/);
    assert.ok(indexOfEvent(events(f), ['docker', /^compose down -v$/]) !== -1, 'the copy was not taken down');
    assert.deepEqual(drillCopies(f), [], 'left the copy behind');
  } finally {
    f.cleanup();
  }
});

test('backup-drill fails when a referenced stored file is missing, and still removes the copy', () => {
  const f = fixture({ drillCopyFiles: 1615 });
  try {
    drillHost(f);
    const result = run(f, 'backup-drill');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /FAIL/);
    assert.match(result.stdout, /1615 stored files for 1616 referenced/);
    assert.ok(indexOfEvent(events(f), ['docker', /^compose down -v$/]) !== -1, 'the copy was not taken down');
    assert.deepEqual(drillCopies(f), [], 'left the copy behind');
  } finally {
    f.cleanup();
  }
});

test('backup-drill fails when the copy has more rows than production', () => {
  const f = fixture({ drillCopyCounts: '300 1374 1601 14 1 9 1616' });
  try {
    drillHost(f);
    const result = run(f, 'backup-drill');
    assert.notEqual(result.status, 0);
    assert.match(result.stdout, /more accounts than production/);
    assert.deepEqual(drillCopies(f), [], 'left the copy behind');
  } finally {
    f.cleanup();
  }
});

test('backup-drill fails when the newest snapshot is old, before restoring anything', () => {
  const f = fixture({ drillSnapshotTime: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString().slice(0, 19).replace('T', ' ') });
  try {
    drillHost(f);
    const result = run(f, 'backup-drill');
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /FAIL: the newest snapshot 450fafe1 is \d+ minutes old/);
    assert.equal(indexOfEvent(events(f), ['drill-manage', /^backup restore /]), -1, 'restored an old snapshot');
  } finally {
    f.cleanup();
  }
});

test('backup-drill refuses an unverified kit before copying the restic profile', () => {
  const f = fixture({ attestationFailure: true });
  try {
    drillHost(f);
    const result = run(f, 'backup-drill');
    assert.notEqual(result.status, 0);
    assert.ok(!events(f).some(([kind, ...rest]) => kind === 'ssh' && rest.join('\t').includes('resticprofile.toml')), 'copied the restic profile');
  } finally {
    f.cleanup();
  }
});

test('backup-drill never puts the restic profile in a command argument', () => {
  const f = fixture();
  try {
    drillHost(f);
    const result = run(f, 'backup-drill');
    assert.equal(result.status, 0, result.stderr || result.stdout);
    for (const event of events(f)) assert.ok(!event.join('\t').includes('restic-password'), `${event[0]} carries the restic password`);
  } finally {
    f.cleanup();
  }
});
