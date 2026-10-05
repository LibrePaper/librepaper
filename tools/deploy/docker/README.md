# A deployment in containers

The stack runs PostgreSQL, LibrePaper, Caddy, Prometheus, Grafana, and host and
PostgreSQL exporters. Set `LIBREPAPER_VERSION` to a release that includes both
the native metrics listener and TOML configuration support. The historical
v0.0.8 binary does not include the metrics listener; releases from before the
TOML configuration change cannot read `config.toml`.

```sh
cd tools/deploy/docker
cp .env.example .env
cp config.toml.example config.toml
# Edit the public origins in config.toml, set LIBREPAPER_VERSION, DOMAIN,
# ACME_EMAIL, all three passwords below, and an OAuth ID/secret pair.
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

Monitoring starts with the rest of the stack. Visit
`https://$DOMAIN/admin/monitoring/` and sign in as `admin` with
`GRAFANA_ADMIN_PASSWORD` from `.env`. Grafana opens the provisioned
LibrePaper operations dashboard, which includes HTTP traffic, latency,
resource refusals, active collaboration, app budgets, host load and memory,
database activity, storage, scrape targets, and firing Prometheus alerts.
Provisioned dashboards and alert rules are maintained in `monitoring/` and
reload when their containers are recreated.

The application metrics listener binds to `0.0.0.0:9091` inside its container
by default. Prometheus scrapes it every 15 seconds on the Compose `internal`
network; no app, Prometheus, exporter, or Grafana port is published on the
host. Caddy routes `/admin/monitoring/` on `DOMAIN` to Grafana and leaves the
documents hostname pointed at LibrePaper only. Grafana login protects its UI,
API, and data-source management. Anonymous access, signups, usage reporting,
update checks, plugin installation, and external plugin key retrieval are
disabled. Prometheus stores bounded aggregate metrics with low-cardinality
route labels; the time series contain no document or user identifiers, request
query strings, or SQL text. Container logs are separate from metric storage
and are capped at 10 MiB per file with three files retained per container.

Prometheus retains at most 30 days of samples and applies an 8 GB TSDB block
size cap. Allow additional filesystem headroom for the active head block and
write-ahead log; retention size is not a hard filesystem quota. Prometheus
data is kept in its own volume and is separate from PostgreSQL backups. The
host exporter mounts `/proc`, `/sys`, and the host root read-only, drops Linux
capabilities, and runs without a writable filesystem. PostgreSQL monitoring
uses the dedicated `librepaper_metrics` login with the `pg_monitor` role; it
exports aggregate built-in statistics and not SQL text.

### Passwords and upgrades

Set all three `POSTGRES_PASSWORD`, `POSTGRES_EXPORTER_PASSWORD`, and
`GRAFANA_ADMIN_PASSWORD` values in `.env` to independent random values
(`openssl rand -hex 32` is suitable for each). The metrics role is
created by the PostgreSQL initialization hook on a new volume. If PostgreSQL
was already initialized before adding monitoring, run the included idempotent
helper once against the running database:

```sh
docker compose exec -T postgres sh -s < monitoring-user.sh
```

The helper reads `POSTGRES_EXPORTER_PASSWORD` from the PostgreSQL container
environment and does not put it on a process command line. To rotate this
account, change the value in `.env`, recreate the PostgreSQL and exporter
containers with `docker compose up -d postgres postgres-exporter`, then run
the helper again. The Grafana
admin password environment variable seeds a fresh Grafana volume only. For a
rotation, change the admin password in Grafana under **Administration → Users
and access → Users → admin**, then replace `GRAFANA_ADMIN_PASSWORD` in
`.env` with that value so the next fresh volume uses it too. Updating only the
environment value does not change the password already stored in Grafana's
volume. Keep that volume protected as administrator credentials and alert
state live there.

The Prometheus volume contains time-series history; keep it out of the
application's document backup set unless monitoring history is needed for a
recovery. Grafana dashboards and data-source configuration are provisioned
from files in this directory, while Grafana's local state remains in its
volume.

### Building a locally built binary

The normal `Dockerfile` downloads a tagged release and verifies its checksum.
Set `LIBREPAPER_VERSION` to a release that includes TOML configuration and
metrics support. The production deployment script checks `admin config check`
on the candidate image before replacing the current config or restarting.
For a locally built static musl binary, copy it to this directory as `librepaper`
and use the alternate Dockerfile:

```sh
docker build -f Dockerfile.local -t librepaper:local .
```

Run that command from `tools/deploy/docker` with the executable in the same
directory. The executable must target the same architecture as the Docker
host; supported targets are `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`.

For the full stack with the local binary, use
`docker compose -f compose.yaml -f compose.local.yaml up -d --build` from this
directory. The local binary path is optional for source builds and forks.

## Backups

Both volumes, together, at one instant. `docker compose exec` the backup
command rather than copying the volume out from under a running server:

```sh
docker compose exec librepaper librepaper admin backup \
  --config /etc/librepaper/config.toml /var/backups/librepaper/$(date +%F)
```

Copy the result off this machine, and test a restore into an empty database
and a path that does not exist. A backup that has never been restored is a
guess.

## Upgrading

Set `LIBREPAPER_VERSION` to the exact compatible release tag and run
`docker compose up -d --build`. The image is built from the
tagged release archive and checked against its checksums. To run a fork, build
a static musl binary and use `Dockerfile.local`.

## S3 instead of local objects

The filesystem profile keeps objects in the `data` volume. To use a bucket,
change the existing `[storage]` table in `config.toml`; add `[storage.s3]` and
provide credentials through explicit references:

```toml
object_store = "s3"

[storage.s3]
bucket = "librepaper-production"
region = "us-east-1"
access_key_id = { env = "LIBREPAPER_S3_ACCESS_KEY_ID" }
secret_access_key = { env = "LIBREPAPER_S3_SECRET_ACCESS_KEY" }
```

Set those names in `.env`. Ambient AWS credential discovery is not used.

The data volume is still where the deployment's own secrets live, so it is
still worth backing up.
