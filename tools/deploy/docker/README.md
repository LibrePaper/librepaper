# Docker Compose deployment

PostgreSQL, LibrePaper and Caddy; monitoring is optional.

## Start

```sh
cd tools/deploy/docker
cp .env.example .env               # LIBREPAPER_VERSION, DOMAIN, ACME_EMAIL, POSTGRES_PASSWORD, OAuth pair
cp config.toml.example config.toml # public origins, [auth.*], [access], [limits], [retention]
docker compose up -d --build
```

Before the first start:
- `DOMAIN` and `DOCS_DOMAIN` (default `docs.DOMAIN`) both need DNS records pointing here first: Caddy gets a certificate for each over HTTP-01. Keep them separate sites: published documents are code. Any other `Host` gets 421.
- Sign-in needs at least one `[auth.github]` or `[auth.google]` table, with both values in `.env`. Callbacks: `https://$DOMAIN/auth/callback` (GitHub), `https://$DOMAIN/auth/callback/google` (Google).
- `[access]`: `publishers` and `commenters` take `["any"]` or GitHub logins, verified Google addresses and `@domain` entries.
- `[retention]`: expiration is off by default; set a lifetime if strangers can publish.

## Volumes

- `postgres`: the database (documents, update log, comments)
- `data`: immutable objects and the secrets that keep sessions and share links valid. Back it up even with S3.
- `caddy-data`: certificates

## Monitoring

```sh
# .env
COMPOSE_FILE=compose.yaml:compose.monitoring.yaml
POSTGRES_EXPORTER_PASSWORD=...   # openssl rand -hex 32
GRAFANA_ADMIN_PASSWORD=...       # openssl rand -hex 32
```

```sh
docker compose up -d
docker compose exec -T postgres sh -s < monitoring-user.sh   # existing database only: create the metrics role
```

- Grafana: `https://$DOMAIN/admin/monitoring/`, user `admin`. No other monitoring port is published.
- Rotate the exporter password: change `.env`, `docker compose up -d postgres postgres-exporter`, rerun `monitoring-user.sh`.
- Rotate the Grafana password in Grafana (Administration, Users and access, Users, admin), then copy it to `.env`: the variable only seeds a fresh volume.
- Prometheus keeps 30 days, capped at 8 GB (not a filesystem quota; leave headroom).

## Backups

```sh
docker compose exec librepaper librepaper admin backup \
  --config /etc/librepaper/config.toml /var/backups/librepaper/$(date +%F)
```

Restore and policy: [Storage and backup](https://librepaper.org/host.html#storage-and-backup).

## Upgrade

```sh
# .env: LIBREPAPER_VERSION=<tag>   (downloaded and checksum-verified)
# or, for a fork: copy a static musl binary here as ./librepaper and set LIBREPAPER_SOURCE=local
docker compose up -d --build
```

## S3

See [Self-hosting](https://librepaper.org/host.html#production-setup); credentials go in `.env`.
