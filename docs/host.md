---
title: "Self-hosting"
---

> **Warning:** LibrePaper is experimental. We do not recommend self-hosting
> yet; this guide is for interested readers and system administrators.

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

The configuration below is the `librepaper.org` production file, included
directly from [tools/deploy-production.toml][production-config]. Adapt its
domains, credentials, storage, and policies before use.

<!-- include: tools/deploy-production.toml -->

Use these commands to check or inspect resolved settings. `check` has no
startup side effects or database connection; `show` includes defaults and
provenance and redacts credentials and the database URL.

```sh
librepaper admin config check --config /etc/librepaper/config.toml
librepaper admin config show --config /etc/librepaper/config.toml
```

## Production setup

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

4. For S3-compatible storage, edit the existing `[storage]` table and enable
   the commented `[storage.s3]` example; do not create a duplicate table.

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
librepaper admin moderate block-account github-handle --config /etc/librepaper/config.toml \
  --actor "on-call@example.org" --reason "automated abuse investigation"
librepaper admin moderate hide-project abusive-project --config /etc/librepaper/config.toml \
  --actor "on-call@example.org" --reason "contains abusive material"
```

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

See [cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/cost-policy.md)
for resource defaults and backup limitations.

## Containers and privacy

[The Docker deployment guide](https://github.com/LibrePaper/librepaper/blob/main/tools/deploy-docker/README.md)
covers Compose, PostgreSQL, HTTPS, and monitoring. Copy
`config.toml.example` to `config.toml` and adapt it before starting. Compose
uses `.env` for its own settings and passes values referenced explicitly by
the TOML.

[Privacy duties for operators](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md)
covers notices and data requests.

[production-config]: https://github.com/LibrePaper/librepaper/blob/main/tools/deploy-production.toml
