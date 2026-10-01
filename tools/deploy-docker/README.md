# A deployment in containers

The stack runs PostgreSQL, LibrePaper, Caddy, Prometheus, Grafana, and host and
PostgreSQL exporters. Set `LIBREPAPER_VERSION=v0.0.9` in `.env` to deploy the
tagged release, which includes the native metrics listener. The historical
v0.0.8 binary does not include that listener.

```sh
cd tools/deploy-docker
cp .env.example .env
# Set LIBREPAPER_VERSION=v0.0.9, DOMAIN, ACME_EMAIL, LIBREPAPER_PUBLISHERS,
# all three passwords described below, and both client ID and client secret
# for GitHub or Google OAuth.
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

**Say who may publish.** `LIBREPAPER_PUBLISHERS` has no default and the server
will not start without it. Publishing requires GitHub or Google, so a
deployment that admits publishers also needs both the client ID and client
secret for at least one OAuth provider. For GitHub, the callback is
`https://$DOMAIN/auth/callback`; Google's is
`https://$DOMAIN/auth/callback/google`.

**Decide about retention.** Off by default, which is right for a server whose
publishers you know and wrong for one strangers may publish to: that is
durable hosting for whatever they upload. `LIBREPAPER_EXPIRE_AFTER=30d`.

## What is where

| | |
| --- | --- |
| `postgres` volume | the database: documents, the update log, comments, collaboration |
| `data` volume | immutable objects, and the secrets that keep sessions and share links valid |
| `caddy-data` volume | the certificates |
| `config.yaml` | `trusted_proxies`, so visitors are rate-limited by their own address rather than the proxy's |

The schema is migrated at start-up, which is why `librepaper` waits on the
database's health check rather than merely on its container.

## Monitoring

Monitoring starts with the rest of the stack. Visit
`https://$DOMAIN/admin/monitoring/` and sign in as `admin` with
`LIBREPAPER_ADMIN_PASSWORD` from `.env`. Grafana opens the provisioned
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
`LIBREPAPER_ADMIN_PASSWORD` values in `.env` to independent random values
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
and access → Users → admin**, then replace `LIBREPAPER_ADMIN_PASSWORD` in
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
Set `LIBREPAPER_VERSION` to `v0.0.9` or a later metrics-capable release. For a
locally built static musl binary, copy it to this directory as `librepaper`
and use the alternate Dockerfile:

```sh
docker build -f Dockerfile.local -t librepaper:local .
```

Run that command from `tools/deploy-docker` with the executable in the same
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
  --data-directory /var/lib/librepaper /var/backups/librepaper/$(date +%F)
```

Copy the result off this machine, and test a restore into an empty database
and a path that does not exist. A backup that has never been restored is a
guess.

## Upgrading

Set `LIBREPAPER_VERSION` to the exact release tag (for example,
`v0.0.9`) and run `docker compose up -d --build`. The image is built from the
tagged release archive and checked against its checksums. To run a fork, build
a static musl binary and use `Dockerfile.local`.

## S3 instead of local objects

The filesystem profile keeps objects in the `data` volume, which is the right
answer for one machine. For a bucket, add to `.env` and they reach the server
as they stand:

```sh
LIBREPAPER_OBJECT_STORE=s3
LIBREPAPER_S3_BUCKET=librepaper-production
LIBREPAPER_S3_REGION=us-east-1
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
```

The data volume is still where the deployment's own secrets live, so it is
still worth backing up.
