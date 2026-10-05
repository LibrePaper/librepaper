---
title: "Self-hosting"
---

> **Warning:** LibrePaper is experimental. We do not recommend self-hosting
> yet; this guide is for interested readers and system administrators.

## Docker Compose

The kit in `deploy/` runs PostgreSQL, LibrePaper and Caddy. Configure the app
in `librepaper.toml`; scheduled backups use the separate `resticprofile.toml`.
Needs Docker Compose 2.24 or newer and ports 80 and 443 reachable from the internet.

```sh
cd deploy
# edit librepaper.toml: [origins] and one [auth.*] table
# First start only: append a private Grafana admin password to .env.
umask 077
printf 'GRAFANA_ADMIN_PASSWORD=%s\n' "$(openssl rand -hex 32)" >> .env
chmod 600 .env
docker compose up -d
```

Before the first start:
- Both origin names need DNS records pointing here. Caddy gets a certificate for each on first request, after the server confirms the name; nothing else is configured.
- PostgreSQL has no password: it is reachable only over a socket shared with the application container.
- OAuth callback URLs: `https://<app-origin>/auth/callback` (GitHub), `https://<app-origin>/auth/callback/google` (Google).
- `[access]`: `publishers` and `commenters` take `["any"]` or GitHub logins, verified Google addresses and `@domain` entries.
- `[retention]` is off by default.
- Compose requires a nonempty `GRAFANA_ADMIN_PASSWORD` from `.env` or the
  environment before it starts. Keep `.env` private and preserve this value:
  Grafana uses it to initialize a new volume, while an existing Grafana volume
  keeps its current password. The production deployment helper continues to
  write this setting atomically with mode 600.

### Remote database

Delete the `postgres` and `postgres-exporter` services in `compose.yaml`, remove the `depends_on` and `pgsocket` lines under `librepaper`, and remove the `postgres` scrape job from `prometheus.yaml`. Set `storage.database_url` in `librepaper.toml` to the full URL. The exporter is configured for the bundled database role and socket. Add `?sslmode=verify-full` for a managed service.

### Volumes

- `postgres`: the database (documents, update log, comments)
- `data`: immutable objects and the secrets that keep sessions and share links valid. Back it up even with S3.
- `backups`: temporary exports and the local restic repository state
- `backup-metrics`: backup status files read by Node Exporter
- `caddy-data`: certificates

### Monitoring

- Grafana at `https://<app-origin>/admin/monitoring/`, user `admin`, with the
  password set in `.env` above. Changing `.env` later does not rotate a password
  in an existing Grafana volume; rotate it in Grafana and update `.env` to keep
  fresh-volume recovery consistent.
- Prometheus, Grafana and exporters start with `docker compose up -d`.
- Prometheus and the exporters stay on the private Compose network. Grafana is
  available through the HTTPS proxy only on the configured app origin. Caddy
  checks the incoming Host with LibrePaper before its monitoring redirect or
  Grafana proxy; Grafana also requires its own login.
- Public Grafana access depends on LibrePaper and its host-check endpoint being
  reachable. Use external uptime monitoring too: the monitoring stack on this
  VPS cannot report an outage when the VPS itself is unreachable.
- Docker marks LibrePaper ready only while its configured database and writer
  are usable. `/health` remains a liveness endpoint. The container allows
  30 seconds for graceful shutdown. Losing the writer session or timing out its
  database probe makes the app exit; Docker then restarts it to reclaim ownership.
- Prometheus keeps 30 days, capped at 8 GB.
- `prometheus.yaml` and `grafana.json` live at the root of `deploy/`; alert
  rules and Grafana provisioning stay under `deploy/monitoring/`.

### Upgrade

```sh
# Edit LIBREPAPER_VERSION in compose.yaml or export it
docker compose stop backup
LIBREPAPER_VERSION=<tag> docker compose up -d --build
```

The backup sidecar starts after the app is healthy. Recreate it after
replacing either configuration file, so its read-only bind mounts see the new files.

### Backups

Backups stay disabled until `resticprofile.toml` has a named `[resticprofile]`
table. A comments-only file disables backups. Keep application settings in
`librepaper.toml`; put restic credentials in `[resticprofile.env]` or use
resticprofile's `password-file` setting. LibrePaper's `{ env = ... }` and
`{ file = ... }` references do not apply in the resticprofile file. Protect
both files from other host users while keeping them readable by UID 10001, and
keep off-host recovery copies of both. Recreate the sidecar and check snapshots:

```sh
docker compose up -d --force-recreate backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile snapshots
```

The sidecar exports with `admin backup`, snapshots, and prunes the export. It
runs daily backups and checks the repository Monday at 06:00. Successful
backups keep 48 hours plus 14 daily and 11 weekly snapshots. Gaps can extend
those ages; stopping backups stops pruning. Pruning runs after a successful
backup, and no fixed deletion window is promised.

