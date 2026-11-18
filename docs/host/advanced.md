---
title: "Advanced features"
---

Everything here is optional. Start with the [simple deployment](simple.html).

An agent can help you run the server. The [agent skill](../agents.html#agent-skill) covers upgrades, logs, moderation and restore drills.

## DNS

| Name | Required | Example |
|---|---|---|
| App | yes | `paper.example` |
| Published documents | yes | `docs.paper.example` |
| Admin console | no | `admin.paper.example` |

- Caddy gets a certificate for a name on its first visit. Before it does, it asks the server (`/api/tls/ask`) whether the name is a configured origin. The three names need no Caddy edits.
- Publish an AAAA record only if the VPS has IPv6.
- Redirecting names, such as `www`, go in `caddy/local.d/*.caddy`. Upgrades never overwrite `local.d`. The comments in `caddy/Caddyfile` show an example.
- If the VPS already runs Caddy for other sites, see [Behind your own Caddy](#behind-your-own-caddy).

## Sign-in and access

Providers, `publishers`, `commenters` and session length are set up in [Sign-in](simple.html#sign-in).

### Anonymous access

A link works without sign-in only when its owner marks it "Anyone with the link" and the matching switch in `[access]` is on. The four anonymous switches are off by default.

```toml
[access]
anonymous_reads    = false  # read links may be opened without signing in
anonymous_comments = false  # comment links may be used without signing in
anonymous_suggests = false  # suggest links may be opened without signing in, at comment or read level
anonymous_edits    = false  # edit links may be opened without signing in, at comment or read level
agent_connectors   = true   # owners may connect apps and agents as connectors (on by default)
```

Links the owner did not mark anonymous still require sign-in. Turning a switch off makes its links require sign-in again.

`agent_connectors` is not an anonymous switch. It lets a project owner connect apps and agents to the project as connectors, and each connection still needs the owner's approval. Set it to `false` to hide the connector in the Agent panel and stop new connections; see [Connect an app or agent](../agents.html#connect-an-app-or-agent).

Someone who is not signed in never edits or suggests. Under an anonymous comment link they can comment. Under an anonymous suggest or edit link they can comment only when `anonymous_comments` is also on, and otherwise they can only read.

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

**Private certificate authority.** A provider behind a private CA needs that CA in the container. Put the CA certificate in `ca.pem` next to `compose.yaml`, then add this to `compose.override.yaml`. Do not edit `compose.yaml`, because every upgrade replaces it.

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

Backups stay off until `resticprofile.toml` has a `[resticprofile]` table. Point it at a remote Restic repository, and put the password and any S3 credentials in the env table:

```toml
[resticprofile]
repository = "s3:s3.amazonaws.com/your-bucket/restic"

[resticprofile.env]
RESTIC_PASSWORD = "your-restic-password"
AWS_ACCESS_KEY_ID = "your-key"
AWS_SECRET_ACCESS_KEY = "your-secret"
```

Bring up the backup sidecar and check the merged profile:

```sh
./manage apply
docker compose exec backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile show   # the merged profile
./manage backup now
```

Each backup holds the database dump, the stored files, both TOML files and `session.key`. Keep the session key: it keeps existing sessions and saved share secrets working. Each backup streams the stored files straight into restic. Only the database dump goes through a temporary file, so the server needs free disk space for the database, not for the stored files.

The image's base profile sets the schedule and retention, and your `resticprofile.toml` is merged over it, so any setting you add there wins. The defaults:

- A backup runs daily at midnight UTC.
- Retention keeps every snapshot from the last 48 hours, the last snapshot of each of the last 14 days, and the last snapshot of each of the last 11 weeks. The oldest snapshot is about ten weeks old.
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
- If you change the schedule, change the Healthchecks period to match (see [backup alerts](#backup-alerts)). For hourly backups, use `schedule = "hourly"` with a 1 hour period and a 2 hour grace. Every run reads all stored files, so revisit an hourly schedule once the data passes about 20 GiB.
- After the first backup has created the repository, add `initialize = false` under `[resticprofile]`. A mistyped bucket or path then fails the run and the alert, instead of quietly starting an empty repository.
- A backup records the database's migration version, and restore checks the restored database against it. After an upgrade that changes the migration history, take a fresh backup.
- A stored file missing from object storage does not stop the backup. It is left out, a `WARNING` line appears in the backup container's log, and the manifest lists it under `missing`. The snapshot is kept, but the run counts as failed, so the alert fires on every run until the file returns or is no longer referenced. A `run-after` of your own in `resticprofile.toml` replaces this check.

### Keys and passwords

- The restic password is fixed for the life of the repository. Losing it loses every backup. Keep it in a password manager, apart from any file it is also rendered into.
- If your secrets are encrypted with SOPS, add a second recovery key as a recipient (for example an age key from `age-keygen`) and keep it in your password manager or on paper. Losing your everyday key then does not lock you out.
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

- Turn on versioning for the backups bucket, so it keeps the old copy of a changed or deleted file. Add a lifecycle rule that removes those old copies after 30 days, so storage does not grow forever.

### A copy the server cannot delete

The server's key can write to the backups bucket and also delete from it. If someone takes over the server, they can erase your live data and your backups together. Versioning does not help if the same key can delete old versions too.

Keep a second copy that the server has no key for. Many object storage services, including AWS S3, OVHcloud and Backblaze B2, can replicate every new file from one bucket to another on their side. The server never talks to the replica, so it cannot damage it.

Set it up in your provider's console:

1. Create a replica bucket and turn on versioning in both buckets.
2. Add a replication rule from the backups bucket to the replica, with delete replication (often called delete markers) on. A file deleted from the backups bucket then stays in the replica as an old version.
3. Copy the files already in the backups bucket. Replication only copies new files, and new snapshots depend on old ones. Use the console's one-time job, such as Batch Replication on AWS, and wait until it reports every file copied.
4. Give the replica a lifecycle rule that removes old versions after 30 days.
5. Create a read-only key for the replica, under a different user from the bucket's owner, because on some providers the owner can always delete. Keep the key on your own computer, never on the server. It needs the two version permissions, since a restore after an attack reads old versions. Some providers reject `s3:GetObjectVersion`; there, leave it out and let `s3:GetObject` read old versions. A new policy can take a few minutes to apply.

```json
{
  "Statement": [{
    "Effect": "Allow",
    "Action": ["s3:GetObject", "s3:GetObjectVersion", "s3:ListBucket", "s3:ListBucketVersions", "s3:GetBucketLocation"],
    "Resource": ["arn:aws:s3:::your-backups-replica", "arn:aws:s3:::your-backups-replica/*"]
  }]
}
```

Replication fails silently, so check the replica from your own computer now and then. `--no-lock` lets restic read without writing, which the read-only key requires. Before you rely on the replica, restore its latest snapshot once.

```sh
export RESTIC_REPOSITORY=s3:s3.provider.example/your-backups-replica/restic
export RESTIC_PASSWORD=... AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
restic snapshots --no-lock --latest 1   # the newest snapshot should be about an hour old
restic check --no-lock
restic restore --no-lock latest --target /tmp/replica-test
```

- **To restore after an attack,** first copy the replica as it was before the damage, for example with `rclone copy --s3-version-at`, and restore from that copy.
- **A key that can add but not delete does not work.** Restic must delete its own lock files, and it stops a backup that cannot do so for about 22 minutes.
- **The replica is a window, not a permanent copy.** Deleted files stay for 30 days. It sits in the same provider account, so someone who takes over that account reaches both buckets. Secure that first: see [Accounts](security.html#accounts).

### Inspect a backup

`./manage` reads the backup repository through the backup container. It needs the kit directory with a configured `resticprofile.toml`, so it also works on any machine with the same kit.

```sh
./manage backup list         # every snapshot: id, time, size
./manage backup show latest  # one snapshot: when it ran, migration version, stored files, database size
```

`list` and `show` change nothing.

### Restore drill

Restore a snapshot into a fresh copy of LibrePaper on another machine, to check that your backups work. Practice this before you need it. The copy publishes ports 80 and 443, so it needs a machine of its own.

```sh
# on another machine with Docker
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
cp /path/to/your/resticprofile.toml .   # the repository and its password are all a restore needs
./manage backup restore latest        # or a snapshot id from ./manage backup list
```

- The restore starts an empty database, loads the snapshot with `admin restore` (which verifies every file against the manifest), then puts the stored files and the snapshot's `session.key` in place, starts LibrePaper and checks `/ready`.
- It refuses to run where LibrePaper is already running, and `admin restore` refuses a database that is not empty, so it cannot overwrite a live deployment.
- Open the copy in a local browser or through an SSH tunnel, and check documents, figures and existing sign-ins. Do not expose it under your production names.
- Keep the kit's own `librepaper.toml`: it points at the empty local database. The copy's backup service stays off, so it never writes to your repository.
- When you are done, remove the copy with `docker compose down -v` and delete `resticprofile.toml`. Both hold real data or keys.

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

`allow_http = true` is for a local test endpoint only.

Figures and other assets reach the browser through links signed for five minutes, straight from the bucket. For that, the bucket must allow your app's origin through CORS for `GET`. Set the rule once, with the app origin from `[origins]` in `librepaper.toml`:

```sh
aws s3api put-bucket-cors --bucket your-bucket --endpoint-url https://s3.region.provider.example \
  --cors-configuration '{"CORSRules":[{"AllowedOrigins":["https://app.example.org"],"AllowedMethods":["GET"],"AllowedHeaders":[],"MaxAgeSeconds":3600}]}'
```

Without the rule everything still works, through the server, only slower. The server's clock must be correct, because the bucket refuses a signature dated outside its tolerance.

Requirements:

- A private bucket used only for this.
- A dedicated access key that can read, write, list and delete in that bucket only.
- Conditional writes. The server never overwrites a stored file: PUT sends `If-None-Match: *`. AWS S3, OVHcloud Object Storage and MinIO support this. Others may not, so test first.

Backups still include user files, read through the same store. The `backup` container therefore needs the same keys: it has them when they are written in `librepaper.toml`, and otherwise you mount the `{ file = ... }` references into it (see [Secrets](#secrets)).

## Health

The VPS cannot report its own outage, so alerts come from free outside services.

### LibrePaper admin console

The console is a page on its own hostname. You sign in with GitHub. It has three tabs:

- **Resources** graphs the server's health over the last 400 days: CPU, memory, free disk, stored bytes, open documents, live connections and database connections in use.
- **Accounts** lists every account, largest first, 50 to a page.
- **Moderation** blocks an account, refuses a name before its first sign-in, and hides a project. It lists what is blocked, refused and hidden, and the last 50 actions with their reasons.

```sh
# Optional. The console at https://admin.paper.example/. First add a DNS record for
# admin.paper.example, and add https://admin.paper.example/auth/callback to the GitHub OAuth app.
gh api users/<login> --jq .id            # your numeric GitHub ID
# librepaper.toml: under [origins] add   admin = "https://admin.paper.example"
#                  and a new table       [admin]
#                                        github_ids = [123456]
./manage apply
```

- Setup takes one DNS record and two lines in `librepaper.toml`. Caddy needs no changes: it asks the server before it obtains a certificate, and the server says yes to the three names.
- You sign in with GitHub, so your GitHub two-factor authentication protects the console. A session lasts 12 hours.
- Only the listed GitHub IDs get in. Remove an ID and restart the server to lock that person out.
- The console needs GitHub sign-in. A deployment with only Google, Entra or custom OpenID Connect sign-in has no console.
- Moderation records your GitHub login as the actor.
- A blocked account is signed out at once. The block follows the account, so changing its name does not lift it. A refused name covers only that exact name: someone who renames their GitHub account before signing in is not caught.
- Erasing an account signs it out and refuses its sign-in until the erasure is done. The same person can then sign in again and gets a fresh account with the starter project. Nothing else carries over.
- Zoom by dragging on any graph, and double-click to return to the chosen range.
- For your own Prometheus, add `[metrics]` with `address = "0.0.0.0:9091"` to `librepaper.toml`. The server then serves `/metrics` on that port inside the Docker network. The kit publishes no host port for it.

Moderation from the command line, for an account found on the console (the ID column is the UUID):

```sh
docker compose exec librepaper librepaper admin moderate block-account <uuid> --actor <you> --reason "<why>"
# Erase an account and every project it owns. Projects purge seven days after deletion. This cannot be undone.
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

- The journal's own limits decide how long logs stay. By default it uses at most 10% of the disk, up to 4 GB. Logs survive a reboot when the journal is persistent, which is the default on Ubuntu.
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
- Turn on the weekly report in your Healthchecks account settings. It is sent on Mondays, with a summary of every check.

## Secrets

Every secret in `librepaper.toml` also accepts `{ file = "/run/secrets/name" }` or `{ env = "NAME" }`.

- Mount the secrets directory with a bind volume in `compose.override.yaml`, into `librepaper` and, when the object storage keys are among them, into `backup`.
- Leave that directory at 0755 and the files readable. A mounted directory keeps its own mode inside the container, and the app runs as another uid. The kit directory's 0700 is what keeps it private.
- Keep `resticprofile.toml` and the restic password backed up somewhere other than the VPS.

## Database elsewhere

Set `storage.database_url` to a TCP URL with `sslmode=verify-full`, and create the role on the provider as a non-superuser database owner. Then:

```sh
cp compose.managed-db.yaml compose.override.yaml
```

The override survives upgrades.

## Limits and retention

Every limit and the retention settings are listed, with their defaults, at the end of `librepaper.toml` (see [the whole file](simple.html#librepapertoml)). Uncomment a line to change it, then run `./manage apply`.

- Per account: `publisher_storage_mib` and `publisher_uploads_per_hour`.
- Per server: `deployment_storage_mib`, `memory_budget_mib` (memory for loading documents), `log_quota_mib` (edit history per document), and `pending_mib` and `pending_scratch_mib` (unsaved edits in memory and on disk).
- The memory budget also caps live connections at 8 per MiB (4,096 at the default 512 MiB), with at most 64 per account and 32 editors per document.
- With [object storage](#object-storage), disk no longer limits `deployment_storage_mib`. Set it to what you are willing to store.
- `[retention]` deletes documents after `expire_after`, counted from the last update by default or from creation (`expire_from`).
- `erase_inactive_after` in `[retention]` erases accounts, and their documents, after that long without a sign-in. It is off by default and must be at least 60 days. The admin console shows how many accounts it would erase in the next 30 days.
- `[legal]` sets `terms_url` and `privacy_url`, which the sign-in page and the share-link notice then link to. Leave them out and no notice is shown.

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

The kit ships its own Caddy, which answers on ports 80 and 443. Many VPS already run a Caddy for other sites, and only one program can hold those ports. In that case, keep your Caddy as the single entry point for every site, and let it forward LibrePaper's names to the LibrePaper container.

Four facts shape this setup:

- The bundled Caddy stays, because `./manage` validates and reloads it. Without host ports it costs one idle container.
- LibrePaper trusts forwarded client addresses only from the edge network, `172.29.0.0/16` (`[proxy] trusted_networks`). Both setups below put your Caddy inside that network, so `librepaper.toml` does not change. Do not widen `trusted_networks`.
- LibrePaper answers 421 to names it is not configured for, so your Caddy must pass `Host` through unchanged. Caddy's `reverse_proxy` does this by default.
- Upgrades replace `compose.yaml` but keep `compose.override.yaml`. Put every change below in that one override file, next to `compose.yaml`. If you already use it (for example for [Database elsewhere](#database-elsewhere)), add these keys to it.

Both setups start the same way. Create `compose.override.yaml` in the `librepaper/` directory. Compose merges it with `compose.yaml`, and `!reset []` removes the bundled Caddy's published ports, so your Caddy keeps 80 and 443.

```yaml
# compose.override.yaml
services:
  caddy:
    ports: !reset []
```

Run `docker compose config` and look under `caddy:`. It should show no `ports:` entry.

Add this override before you first start the kit, because the bundled Caddy cannot start while your Caddy holds ports 80 and 443. On an installation that is already running, apply it with `docker compose up -d`. `./manage apply` is not enough, because it recreates only the app and backup containers.

```sh
# Recreate every service whose configuration changed, including the bundled Caddy.
docker compose up -d
```

Next, follow [Caddy in Docker](#caddy-in-docker) if your Caddy runs as a Docker container, or [Caddy on the host](#caddy-on-the-host) if you installed it with `apt` or similar. Both end with the same [site block](#site-block).

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

A Caddy on the host cannot look up service names, so give LibrePaper a fixed address on the edge network and point your Caddy at it. The host reaches that network from `172.29.0.1`, inside the trusted range. This needs kit v0.0.47 or later. Under `services:` in the same override file, next to `caddy:`, add:

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

With Caddy in Docker, the upstream is `librepaper:8080` as shown. With Caddy on the host, use `172.29.200.10:8080`. The `encode` and `header` lines copy what the bundled Caddy does. Reload your Caddy (`caddy reload`, or its container equivalent) and open `https://paper.example`.

## Without Docker

Run one `librepaper admin serve` process against durable PostgreSQL and file storage, behind an HTTPS proxy that preserves `Host` and forwards the client address. Configure LibrePaper to trust only that proxy's network. The server applies migrations at startup, so its database role must own the schema. See the [CLI guide](../cli.html) for config commands.
