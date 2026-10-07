# SPEC: simple deploy kit

Status: decided 2026-10-07, not started. Supersedes SPEC-simplify-deploy-fable.md.

One VPS, Docker only, one file to edit, `docker compose up -d`. The scoped-role
kit merged on 2026-10-05 (Kata yb5p, yzve) is rolled back to the socket-trust
design of commit da5563c9, keeping what landed since that still earns its
place: the published images, `caddy/local.d`, bounded logs, the restic sidecar,
on-demand TLS. Production is converted once, in place, in one transaction,
with no data movement. The binary does not change.

## Why

- On one host, the host is the security boundary. Five SCRAM roles, a one-shot
  `migrate` service and per-service secret files defend against an attacker
  who already has everything they defend. They cost a 1,017-line Python
  `setup`, a `secrets/` directory, Python and openssl on the host, four Compose
  overlays, a conversion path for old installs, and 1,300 lines of tests.
- Scaling later means a bigger VPS or a managed PostgreSQL. A managed database
  needs TCP, a password and TLS whatever the kit does, and its roles are
  created on the provider by hand. The only property that matters for that
  move is that the app runs without superuser, and the kit keeps it.
- Verified: the config loader accepts a literal or `{ file = "..." }` for every
  secret; `server.migrate` defaults to true; the server creates
  `/var/lib/librepaper/secrets/session.key` on first start and refuses to run
  without it once the catalog has state; `/api/tls/ask` exists in v0.0.21. No
  Rust change is needed.

## The operator's experience

This is the acceptance test for the kit, in `docs/host.md`, verbatim:

```sh
# Docker Engine with Compose v2 is the only requirement. Open ports 80 and 443.
# Point two DNS names at the VPS: the app and the published documents.
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
$EDITOR librepaper.toml      # the two hostnames, a GitHub OAuth client id and secret
docker compose up -d
curl -fsS https://paper.example/ready
```

- Nothing is generated. The only credentials are the ones a third party issues.
- No Python, no openssl, no `setup`, no `.env`, no `secrets/`.
- Backups, monitoring and a managed database are each one short optional
  section with one edit and one command.
- Upgrade: re-extract the archive, `docker compose pull`, `docker compose up -d`.

## Design

### Kit files

`deploy/` in the repository, and the archive, contain exactly:

```
librepaper/
  compose.yaml             one file; monitoring behind a Compose profile
  compose.managed-db.yaml  copied to compose.override.yaml for a database elsewhere
  librepaper.toml          the one file to edit; secrets inline
  resticprofile.toml       backups; idle until it has a [resticprofile] table
  postgres/init.sql        two roles and one database; no passwords
  caddy/Caddyfile          unchanged; on-demand TLS, local.d imports
  prometheus.yaml, grafana.json, monitoring/   unchanged provisioning
  README.md                ten lines pointing at docs/host.md
```

`deploy/Dockerfile` and `deploy/backup/` stay in the repository as the image
build context. They are not in the archive.

Deleted from `deploy/`: `setup`, `postgres/roles.sql`, `postgres/init-roles.sh`,
`compose.external-db.yaml`, `compose.local-binary.yaml`,
`compose.local-build.yaml`, `compose.monitoring.yaml`,
`compose.production.yaml`, `monitoring/grafana-entrypoint.sh`.

### compose.yaml

- `name: librepaper`. Image tags are literals, `ghcr.io/librepaper/librepaper:v0.0.21`
  and `ghcr.io/librepaper/librepaper-backup:v0.0.21`, no `${}` interpolation.
- `postgres`: `postgres:17.11-alpine`, `network_mode: none`,
  `POSTGRES_HOST_AUTH_METHOD: trust`, volumes `postgres` (data), `pgsocket`
  at `/var/run/postgresql`, `./postgres/init.sql` under
  `/docker-entrypoint-initdb.d/`. Healthcheck `pg_isready -U librepaper -d librepaper`.
- `librepaper`: the image's own CMD. Volumes `data`, `backups`, `pgsocket`,
  `./librepaper.toml:/etc/librepaper/librepaper.toml:ro`. `depends_on`
  postgres healthy. Networks `edge`, `monitoring`, `default`.
- `backup`: volumes `data`, `backups`, `backup-metrics`, `pgsocket`, both
  TOML files read-only, the existing tmpfs. No `secrets:`, no
  `AWS_SHARED_CREDENTIALS_FILE`. Network `default`.
