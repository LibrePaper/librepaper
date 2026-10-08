---
title: "Self-hosting"
---

> LibrePaper is experimental. Self-host only if you can maintain its database,
> files, credentials, and recovery copies.

## Install

```sh
# Docker Engine with Compose v2 is the only requirement. Open ports 80 and 443.
# Point two DNS names at the VPS: the app and the published documents.
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
$EDITOR librepaper.toml      # the two hostnames, a GitHub OAuth client id and secret
docker compose up -d
curl -fsS https://paper.example/ready
```

- The directory is private to you (mode 700). Files inside stay readable (0644) so the containers can read their bind mounts; never chmod 600 librepaper.toml.
- GitHub OAuth callback URL: `https://paper.example/auth/callback`. Google's is `/auth/callback/google`.

## Check

```sh
docker compose ps
curl -fsS https://paper.example/ready
docker compose logs librepaper
```

Use the third command if the app does not answer.

## Backups and recovery

Backups are idle until `resticprofile.toml` has a `[resticprofile]` table. Configure a remote Restic repository; the password and any S3 credentials go in the env table:

```toml
[resticprofile]
repository = "s3:s3.amazonaws.com/your-bucket/restic"

[resticprofile.env]
RESTIC_PASSWORD = "your-restic-password"
AWS_ACCESS_KEY_ID = "your-key"
AWS_SECRET_ACCESS_KEY = "your-secret"
```

Bring up the backup sidecar and verify:

```sh
docker compose up -d --force-recreate backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile show
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup
```

Backups contain the database dump, referenced objects, both TOML files, and `session.key`. Keep the session key to preserve existing sessions and saved share secrets. Set up [alerts](#alerts) so a failed backup emails you. See [recovery.md](recovery.html) for a restore drill. Practice a restore first on a machine that is not the VPS.

## Monitoring

```sh
# Optional. Graphs of the last 400 days at https://admin.paper.example/, behind a password.
# DNS: admin.paper.example -> this VPS, like the other two names.
head -c 24 /dev/urandom | base64         # the password
# librepaper.toml: under [origins] add   admin = "https://admin.paper.example"
#                  and a new table       [admin]
#                                        password = "<the line above>"
docker compose up -d --force-recreate librepaper
```

- One DNS record and two lines in the one file. Nothing in Caddy: it asks the server before obtaining a certificate, and the server says yes to three names.
- The browser asks for the password once per session. User name: anything.
- Zoom by dragging on any graph; double-click to return to the chosen range.

## Alerts

The VPS cannot report its own outage, so both alerts come from free outside services. Each emails you the day something fails.

**Uptime: UptimeRobot** ([uptimerobot.com](https://uptimerobot.com))

- New monitor, type HTTP(s), URL `https://paper.example/ready`, interval 5 minutes.
- `/ready` answers 200 only when the app serves requests; the home page alone can look fine while the app is down.
- Add your email as an alert contact.

**Backups: Healthchecks** ([healthchecks.io](https://healthchecks.io))

A backup can fail while the site stays up (a revoked S3 key, a full repository). Healthchecks alerts when a run fails or when no success arrives in time.

| Check | Schedule | Period | Grace |
|---|---|---|---|
| backup | Simple | 1 day | 12 hours |
| repository check | Simple | 7 days | 1 day |

- Copy each check's ping URL (`https://hc-ping.com/<uuid>`) into the hooks in [recovery.md](recovery.html#external-backup-alerts), add them to `resticprofile.toml`, then recreate the sidecar and take a backup:

```sh
docker compose up -d --force-recreate backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup
# the backup check shows a start and a success ping within seconds
```

- Keep ping URLs private: anyone with one can send a fake success.
- Account settings: turn on the weekly report for a Monday summary of every check.

## Upgrade

```sh
cd ..   # the directory that contains librepaper/
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz --exclude='*/librepaper.toml' --exclude='*/resticprofile.toml'
cd librepaper && docker compose pull && docker compose up -d
```

The server migrates the schema at startup. When a release note says the kit changed, this same procedure applies.

After editing `librepaper.toml` or `resticprofile.toml`, recreate the containers that read them. Compose compares service definitions, not the bytes behind a mounted file:

```sh
docker compose up -d --force-recreate librepaper backup
```

## Database elsewhere

Set `storage.database_url` to a TCP URL with `sslmode=verify-full`. Create the role on the provider as a non-superuser database owner. Then:

```sh
cp compose.managed-db.yaml compose.override.yaml
```

The override survives upgrades.

## Secrets in files

Every secret in `librepaper.toml` also accepts `{ file = "/run/secrets/name" }`. Mount the directory with a bind volume in `compose.override.yaml`. Leave that directory at 0755 and the files readable: a mounted directory keeps its own mode inside the container, and the app runs as another uid. The kit directory's 0700 is what keeps it private.

## Without Docker

Run one `librepaper admin serve` process against durable PostgreSQL and file storage, behind an HTTPS proxy that preserves `Host` and forwards the client address. Configure LibrePaper to trust only that proxy's network. The server applies migrations at startup, so its database role must own the schema. See the [CLI guide](cli.html) for config commands.

For operator privacy and data requests, see [privacy duties](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md).
