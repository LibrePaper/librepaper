---
title: "Advanced features"
---

Everything here is optional. Start with the [simple deployment](simple.html).

## DNS

| Name | Required | Example |
|---|---|---|
| App | yes | `paper.example` |
| Published documents | yes | `docs.paper.example` |
| Admin graphs | no | `admin.paper.example` |

- Caddy obtains a certificate for a name on its first visit, after asking the server (`/api/tls/ask`) whether it is a configured origin. No Caddy edits are needed for the three names.
- Publish an AAAA record only if the VPS has IPv6.
- Extra names that redirect (for example `www`) go in `caddy/local.d/*.caddy`. Upgrades never overwrite `local.d`. See the comments in `caddy/Caddyfile`.

## Sign-in and access

At least one sign-in provider is needed.

| Provider | Callback URL |
|---|---|
| GitHub | `https://paper.example/auth/callback` |
| Google | `https://paper.example/auth/callback/google` |

Google uses an `[auth.google]` table with `client_id` and `client_secret`, like `[auth.github]`.

`[access]` says who may publish and comment:

```toml
[access]
publishers = ["any"]
commenters = ["any"]
```

Each value is a list of:

- `"any"`: any signed-in account.
- A GitHub login.
- A verified Google address.
- `"@example.org"`: a whole domain.
- `"anyone"` (commenters only): anonymous comments, no sign-in.

## Backups

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

Backups contain the database dump, referenced objects, both TOML files, and `session.key`. Keep the session key to preserve existing sessions and saved share secrets.

Schedule and retention (set by the sidecar image):

- A backup runs daily at midnight UTC.
- Retention: keep all snapshots from the last 48 hours, then 14 daily and 11 weekly.
- A `restic check` of 10% of the data runs on Mondays at 06:00.

Notes:

