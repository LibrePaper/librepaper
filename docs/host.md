---
title: "Self-hosting"
---

> **Warning:** LibrePaper is experimental. We do not recommend self-hosting
> yet; this guide is for interested readers and system administrators.

## Docker Compose

The kit in `tools/deploy/docker/` runs PostgreSQL, LibrePaper and Caddy; you write one file.

```sh
cd tools/deploy/docker
cp config.toml.example config.toml
# edit [origins] and one [auth.*] table
docker compose up -d
```

Before the first start:
- Both origin names need DNS records pointing here before the first start: Caddy gets a certificate for each on first request.
- OAuth callback URLs: `https://<app-origin>/auth/callback` (GitHub), `https://<app-origin>/auth/callback/google` (Google).
- `[access]`: `publishers` and `commenters` take `["any"]` or GitHub logins, verified Google addresses and `@domain` entries.
- `[retention]` is off by default.

### Remote database

Set `storage.database_url` in `config.toml` to the full database URL and use the remote compose file:

```sh
docker compose -f compose.remote.yaml up -d
```

For a managed service, add `?sslmode=verify-full` to the URL.

### Volumes

- `postgres`: the database (documents, update log, comments)
- `data`: immutable objects and the secrets that keep sessions and share links valid. Back it up even with S3.
- `caddy-data`: certificates

### Upgrade

```sh
# Edit LIBREPAPER_VERSION in compose.yaml or export it
LIBREPAPER_VERSION=v0.0.10 docker compose up -d --build
```

### Backups

```sh
docker compose exec librepaper librepaper admin backup \
  --config /etc/librepaper/config.toml /var/backups/librepaper/$(date +%F)
```

Restore: see [Storage and backup](#storage-and-backup).

## Configuration

Server and admin commands use one TOML file. `admin serve` defaults to
`/etc/librepaper/config.toml`; use `--config PATH` to choose another file.

- Literals use TOML syntax.
- `{ env = "NAME" }` and `{ file = "path" }` replace one whole value.
  Referenced strings are used as-is; numbers, booleans, and arrays use TOML
  syntax.
- File paths are config-relative; trailing CR/LF is stripped from references.

Missing or empty references, invalid keys/types, and old server-setting flags
fail. There is no interpolation or automatic application-setting override.

Example configuration:

```toml
[server]
address = "0.0.0.0:8080"
local_companion = false

[origins]
app = "https://app.example.org"
docs = "https://docs.example.org"

[storage]
directory = "/var/lib/librepaper"
database_url = { env = "LIBREPAPER_DATABASE_URL" }
database_connections = 20
fsync = true
object_store = "filesystem"

[auth.github]
client_id = { env = "LIBREPAPER_GITHUB_CLIENT_ID" }
client_secret = { env = "LIBREPAPER_GITHUB_CLIENT_SECRET" }

[proxy]
trusted_networks = ["172.29.0.0/16"]
```

Use these commands to check or inspect resolved settings. `check` has no
startup side effects or database connection; `show` includes defaults and
provenance and redacts credentials and the database URL.

```sh
librepaper admin config check --config /etc/librepaper/config.toml
librepaper admin config show --config /etc/librepaper/config.toml
```

## Without Docker

1. Create a PostgreSQL role and database with a generated password:

   ```sql
   create role librepaper login password 'replace-with-a-generated-secret';
   create database librepaper owner librepaper;
   ```

2. Supply the complete database URL and the configured OAuth values through
   environment or secret-file references. Keep secrets out of the TOML.

   ```sh
   export LIBREPAPER_DATABASE_URL='postgresql://librepaper:SECRET@127.0.0.1/librepaper'
   librepaper admin serve --config /etc/librepaper/config.toml
   ```

3. Keep PostgreSQL and the data directory on durable storage. Run only one
   `admin serve` per database. Put the server behind an HTTPS reverse proxy
   that preserves `Host`, appends the actual client address to
   `X-Forwarded-For`, and trusts only the proxy network.

## Storage and backup

Use the server config for backups. A restore config must identify the target
database; the database must be empty and the destination directory must not
exist. The destination is positional:

```sh
backup="/backups/librepaper-$(date +%F)"
librepaper admin backup --config /etc/librepaper/config.toml "$backup"
librepaper admin restore --config /etc/librepaper/restore.toml \
  "$backup" /librepaper-restored
```

Copy each backup off the machine, and test a restore: a backup that has never
been restored is a guess.

See [cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/cost-policy.md)
for resource defaults and backup limitations.

## Privacy

[Privacy duties for operators](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md)
covers notices and data requests.
