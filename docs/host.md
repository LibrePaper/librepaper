---
title: "Self-hosting"
---

> **Warning:** LibrePaper is experimental software that is evolving rapidly. We do not recommend self-hosting at the moment. This page nevertheless includes some notes for interested readers and system administrators.

## Deploy

### Quick start

```sh
librepaper admin serve --port 8081 --publishers YOUR-GITHUB-LOGIN
```

### Production

Requires separate hostnames (reader and `docs.*`); use `--docs-origin` to override. Set `--origin`; unrecognized hosts receive 421.

Install PostgreSQL separately:

```sql
create role librepaper login password 'replace-with-a-generated-secret';
create database librepaper owner librepaper;
```

Connect with peer authentication or loopback:

```sh
export LIBREPAPER_DATABASE_URL=postgresql:///librepaper
# or
export LIBREPAPER_DATABASE_URL='postgresql://librepaper:SECRET@127.0.0.1/librepaper'
```

Keep `/var/lib/librepaper/objects` and PostgreSQL on SSD. For S3-compatible storage:

```sh
export LIBREPAPER_OBJECT_STORE=s3
export LIBREPAPER_S3_REGION=us-east-1
export LIBREPAPER_S3_BUCKET=librepaper-production
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
# LIBREPAPER_S3_ENDPOINT for other providers
# LIBREPAPER_S3_ALLOW_HTTP=true only for trusted local endpoints
```

Run behind an HTTPS reverse proxy:

```sh
librepaper admin serve \
  --origin https://paper.example \
  --publishers YOUR_GITHUB_LOGIN \
  --data-directory /var/lib/librepaper \
  --database-connections 20 \
  --port 8080
```

Proxy must preserve `Host` header. Only one `admin serve` per database.

### Moderation

Moderation commands connect to the database via `--database-url` or `LIBREPAPER_DATABASE_URL`. Database access is the operator authorization boundary.

```sh
librepaper admin moderate block-account github-handle \
  --actor "on-call@example.org" --reason "automated abuse investigation"
librepaper admin moderate unblock-account github-handle \
  --actor "on-call@example.org" --reason "appeal reviewed"

librepaper admin moderate hide-project abusive-project \
  --actor "on-call@example.org" --reason "contains abusive material"
librepaper admin moderate unhide-project abusive-project \
  --actor "on-call@example.org" --reason "review completed"
```

Every command requires `--actor` and `--reason`. `moderation_audit` table records actor, timestamp, action, target, and reason. Accounts accept handle or UUID.

Blocking: revokes sessions, denies access and writes. Unblocking: permits fresh sign-in; previously issued credentials stay revoked.

Hiding: preserves data while denying document, asset, source, history, export, and socket access. Unhiding restores previous access rules.

Open sockets recheck authorization every 2 seconds, then disconnect; frames already in flight may still arrive. Preserve `moderation_audit` with normal backups.

## Storage

`librepaper admin serve` stores catalog, objects, server state and session secrets in `--data-directory` (default `librepaper-data`). Restore needs an empty database and a path that does not exist yet.

Storage limits:

| Flag | Default |
| --- | --- |
| `--publisher-storage-limit` | 50 MB |
| `--deployment-storage-limit` | 5120 MB |
| `--publisher-upload-limit` | 30 uploads/hour |
| `log_quota_mb` (advanced config) | 32 MB |

```sh
librepaper admin serve --publisher-storage-limit 500 --deployment-storage-limit 10240
```

Backup and restore:

```sh
librepaper admin backup --data-directory /var/lib/librepaper /backups/librepaper-$(date +%F)
createdb librepaper_restore
LIBREPAPER_DATABASE_URL=postgresql:///librepaper_restore librepaper admin restore /backups/librepaper-2026-09-13 /librepaper-restored
```

See [cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/cost-policy.md) for defaults and schema.

## Environment variables

Every flag has an environment variable; `librepaper admin serve --help` names each one.

Advanced config in optional YAML (`--config` or `LIBREPAPER_CONFIG`):
- `trusted_proxies`: proxy networks (CIDR)
- `backup`: `destination_class`, `frequency` (seconds), `retained_count`, `encrypted`, `warning_count`

Secrets (environment only):
- `LIBREPAPER_GITHUB_CLIENT_SECRET`
- `LIBREPAPER_GOOGLE_CLIENT_SECRET`

Service settings:
- `LIBREPAPER_ASSET_MIRROR`: HTTPS URL (default: project mirror)

## Fonts

```sh
librepaper admin serve --typst-fonts /srv/librepaper/fonts
```

## Rights

`--publishers`: who may upload; `--commenters`: who may annotate.

```sh
librepaper admin serve --publishers alice,anne@example.org --commenters @example.org
```

| Entry | Meaning |
| --- | --- |
| `alice` | GitHub login |
| `alice@example.org` | Google account with verified email |
| `@example.org` | any Google account on that domain (exact match) |
| `any` or `anyone` | any signed-in account |

Defaults: `--publishers` has no default; `--commenters` defaults to `anyone`. Both are ceilings; document access and share links require sign-in. Domain match is exact: `@example.org` admits `alice@example.org` only, not `alice@mail.example.org`.

Forwarded client identity is trusted only from networks in advanced config:

```yaml
trusted_proxies:
  - 127.0.0.1/32
  - ::1/128
```

Proxy must append actual address to `X-Forwarded-For`. Google sign-in: verified Gmail and Google Workspace only.

## OAuth

At least one OAuth client (GitHub or Google) is required.

**GitHub:** Create at [github.com/settings/developers](https://github.com/settings/developers) with `/auth/callback`. Pass:
- `--github-client-id` or `LIBREPAPER_GITHUB_CLIENT_ID`
- `LIBREPAPER_GITHUB_CLIENT_SECRET` only (env var)

**Google:** Create Web application at [console.cloud.google.com](https://console.cloud.google.com) with `/auth/callback/google`. Pass:
- `--google-client-id` or `LIBREPAPER_GOOGLE_CLIENT_ID`
- `LIBREPAPER_GOOGLE_CLIENT_SECRET` only (env var)
- Publish consent screen

## Retention

```sh
librepaper admin serve --document-expire-after 24h      # LIBREPAPER_EXPIRE_AFTER; default never
librepaper admin serve --document-expire-from created   # LIBREPAPER_EXPIRE_FROM; default updated
```

## Containers

See [tools/deploy-docker](https://github.com/LibrePaper/librepaper/blob/main/tools/deploy-docker/README.md) for Docker Compose setup with PostgreSQL, HTTPS proxy, and monitoring.

```sh
cd tools/deploy-docker
cp .env.example .env
# Set DOMAIN, ACME_EMAIL, POSTGRES_PASSWORD, LIBREPAPER_ADMIN_PASSWORD
# Set GitHub/Google OAuth client ID and secret
# Set LIBREPAPER_PUBLISHERS and LIBREPAPER_COMMENTERS if needed
docker compose up -d --build
```

Both `DOMAIN` and `docs.DOMAIN` must resolve before first start for HTTP-01 certificates. Other names get 421; redirect in `Caddyfile`. `DOCS_DOMAIN` defaults to `docs.$DOMAIN`.

## Privacy

Default asset mirror sees your IP and asset requests. To self-host, use `--asset-mirror URL` with HTTPS and CORS for GET/HEAD.

[Privacy duties for operators](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md) covers publishing a notice and answering data requests.
