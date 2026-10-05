---
title: "Self-hosting"
---

> LibrePaper is experimental. Self-host only if you can maintain the database,
> storage, credentials, and recovery copies described here.

## Docker Compose quickstart

The versioned kit in `deploy/` includes pinned app and backup images, Caddy,
templates, and a setup helper. Install Docker Engine with Compose **2.24.4 or
newer**, Python **3.11 or newer**, `openssl`, and `curl`. Open TCP ports 80 and
443. `deploy/setup` validates config and writes non-secret Compose state; it
does not start services or replace an existing database. Read the separate
[credential guide](credentials.html) for the secret file inventory and rotation
procedures.

Set DNS for the app and docs origins to this host; the site origin is optional.
In `deploy/`, edit `librepaper.toml` with those origins, access rules, and at least one OAuth
provider. Put provider credentials in separate files under `secrets/`, using
the names `github_client_id`, `github_client_secret`, `google_client_id`, and
`google_client_secret` as needed. Restrict the directory to mode 0700 and files
to 0444. Never put credentials in `.env`, `librepaper.toml`, shell arguments,
or a shared `all-secret.env`. For monitoring, add
`secrets/grafana_admin_password` before initialization.

```sh
cd deploy
umask 077
mkdir -m 700 -p secrets
read -r -p 'GitHub OAuth client ID: ' github_id
read -r -s -p 'GitHub OAuth client secret: ' github_secret; printf '\n'
printf '%s' "$github_id" | ./setup set-secret github_client_id
printf '%s' "$github_secret" | ./setup set-secret github_client_secret
unset github_id github_secret
./setup init --database local
./setup check
docker compose up -d --wait
```

Local initialization creates separate random URLs/passwords for the app,
migrations, backup, and optional metrics exporter. PostgreSQL stays on the
private Compose network; it has no public host port. The runtime role cannot
apply DDL. A one-shot migration service uses the database owner role. Check
`./setup status`, `./setup check`, and `docker compose ps`. `/health` is a
liveness endpoint; use `/ready` and container health to confirm database and
writer readiness.

| Service | Status | Purpose |
| --- | --- | --- |
| `librepaper` | Core | Single app writer and HTTP API |
| `migrate` | Core, one-shot | Applies schema changes with the owner role |
| `postgres` | Core for local DB mode | Private local PostgreSQL |
| `backup` | Core sidecar | Reports disabled/failing status; schedules work only when configured |
| `caddy` | Core | HTTPS, certificates, reverse proxy, and static site |
| Prometheus, Grafana, exporters | Optional | Private monitoring and database metrics |

Caddy is the only service that publishes ports; the database and monitoring
endpoints remain private.

## Credentials and external PostgreSQL

Application credentials are mounted individually, read-only, from files in
`deploy/secrets/`. Rotate a file by writing a replacement to a temporary file
in the same protected directory, setting mode 0444, and atomically renaming it
over the old file. Recreate the services that consume it. Rotate an OAuth
secret at the provider too. For Grafana, rotate through Grafana's administrator
interface and then update its secret file; changing the file alone does not
change a password already stored in an existing Grafana volume. Revoke OAuth by
disabling the provider credential, removing its file and `[auth.*]` table, and
recreating the app.

For a database password rotation, change one role at a time, atomically replace
that service's URL file with the new URL, then recreate that role's consumer.
The owner URL is used only by migrations, the runtime URL by the app, and the
backup URL by the backup sidecar. Revoke a database credential with
`ALTER ROLE role_name NOLOGIN` and terminate its existing sessions; remove its
URL file after confirming no service still needs it. Re-enable the role only
after installing a fresh password and updating the matching URL file.

