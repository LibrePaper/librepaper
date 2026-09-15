# Self-managed hosting

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

Run LibrePaper behind an HTTPS reverse proxy and pass
`X-Forwarded-Proto: https`. Start worker-capable LibrePaper processes before
admitting traffic after a restore. Horizontally scaled deployments use the same
schema and object interface; use a bounded pool per process and route a
document's WebSocket connections consistently to one application process.