- The restic password is fixed for the life of the repository. Keep a copy off the VPS: losing it loses every backup.
- A backup records the database's migration version, and `admin restore` refuses a backup whose version differs from the binary's. Take a fresh backup after upgrading to a release that changes the migration history.
- Set up [backup alerts](#backup-alerts) so a failed backup emails you.

### Restore drill

Restore one remote Restic snapshot into a new deployment on a machine that is not the VPS. The kit publishes ports 80 and 443 and claims a fixed subnet, so two copies cannot share a host. Practice this before you need it.

```sh
# On a machine that is not the VPS. Docker is the only requirement.
mkdir -m 700 librepaper-recovery && cd librepaper-recovery
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
cp /path/to/your/resticprofile.toml .   # the repository and RESTIC_PASSWORD are all restore needs
# Keep the kit's own librepaper.toml: it points at the empty local database. Your copy may point at a managed database, or at secret files this kit does not mount.
mkdir -m 700 staging
docker compose up -d --wait postgres                                 # an empty database; do not start the app yet
docker compose run --rm --no-deps --user 0 -v ./staging:/restore --entrypoint resticprofile backup \
  -c /etc/resticprofile/profiles.toml -n resticprofile restore <snapshot-id> --target /restore
docker compose run --rm --no-deps --user 0 -v ./staging:/restore:ro librepaper \
  admin restore --config /etc/librepaper/librepaper.toml /restore/var/backups/librepaper/current /var/lib/librepaper/recovered
docker compose run --rm --no-deps --user 0 -v ./staging:/restore:ro --entrypoint sh librepaper -c '
  install -d -o 10001 -g 65534 /var/lib/librepaper/objects /var/lib/librepaper/secrets
  cp -a /var/lib/librepaper/recovered/objects/. /var/lib/librepaper/objects/
  install -o 10001 -g 65534 -m 0600 /restore/var/lib/librepaper/secrets/session.key /var/lib/librepaper/secrets/session.key
  chown -R 10001:65534 /var/lib/librepaper/objects'
docker compose up -d --wait
docker compose exec librepaper wget -qO- http://127.0.0.1:8080/ready
```

`admin restore` verifies the manifest and object hashes and loads the database in one transaction; it refuses a non-empty database or an existing destination. Use the session key from the same snapshot, never one from another deployment.

Open the app through a local browser or SSH tunnel and verify rendered content, assets, and existing sessions.

## Object storage

Keep user files in an S3-compatible bucket instead of the `data` volume.

```toml
[storage]
object_store = "s3"

[storage.s3]
endpoint = "https://s3.region.provider.example"
region = "region"
bucket = "your-bucket"
access_key_id = "..."
secret_access_key = "..."
```

`allow_http = true` exists only for a local test endpoint.

Requirements:

- A private bucket used only for this.
- A dedicated access key that can read, write, list and delete in that bucket only.
- Conditional writes (`If-None-Match: *` on PUT), because the server never overwrites a stored file. AWS S3, OVHcloud Object Storage and MinIO support them. Others may not: test first.

Backups still include user files: the backup job reads them through the same store, so the `backup` container needs the same keys. With keys written in `librepaper.toml` it already has them; with `{ file = ... }` references, mount the files into `backup` too (see [Secrets](#secrets)). `session.key` and the server's own state stay in the `data` volume.

### Move existing files

For an install that started with the volume: stop the app and backup, copy `objects/` into the bucket with rclone, check, then switch the config and start.

```sh
docker compose stop librepaper backup
# keys on stdin, never in the command line
printf '%s\n%s\n' "$ACCESS_KEY_ID" "$SECRET_ACCESS_KEY" | docker run --rm -i -v librepaper_data:/data:ro --entrypoint sh rclone/rclone:1.68.2 -c 'read -r RCLONE_S3_ACCESS_KEY_ID; read -r RCLONE_S3_SECRET_ACCESS_KEY; export RCLONE_S3_ACCESS_KEY_ID RCLONE_S3_SECRET_ACCESS_KEY; rclone copy /data/objects :s3,provider=Other,endpoint=s3.region.provider.example,region=region:your-bucket --checksum && rclone check /data/objects :s3,provider=Other,endpoint=s3.region.provider.example,region=region:your-bucket --one-way --checksum'
$EDITOR librepaper.toml   # object_store = "s3" and [storage.s3]
docker compose up -d --force-recreate librepaper backup
```

- The endpoint in rclone's inline remote has no `https://`: a colon would end the settings. rclone uses HTTPS by default.
- The volume is `librepaper_data` because the compose project is `librepaper`.
- The volume copy stays as a rollback until you delete it.

### Restore with object storage

`admin restore` writes user files to local disk. After restoring, copy `objects/` into the bucket the same way before starting the app on S3.

## Health

The VPS cannot report its own outage, so alerts come from free outside services.

### Operator graphs

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
- For your own Prometheus, add `[metrics]` with `address = "0.0.0.0:9091"` to `librepaper.toml`: the server then serves `/metrics` on that port inside the Docker network. The kit publishes no host port for it, and the graphs above do not need it.

### Uptime

UptimeRobot ([uptimerobot.com](https://uptimerobot.com)) emails you the day the site is down.

- New monitor, type HTTP(s), URL `https://paper.example/ready`, interval 5 minutes.
- `/ready` answers 200 only when the app serves requests; the home page alone can look fine while the app is down.
- Add your email as an alert contact.

### Backup alerts

A backup can fail while the site stays up (a revoked S3 key, a full repository). Healthchecks ([healthchecks.io](https://healthchecks.io)) alerts when a run fails or when no success arrives in time.

| Check | Schedule | Period | Grace |
|---|---|---|---|
| backup | Simple | 1 day | 12 hours |
| repository check | Simple | 7 days | 1 day |

That means an alert after 36 hours without a successful backup, or 8 days without a check, and at once on a failed run.

Copy each check's ping URL (`https://hc-ping.com/<uuid>`) into these hooks and add them to `resticprofile.toml`:

```toml
[[resticprofile.backup.send-before]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>/start"
[[resticprofile.backup.send-after]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>"
[[resticprofile.backup.send-after-fail]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>/fail"

[[resticprofile.check.send-before]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>/start"
[[resticprofile.check.send-after]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>"
[[resticprofile.check.send-after-fail]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>/fail"
```

Then recreate the sidecar and take a backup:

```sh
docker compose up -d --force-recreate backup
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup
# the backup check shows a start and a success ping within seconds
```

- Keep ping URLs private: anyone with one can send a fake success.
- Account settings: turn on the weekly report (sent on Mondays) for a summary of every check.

## Secrets

Every secret in `librepaper.toml` also accepts `{ file = "/run/secrets/name" }` or `{ env = "NAME" }`.

- Mount the secrets directory with a bind volume in `compose.override.yaml`, into `librepaper` and, when the object storage keys are among them, into `backup`.
- Leave that directory at 0755 and the files readable: a mounted directory keeps its own mode inside the container, and the app runs as another uid.
- The kit directory's 0700 is what keeps it private.
- Keep `resticprofile.toml` and the restic password backed up somewhere other than the VPS.

## Database elsewhere

Set `storage.database_url` to a TCP URL with `sslmode=verify-full`. Create the role on the provider as a non-superuser database owner. Then:

```sh
cp compose.managed-db.yaml compose.override.yaml
```

The override survives upgrades.

## Limits and retention

Every limit and the retention settings are listed, with their defaults, at the end of `librepaper.toml` (see [the whole file](simple.html#the-whole-file)). Uncomment a line to change it, then recreate the app.

- Per account: storage (`publisher_storage_mib`) and uploads per hour (`publisher_uploads_per_hour`).
- Whole server: total storage (`deployment_storage_mib`), memory for loading documents (`memory_budget_mib`), edit history per document (`log_quota_mib`), and unsaved edits in memory and on disk (`pending_mib`, `pending_scratch_mib`).
- With [object storage](#object-storage), disk is no longer the constraint on `deployment_storage_mib`: set it to what you are willing to store.
- `[retention]` deletes documents after `expire_after`, counted from the last update (default) or from creation.

## Without Docker

Run one `librepaper admin serve` process against durable PostgreSQL and file storage, behind an HTTPS proxy that preserves `Host` and forwards the client address. Configure LibrePaper to trust only that proxy's network. The server applies migrations at startup, so its database role must own the schema. See the [CLI guide](../cli.html) for config commands.

For operator privacy and data requests, see [privacy duties](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/privacy-operators.md).
