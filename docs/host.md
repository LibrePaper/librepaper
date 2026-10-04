---
title: "Self-hosting"
---

> **Warning:** LibrePaper is experimental software that is evolving rapidly. We do not recommend self-hosting at the moment. These notes are for interested readers and system administrators.

## Configuration

The server and admin commands read one TOML file. `admin serve` defaults to
`/etc/librepaper/config.toml`; pass `--config PATH` to choose another file.
Application settings come from TOML. A value can be a literal or an explicit
`{ env = "NAME" }` / `{ file = "path" }` reference. References are required
when present, and relative file paths resolve from the TOML file's directory.
The PostgreSQL driver can also use standard `PG*` variables for URL components
omitted from the database URL; use a complete URL for predictable setup.

References replace one complete value. Strings need no surrounding quotes;
numbers, booleans, and arrays use TOML syntax, such as `8080`, `true`, or
`["alice", "bob"]`. Trailing line endings are stripped from referenced values.
Missing or empty references, unknown keys, and invalid types stop startup
with a configuration error. Server-setting flags and automatic
`LIBREPAPER_*` overrides are no longer supported.

```toml
[server]
bind = "0.0.0.0"
port = 8080
origin = "https://paper.example"
docs_origin = "https://docs.paper.example"
local_companion = false

[storage]
directory = "/var/lib/librepaper"
database_url = { env = "LIBREPAPER_DATABASE_URL" }
database_connections = 20
fsync = true
object_store = "filesystem"

[auth.github]
client_id = { env = "LIBREPAPER_GITHUB_CLIENT_ID" }
client_secret = { env = "LIBREPAPER_GITHUB_CLIENT_SECRET" }

[access]
publishers = ["YOUR-GITHUB-LOGIN"]
commenters = ["anyone"]
```

At least one OAuth provider is needed to sign in. Each configured provider
requires both a client ID and secret. Omit the whole `[auth.github]` or
`[auth.google]` table when that provider is unused. A missing or empty
referenced value is an error; it is never treated as an absent provider.
Validate and inspect the resolved file with:

```sh
librepaper admin config check --config /etc/librepaper/config.toml
librepaper admin config show --config /etc/librepaper/config.toml
```

`check` only parses and resolves configuration; it does not start the server
or connect to the database. `show` prints resolved settings, defaults, and
their source, while redacting database and credential values.

## Production

Use separate hostnames for the reader and published documents. Set `origin`
and `docs_origin`; the server rejects unrecognized hosts with 421. Install
PostgreSQL separately:

```sql
create role librepaper login password 'replace-with-a-generated-secret';
create database librepaper owner librepaper;
```

The database URL is supplied only because the config explicitly references
`LIBREPAPER_DATABASE_URL`:

```sh
export LIBREPAPER_DATABASE_URL='postgresql://librepaper:SECRET@127.0.0.1/librepaper'
librepaper admin serve --config /etc/librepaper/config.toml
```

Keep the data directory and PostgreSQL on durable storage. To use S3-compatible
storage, add the following to the TOML file and set the two referenced
environment variables. Do not rely on ambient AWS credential discovery:

```toml
[storage]
object_store = "s3"

[storage.s3]
endpoint = "https://s3.example"
region = "us-east-1"
bucket = "librepaper-production"
access_key_id = { env = "LIBREPAPER_S3_ACCESS_KEY_ID" }
secret_access_key = { env = "LIBREPAPER_S3_SECRET_ACCESS_KEY" }
```

Run behind an HTTPS reverse proxy that preserves `Host`. Configure only the
proxy's network under `[proxy]` and make it append the actual client address to
`X-Forwarded-For`:

```toml
[proxy]
trusted_proxies = ["127.0.0.1/32", "::1/128"]
```

Only one `admin serve` should run per database.

## Moderation

Moderation commands use the same configuration file as the server. Database
access is the operator authorization boundary.

```sh
librepaper admin moderate block-account github-handle \
  --config /etc/librepaper/config.toml \
  --actor "on-call@example.org" --reason "automated abuse investigation"
librepaper admin moderate hide-project abusive-project \
  --config /etc/librepaper/config.toml \
  --actor "on-call@example.org" --reason "contains abusive material"
```

Every command requires `--actor` and `--reason`. The `moderation_audit` table
records actor, timestamp, action, target, and reason. Blocking revokes sessions
and denies access and writes. Hiding preserves data while denying document,
asset, source, history, export, and socket access. Open sockets recheck
authorization every 2 seconds; frames already in flight may still arrive.

## Storage, limits, and backup

The `[storage]` table selects the catalog database, object store, and local
server data directory. Restore requires an empty database and a destination
directory that does not exist yet.

Optional capacity settings live under `[limits]`; sizes are MiB:

```toml
[limits]
publisher_storage_mb = 500
deployment_storage_mb = 10240
publisher_uploads_per_hour = 30
```

Expiration is disabled unless configured. `expire_from` accepts `created` or
`updated`:

```toml
[retention]
expire_after = "24h"
expire_from = "created"
```

Back up and restore with the same config used by the server. For restore, the
config must identify the target database; the destination path remains a
positional argument:

```sh
librepaper admin backup --config /etc/librepaper/config.toml \
  /backups/librepaper-$(date +%F)
librepaper admin restore --config /etc/librepaper/restore.toml \
  /backups/librepaper-2026-09-13 /librepaper-restored
```

See [cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/cost-policy.md)
for resource defaults and backup limitations.

## Rights and OAuth

`[access].publishers` controls who may upload; `commenters` controls who may
annotate. Use arrays of GitHub logins, verified Google addresses, or `@domain`
entries. For publishers, `"any"` allows any signed-in account; for commenters,
`"anyone"` does the same. Configure a nonempty publishers policy explicitly.
Document access and share links still require sign-in. Domain matching is
exact: `@example.org` admits `alice@example.org`, not `alice@mail.example.org`.

GitHub OAuth applications use `/auth/callback`; Google Web applications use
`/auth/callback/google`. Keep IDs and secrets outside the TOML file by using
explicit environment or file references. Google sign-in accepts verified
Gmail and Google Workspace addresses.

## Containers

See [tools/deploy-docker](https://github.com/LibrePaper/librepaper/blob/main/tools/deploy-docker/README.md)
for Compose setup with PostgreSQL, HTTPS proxy, and monitoring. Copy
`config.toml.example` to `config.toml`, set the public origins, and edit the
file for OAuth and access policy. `.env` supplies Compose settings and values
that the TOML explicitly references as application settings.

## Privacy

The default asset mirror sees the visitor's IP and asset requests. To self-host
the pinned assets, set `assets.mirror` in TOML to an HTTPS mirror with CORS for
GET and HEAD.

[Privacy duties for operators](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md)
covers publishing a notice and answering data requests.