- `caddy`: unchanged. Networks `edge`, `default`.
- `prometheus`, `grafana`, `node-exporter`, `postgres-exporter`:
  `profiles: [monitoring]`. Grafana reads
  `GF_SECURITY_ADMIN_PASSWORD__FILE: /run/secrets/grafana_admin_password` from
  the one Compose secret in the file, `file: ./monitoring/grafana_admin_password`.
  The exporter connects over `pgsocket` as `librepaper_metrics`.
- Networks: `edge` with the fixed `172.29.0.0/16` subnet that
  `[proxy].trusted_networks` names, `monitoring` internal. Egress is the
  implicit `default` network.
- Volumes: `postgres`, `pgsocket`, `data`, `backups`, `backup-metrics`,
  `caddy-data`, `caddy-config`, `prometheus`, `grafana`. Same names as today,
  so every volume except `postgres` carries over on production untouched.
- Operator changes never go in `compose.yaml`, which every upgrade replaces.
  They go in `compose.override.yaml`, which Compose loads on its own and the
  archive never contains. `compose.managed-db.yaml` is the kit's template for
  the one common case:

```yaml
# cp compose.managed-db.yaml compose.override.yaml, then set database_url in librepaper.toml
services:
  postgres:
    profiles: !override [unused]
  librepaper:
    depends_on: !reset []
  backup:
    depends_on: !reset []
  postgres-exporter:
    profiles: !override [unused]   # or set DATA_SOURCE_NAME to the provider's monitoring URL
```

  The empty `pgsocket` mounts stay; they are harmless. `!override` matters:
  Compose appends list values on merge, so a plain `profiles: [unused]` on
  the exporter yields `[monitoring, unused]` and the exporter still starts
  with the monitoring profile. Verified on Compose 5.4.

### Database

`postgres/init.sql`, the whole file:

```sql
CREATE ROLE librepaper LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE;
CREATE DATABASE librepaper OWNER librepaper;
CREATE ROLE librepaper_metrics LOGIN NOSUPERUSER;
GRANT pg_monitor TO librepaper_metrics;
```

- The app, the backup sidecar and `pg_dump` connect as `librepaper` over the
  socket. The server migrates at startup (default). No `migrate` service.
- Trust applies to the socket only: the container has no network.
- The trust boundary, stated plainly: the three containers that mount the
  socket (app, backup, exporter) can each connect as any role, including the
  superuser `postgres`. They are one trust domain. That is accepted: on one
  VPS they already share the data they could reach, and the previous kit's
  per-service passwords defended against a container compromise that gains
  nothing more than it already has. `librepaper NOSUPERUSER` is not about
  containers; it stops a SQL injection in the app from running
  `COPY ... TO PROGRAM` or altering roles. `librepaper_metrics` is the
  exporter's default identity, a convention that costs one line, not a wall.
- Peer authentication would enforce identity at the socket, but it maps the
  client uid to a name in the postgres container's `/etc/passwd`, where uid
  10001 has none. Rejected as not worth a custom image.

### librepaper.toml

The da5563c9 `config.toml`, with these differences:

```toml
[storage]
directory = "/var/lib/librepaper"
database_url = "postgresql:///librepaper?host=/var/run/postgresql&user=librepaper"
# A database elsewhere: "postgresql://user:password@db.example/librepaper?sslmode=verify-full"
```

- No `[server] migrate`. `[metrics] address = "0.0.0.0:9091"` stays on, as in
  the current kit and production: it listens on the internal `monitoring`
  network only, and a Prometheus that scrapes a closed port would fire alerts
  the moment the profile is enabled.
- `[auth.github]` holds `client_id` and `client_secret` as literals. One
  comment line says `client_secret = { file = "/run/secrets/github" }` also
  works with a bind mount, for operators who keep secrets in files.
- `[access]` stays, the loader requires it.

### Secrets

| Credential | Where | Who creates it |
| --- | --- | --- |
| OAuth client id and secret | `librepaper.toml` | the operator, from GitHub or Google |
| Database | none | nobody; socket trust |
| `session.key` | `data` volume, `/var/lib/librepaper/secrets/` | the server, first start |
| Restic repository and password | `resticprofile.toml` | the operator |
| Grafana admin password | `monitoring/grafana_admin_password` | the operator, only with monitoring |

- The archive's top directory is mode 0700. Files inside stay 0644 so the
  containers, which run as other uids, can read their bind mounts. This is the
  current `secrets/` trick (dir 700, files readable) applied to the whole kit.
  Never tell an operator to `chmod 600 librepaper.toml`: the app would fail
  to read it.
