# Self-managed hosting

## Origins and database

Production requires different reader and document hostnames; different ports are insufficient. DNS and HTTPS certificates must cover both. The default document host is `docs.` plus the reader host; use `--docs-origin` for another host. Unrecognized `Host` values receive 421. Without `--origin`, the server serves loopback only.

Install PostgreSQL separately; LibrePaper has no SQLite mode and does not manage the database. Create a dedicated role and database:

```sql
create role librepaper login password 'replace-with-a-generated-secret';
create database librepaper owner librepaper;
```

For peer authentication, run as the matching operating-system user:

```sh
export LIBREPAPER_DATABASE_URL=postgresql:///librepaper
```

Or use a protected service environment file and loopback PostgreSQL URL:

```sh
export LIBREPAPER_DATABASE_URL='postgresql://librepaper:SECRET@127.0.0.1/librepaper'
```

The filesystem object store keeps immutable objects under `/var/lib/librepaper/objects`; keep it and PostgreSQL on SSD. For a separate S3-compatible bucket:

```sh
export LIBREPAPER_OBJECT_STORE=s3
export LIBREPAPER_S3_REGION=us-east-1
export LIBREPAPER_S3_BUCKET=librepaper-production
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
```

Set `LIBREPAPER_S3_ENDPOINT` for other providers. Use `LIBREPAPER_S3_ALLOW_HTTP=true` only for a trusted local test endpoint. Configure provider alerts and spending caps.

Run one application process per deployment, behind an HTTPS reverse proxy:

```sh
librepaper admin serve \
  --origin https://paper.example \
  --publishers YOUR_GITHUB_LOGIN \
  --data-directory /var/lib/librepaper \
  --database-connections 20 \
  --port 8080
```

The proxy must preserve the original `Host`. The configured origin supplies the scheme and OAuth callback URL; forwarded-protocol headers are not needed. Set `trusted_proxies` in advanced config to honor forwarded client addresses for rate limiting. A second `admin serve` process against the same database refuses to start.

## Backup and restore

Install PostgreSQL client tools on the backup host. Backup works with filesystem or S3 objects and copies all objects referenced by its database snapshot, including retired bases. The policy configuration does not schedule backups or prune old directories; schedule and retain them yourself and copy recovery points off-host.

```sh
librepaper admin backup \
  --data-directory /var/lib/librepaper \
  /srv/backups/librepaper-$(date +%F)

createdb librepaper_restore
LIBREPAPER_DATABASE_URL=postgresql:///librepaper_restore \
  librepaper admin restore \
  /srv/backups/librepaper-2026-09-13 \
  /var/lib/librepaper-restored
```

Restore to an empty database and a path that does not exist. See [cost and storage policy](cost-policy.md) for physical storage and backup sizing.

## Resource limits

See the [cost policy](cost-policy.md#defaults) for defaults and settings. Pass advanced configuration with `--config`; inspect operating usage at `/api/status` over loopback. Quota refusals leave edits unsaved for retry. Memory reservation failure returns `busy`; decode failure or a completed build exceeding 10 seconds marks the document unreadable. The time check does not interrupt work. Recovery requires fixing the cause and resetting or re-admitting the document, for example after restarting the process. Trimming history may itself fail when a document is unreadable.

## Containers

`deploy/docker` provides Compose for PostgreSQL, LibrePaper, and the HTTPS proxy. Both `DOMAIN` and `docs.DOMAIN` must resolve to the host before first start for HTTP-01 certificates.

```sh
cd deploy/docker
cp .env.example .env
docker compose up -d
```

The container uses `--no-local`; rendering through the local companion belongs on the editor's machine. See `deploy/docker/README.md` for backup, upgrade, and object-bucket instructions.
