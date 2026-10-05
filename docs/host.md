---
title: "Self-hosting"
---

> LibrePaper is experimental. Self-host only if you can maintain its database,
> files, credentials, and recovery copies.

## Single VPS setup

Download the matching deployment kit and checksum from [GitHub Releases](https://github.com/LibrePaper/librepaper/releases).
The kit's [README](https://github.com/LibrePaper/librepaper/blob/main/deploy/README.md)
has extraction instructions. You need Docker Engine with Compose 2.24.4 or
newer, Python 3.11 or newer, `openssl`, and `curl`. Allow inbound TCP ports 80
and 443. Point two different hostnames to the VPS: one for the app and one for
published documents. Replace `paper.example` and `docs.paper.example` below
with your own hostnames.

In `deploy/librepaper.toml`, set `[origins].app` to `https://paper.example`,
`[origins].docs` to `https://docs.paper.example`, set access rules, and enable
at least one OAuth provider. The template uses GitHub. Create a GitHub OAuth
app with callback `https://paper.example/auth/callback`, then enter its
credentials below. `set-secret` creates protected files under `secrets/`.

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

Caddy obtains TLS certificates and is the only service that publishes ports.
Check `./setup status`, `docker compose ps`, and
`curl -fsS https://paper.example/ready` after startup.

## Optional monitoring

Monitoring is off by default. Set a private Grafana password, enable the
overlay, then start the services:

```sh
read -r -s -p 'Grafana password: ' grafana_password; printf '\n'
printf '%s' "$grafana_password" | ./setup set-secret grafana_admin_password
unset grafana_password
./setup init --monitoring
./setup check
docker compose up -d --wait
```

Grafana is at `https://paper.example/admin/monitoring/`. Prometheus and exporters
stay private. Use an external uptime monitor because this VPS cannot report
its own outage.

## Database and scaling

The local PostgreSQL service has no public port. The app, migrations, backup,
and metrics exporter use separate credentials. For a new install with managed
PostgreSQL, follow the [credential guide](credentials.html): create
separate runtime, migration, backup, and metrics roles; supply their URLs in
the corresponding `database_*_url` files; require `sslmode=verify-full`; and
run `./setup init --database external`. Keep connection capacity for every
service. Use session pooling if you have a pooler.

Run one app instance and one backup scheduler per database. Scale vertically
or move PostgreSQL and object storage to managed services.

## Backups and alerts

Backups are disabled until `resticprofile.toml` has a `[resticprofile]` table.
Configure a remote Restic repository and its password or credentials in that
file, then recreate and check the backup service:

```sh
docker compose up -d --force-recreate backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml \
  -n resticprofile show
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml \
  -n resticprofile backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml \
  -n resticprofile snapshots
```

Backups contain a database dump, referenced objects, config, and `session.key`.
Keep the matching key to preserve existing sessions and saved share secrets.
Move the repository and a matching credential packet off the VPS. Confirm a
successful remote snapshot and practice an isolated restore before storing
important documents. See the [recovery guide](recovery.html).

Grafana reports status but cannot alert if the VPS or backup scheduler is
unavailable. Configure separate external checks for backups and repository
checks, with missed-success deadlines of 36 hours and 8 days. Keep ping URLs
private. The [recovery guide's alert example](recovery.html#external-backup-alerts)
shows start, success, and failure hooks for both jobs.

## Upgrades

Before a release upgrade, confirm a recent off-host backup. Select matching
images, stop the app and backup service, migrate, then start the stack:

```sh
./setup init --version <release-tag> --no-local-build
docker compose pull librepaper migrate backup
docker compose stop librepaper backup
docker compose run --rm --no-deps migrate
docker compose up -d --wait
```

An incompatible migration may require restoring the matching database, files,
config, and session key. Do not start an older binary on a newer schema unless
it supports that schema.

Older local installs using PostgreSQL socket trust need an explicit role
upgrade. Preserve an offline copy of `.env`, config, database and data volumes,
and `session.key`. Stage a config with
`storage.database_url = { file = "/run/secrets/database_url" }` and
`[server] migrate = false` in `librepaper.toml.candidate`, then run:

```sh
./setup upgrade --yes --version <compatible-release> \
  --config-file ./librepaper.toml.candidate
mv librepaper.toml.candidate librepaper.toml
./setup check
docker compose pull librepaper migrate backup
docker compose run --rm --no-deps migrate
docker compose up -d --wait
```

Setup imports legacy `.env` credentials. If it refuses unsupported quoting,
escaping, or interpolation, keep the old file and use the safe repair steps in
the [credential guide](credentials.html). Do not change quoting blindly.

## Without Docker

Run one `librepaper admin serve` process against durable PostgreSQL and file
storage, behind an HTTPS proxy that preserves `Host` and forwards the client
address. Configure LibrePaper to trust only that proxy's network. The server applies
migrations at startup, so its database role must own the schema. See the
[CLI guide](cli.html) for config commands.

For operator privacy and data requests, see [privacy duties](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md).
