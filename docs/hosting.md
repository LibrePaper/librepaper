# Self-managed hosting

## Two origins

A deployment answers on two hostnames and refuses every other. The reader is
one; published documents are the other. An uploaded document is code, and what
stops it reaching a reader's session is that the browser sees the two as
different origins, so this is the first thing to get right and the one setting
with no safe default.

```sh
librepaper admin serve --origin https://paper.example
```

That serves the reader on `paper.example` and documents on
`docs.paper.example`. Both names need a DNS record pointing at this deployment
and a certificate covering them; a wildcard or a SAN entry covers the second.
Pass `--docs-origin` if documents belong on an unrelated name. The two must be
different hosts. A different port is not enough, because cookies ignore ports,
and the server refuses to start if both are the same host.

The origin also fixes the scheme the server believes it is on, which is what
decides whether session cookies are marked `Secure` and carry the `__Host-`
prefix, and it is the origin OAuth callbacks are built from. A request arriving
with any other `Host` is answered with 421 rather than served on a guess.

Without `--origin` the deployment answers on loopback only, which is what
development uses. That is not a production configuration: a reverse proxy
forwarding a public name to it will get 421 for every request.

LibrePaper uses PostgreSQL for all relational and collaboration metadata. A
small deployment can run PostgreSQL and LibrePaper on the same machine. The
application does not install or supervise PostgreSQL and has no SQLite mode.

Create a dedicated role and database using your operating system's PostgreSQL
package:

```sql
create role librepaper login password 'replace-with-a-generated-secret';
create database librepaper owner librepaper;
```

Prefer peer authentication over a protected Unix socket when the service runs
as the matching operating-system user:

```sh
export LIBREPAPER_DATABASE_URL=postgresql:///librepaper
librepaper admin serve \
  --data-directory /var/lib/librepaper \
  --database-connections 20 \
  --port 8080
```

For password authentication, bind PostgreSQL only to loopback and use a secret
environment file readable by the LibrePaper service account:

```sh
export LIBREPAPER_DATABASE_URL='postgresql://librepaper:SECRET@127.0.0.1/librepaper'
```

The low-cost filesystem profile stores immutable objects below
`/var/lib/librepaper/objects`. Keep the database and object directory on SSD
storage. The recommended small production profile keeps PostgreSQL and the
application on the same host while putting objects in an S3-compatible bucket:

```sh
export LIBREPAPER_OBJECT_STORE=s3
export LIBREPAPER_S3_REGION=us-east-1
export LIBREPAPER_S3_BUCKET=librepaper-production
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
librepaper admin serve --data-directory /var/lib/librepaper
```

Set `LIBREPAPER_S3_ENDPOINT` for a non-AWS provider and
`LIBREPAPER_S3_ALLOW_HTTP=true` only for a trusted local test endpoint. Bucket
versioning, lifecycle expiry for temporary prefixes, billing alerts, and a hard
provider spending cap are recommended.

Install PostgreSQL client tools on the machine that runs backups. For local
objects, create a complete recovery point with:

```sh
librepaper admin backup create \
  --database-url "$LIBREPAPER_DATABASE_URL" \
  --data-directory /var/lib/librepaper \
  /srv/backups/librepaper-$(date +%F)
```

Copy the resulting directory off the primary machine. Test restoration against
an empty database and a path that does not exist:

```sh
createdb librepaper_restore
LIBREPAPER_DATABASE_URL=postgresql:///librepaper_restore \
  librepaper admin backup restore \
  /srv/backups/librepaper-2026-09-13 \
  /var/lib/librepaper-restored
```

Run LibrePaper behind an HTTPS reverse proxy, and give it `--origin` with the
public `https:` URL. The scheme comes from that origin rather than from a
forwarded header, so the proxy does not have to send `X-Forwarded-Proto`; set
`trusted_proxies` in the advanced configuration file if you want forwarded
client addresses honored for rate limiting. The proxy must pass the original
`Host` through unchanged, since a proxy that rewrites it will have its requests
refused. One `admin serve` process owns a deployment's writes; a second one
started against the same database refuses to start.

## Resource limits

The server keeps a document's collaborative state as one compressed base
plus a log of updates, and a document is only ever fully decoded in memory
while something needs it, an editor typing, a reader's projection, a
compaction. Three limits protect a deployment from a document that asks for
more of that than it should get, and from every document asking at once.