- Production keeps its secrets in files via `{ file = }` references and a
  bind mount in `compose.override.yaml`. The kit does not know about it.

### Backups

- Unchanged sidecar. `resticprofile.toml` sample shows `repository` under
  `[resticprofile]` and `RESTIC_PASSWORD` plus any S3 credentials in an
  `[resticprofile.env]` table. resticprofile has no `password` key: verified
  on the v0.0.21 image, it passes one through as a restic flag that restic
  rejects. No secret mounts.
- Both TOML files and `/var/lib/librepaper/secrets` are already in the restic
  `source` list, so a restore brings back config and session key.

### Monitoring

```sh
# Optional. Grafana is at https://paper.example/admin/monitoring/
head -c 24 /dev/urandom | base64 > monitoring/grafana_admin_password
echo COMPOSE_PROFILES=monitoring > .env
docker compose up -d
```

- The `.env` line is how Compose remembers a profile. The default kit has no
  `.env`; this one is created by the operator who wants monitoring.
- The app's metrics listener is already on, so the profile is the only step.
- Caddy's `forward_auth` host check and Grafana's own login stay.
- With a database elsewhere, `postgres-exporter` has no socket: the managed-db
  override disables it or gives it the provider's monitoring URL.

### Upgrades

```sh
cd ..   # the directory that contains librepaper/
curl -fsSL .../librepaper-deploy.tar.gz | tar xz --exclude='*/librepaper.toml' --exclude='*/resticprofile.toml'
cd librepaper && docker compose pull && docker compose up -d
```

- The archive from a tag pins that tag, so compose.yaml, Caddyfile, init.sql
  and the binary always agree. Operator files are never in the archive
  (`caddy/local.d/`, `monitoring/grafana_admin_password`, `.env`,
  `compose.override.yaml`) and the two config files are excluded on extract.
  Everything an operator customises therefore survives an upgrade; the
  managed-db case is tested that way.
- The server migrates the schema at startup. Nothing to stop, run or restart
  by hand.

### Release packaging

- `tools/release/package-deploy-kit.sh` does `git archive` of the kit files
  listed above from the release commit, strips the `deploy/` prefix under a
  `librepaper/` top directory at mode 0700, rewrites the two image tags to the
  release tag, and writes `librepaper-deploy.tar.gz`. No `.env`, no `RELEASE`
  file, no `.sha256`. Fixed name, so `releases/latest/download/` works.
- `publish-docker.yml` uploads that one file. The in-repo compose.yaml literal
  is bumped when a release is cut; `packaging-test.sh` asserts the archive's
  tags equal the release tag and that no file outside the list is present.

## Production

Live facts: v0.0.21 since 2026-10-06, host `ubuntu@librepaper.org`, directory
`~/librepaper`, Compose project `librepaper`. The role catalog, from
`deploy/setup` lines 786 to 803 and 959 (the legacy upgrade path production
took): `librepaper` is the initdb superuser (OID 10, cannot be demoted, set
NOLOGIN), `librepaper_bootstrap` is a LOGIN superuser with a generated
password, and `librepaper_owner`, `librepaper_app`, `librepaper_backup`,
`librepaper_metrics` are the scoped roles. The socket accepts trust. So the
cluster is converted in place, in one transaction, with no data movement.

### Layout and helper

- Host: the kit as extracted, plus `librepaper.toml` (from
  `tools/deploy/production.toml`), `secrets/` with the five OAuth and Grafana
  files the helper writes from SOPS, `monitoring/grafana_admin_password`,
  `.env` with `COMPOSE_PROFILES=monitoring`, `compose.override.yaml` mounting
  `./secrets:/run/secrets:ro` into `librepaper` and `./site:/srv/site:ro` into
  `caddy`, `caddy/local.d/` for the other app on the VPS. `chmod 700 ~/librepaper`.
- `tools/deploy/production.toml`: the socket `database_url` literal, no
  `[server] migrate`, `{ file = }` OAuth references as today.
- `tools/deploy/production` keeps `deploy VERSION`, `site`, `verify`, `logs`.
  `deploy-local` and `upgrade-database` are deleted. `deploy` is: DNS check,
  decrypt keys, `make site`, rsync kit files excluding operator files, sed the
  tag into the host compose.yaml, write secrets, override and `.env`, stage
  and validate `librepaper.toml.candidate` with `admin config check` and the
  Caddyfile candidate with `caddy validate`, install both,
  `docker compose pull`, `docker compose up -d --wait`, `caddy reload`,
  `docker image prune -f`, `verify`. Version floor stays v0.0.21.