To migrate an older combined `config.toml`, keep the application tables in
`librepaper.toml` and move the complete `[resticprofile]` subtree, including
its nested tables, to `resticprofile.toml`. Remove that subtree from the app
file; the app now rejects it. Keep `resticprofile.toml` comments-only to leave
backups disabled.

Override repository, credentials, schedules, retention and notifications only.
Keep the shipped sources, export hooks, lock and status paths. For versioned
object stores, expire noncurrent versions too; pruning cannot remove them.

Grafana reports status but does not deliver external alerts. Configure separate
dead-man URLs for backup and check in each profile. This backup example uses
Healthchecks.io-style ping URLs:

```toml
[[resticprofile.backup.send-before]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>/start"
[[resticprofile.backup.send-after]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>"
[[resticprofile.backup.send-after-fail]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>/fail"
```

Repeat under `resticprofile.check` with a separate ID. Set external freshness
limits to 36 hours for backup and 8 days for check; update them when schedules
change. The dashboard checks the same freshness limits. Manual resticprofile
commands use the same lock as scheduled jobs. Do not run restic directly while
a job is active. The app reads `/etc/librepaper/librepaper.toml`; resticprofile
reads `/etc/resticprofile/resticprofile.toml`, and its base profile includes
only that backup file. `prometheus.yaml` and `grafana.json` remain at the root
of `deploy/`; monitoring rules and provisioning remain under
`deploy/monitoring/`.

Restic supports local, SFTP, REST server and S3-compatible repositories. A
local repository can live at `local:/var/backups/librepaper/repository`; it is
only on the `backups` volume, so copy it off-host. Put `RESTIC_PASSWORD` and
S3 credentials in a mode-600 `.env` if not using `[resticprofile.env]`. For
SFTP, mount private key and `known_hosts` files read-only into `backup`, for
example at `/run/secrets/restic_ssh_key` and
`/run/secrets/restic_known_hosts`, and configure restic's SSH command to use
those paths.

For a one-time verified export without a restic snapshot:

```sh
docker compose exec librepaper librepaper admin backup \
  --config /etc/librepaper/librepaper.toml /var/backups/librepaper/manual
```

## Configuration

Server and admin commands use `librepaper.toml`. `admin serve` defaults to
`/etc/librepaper/librepaper.toml`; use `--config PATH` to choose another file.

- Application literals use TOML syntax. In `librepaper.toml`, `{ env = "NAME" }`
  and `{ file = "path" }` replace one whole value.
  Referenced strings are used as-is; numbers, booleans, and arrays use TOML
  syntax in LibrePaper application tables. Backup settings belong in the
  separate `resticprofile.toml` file and use resticprofile's own syntax. The
  app rejects misplaced `[resticprofile]` and legacy `[backup]` tables.
- LibrePaper file references in the application config are config-relative;
  trailing CR/LF is stripped. Resticprofile uses its own configuration and
  secret handling rules.

Missing or empty references, invalid keys/types, and old server-setting flags
fail. There is no interpolation or automatic application-setting override.

The shipped `librepaper.toml` is the application reference: every table has a
comment. The `[metrics]` table is enabled for the Docker monitoring stack; the
commented-out `[limits]`, `[retention]` and `[server]` tables are optional.
The separate `resticprofile.toml` is comments-only by default, which disables
the backup sidecar until a `[resticprofile]` table is added.
Application secrets may be literals in a protected file readable by the
container user, or `{ env = "NAME" }` and `{ file = "path" }` references.

Use these commands to check or inspect resolved settings. `check` has no
startup side effects or database connection; `show` includes defaults and
provenance and redacts credentials and the database URL.

```sh
librepaper admin config check --config /etc/librepaper/librepaper.toml
librepaper admin config show --config /etc/librepaper/librepaper.toml
```

## Without Docker

1. Create a PostgreSQL role and database with a generated password:

   ```sql
   create role librepaper login password 'replace-with-a-generated-secret';
   create database librepaper owner librepaper;
   ```

2. Supply the complete database URL, as a literal in `librepaper.toml` or
   through a reference:

   ```sh
   export LIBREPAPER_DATABASE_URL='postgresql://librepaper:SECRET@127.0.0.1/librepaper'
   librepaper admin serve --config /etc/librepaper/librepaper.toml
   ```

3. Keep PostgreSQL and the data directory on durable storage. Run only one
   `admin serve` per database. Put the server behind an HTTPS reverse proxy
   that preserves `Host`, appends the actual client address to
   `X-Forwarded-For`, and trusts only the proxy network.

4. For S3-compatible storage, set `object_store = "s3"` in `[storage]` and
   add a `[storage.s3]` table with `endpoint`, `region`, `bucket`,
   `access_key_id` and `secret_access_key`. Credentials come only from that
   table; ambient AWS credentials are not used.

## Storage and backup

