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

- `"any"` means any signed-in account.
- A GitHub login.
- A verified Google address.
- `"@example.org"` means a whole domain.
- `"anyone"` means anonymous comments with no sign-in. It is valid for commenters only.

## Backups

Backups are idle until `resticprofile.toml` has a `[resticprofile]` table. Configure a remote Restic repository. The password and any S3 credentials go in the env table:

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
./manage apply
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile show   # the merged profile
./manage backup now
```

Backups contain the database dump, referenced objects, both TOML files, and `session.key`. Keep the session key to preserve existing sessions and saved share secrets.

Schedule and retention come from a base profile inside the backup image. Your `resticprofile.toml` is merged over it, so any setting you add there wins. The defaults:

- A backup runs daily at midnight UTC.
- Retention keeps all snapshots from the last 48 hours, then 14 daily and 11 weekly.
- A `restic check` of 10% of the data runs on Mondays at 06:00.

To change them, add the keys you want to `resticprofile.toml`, then run `./manage apply`. It recreates the sidecar, which installs the schedule when it starts:

```toml
[resticprofile.backup]
schedule = "*-*-* 03:30"   # every day at 03:30 UTC

[resticprofile.retention]
keep-daily = 30
keep-weekly = 52
```

```sh
./manage apply
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile show   # the merged result
```

- Leave `source`, `run-before` and `run-finally` alone: they produce the database export that every snapshot contains.
- If you change the schedule, change the Healthchecks period to match (see [backup alerts](#backup-alerts)).

Notes:

- The restic password is fixed for the life of the repository. Keep a copy off the VPS: losing it loses every backup.
- A backup records the database's migration version, and `admin restore` refuses a backup whose version differs from the binary's. Take a fresh backup after upgrading to a release that changes the migration history.
- Set up [backup alerts](#backup-alerts) so a failed backup emails you.

### Inspect a backup

The download includes `./manage`, which reads the backup repository through the backup container. It needs only a configured `resticprofile.toml`, so it also works from another machine with a copy of that file.

```sh
./manage backup list         # every snapshot: id, time, size
./manage backup show latest  # one snapshot: when it ran, migration version, stored files, database size
```

`list` and `show` change nothing.

### Restore drill

Restore a snapshot into a fresh copy of LibrePaper on a machine that is not the VPS, to check that your backups work. Practice this before you need it. The copy publishes ports 80 and 443, so it needs a machine of its own.

```sh
# on another machine with Docker
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
cp /path/to/your/resticprofile.toml .   # the repository and its password are all a restore needs
./manage backup restore latest        # or a snapshot id from ./manage backup list
```

- It starts an empty database, restores the snapshot, and loads it with `admin restore`. That command verifies every file against the backup's manifest. It then puts the stored files and the snapshot's own `session.key` in place, starts LibrePaper and checks `/ready`.
- It refuses to run where LibrePaper is already running, and `admin restore` refuses a database that is not empty, so it cannot overwrite a live deployment.
- Open the copy in a local browser or through an SSH tunnel, and check documents, figures and that existing sign-ins still work. Do not expose it under your production names.
- Keep the kit's own `librepaper.toml` for the drill: it points at the empty local database.

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
- The store must support conditional writes: PUT sends `If-None-Match: *`, because the server never overwrites a stored file. AWS S3, OVHcloud Object Storage and MinIO support them. Others may not: test first.

Backups still include user files: the backup job reads them through the same store, so the `backup` container needs the same keys. With keys written in `librepaper.toml`, it already has them. With `{ file = ... }` references, mount the files into `backup` too (see [Secrets](#secrets)). `session.key` and the server's own state stay in the `data` volume.

## Health

The VPS cannot report its own outage, so alerts come from free outside services.

### LibrePaper admin console

The console is a password-protected page on its own hostname. It graphs the server's health over the last 400 days: CPU, memory, free disk, stored bytes, open documents, live connections and database connections in use.

```sh
# Optional. Graphs of the last 400 days at https://admin.paper.example/, behind a password.
# DNS: admin.paper.example -> this VPS, like the other two names.
head -c 24 /dev/urandom | base64         # the password
# librepaper.toml: under [origins] add   admin = "https://admin.paper.example"
#                  and a new table       [admin]
#                                        password = "<the line above>"
./manage apply
```

- It takes one DNS record and two lines in the one file. Caddy needs no changes. Caddy asks the server before obtaining a certificate, and the server says yes to three names.
- The browser asks for the password once per session. The user name can be anything.
- Zoom by dragging on any graph. Double-click to return to the chosen range.
- For your own Prometheus, add `[metrics]` with `address = "0.0.0.0:9091"` to `librepaper.toml`: the server then serves `/metrics` on that port inside the Docker network. The kit publishes no host port for it, and the graphs above do not need it.

### Uptime

[UptimeRobot](https://uptimerobot.com) emails you the day the site is down.

- Create an HTTP(s) monitor for `https://paper.example/ready`, checked every 5 minutes.
- `/ready` answers 200 only when the app serves requests. The home page alone can look fine while the app is down.
- Add your email as an alert contact.

### Backup alerts

A backup can fail while the site stays up. Causes include a revoked S3 key and a full repository. [Healthchecks](https://healthchecks.io) alerts when a run fails or when no success arrives in time.

| Check | Schedule | Period | Grace |
|---|---|---|---|
| backup | Simple | 1 day | 12 hours |
| repository check | Simple | 7 days | 1 day |

That means an alert after 36 hours without a successful backup, or 8 days without a check, and at once on a failed run.

Copy each check's ping URL into these hooks and add them to `resticprofile.toml`. A ping URL looks like `https://hc-ping.com/<uuid>`.

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

Then apply the change and take a backup:

```sh
./manage apply
./manage backup now
# the backup check shows a start and a success ping within seconds
```

- Keep ping URLs private: anyone with one can send a fake success.
- Turn on the weekly report in account settings. It is sent on Mondays, with a summary of every check.

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

Every limit and the retention settings are listed, with their defaults, at the end of `librepaper.toml` (see [the whole file](simple.html#librepapertoml)). Uncomment a line to change it, then run `./manage apply`.

- Each account is limited in storage (`publisher_storage_mib`) and uploads per hour (`publisher_uploads_per_hour`).
- The whole server is limited in total storage (`deployment_storage_mib`), memory for loading documents (`memory_budget_mib`), edit history per document (`log_quota_mib`), and unsaved edits in memory and on disk (`pending_mib`, `pending_scratch_mib`).
- With [object storage](#object-storage), disk is no longer the constraint on `deployment_storage_mib`: set it to what you are willing to store.
- `[retention]` deletes documents after `expire_after`, counted from the last update by default or from creation.

## Without Docker

Run one `librepaper admin serve` process against durable PostgreSQL and file storage, behind an HTTPS proxy that preserves `Host` and forwards the client address. Configure LibrePaper to trust only that proxy's network. The server applies migrations at startup, so its database role must own the schema. See the [CLI guide](../cli.html) for config commands.

## Privacy

- Publish a privacy notice with your identity and contact, your retention settings, your storage and hosting providers, and how your proxy handles logs.
- LibrePaper does not log visitor requests or store visitor IP addresses. Your reverse proxy, CDN and hosting provider may keep their own logs.
- Accounts and documents are stored in plaintext.
- `librepaper export ID DIR` exports one project. There is no account-wide export.
- An account owner erases their account in **Settings > Account > Erase this account**.
- Restoring a backup can bring back data erased after that backup was taken. Keep a list of erasure requests outside the backups and apply it again after a restore.
