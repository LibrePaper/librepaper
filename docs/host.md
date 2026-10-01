---
title: "Running a server"
---

## Deploy

LibrePaper is a single static binary with reader, renderers, and server compiled in.

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

## Storage

`librepaper admin serve` stores catalog, objects, server state, and session secrets in `--data-directory` (default `librepaper-data`). Back up and restore to an empty database and non-existent path.

Storage limits:

| Flag | Default |
| --- | --- |
| `--publisher-storage-limit` | 100 MB |
| `--deployment-storage-limit` | 5120 MB |
| `--publisher-upload-limit` | 500 uploads/hour |
| `log_quota_mb` (advanced config) | 32 MB |

```sh
librepaper admin serve --publisher-storage-limit 500 --deployment-storage-limit 10240
```

Per-document log (`log_quota_mb` in advanced config) bounds edits. Backup and restore:

```sh
librepaper admin backup --data-directory /var/lib/librepaper /backups/librepaper-$(date +%F)
createdb librepaper_restore
LIBREPAPER_DATABASE_URL=postgresql:///librepaper_restore librepaper admin restore /backups/librepaper-2026-09-13 /librepaper-restored
```

See [cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/cost-policy.md) for defaults, YAML schema, `/api/status` endpoint, and capacity accounting.

## Environment variables

Flags map to env vars: `--foo-bar` becomes `LIBREPAPER_FOO_BAR` (flag wins if both set). See `librepaper admin serve --help` for the full list.

Advanced config in optional YAML (`--config PATH` or `LIBREPAPER_CONFIG`):

- `trusted_proxies`: list of proxy networks (CIDR)
- `backup`: metadata for operator-managed backups (`destination_class`, `frequency` in seconds, `retained_count`, `encrypted`, `warning_count`)

Secrets (environment-only):

- `LIBREPAPER_GITHUB_CLIENT_SECRET`: GitHub OAuth client secret
- `LIBREPAPER_GOOGLE_CLIENT_SECRET`: Google OAuth client secret

Service settings (CLI flags also available):

- `LIBREPAPER_ASSET_MIRROR`: HTTPS URL for wasm/LaTeX (default: project mirror)
- `LIBREPAPER_EXPIRE_AFTER`: delete documents after duration, e.g. `24h` (default: never)
- `LIBREPAPER_EXPIRE_FROM`: `updated` (default) or `created`

## Fonts

Serve custom Typst fonts from a directory:

```sh
librepaper admin serve --typst-fonts /srv/librepaper/fonts
```

## Browser compiler assets

Browsers fetch renderers and LaTeX from an asset mirror, not from the app. `assets.lock` pins assets; nothing is rewritten, so old binaries keep working. To self-host, pass `--asset-mirror https://host/` with paths from `assets.lock`, HTTPS, and CORS for GET/HEAD. See [asset mirrors](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md).

## Rights

`--publishers` lists who may upload; `--commenters` lists who may annotate. Both accept comma-separated entries:

```sh
librepaper admin serve --publishers alice,anne@example.org --commenters @example.org
```

| Entry | Meaning |
| --- | --- |
| `alice` | GitHub login |
| `alice@example.org` | Google account with verified email |
| `@example.org` | any Google account on that domain (exact match after `@`) |
| `any` | any signed-in account |
| `anyone` | unsigned-in commenting (publishing rejected) |

`--publishers` has no default. `--commenters` defaults to `anyone`. Both are ceilings.

A domain matches exactly: `@example.org` admits `alice@example.org`, not `alice@mail.example.org`.

Forwarded client identity is trusted only from networks in advanced config:

```yaml
trusted_proxies:
  - 127.0.0.1/32
  - ::1/128
```

Proxy must append actual address to `X-Forwarded-For`.

Google sign-in: verified Gmail and Google Workspace only. After upgrading, sign in again and run `librepaper login`. Logout removes cookie; revoke session to invalidate copies.

## OAuth

Publishing always needs at least one OAuth client, GitHub or Google.

GitHub: create app at [github.com/settings/developers](https://github.com/settings/developers) with `/auth/callback`. Pass id via `--github-client-id` or `LIBREPAPER_GITHUB_CLIENT_ID`; secret via `LIBREPAPER_GITHUB_CLIENT_SECRET` only.

Google: create *Web application* at [console.cloud.google.com](https://console.cloud.google.com) with `/auth/callback/google`. Pass id via `--google-client-id` or `LIBREPAPER_GOOGLE_CLIENT_ID`; secret via `LIBREPAPER_GOOGLE_CLIENT_SECRET`. Publish consent screen.

`librepaper logout` deletes terminal token.

## Retention

Set lifetime for public deployments: `--document-expire-after 24h`. Use `--document-expire-from created` to expire from upload date.

## Containers

`tools/deploy-docker` provides Compose for PostgreSQL, LibrePaper, HTTPS proxy, and internal monitoring. The current v0.0.8 release has no metrics endpoint, so start with a local static musl binary until a metrics-capable v0.0.9 or later release is published. Both `DOMAIN` and `docs.DOMAIN` must resolve before first start for HTTP-01 certificates. Other names, such as `www`, get a 421 from the server: redirect them in the `Caddyfile` (example at its end).

```sh
# From the repository root, after building a static binary for this host.
cp target/x86_64-unknown-linux-musl/release/librepaper tools/deploy-docker/librepaper
cd tools/deploy-docker
cp .env.example .env
# Set DOMAIN, ACME_EMAIL, LIBREPAPER_PUBLISHERS, and independent random values
# for POSTGRES_PASSWORD, POSTGRES_EXPORTER_PASSWORD, LIBREPAPER_ADMIN_PASSWORD.
# Set both client ID and client secret for GitHub or Google OAuth.
docker compose -f compose.yaml -f compose.local.yaml up -d --build
```

`DOCS_DOMAIN` defaults to `docs.$DOMAIN`; both names must resolve to this host. The container uses `--no-local`. See [the Docker deployment guide](https://github.com/LibrePaper/librepaper/blob/main/tools/deploy-docker/README.md) for password rotation, backup, and release upgrades.

The official instance also runs free, open-source Prometheus, Grafana, PostgreSQL exporter, and Node Exporter containers. Grafana is available at `/admin/monitoring/` on the app hostname and requires its built-in admin login. Prometheus and the exporters have no public ports; Caddy exposes only the Grafana path on the app hostname. The metrics listener binds inside the app container and reports aggregate counts and health, without user or document identifiers. See [production operations](dev/production.html#monitoring) for deployment, password retrieval, and rotation.

## Privacy

The default asset mirror sees your IP and asset requests. To self-host and keep requests private, use `--asset-mirror URL` with your binary's `assets.lock` paths, HTTPS, and CORS for GET/HEAD.