The backup contains a consistent PostgreSQL dump, referenced objects, server
config and session key. Without the key, sessions are invalidated and the app
cannot reveal old share URLs, though URLs users already hold still work.
Restore with the matching LibrePaper release, an empty database and new path.
Keep the original deployment intact until recovery is verified.

1. Stop the app and backup sidecar. Preserve the original database, data volume,
   both configs and remote repository. Create `compose.recovery.yaml` in `deploy/`
   with an unused subnet to avoid colliding with production:

   ```yaml
   services:
     librepaper:
       ports:
         - 127.0.0.1:18080:8080
   networks:
     edge:
       ipam:
         config: !override
           - subnet: 172.31.0.0/16
   ```

   Update the recovery config's `proxy.trusted_networks` to that subnet. Start
   PostgreSQL in a separate project for a new database and data volume:
   `docker compose -f compose.yaml -f compose.recovery.yaml -p librepaper-recovery up -d postgres`.
2. Restore the snapshot into a temporary host directory. Override the entrypoint
   to avoid starting the scheduler:

   ```sh
   mkdir -m 700 /tmp/librepaper-restore
   docker compose run --rm --no-deps \
     -v /tmp/librepaper-restore:/restore \
     --user 0 --entrypoint resticprofile backup \
     -c /etc/resticprofile/profiles.toml -n resticprofile restore <snapshot> --target /restore
   ```

   Inspect the restored hierarchy and copy both
   `etc/librepaper/librepaper.toml` and
   `etc/resticprofile/resticprofile.toml` to the recovery project.
3. Copy the app config to `librepaper-recovery.toml`. Set
   `storage.directory = "/var/lib/librepaper"` and point the database URL at
   the recovery DB; remove or replace inherited production URLs. Keep the
   matching backup config for access to the snapshot. Restore as
   root because the temporary directory is mode 700. Use the matching release,
   an empty DB and a nonexistent child path (the volume root exists):

   ```sh
   LIBREPAPER_VERSION=<snapshot-release> docker compose -f compose.yaml -f compose.recovery.yaml -p librepaper-recovery build librepaper
   LIBREPAPER_VERSION=<snapshot-release> LIBREPAPER_CONFIG_FILE=librepaper-recovery.toml docker compose -f compose.yaml -f compose.recovery.yaml -p librepaper-recovery run --rm --no-deps --user 0 \
     -v /tmp/librepaper-restore:/restore:ro librepaper admin restore \
     --config /etc/librepaper/librepaper.toml /restore/var/backups/librepaper/current /var/lib/librepaper/recovered
   ```

4. Install `recovered/objects` and restored `secrets/` at the new volume root
   with UID 10001 ownership:

   ```sh
   docker compose -f compose.yaml -f compose.recovery.yaml -p librepaper-recovery run --rm --no-deps --user 0 \
     -v /tmp/librepaper-restore:/restore:ro --entrypoint sh librepaper -c \
     'cp -a /var/lib/librepaper/recovered/objects /var/lib/librepaper/ && cp -a /restore/var/lib/librepaper/secrets /var/lib/librepaper/ && chown -R 10001:65534 /var/lib/librepaper/objects /var/lib/librepaper/secrets'
   ```

   For S3, upload and verify each manifest object
   under the same key in a new recovery bucket, preserving the original. Point
   recovery at that bucket, or switch explicitly to filesystem storage and
   remove the old S3 settings. Never start against an empty or unverified store.
5. Start only the recovery app (not Caddy, whose public ports belong to the
   original project), then verify it at `http://127.0.0.1:18080`:

   ```sh
   LIBREPAPER_VERSION=<snapshot-release> LIBREPAPER_CONFIG_FILE=librepaper-recovery.toml \
     docker compose -f compose.yaml -f compose.recovery.yaml -p librepaper-recovery \
     up -d --no-deps --wait librepaper
   ```

   Reapply known deletions requested after the snapshot; restore does not replay
   a deletion ledger.

Record the release, snapshot age, elapsed time and outcome in restore drills.
Do not claim a recovery time until measured.

See [cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/cost-policy.md)
for resource defaults and backup limitations.

## Moderation

Moderation uses the server config. Database access is the operator
authorization boundary. Every command needs an actor and reason; the
`moderation_audit` table records actor, timestamp, action, target, and reason.

| Command | Effect |
| --- | --- |
| `block-account` | Revokes sessions and denies access and writes. |
| `hide-project` | Keeps data but blocks document, asset, source, history, export, and socket access. |

Open sockets recheck authorization every two seconds; frames already in flight
may still arrive.

```sh
librepaper admin moderate block-account github-handle --config /etc/librepaper/librepaper.toml \
  --actor "on-call@example.org" --reason "automated abuse investigation"
librepaper admin moderate hide-project abusive-project --config /etc/librepaper/librepaper.toml \
  --actor "on-call@example.org" --reason "contains abusive material"
```

## Privacy

[Privacy duties for operators](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md)
covers notices and data requests.