For external PostgreSQL, create separate roles for runtime, migrations,
backups, and metrics. Grant the runtime role only needed DML rights, make the
migration role the schema owner, keep the backup role read-only, and grant the
metrics role `pg_monitor`. Supply complete URLs in
`database_app_url`, `database_owner_url`, `database_backup_url`, and
`database_metrics_url` under `secrets/`; use generated URL-safe passwords.
For example, read the URL from a protected file rather than putting it on the
command line: `./setup set-secret database_app_url < /secure/path/app-url`.
Then run `./setup init --database external`. Require `sslmode=verify-full`
and a trusted server certificate. Do not reuse a superuser URL across
services. The app holds its writer lease on a PostgreSQL session, so use
session pooling if a pooler is present; transaction pooling can move the lease
to a different backend session. Set `storage.database_connections` within the
database connection limit, leaving capacity for migrations, backups, and
monitoring.

The supported topology has one app writer and one active backup scheduler per
database. Room state, in-memory caches, and background jobs are process-local,
and the writer lease rejects a second writer. Scale vertically or move the
database and object storage to managed services while keeping one writer. Do
not run two deployments against one database or data volume.

## Optional monitoring

Monitoring is opt-in. Supply a password without echoing it, then run
`./setup init --monitoring` and `./setup check`:

```sh
read -r -s -p 'Grafana admin password: ' grafana_password; printf '\n'
printf '%s' "$grafana_password" | ./setup set-secret grafana_admin_password
unset grafana_password
./setup init --monitoring
./setup check
```

Grafana is served at
`https://<app-origin>/admin/monitoring/`; Prometheus and exporters stay on the
private network. Use an external uptime monitor because the local stack cannot
report a VPS outage.

The backup sidecar starts even when backups are disabled so it can report the
disabled state. An idle sidecar is not evidence of a successful backup.
Backups stay disabled until `resticprofile.toml` has a valid
`[resticprofile]` table.

## Backups and recovery

Keep application settings in `librepaper.toml`; repository credentials and
backup settings belong in `resticprofile.toml` and use resticprofile's own
secret handling. Configure a Restic repository, password, and any remote
credentials, then recreate the backup container and validate the merged
profile:

```sh
docker compose up -d --force-recreate backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml \
  -n resticprofile show
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml \
  -n resticprofile backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml \
  -n resticprofile snapshots
```

Grafana can show backup status, but it cannot reliably alert when the VPS,
application, or backup scheduler is unavailable. Add two separate external
dead-man checks, one for scheduled backups and one for repository checks. For
example, create distinct Healthchecks-style ping URLs and put their tokens only
in the private `resticprofile.toml` on the host and in the encrypted recovery
packet. Do not commit these URLs or put them in `.env`.

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

