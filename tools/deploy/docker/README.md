# A deployment in containers

The stack runs PostgreSQL, LibrePaper and Caddy; monitoring is optional (see below).
Set `LIBREPAPER_VERSION` to a release that includes both
the native metrics listener and TOML configuration support. The historical
v0.0.8 binary does not include the metrics listener; releases from before the
TOML configuration change cannot read `config.toml`.

```sh
cd tools/deploy/docker
cp .env.example .env
cp config.toml.example config.toml
# Edit the public origins in config.toml, set LIBREPAPER_VERSION, DOMAIN,
# ACME_EMAIL, POSTGRES_PASSWORD, and an OAuth ID/secret pair.
docker compose up -d --build
```

## Before the first start

**Both names must resolve here.** A deployment answers on two hostnames: the
reader on `DOMAIN`, published documents on `DOCS_DOMAIN` (`docs.DOMAIN` unless
set). An uploaded document is code, and what keeps it out of a reader's session
is that the browser sees the two as different sites, so this is the setting
with no safe default. The proxy takes a certificate for each over HTTP-01,
which it cannot do before the `A`/`AAAA` records exist. The server refuses any
other `Host` with 421.

**Configure sign-in and access.** Uncomment an `[auth.github]` or
`[auth.google]` table in `config.toml` and set both referenced values in `.env`.
Missing or empty references fail configuration loading. At least one provider
is needed for sign-in and publishing. Set `access.publishers` to `["any"]`
for any signed-in account, or list allowed GitHub logins, verified Google
addresses, and `@domain` entries. `access.commenters` uses the same format and
defaults to any signed-in account. Document access requires sign-in. For
GitHub, the callback is
`https://$DOMAIN/auth/callback`; Google's is
`https://$DOMAIN/auth/callback/google`.

Set retention under `[retention]` in `config.toml`. Expiration is disabled by
default; consider a lifetime when strangers can publish. The config example
sets 50 MiB retained storage and 30 uploads per publisher per hour; edit
`[limits]` there to change those settings.

## What is where

| | |
| --- | --- |
| `postgres` volume | the database: documents, the update log, comments, collaboration |
| `data` volume | immutable objects, and the secrets that keep sessions and share links valid |
| `caddy-data` volume | the certificates |
| `config.toml` | all application settings, including the trusted proxy network |

The schema is migrated at start-up, which is why `librepaper` waits on the
database's health check rather than merely on its container.

## Monitoring

Opt in by listing both files in `.env`, and set the two passwords:

```sh
COMPOSE_FILE=compose.yaml:compose.monitoring.yaml
POSTGRES_EXPORTER_PASSWORD=...   # openssl rand -hex 32
GRAFANA_ADMIN_PASSWORD=...       # openssl rand -hex 32
```

Then `docker compose up -d`. Visit `https://$DOMAIN/admin/monitoring/` and sign
in as `admin`. Grafana opens the provisioned LibrePaper operations dashboard:
traffic, latency, refusals, collaboration, budgets, host load, database
activity, storage, scrape targets and firing alerts. Dashboards and alert rules
live in `monitoring/` and reload when their containers are recreated.

- No app, Prometheus, exporter or Grafana port is published. Prometheus scrapes
  the app's metrics listener (`0.0.0.0:9091` in the container) every 15 seconds
  over the `internal` network.
- Caddy routes `/admin/monitoring/` on `DOMAIN` to Grafana. The documents
  hostname never reaches it.
- Grafana disables anonymous access, signups, usage reporting, update checks and
  plugin installation.
- Metrics are aggregate with low-cardinality route labels: no document or user
  identifiers, query strings or SQL text.
- Container logs are capped at 10 MiB per file, three files per container.
- Prometheus keeps 30 days and an 8 GB block cap in its own volume, outside the
  document backups. The cap is not a filesystem quota; leave headroom.
- The host exporter mounts `/proc`, `/sys` and `/` read-only, drops all
  capabilities and has no writable filesystem.
- PostgreSQL monitoring uses the `librepaper_metrics` login with `pg_monitor`.

### Passwords and upgrades

The metrics role is created by the PostgreSQL init hook on a new volume. To
enable monitoring on an existing database, run the idempotent helper once:

```sh
docker compose exec -T postgres sh -s < monitoring-user.sh
```

It reads `POSTGRES_EXPORTER_PASSWORD` from the container environment, never a
command line. To rotate that password, change `.env`, run
`docker compose up -d postgres postgres-exporter`, then run the helper again.

`GRAFANA_ADMIN_PASSWORD` seeds a fresh Grafana volume only. To rotate it, change
the password in Grafana under **Administration, Users and access, Users,
admin**, then put the same value in `.env` for the next fresh volume. Keep the
Grafana volume protected: administrator credentials and alert state live there.

## Backups

```sh
docker compose exec librepaper librepaper admin backup \
  --config /etc/librepaper/config.toml /var/backups/librepaper/$(date +%F)
```

Restore and the rest of backup policy: [Storage and backup](https://librepaper.org/host.html#storage-and-backup).

## Upgrading

Set `LIBREPAPER_VERSION` to the exact compatible release tag and run
`docker compose up -d --build`. The image is built from the tagged release
archive and checked against its checksums.

To run a fork or an unreleased build, copy a static musl binary (same
architecture as the host) here as `librepaper`, then:

```sh
# .env
LIBREPAPER_SOURCE=local
```

```sh
docker compose up -d --build
```

## S3 instead of local objects

The filesystem profile keeps objects in the `data` volume, which still holds the deployment's secrets and is worth backing up.
To use S3, see [Self-hosting](https://librepaper.org/host.html#production-setup); put its credentials in `.env`.