- `production.test.mjs` is rewritten around what the helper still guards:
  secrets never in argv, `.env` or output; a rejected candidate config or
  Caddyfile leaves the running stack untouched; the version floor; the
  `verify` checks. One test per guard, not per line.

### One-time conversion

Temporary code, deleted once production is converted:
`tools/deploy/production convert-database` (operator side) running
`tools/deploy/convert-database.sh` on the host over ssh, and the rehearsal
`tools/test/deploy/convert.sh` (suite `deploy-convert`).

Operator side, before the host script: capture and locally restore the
encrypted recovery packet exactly as before v0.0.21. Then
`tar czf ~/librepaper-convert/host-files-before.tar.gz` of `~/librepaper`
(files only, no volumes): with it, `docker compose up -d` brings the old stack
back if the conversion stops before its transaction commits. Then run the
`deploy` preparation steps (sync the new kit, secrets, `monitoring/grafana_admin_password`,
override, `.env` with `COMPOSE_PROFILES=monitoring`, validated config) without
starting anything.

Host script, in order, stopping on the first error:

```sh
# Preconditions, all read-only, or abort:
#   pg_roles for OID 10 and rolname LIKE 'librepaper%' is exactly the catalog above
#   librepaper_app and librepaper_backup own no objects (DROP OWNED would drop them)
#   librepaper_bootstrap owns nothing in the postgres database
docker compose stop librepaper backup postgres-exporter     # only this script is connected
docker compose exec -T postgres pg_dump -U librepaper_owner -d librepaper -Fc > ~/librepaper-convert/librepaper.dump   # the fallback
docker compose exec -T postgres psql -U librepaper_bootstrap -d librepaper -v ON_ERROR_STOP=1 -1 -f - <<'SQL'
ALTER ROLE librepaper RENAME TO postgres;          -- the initdb superuser, as a fresh install names it
ALTER ROLE postgres LOGIN CREATEDB CREATEROLE;     -- setup upgrade had removed these; a fresh initdb role has them
ALTER ROLE librepaper_owner RENAME TO librepaper;
ALTER ROLE librepaper INHERIT PASSWORD NULL;       -- a fresh CREATE ROLE has no password
DROP OWNED BY librepaper_app, librepaper_backup;   -- revokes their grants and default ACL entries
DROP ROLE librepaper_app, librepaper_backup;
ALTER ROLE librepaper_metrics PASSWORD NULL;
GRANT CONNECT ON DATABASE librepaper TO PUBLIC;    -- the fresh default; setup had revoked it
SQL
docker compose exec -T postgres psql -U postgres -d librepaper -v ON_ERROR_STOP=1 -1 -f - <<'SQL'
DROP OWNED BY librepaper_bootstrap;                -- a role cannot drop itself, hence the reconnect
DROP ROLE librepaper_bootstrap;
SQL
docker compose up -d --wait --remove-orphans       # postgres is recreated without a network, with the socket volume; the app validates the schema; the old migrate container goes
rm -f setup postgres/roles.sql postgres/init-roles.sh .setup-state.json monitoring/grafana-entrypoint.sh \
  compose.external-db.yaml compose.local-binary.yaml compose.local-build.yaml compose.monitoring.yaml compose.production.yaml \
  secrets/database_app_url secrets/database_owner_url secrets/database_backup_url secrets/database_metrics_url secrets/database_metrics_uri secrets/database_metrics_user \
  secrets/database_app_password secrets/database_owner_password secrets/database_backup_password secrets/database_metrics_password \
  secrets/postgres_bootstrap_password secrets/grafana_admin_password
```

- `psql -1` runs the file in one transaction: either every statement applies
  or none does. Role DDL is transactional in PostgreSQL. If the first block
  fails, nothing changed; untar the host files and `docker compose up -d`.
  If the second block fails, production has one extra superuser and still
  works; fix by hand.
- No volume is copied, renamed or removed. The `postgres` data volume is
  reused as is; its `pg_hba.conf` keeps `local all all trust` from initdb and
  host lines that nothing can reach. The `data` volume is not touched:
  objects and `session.key` stay. An existing session cookie must still sign
  in afterwards; `verify` checks readiness, OAuth, monitoring, site and docs.
- Files are deleted by name. A glob like `compose.*.yaml` would take
  `compose.override.yaml` with it.
- The dump and the host-files tarball are deleted a week later by hand.
- Never `docker compose down`, `down --volumes` or prune on this host: Caddy
  also serves another app from `caddy/local.d`.

