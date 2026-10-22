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

Providers, `publishers`, `commenters` and session length are set up in [Sign-in](simple.html#sign-in).

Anonymous comments need no sign-in. Turn them on in `[access]`:

```toml
[access]
anonymous_comments = true
```

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

- A backup records the database's migration version, and `admin restore` refuses a backup whose version differs from the binary's. Take a fresh backup after upgrading to a release that changes the migration history.
- Set up [backup alerts](#backup-alerts) so a failed backup emails you.
- After the first backup has created the repository, add `initialize = false` under `[resticprofile]`. A mistyped bucket or path then fails the run and the alert, instead of quietly starting an empty repository.
- For hourly backups, set `schedule = "hourly"` and give the Healthchecks check a 1 hour period and a 2 hour grace. Every run reads all stored files, so revisit an hourly schedule once the data passes about 20 GiB.
- A stored file that is missing from object storage does not stop the backup. The backup leaves it out, prints a `WARNING` line in the backup container's log, and lists it under `missing` in the backup's manifest. Look for those lines now and then.

### Keys and passwords

- The restic password is fixed for the life of the repository. Losing it loses every backup.
- Keep the restic password in a password manager, separate from any file it is also rendered into.
- If your secrets are encrypted with SOPS and a PGP key, keep an offline copy of the private key.
- Keep the two-factor recovery codes in the same place.

### Bucket keys and versioning

- Give the app and the backups separate buckets and separate keys.
- Limit each key to its own bucket, with only the actions it needs.

```json
{
  "Statement": [{
    "Effect": "Allow",
    "Action": [
      "s3:GetObject", "s3:PutObject", "s3:DeleteObject",
      "s3:ListBucket", "s3:GetBucketLocation",
      "s3:ListBucketMultipartUploads", "s3:ListMultipartUploadParts", "s3:AbortMultipartUpload"
    ],
    "Resource": ["arn:aws:s3:::your-backups-bucket", "arn:aws:s3:::your-backups-bucket/*"]
  }]
}
```

- Turn on versioning for the backups bucket. The bucket then keeps the old copy when a file is changed or deleted, which protects against mistakes. Add a lifecycle rule that removes those old copies after 30 days, so storage does not grow forever.

### A copy the server cannot delete

The server needs a key that can write to the backups bucket, and that key can also delete from it. If someone takes over the server, they can erase your live data and your backups in one go. Bucket versioning keeps old copies of changed or deleted files, but on some providers the same key can delete those old copies too.

The fix is a second copy that the server has no key for. Many object storage services, including AWS S3, OVHcloud and Backblaze B2, can copy every new file from one bucket to another bucket on their side. This is called replication. The server never talks to the second bucket, so it cannot damage it.

Set it up in your provider's console:

1. Create a second bucket, the replica, and turn on versioning in both buckets.
2. Add a replication rule from the backups bucket to the replica. Turn on the option to replicate deletes, often called delete markers. A file deleted from the backups bucket then stays in the replica as an old version, which you can still restore.
3. Copy the files that are already in the backups bucket. Replication only copies new files, and new snapshots depend on old ones. Most consoles have a one-time job for this, such as Batch Replication on AWS. Wait until it reports every file copied.
4. Give the replica a lifecycle rule that removes old versions after 30 days. Without it, the replica keeps every file forever.
5. Create a read-only key for the replica, under a different user from the one that owns the bucket. On some providers the owner can always delete, whatever its policy says. Keep that key on your own computer, never on the server. It needs the two version permissions, because a restore after an attack reads old versions.

```json
{
  "Statement": [{
    "Effect": "Allow",
    "Action": ["s3:GetObject", "s3:GetObjectVersion", "s3:ListBucket", "s3:ListBucketVersions", "s3:GetBucketLocation"],
    "Resource": ["arn:aws:s3:::your-backups-replica", "arn:aws:s3:::your-backups-replica/*"]
  }]
}
```

Replication runs in the background, and nothing warns you if it stops. Check the replica from your own computer now and then. The `--no-lock` flag lets restic read without writing, which a read-only key cannot do.

```sh
export RESTIC_REPOSITORY=s3:s3.provider.example/your-backups-replica/restic
export RESTIC_PASSWORD=... AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
restic snapshots --no-lock --latest 1   # the newest snapshot should be about an hour old
restic check --no-lock
```

Before you rely on the replica, restore the latest snapshot from it once with the same variables. It should finish without errors.

```sh
restic restore --no-lock latest --target /tmp/replica-test
```

- **To restore after an attack,** first copy the replica as it was before the damage, for example with `rclone copy --s3-version-at`. Then restore from that copy.
- **A key that can add but not delete does not work.** Restic must delete its own lock files, and it stops a backup that cannot do so for about 22 minutes.
- **The replica sits in the same provider account.** Someone who takes over that account reaches both buckets, which is why [Accounts](security.html#accounts) comes first.
- **The replica gives you a window, not a permanent copy.** Deleted files stay for 30 days. Damage that goes unnoticed for longer cannot be undone from the replica.

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

## Other apps on the same server

Another web app can share the kit's Caddy. It then needs no host ports of its own, so it gets HTTPS and nothing skips Caddy. Join the app to LibrePaper's edge network in the app's own compose file:

```yaml
services:
  web:
    ports: !reset []   # publish nothing on the host
    networks:
      default:
      librepaper:
        aliases: [otherapp]
networks:
  librepaper:
    name: librepaper_edge
    external: true
```

Then give Caddy a site block in `caddy/local.d/otherapp.caddy`, with the port the app listens on inside its container:

```
other.example {
	reverse_proxy otherapp:8000
}
```

```sh
# in the other app's directory
docker compose up -d
# in the LibrePaper directory
docker compose exec -T caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
docker compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile
```

- Point the app's DNS name at the server before the reload. Caddy obtains the certificate for a named site block when it loads the block.
- Upgrades never overwrite `local.d`.
- Start LibrePaper before the other app, because LibrePaper's project creates the network. Do not run `docker compose down` in the LibrePaper directory while the other app is attached.

## Behind your own Caddy

The kit ships its own Caddy, which answers on ports 80 and 443. Many VPS already run a Caddy for other sites, and only one program can hold those ports. In that case, keep your Caddy as the single entry point for every site and let it forward LibrePaper's names to the LibrePaper container. The bundled Caddy keeps running without host ports, so it no longer competes with yours.

The setup rests on four facts about the kit:

- The bundled Caddy stays in place because `./manage` validates and reloads it. Without host ports it receives no traffic and costs one idle container.
- LibrePaper trusts forwarded client addresses only from the kit's edge network, `172.29.0.0/16` (`[proxy] trusted_networks` in `librepaper.toml`). Both setups below put your Caddy inside that network, so `librepaper.toml` needs no change. Do not widen `trusted_networks`.
- LibrePaper answers 421 to names it is not configured for, so your Caddy must pass `Host` through unchanged. Caddy's `reverse_proxy` does this by default.
- Upgrades replace `compose.yaml` but keep `compose.override.yaml`, so every change below goes in the override, next to `compose.yaml`. There is only one override file. If you already use one (for example for [Database elsewhere](#database-elsewhere)), add these keys to it.

Both setups start the same way. In `compose.yaml`, the bundled Caddy publishes ports 80 and 443 on the host. Create `compose.override.yaml` in the `librepaper/` directory, next to `compose.yaml`, with the content below. Docker Compose reads both files and merges them, and `!reset []` replaces the bundled Caddy's port list with an empty one. The bundled Caddy then publishes no ports, and your Caddy keeps 80 and 443.

```yaml
# compose.override.yaml
services:
  caddy:
    ports: !reset []
```

To check the result, run `docker compose config` and look under `caddy:`. It should show no `ports:` entry.

Add this override before you first start the kit, because the bundled Caddy cannot start while your Caddy holds ports 80 and 443. On an installation that is already running, apply it with `docker compose up -d`. `./manage apply` is not enough, because it recreates only the app and backup containers.

```sh
# Recreate every service whose configuration changed, including the bundled Caddy.
docker compose up -d
```

What comes next depends on where your Caddy runs. If it runs as a Docker container, follow [Caddy in Docker](#caddy-in-docker). If you installed it on the host, for example with `apt`, follow [Caddy on the host](#caddy-on-the-host). Both end with the same [site block](#site-block).

### Caddy in Docker

A Caddy container cannot see LibrePaper until the two share a Docker network. Connect your Caddy to LibrePaper's edge network, and it reaches the app by its service name, `librepaper`. Add this to your Caddy's own compose file:

```yaml
services:
  caddy:
    networks: [default, librepaper]
networks:
  librepaper:
    name: librepaper_edge
    external: true
```

LibrePaper's Compose project creates that network, so start LibrePaper (`docker compose up -d` in its directory) before your Caddy. Do not run `docker compose down` in the LibrePaper directory, because it tries to remove the network your Caddy is attached to. Use `docker compose up -d` instead.

### Caddy on the host

A Caddy installed on the host is outside Docker and cannot look up service names like `librepaper`. Instead, give LibrePaper a fixed address on the edge network and point your Caddy at that address. The host reaches that network from `172.29.0.1`, which is inside the trusted range. This setup needs the kit from v0.0.47 or later, whose `./manage check` works with a fixed address. Under `services:` in the same override file, next to `caddy:`, add:

```yaml
  librepaper:
    networks:
      edge:
        ipv4_address: 172.29.200.10
```

Then run `docker compose up -d`, which recreates LibrePaper at that address.

### Site block

Your Caddy now reaches LibrePaper, but it does not yet know which names belong to it. One site block tells it to forward those names to LibrePaper and to obtain their certificates. List the names from the [DNS](#dns) table, and leave out `admin.paper.example` unless you configured the admin console.

```
paper.example, docs.paper.example, admin.paper.example {
	encode zstd gzip
	header Strict-Transport-Security "max-age=31536000; includeSubDomains"
	reverse_proxy librepaper:8080
}
```

With Caddy in Docker, the upstream is `librepaper:8080` as shown. With Caddy on the host, use `172.29.200.10:8080`. The `encode` and `header` lines copy what the bundled Caddy does. Reload your Caddy (`caddy reload` or its container equivalent) and open `https://paper.example`.

## Without Docker

Run one `librepaper admin serve` process against durable PostgreSQL and file storage, behind an HTTPS proxy that preserves `Host` and forwards the client address. Configure LibrePaper to trust only that proxy's network. The server applies migrations at startup, so its database role must own the schema. See the [CLI guide](../cli.html) for config commands.
