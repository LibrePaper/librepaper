---
title: "Advanced features"
---

Everything here is optional. Start with the [simple deployment](simple.html).

## DNS

| Name | Required | Example |
|---|---|---|
| App | yes | `paper.example` |
| Published documents | yes | `docs.paper.example` |
| Admin console | no | `admin.paper.example` |

- Caddy obtains a certificate for a name on its first visit, after asking the server (`/api/tls/ask`) whether it is a configured origin. No Caddy edits are needed for the three names.
- Publish an AAAA record only if the VPS has IPv6.
- Extra names that redirect (for example `www`) go in `caddy/local.d/*.caddy`. Upgrades never overwrite `local.d`. See the comments in `caddy/Caddyfile`.
- If the VPS already runs Caddy for other sites, see [Behind your own Caddy](#behind-your-own-caddy).

## Sign-in and access

At least one sign-in provider is needed.

| Provider | Callback URL |
|---|---|
| GitHub | `https://paper.example/auth/callback` |
| Google | `https://paper.example/auth/callback/google` |
| Microsoft Entra ID | `https://paper.example/auth/callback/entra` |
| Other OpenID Connect | `https://paper.example/auth/callback/oidc-<name>` |

Each provider has its own table in `librepaper.toml`. The keys are:

- `[auth.github]` and `[auth.google]` take `client_id` and `client_secret`.
- `[auth.entra]` takes `tenant`, `client_id`, `client_secret` and `label`. The tenant is the Directory (tenant) ID, a GUID. The label is the button text and defaults to `Microsoft`.
- `[auth.oidc.<name>]` takes `issuer`, `client_id`, `client_secret`, `label` and `scopes`. The label is required. The scopes default to `openid email profile`. You can add up to 8 tables.
- `[auth]` takes `session_lifetime`, from `1h` to `30d`. It is optional. Absent, a browser session lasts 30 days. Set, it bounds sessions to that. Sessions do not renew on use, and a CLI token never lasts longer than the browser session that approved it.

`[access]` says who may publish and comment:

```toml
[access]
publishers = ["any"]
commenters = ["any"]
```

Each value is a list of:

- `"any"` means any signed-in account.
- A GitHub login.
- An email address a configured provider has verified (Google, Entra, or a custom OpenID Connect provider).
- `"@example.org"` means a whole domain.
- `"anyone"` means anonymous comments with no sign-in. It is valid for commenters only.

### Identity providers

The server records each provider id and its issuer in the `sign_in_issuers` table on first start. If the configured issuer changes later, the server refuses to start. Its message begins with `auth provider <id> was bound to issuer <old> and is now configured with <new>.`

To rebind an id on purpose, delete its row and restart the app.

```sh
# Only when the new issuer is the same identity provider at a new address, so
# every subject stays the same (Keycloak dropped its /auth prefix in version 17,
# or the hostname moved). A different provider under the same id would hand old
# accounts to strangers: give it a new name instead.
docker compose exec postgres psql -U librepaper -d librepaper -c "DELETE FROM sign_in_issuers WHERE provider = 'oidc-keycloak'"
docker compose restart librepaper   # binds the new issuer on start
```

**Private certificate authority.** A provider behind a private CA needs that CA in the container. Put the CA certificate in `ca.pem` next to `compose.yaml`. Then add this to `compose.override.yaml`, because `compose.yaml` is replaced on every upgrade:

```yaml
services:
  librepaper:
    volumes:
      - ./ca.pem:/etc/ssl/private-ca.pem:ro
    environment:
      - SSL_CERT_FILE=/etc/ssl/private-ca.pem
```

Run `docker compose up -d` to apply it.

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

Each backup streams the database and every stored file straight into restic as one archive. Only the database dump is written to a temporary file first, so the server needs free disk space for the database, not for the stored files.

Schedule and retention come from a base profile inside the backup image. Your `resticprofile.toml` is merged over it, so any setting you add there wins. The defaults:

- A backup runs daily at midnight UTC.
- Retention keeps every snapshot from the last 48 hours, plus the last snapshot of each of the last 14 days and of each of the last 11 weeks. The rules overlap, so the oldest snapshot is about ten weeks old.
- Days and weeks without a snapshot do not count, so a run of failed backups keeps older snapshots longer.
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

- Leave `source` alone: it streams the database export that every snapshot contains.
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
- The copy's backup service stays off, so it never writes to your repository. When you are done, remove the copy with `docker compose down -v` and delete `resticprofile.toml`. Both hold real data or keys.

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

The console is a page on its own hostname. You sign in with GitHub. It has three tabs:

- **Resources** graphs the server's health over the last 400 days: CPU, memory, free disk, stored bytes, open documents, live connections and database connections in use.
- **Accounts** lists the 50 accounts holding the most storage.
- **Moderation** blocks an account, refuses a name before its first sign-in, and hides a project. It lists what is blocked, refused and hidden, and the last 50 actions with their reasons.

```sh
# Optional. The admin console at https://admin.paper.example/, signed in with GitHub.
# DNS: admin.paper.example -> this VPS, like the other two names.
# GitHub: in the OAuth app, add https://admin.paper.example/auth/callback to its callback URLs.
gh api users/<login> --jq .id            # your numeric GitHub ID
# librepaper.toml: under [origins] add   admin = "https://admin.paper.example"
#                  and a new table       [admin]
#                                        github_ids = [123456]
./manage apply
```

- It takes one DNS record and two lines in the one file. Caddy needs no changes. Caddy asks the server before obtaining a certificate, and the server says yes to three names.
- You sign in with GitHub, so your GitHub two-factor authentication protects the console. A session lasts 12 hours.
- Only the listed GitHub IDs get in. Remove an ID and restart the server to lock that person out at once.
- Moderation records your GitHub login as the person who acted.
- The console needs GitHub sign-in. A deployment with only Google, Entra or custom OpenID Connect sign-in has no console.
- A blocked account is signed out at once. Its block follows the account, so changing the name does not lift it.
- Erasing an account signs it out at once and refuses its sign-in until the erasure is done. The same person can then sign in again and gets a fresh account with the starter project, and nothing else carries over.
- A refused name covers only that name. Someone who renames their GitHub account before signing in is not caught.
- Zoom by dragging on any graph. Double-click to return to the chosen range.
- For your own Prometheus, add `[metrics]` with `address = "0.0.0.0:9091"` to `librepaper.toml`: the server then serves `/metrics` on that port inside the Docker network. The kit publishes no host port for it, and the graphs above do not need it.

```sh
# Block an account found on the console. The ID column is the UUID.
docker compose exec librepaper librepaper admin moderate block-account <uuid> --actor <you> --reason "<why>"
# Erase an account and every project it owns. Projects purge after seven days. The service starts within the hour. This cannot be undone.
docker compose exec librepaper librepaper admin moderate erase-account <uuid> --actor <you> --reason "<why>"
```

### Uptime

[UptimeRobot](https://uptimerobot.com) emails you the day the site is down.

- Create an HTTP(s) monitor for `https://paper.example/ready`, checked every 5 minutes.
- `/ready` answers 200 only when the app serves requests. The home page alone can look fine while the app is down.
- Add your email as an alert contact.

### Logs

Every upgrade and every `./manage apply` recreates the app and backup containers, and Docker deletes a container's logs with it. To keep them, send both to the host's journal. This needs systemd-journald, which most Linux servers run. Add this to `compose.override.yaml`, then run `./manage apply`:

```yaml
services:
  librepaper:
    logging:
      driver: journald
      options:
        tag: "{{.Name}}"
  backup:
    logging:
      driver: journald
      options:
        tag: "{{.Name}}"
```

```sh
./manage logs librepaper                                    # follows the running app, as before
sudo journalctl -t librepaper-librepaper-1 --since "1 day ago"   # any time, across upgrades
```

- The journal's own limits decide how long logs stay. By default it uses at most 10% of the disk, up to 4 GB.
- Logs survive a reboot when the journal is persistent, which is the default on Ubuntu.
- Without this, `./manage logs` shows only what the current containers logged since they were last recreated.

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
- Live connections are capped from the memory budget, 8 per MiB (4,096 at the default 512 MiB), with at most 64 per account and 32 editors per document.
- With [object storage](#object-storage), disk is no longer the constraint on `deployment_storage_mib`: set it to what you are willing to store.
- `[retention]` deletes documents after `expire_after`, counted from the last update by default or from creation.
- `erase_inactive_after` in `[retention]` erases accounts that have not signed in for that long, with every document they own. It is off by default and must be at least 60 days. The admin console shows how many accounts it will erase in the next 30 days.

## Behind your own Caddy

When the VPS already runs Caddy for other sites, that Caddy owns ports 80 and 443. Keep it and let it proxy to LibrePaper.

- The bundled Caddy keeps running but publishes no host ports. `./manage check` and `./manage apply` still validate and reload it. It costs one idle container.
- LibrePaper trusts forwarded client addresses only from the kit's edge network, `172.29.0.0/16` (`[proxy] trusted_networks` in `librepaper.toml`). Both setups below put your Caddy inside that network, so `librepaper.toml` needs no change. Do not widen `trusted_networks`.
- Your Caddy must pass `Host` through unchanged. Caddy's `reverse_proxy` does this by default. LibrePaper answers 421 to names it is not configured for.
- Make the changes in `compose.override.yaml`, next to `compose.yaml`. Upgrades replace `compose.yaml` and keep the override. There is only one override file. If you already use one (for example for [Database elsewhere](#database-elsewhere)), add these keys to it.

Both setups start with this override for the bundled Caddy:

```yaml
services:
  caddy:
    ports: !reset []
```

### Caddy in Docker

Attach your Caddy container to the network `librepaper_edge`. In your Caddy's own compose file:

```yaml
services:
  caddy:
    networks: [default, librepaper]
networks:
  librepaper:
    name: librepaper_edge
    external: true
```

Run `./manage apply` in the LibrePaper directory before you start your Caddy. LibrePaper's Compose project creates that network. Do not run `docker compose down` in the LibrePaper directory: it tries to remove the network your Caddy is attached to. Use `./manage apply` or `docker compose up -d` instead.

### Caddy on the host

Give LibrePaper a fixed address on the edge network. Your host Caddy reaches it from `172.29.0.1`, which is inside the trusted network. Under `services:` in the same override file, next to `caddy:`, add:

```yaml
  librepaper:
    networks:
      edge:
        ipv4_address: 172.29.200.10
```

Then run `./manage apply`.

### Site block

Add a site block to your Caddy. List the names from the [DNS](#dns) table, and leave out `admin.paper.example` unless you configured the admin console.

```
paper.example, docs.paper.example, admin.paper.example {
	encode zstd gzip
	header Strict-Transport-Security "max-age=31536000; includeSubDomains"
	reverse_proxy librepaper:8080
}
```

Use `librepaper:8080` as the upstream with Caddy in Docker. With Caddy on the host, use `172.29.200.10:8080`.

The `encode` and `header` lines copy what the bundled Caddy does. Then reload your Caddy (`caddy reload` or its container equivalent) and open `https://paper.example`.

## Without Docker

Run one `librepaper admin serve` process against durable PostgreSQL and file storage, behind an HTTPS proxy that preserves `Host` and forwards the client address. Configure LibrePaper to trust only that proxy's network. The server applies migrations at startup, so its database role must own the schema. See the [CLI guide](../cli.html) for config commands.