Acceptance for "identical to a fresh install": the rehearsal diffs, between a
converted fixture and a fresh new-kit install, `pg_roles` (name, super, login,
createdb, createrole, inherit, password null), the database owner and ACL,
the `public` schema owner and ACL, `pg_default_acl`, and the owner of every
relation, sequence and type in `public`. Accepted differences: the `public`
schema being owned by `librepaper` directly rather than through
`pg_database_owner`; raw `datacl` and `nspacl` being explicit where a fresh
install has NULL (an ACL never returns to NULL once touched, so the diff
compares effective privileges per role and for PUBLIC instead); and the extra
`host ... scram-sha-256` lines in `pg_hba.conf`.

Rehearsal (`tools/test/deploy/convert.sh`): build a production-shaped
fixture the way the deleted `install.sh` built its legacy case, from the
pre-cutover tree (`git archive ba9fc74c deploy`): old socket-trust kit with
`POSTGRES_USER=librepaper`, `setup upgrade` to scoped roles, the `v0.0.21`
image, an account row seeded as `install.sh` did. Then: copy the new kit
files in, tar the host files, run the host script, assert the catalog diff
above, the row counts, `_sqlx_migrations` byte for byte, and that
`admin serve` answers `/ready`. Then run `docker compose up -d` a second
time to show the converted stack is a normal deployment. Then a second
fixture: inject a failing statement at the end of the first SQL block,
assert the catalog is unchanged, untar the host files, and assert the old
stack comes back. The rehearsal must pass before the host script runs on
production.

Self-hosters on the scoped kit published with v0.0.21 are not converted.
`docs/recovery.md` says: back up with the sidecar, install the new kit,
restore.

## Repository changes

Delete:

- `deploy/setup`, `deploy/postgres/roles.sql`, `deploy/postgres/init-roles.sh`,
  the five overlay files, `deploy/monitoring/grafana-entrypoint.sh`
- `tools/deploy/setup.test.mjs`, `tools/test/deploy/install.sh`,
  `tools/test/deploy/roles.sh`, `tools/test/backup/database-role.sh`
- `docs/credentials.md` and its `nav.js` entry
- `.gitignore` and `.dockerignore` entries for `deploy/secrets`,
  `.setup-state.json`; add `deploy/.env`, `deploy/compose.override.yaml`,
  `deploy/monitoring/grafana_admin_password`

Rewrite:

- `deploy/compose.yaml`, `deploy/librepaper.toml`, `deploy/resticprofile.toml`
  sample, `deploy/README.md`, `deploy/postgres/init.sql` (new)
- `tools/deploy/production`, `production.toml`, `production.test.mjs`
- `tools/release/package-deploy-kit.sh`, `packaging-test.sh`,
  `packaging.test.mjs`, the upload step in `publish-docker.yml`
- `tools/deploy/compose.test.mjs`: `docker compose config` validates with and
  without the profile, and with `compose.managed-db.yaml` as the override
  (no postgres service, no `depends_on`); postgres has `network_mode: none`;
  the two image tags are equal; no `secrets:` reaches a default-profile
  service
- `tools/dev/demo-compose`, `tools/dev/demo/compose.yaml`, `config.toml`,
  `config-oauth.toml`: no `setup`, no secrets dir, socket URL; checkout
  builds and loopback ports stay; `make demo` keeps working
- `tools/test/backup/drill.sh` and `sidecar.sh`: use the fixture's superuser
  URL directly instead of creating scoped test roles
- `tools/test/suite`: drop `deploy-roles` and `deploy-install`; add
  `deploy-install` with two variants, local database (fresh kit with a
  checkout-built image boots and answers `/ready` from inside the app
  container) and managed database (the kit's override plus a disposable TCP
  PostgreSQL from `tools/test/postgres.sh`, then the upgrade step re-extracts
  the kit over it and the override survives); and the temporary
  `deploy-convert`

Keep: `deploy/Dockerfile` (all targets), `deploy/backup/`, `deploy/caddy/`,
`deploy/monitoring/`, `prometheus.yaml`, `grafana.json`, every Rust crate,
`tools/deploy/keys.yaml`, `runbook.enc`.

Docs, in the house style (short, bullets, `sh` blocks with `#` comments, no
dashes):