**The per-document log quota** bounds one document's compaction base plus
every row since it, 32 MiB by default. An update that would push a document
past its quota is refused with a retryable reason; the document's existing
log is untouched and semantic commands (restore, comment, label) still work
against it. This is the real bound on how long a cache rebuild for that
document can occupy a thread, which is why it is a per-document ceiling and
not a deployment-wide one. The base and rows count toward the owner's retained
storage.

**The memory budget** is one process-wide figure, 512 MiB by default, shared
by every resident cache entry, in-flight build, temporary fork and
projection output buffer across every document the process is holding. It is
also what request bodies and outgoing state transfers reserve from. A mutation
must declare its content length, a request without one is refused with 411.
Four times the content length is reserved from the budget before the body is
read; a request that cannot reserve is refused with a retryable 429. A request
that cannot reserve its decoded share waits briefly and then fails with
`busy`. Alongside it there is a second, separate bound on how many documents
may be decoded at once: `min(cores, 4)`. Memory and CPU run out
independently, and a machine with room for a hundred small documents can
still be brought down by a hundred simultaneous decodes. A document's own
lock keeps only one build running for that document at a time; the
`min(cores, 4)` ceiling is what keeps four large documents from taking every
core the relay path needs. Compaction exports count against the same
ceiling, because they are the same uninterruptible work over the same
decoded document.

Both can be raised in the advanced configuration file: `log_quota_mb` for the
per-document log quota and `memory_budget_mb` for the process-wide memory
budget. A cold build reserves the resident estimate and a transient one on top,
fourteen times the log bytes at the measured factors, so the server refuses to
start with a memory budget below that at the log quota; raise the two together.

**The pending-source budget** bounds what the deployment is holding *unsaved*
while PostgreSQL is slow or unavailable: 64 MiB across every document's buffer
by default, plus a separate 64 MiB for what writing a row costs while the write
is in flight. It is deliberately separate from the memory budget above, because
a decoded document is a cache that can be dropped to make room and somebody's
unsent typing is not. It is also deliberately two pools rather than one, so
that a deployment whose buffers are full can still write them out, with a
single pool there would be no room left to encode the row that would free the
room. A document's own 4 MiB buffer ceiling still applies on top, so one
document cannot spend the whole allowance.

An update refused for either reason comes back as retryable and carries the
server's head vector; the browser keeps the edit, leaves it showing as unsaved,
and resends it on a backoff without the person having to type again. No work is
ever dropped to free memory. Both figures are visible at `GET /api/status`
under `pending_budget`, together with how many updates have been refused, and
both can be raised with `pending_mb` and `pending_scratch_mb` in the advanced
configuration file. The server refuses to start with a scratch ceiling too
small to write one maximum-size row, since that would admit work it could never
persist.

**Unreadable.** A build that breaches its wall-clock bound (10 seconds) or
its memory reservation marks that document unreadable: its projection and
semantic commands answer 503 until it recovers. Ingest and flush keep
running underneath, so an editor can keep typing and the log keeps growing,
but nobody, including the document's own editors, can read a rendered
projection of it until someone shrinks it. Recovery happens automatically
the next time a build for that document succeeds, which an editor can
trigger by trimming the document's history from the settings page, or by
exporting, shrinking and re-importing the document, or an operator can trigger
after raising `log_quota_mb` or `memory_budget_mb` in the configuration file.

These limits exist because encoded bytes bound ingress and storage, but they
do not bound the CPU and memory a decode of those bytes costs, and Loro's
synchronous work cannot be interrupted once it starts. A limit here is
therefore a limit on what gets scheduled, not a way to reclaim a thread that
is already running.

## Containers

`deploy/docker` holds a compose file for the whole of the above: PostgreSQL,
LibrePaper, and a proxy that holds a certificate for each of the two origins
and passes `Host` through unchanged. The LibrePaper image is the released
static binary on Alpine, fetched at build time and checked against the release
checksums; there is no toolchain in it and nothing is assembled at start-up.

```sh
cd deploy/docker
cp .env.example .env
docker compose up -d
```

`DOMAIN` and `docs.DOMAIN` must both resolve to the host before the first
start, because the proxy takes its certificates over HTTP-01. The container
runs with `--no-local`: the loopback companion renders Quarto with the tools on
the editor's own machine, and in a container there are none. `deploy/docker/README.md`
covers backups, upgrades, and moving objects to a bucket.
