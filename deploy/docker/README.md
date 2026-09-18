# A deployment in containers

Three containers: PostgreSQL, LibrePaper, and a proxy holding the certificates
for the deployment's two origins. LibrePaper itself is one static binary with
the pages and the renderers compiled in, so its image is that file on Alpine
and nothing else; the parts worth supervising are the database and the
certificates, and those are what the other two are for.

```sh
cd deploy/docker
cp .env.example .env     # and fill it in
docker compose up -d
```

## Before the first start

**Both names must resolve here.** A deployment answers on two hostnames: the
reader on `DOMAIN`, published documents on `docs.DOMAIN`. An uploaded document
is code, and what keeps it out of a reader's session is that the browser sees
the two as different sites, so this is the setting with no safe default. The
proxy takes a certificate for each over HTTP-01, which it cannot do before the
`A`/`AAAA` records exist. The server refuses any other `Host` with 421.

**Say who may publish.** `LIBREPAPER_PUBLISHERS` has no default and the server
will not start without it. Publishing requires GitHub or Google, so a
deployment that admits publishers also needs an OAuth client, with its two
URLs pointing at `https://$DOMAIN` and `https://$DOMAIN/auth/callback`.

**Decide about retention.** Off by default, which is right for a server whose
publishers you know and wrong for one strangers may publish to: that is
durable hosting for whatever they upload. `LIBREPAPER_EXPIRE_AFTER=30d`.

## What is where

| | |
| --- | --- |
| `postgres` volume | the database: bundles, comments, collaboration |
| `data` volume | immutable objects, and the secrets that keep sessions and share links valid |
| `caddy-data` volume | the certificates |
| `config.yaml` | `trusted_proxies`, so visitors are rate-limited by their own address rather than the proxy's |

The schema is migrated at start-up, which is why `librepaper` waits on the
database's health check rather than merely on its container.

## Backups

Both volumes, together, at one instant. `docker compose exec` the backup
command rather than copying the volume out from under a running server:

```sh
docker compose exec librepaper librepaper admin backup create \
  --data-directory /var/lib/librepaper /var/backups/librepaper/$(date +%F)
```

Copy the result off this machine, and test a restore into an empty database
and a path that does not exist. A backup that has never been restored is a
guess.

## Upgrading

Set `LIBREPAPER_VERSION` to the new tag and `docker compose up -d --build`.
The image is built from the published release archive and checked against the
release checksums, so an upgrade either gets the binary that was released or
fails. To run a fork instead, build it with `make build` and replace the fetch
in the `Dockerfile` with a `COPY`.

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