- `docs/host.md`: the install block above, then Check, Backups, Monitoring,
  Upgrade, Database elsewhere (managed PostgreSQL: TCP URL with
  `sslmode=verify-full`, roles created on the provider,
  `cp compose.managed-db.yaml compose.override.yaml`),
  Secrets in files (two lines), Without Docker. Under 100 lines. Keep an
  anchor `#backups-and-recovery` for `privacy.md`.
- `docs/recovery.md`: the restore drill on a machine that is not the VPS (the
  kit publishes 80 and 443 and claims a fixed subnet, so two copies cannot
  share a host); no DNS is needed to restore, check with
  `docker compose exec librepaper wget -qO- http://127.0.0.1:8080/ready`;
  drop every `setup`, role and URL-file step; keep the external alert hooks
  section; one paragraph for scoped-kit installs: back up, reinstall, restore.
- `deploy/README.md`: extract, edit, up, link.

## Rollout

1. One branch, worktrees per piece: kit, production helper, release packaging,
   tests and demo, docs, conversion. Central review, then `make test`, then
   `tools/test/suite deploy-install`, `backup-sidecar`, `deploy-convert`.
2. Merge. Convert production with the v0.0.21 images and the new kit
   (recovery packet, host-files tarball, `convert-database`, then `verify`
   and a sign-in with an existing cookie).
3. Cut v0.0.22 so `releases/latest/download/librepaper-deploy.tar.gz` carries
   the new kit. `tools/deploy/production deploy v0.0.22` is the first routine
   upgrade through the new path.
4. Follow-up commit: delete `convert-database`, `convert-database.sh`,
   `convert.sh` and the `deploy-convert` suite entry. A week later, delete
   `~/librepaper-convert` on the host (the dump and the host-files tarball).

## Traps

- Bind-mounted files are read by the container uid, not the operator. Protect
  with the directory mode (0700), never the file mode.
- The initdb role (OID 10) cannot lose SUPERUSER on PostgreSQL 16 and later;
  it can be renamed and given LOGIN. A session cannot rename or drop its own
  role, which is why the conversion reconnects as `postgres` for the last
  two statements.
- `DROP OWNED BY` drops objects the role owns as well as revoking its grants.
  The preconditions prove the app and backup roles own nothing before it
  runs. The rehearsal's catalog diff is what shows the default ACL entries
  went with them.
- `_sqlx_migrations` must be unchanged or `admin serve` refuses the schema.
  Compare it explicitly even though nothing should touch it.
- A fresh named volume mounted at `/var/run/postgresql` inherits the image
  directory's 2777 mode, which is what lets uid 10001 and the exporter reach
  the socket. Do not pre-create that directory with other modes. Socket paths
  must be short; never put one under a scratchpad path in tests.
- The app's image runs `pg_dump` 17.11 against server 17.11. Bumping one
  means bumping the other.
- Operator state lives only in files the archive never contains:
  `compose.override.yaml`, `.env`, `caddy/local.d/`,
  `monitoring/grafana_admin_password`, and the two excluded TOML files. Any
  instruction that has an operator edit `compose.yaml` is a bug.
- `docker compose up -d` on production recreates Caddy when its network list
  changes. Seconds of downtime for the other app on the VPS; acceptable,
  announce nothing, do it once.
- The version floor stays v0.0.21: older binaries lack `/api/tls/ask` and
  Caddy would issue no certificates.

## Rejected

- An interactive `./setup install` that prompts for hostnames and credentials.
  It keeps every generated credential and adds a lifecycle (retry, rotate,
  recover) to maintain. Once nothing is generated, an installer has nothing
  to do; a 15-line TOML with comments is easier to read, diff and back up.
- Converting production by dump and restore into a fresh volume. It moves
  data for no reason once a login superuser exists, needs a consistent copy of
  a live cluster for its rollback, and leaves `pg_restore` to guess at the
  `public` schema's ownership. The first draft of this spec did it that way.
- Single-user mode for the conversion. Not needed: `librepaper_bootstrap` can
  log in over the socket.
- Peer authentication on the socket. See Database.
- Keeping scoped roles "for the managed-database future". Those roles are
  created on the provider by hand; the kit's generator never leaves the VPS.
- Built-in TLS, embedded database, built-in sign-in: as in the previous spec.

## Deferred

- `server.migrate = false`, `admin migrate` and `validate_schema` stay in the
  binary and are not dead. Nothing in the repository uses them after this
  change, but they are the hook for running a migration apart from the
  serving process, which a managed database with a separate schema owner or a
  future lease handoff between two app containers would both need. Keep them.
- Grafana behind LibrePaper sign-in instead of its own password needs an
  operator role the server does not have.