[[resticprofile.check.send-before]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>/start"
[[resticprofile.check.send-after]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>"
[[resticprofile.check.send-after-fail]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>/fail"
```

Set the external missed-success deadlines to 36 hours for backup and 8 days
for repository checks, matching the shipped schedules. Change those deadlines
when you change the schedules. These alerts still reach you when the app is
healthy but backups or repository checks stop succeeding.

The default profile schedules a daily backup and a Monday repository check.
Each backup contains a consistent PostgreSQL dump, referenced objects, both
configs, and the session key. Move the repository off-host. Keep an encrypted
off-host recovery packet with the matching kit/release, both configs, Restic
repository location and password, remote-storage and OAuth provider
credentials, a way to recreate database roles, and the matching session key.
Keep access separate from the VPS.

Before serving real documents, make a successful remote snapshot and restore it
into a separate Compose project, database, and data volume. Verify a restored
document, its objects, and the session key without replacing production data.
Record release, snapshot age, elapsed time, and outcome; repeat after changing
the backup format, storage backend, credentials, or a major release. A green
backup status alone does not demonstrate that recovery works. Follow the
[isolated recovery procedure](recovery.html) for the owner-role restore command,
volume ownership, port and subnet isolation, and post-restore checks.

Losing the session key invalidates existing sessions and prevents the server
from revealing old share URLs. Existing URLs can still work if their database
rows and objects are restored. Restore the matching key from the off-host
packet rather than generating a replacement.

## Upgrades and rollback

For a normal release upgrade, first confirm a recent successful off-host
snapshot and `./setup check`. Pull matching images, stop both the app and
backup scheduler, run the one-shot migration, then start the consumers:

```sh
./setup init --version <release-tag> --no-local-build
docker compose pull librepaper migrate backup
docker compose stop librepaper backup
docker compose run --rm --no-deps migrate
docker compose up -d --wait
```

Do not use `docker compose up -d` alone as a schema-upgrade procedure. The
version must select matching app, migration, and backup images. To roll back
after an incompatible migration, stop the app and backup scheduler and restore
the matching database, data, configuration, and session key from a known-good
snapshot. Do not start an older binary on a newer schema unless that release
supports it.

Older local installs that use PostgreSQL socket trust need an explicit role
upgrade. First preserve an offline copy of the old `.env`, app config, database
volume, data volume, and session key. Stage a config with
`storage.database_url = { file = "/run/secrets/database_url" }` and
`[server] migrate = false` as `librepaper.toml.candidate`. Then run:

```sh
./setup upgrade --yes --version <compatible-release> \
  --config-file ./librepaper.toml.candidate
mv librepaper.toml.candidate librepaper.toml
./setup check
docker compose pull librepaper migrate backup
docker compose run --rm --no-deps migrate
docker compose up -d --wait
```

The upgrade imports old `.env` credentials into individual files, so keep that
file intact until `setup upgrade` succeeds. It stops the app and backup
scheduler and preserves the database volume and session key. Install the new
config only after role conversion succeeds. The official host can use
`tools/deploy/production upgrade-database` to stage and activate that config
around the same role conversion. Routine deployment refuses legacy state.

If the upgrade names an `.env` key with quoted, escaped, interpolated, or
commented syntax, it refuses before writing secrets or stopping services.
Keep the offline copy, determine the exact value the old Compose setup used,
write it to the matching file with `./setup set-secret <name>` via stdin, then
remove that key from `.env` and retry. Do not strip quotes or escapes by hand;
see the [credential guide](credentials.html) for safe stdin handling.

## Configuration

The app reads `/etc/librepaper/librepaper.toml`; the backup process reads
`/etc/resticprofile/resticprofile.toml`. Application `{ file = "..." }`
references supply one whole value from a mounted secret file. Database URLs
are secrets and should not be literals. Backup settings use resticprofile's
own syntax; the app rejects legacy `[backup]` and `[resticprofile]` tables in
its configuration.

`admin config check` validates configuration without a database connection.
`admin config show` displays defaults and provenance while redacting
credentials and the database URL. Keep one `admin serve` process per database.
For S3-compatible storage, configure `[storage.s3]` explicitly; ambient AWS
credentials are not used.

### Without Docker

Install PostgreSQL and create a dedicated database role and database. The
standalone server defaults to applying migrations at startup, so this role must
own the schema. Set `storage.database_url` to a protected literal or a file
reference, then run one server process:

```sh
export LIBREPAPER_DATABASE_URL='postgresql://librepaper:SECRET@127.0.0.1/librepaper'
librepaper admin config check --config /etc/librepaper/librepaper.toml
librepaper admin serve --config /etc/librepaper/librepaper.toml
```

Keep the PostgreSQL cluster and `storage.directory` on durable storage and
include the data directory and session key in the recovery plan. Put the
server behind an HTTPS reverse proxy that preserves the request `Host`, appends
the actual client address to `X-Forwarded-For`, and trusts only that proxy's
network in `[proxy].trusted_networks`. Run exactly one `admin serve` process
per database: the writer lease stops a second process from accepting writes,
and room state, caches, and scheduled work are process-local. A native install
can use the same external PostgreSQL, object storage, backup, and recovery
principles described above; Docker Compose's scoped `app` and `migrate` roles
are provisioned by `deploy/setup` and are not required for this mode.

## Moderation and privacy

Database access is the operator authorization boundary. Every moderation
command needs an actor and reason; the `moderation_audit` table records actor,
timestamp, action, target, and reason.

| Command | Effect |
| --- | --- |
| `block-account` | Revokes sessions and denies access and writes. |
| `hide-project` | Keeps data but blocks document, asset, source, history, export, and socket access. |

Open sockets recheck authorization every two seconds; frames already in flight
may still arrive. [Privacy duties for operators](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md)
covers notices and data requests.
