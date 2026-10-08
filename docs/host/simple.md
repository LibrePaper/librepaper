---
title: "Simple deployment"
notice: "LibrePaper is experimental. Self-host only if you can maintain its database, files, credentials, and recovery copies."
---

The setup runs on one VPS with Docker only, and you edit one file. Backups, object storage, alerts and the rest are in [Advanced features](advanced.html).

## LibrePaper

### How it works

Four programs run side by side on the VPS, each in its own Docker container:

| Service | What it does |
|---|---|
| Caddy | Answers on ports 80 and 443, obtains HTTPS certificates automatically, and passes requests to the app. |
| LibrePaper | The app: documents, sharing, comments, publishing, sign-in. |
| PostgreSQL | The database. |
| Backup | Copies the database and files to off-site storage every night, once you configure it. |

Your data lives in two places, both on the VPS's disk (Docker volumes):

- **The database** (`postgres` volume) holds accounts, sharing, comments, and each document's edit history.
- **Files** (`data` volume) hold uploaded figures, compacted document snapshots, and `session.key`, which signs sign-ins and share links.

Everything is on one machine, so losing the VPS loses it all until [backups](advanced.html#backups) are set up. The same page explains how to move files to an S3 bucket or the database to a managed service.

### What you need

- Use a virtual private server (for example OVHcloud, Hetzner, DigitalOcean or Linode).
- Install Docker Engine and Compose v2 on it.
- Open ports 80 and 443.
- Point two DNS names at it: one for the app (`paper.example`) and one for published documents (`docs.paper.example`).
- Create a GitHub OAuth app, a Google OAuth client, or both. See [Sign-in](#sign-in).

## Install

Each LibrePaper release comes with one download that contains everything the VPS needs: the Compose file that starts the four services, `librepaper.toml` for your settings, and the backup and Caddy configuration. It also includes `./manage`, the helper used below. Run this on the VPS, over SSH. It unpacks into a `librepaper/` directory that is private to you (mode 700).

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
```

## Sign-in

LibrePaper has no passwords of its own. People sign in with GitHub or Google, and the server never sees or stores a password. You register LibrePaper once with the provider, which gives you a client ID and a client secret for `librepaper.toml`. At least one provider is required. Both can be enabled.

**GitHub.** LibrePaper requests no scopes and receives the account's public login name.

- See [Creating an OAuth app](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app). On GitHub, the path is Settings, Developer settings, OAuth Apps, New OAuth App.
- Homepage URL: `https://paper.example`
- Authorization callback URL: `https://paper.example/auth/callback`
- Copy the client ID, generate a client secret, and put both in `[auth.github]`.

**Google.** LibrePaper requests the `openid`, `email` and `profile` scopes and receives the account's name and email address. A Gmail address must be verified by Google. An address at another domain signs in only if that domain uses Google Workspace.

- See [Setting up OAuth 2.0](https://support.google.com/cloud/answer/6158849). In Google Cloud Console, the path is APIs and Services, Credentials, Create credentials, OAuth client ID, Web application. Google asks you to configure the consent screen first.
- Authorized redirect URI: `https://paper.example/auth/callback/google`
- Put the client ID and secret in `[auth.google]`:

```toml
[auth.google]
client_id = "replace"
client_secret = "replace"
```

## Configure

Everything is set in one file, `librepaper.toml`. You need your two origins and at least one sign-in provider. Everything else has a working default. Leave the files readable (0644). The containers read them through bind mounts, and the 700 directory is what keeps them private. Never `chmod 600 librepaper.toml`.

```sh
$EDITOR librepaper.toml
```

### Origins

```toml
[origins]
app = "https://paper.example"
docs = "https://docs.paper.example"
```

`app` is where people use LibrePaper. `docs` serves published documents and must be a different host. A published document can run its own scripts (an HTML page, an interactive figure). On a separate host the browser treats it as a different website, so those scripts cannot see a reader's LibrePaper sign-in or act on their behalf.

### librepaper.toml

This is `librepaper.toml` as it comes in the download. The comments say what each setting does. The [advanced features](advanced.html) explain the optional ones.

```toml
# The one file to edit: set the two origins and one [auth.*] table, then docker compose up -d.
# Everything not listed here has a default; see docs/host/advanced.md.

# app is the application. docs serves published documents and must be a
# different host: a document can run its own scripts, and only a separate host
# keeps them away from a reader's sign-in. Both names need a DNS record pointing here.
[origins]
app = "https://paper.example"
docs = "https://docs.paper.example"

# Optional. Operational graphs at this name, behind the password in [admin].
# One more DNS record pointing here; Caddy obtains its certificate on first visit.
# admin = "https://admin.paper.example"

# Sign-in needs at least one provider. Callback URLs:
# GitHub  https://paper.example/auth/callback
# Google  https://paper.example/auth/callback/google
[auth.github]
client_id = "replace"
client_secret = "replace"

# [auth.google]
# client_id = "replace"
# client_secret = "replace"

# The bundled database, over the socket compose.yaml shares with it. For a
# database elsewhere, write the full URL here, with ?sslmode=verify-full for a managed service:
# database_url = "postgresql://user:password@db.example/librepaper?sslmode=verify-full"
[storage]
directory = "/var/lib/librepaper"
database_url = "postgresql:///librepaper?host=/var/run/postgresql&user=librepaper"
# database_connections = 20         # PostgreSQL connection pool size

# Who may publish and comment: "any" signed-in account, a GitHub login, a
# verified Google address, or "@example.org". "anyone" also allows
# anonymous comments.
[access]
publishers = ["any"]
commenters = ["any"]

# Matches the edge network in compose.yaml; do not widen.
[proxy]
trusted_networks = ["172.29.0.0/16"]

# Optional, with origins.admin. Paste 24 random bytes in base64 (head -c 24 /dev/urandom | base64).
# [admin]
# password = "replace"

# Optional. Every limit, with its default; uncomment a line to change it.
# [limits]
# Per account:
# publisher_storage_mib = 50         # storage per account, in MiB
# publisher_uploads_per_hour = 30    # projects, forks and figure uploads per rolling hour
# Whole server:
# deployment_storage_mib = 5120      # storage for all accounts together; uploads are refused when full
# memory_budget_mib = 512            # memory for loading documents
# log_quota_mib = 32                 # edit history one document holds before compaction; scale with memory_budget_mib
# pending_mib = 64                   # unsaved edits held in memory
# pending_scratch_mib = 161          # unsaved edits spilled to disk; the default follows log_quota_mib
# session_peer_queue_frames = 256    # messages queued per editing connection before it is dropped

# Optional. Without it, documents stay until deleted.
# [retention]
# expire_after = "30d"
# expire_from = "updated"            # or "created"
```

### Update configuration

The safe way is to edit a copy. `./manage apply` checks it before anything changes:

```sh
cp librepaper.toml librepaper.toml.candidate
# edit librepaper.toml.candidate, then:
./manage apply
```

- It validates the candidate, installs it over `librepaper.toml`, recreates the app and backup containers, reloads Caddy and waits for `/ready`.
- If a check fails, nothing changes.
- Editing `librepaper.toml` directly and running `./manage apply` works too.

## Deploy

Docker pulls the four images and starts the services. The database is created on the first start.

```sh
docker compose up -d
```

`./manage status` shows the result.

Once it is up, everything from [How it works](#how-it-works) is running, and the data lives where that section says.

## Check

```sh
./manage status    # services, pinned version, /ready, free disk
./manage check     # validates librepaper.toml, resticprofile.toml and the Caddyfile; warns if a name does not resolve
```

Caddy obtains a certificate the first time a name is visited, so the first request can take a few seconds. Open `https://paper.example` in a browser and sign in. If something fails:

```sh
./manage logs librepaper    # why the app is not answering
```

## Upgrade

```sh
./manage upgrade            # the latest release
./manage upgrade v0.0.25    # a specific release
```

- It backs up first when backups are configured. `--no-backup` skips this.
- It checks your configuration with the new release before switching. If a check fails, nothing switches.
- It keeps your `.toml` files and `caddy/local.d/`.
- It refuses to move to an older release.

The server migrates the schema at startup. `compose.yaml` is replaced on every upgrade. Put your own changes in `compose.override.yaml`.
