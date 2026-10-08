---
title: "Simple deployment"
---

One VPS, Docker only, one file to edit. Backups, object storage, alerts and the rest are in [Advanced features](advanced.html).

> LibrePaper is experimental. Self-host only if you can maintain its database,
> files, credentials, and recovery copies.

## Install

### How it works

Four programs run side by side on the VPS, each in its own Docker container:

| Service | What it does |
|---|---|
| Caddy | Answers on ports 80 and 443, obtains HTTPS certificates automatically, and passes requests to the app. |
| LibrePaper | The app: documents, sharing, comments, publishing, sign-in. |
| PostgreSQL | The database. |
| Backup | Copies the database and files to off-site storage every night, once you configure it. |

Your data lives in two places, both on the VPS's disk (Docker volumes):

- **The database** (`postgres` volume): accounts, sharing, comments, and each document's edit history.
- **Files** (`data` volume): uploaded figures and compacted document snapshots, plus `session.key`, which signs sign-ins and share links.

Everything is on one machine, so losing the VPS loses it all until [backups](advanced.html#backups) are set up. The same page explains how to move files to an S3 bucket or the database to a managed service.

### What you need

- A virtual private server (for example OVHcloud, Hetzner, DigitalOcean or Linode).
- Docker Engine and Compose v2 on it.
- Ports 80 and 443 open.
- Two DNS names pointing at it: one for the app (`paper.example`) and one for published documents (`docs.paper.example`).
- A GitHub OAuth app, a Google OAuth client, or both: see [Sign-in](#sign-in) under Configure.

### Download

Each LibrePaper release comes with one download that contains everything the VPS needs: the Compose file that starts the four services, `librepaper.toml` for your settings, and the backup and Caddy configuration. Run this on the VPS, over SSH; it unpacks into a `librepaper/` directory that is private to you (mode 700).

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
```

## Configure

Everything is set in one file, `librepaper.toml`. You need your two origins and at least one sign-in provider; everything else has a working default.

```sh
$EDITOR librepaper.toml
```

### Origins

```toml
[origins]
app = "https://paper.example"
docs = "https://docs.paper.example"
```

`app` is where people use LibrePaper. `docs` serves published documents and must be a different host: a document is code, and the browser is what keeps it out of a reader's session.

### Sign-in

LibrePaper has no passwords of its own: people sign in with GitHub or Google, and the server never sees or stores a password. You register LibrePaper once with the provider, which gives you a client ID and a client secret for `librepaper.toml`. At least one provider is required; set up both to let people choose.

**GitHub** suits most researchers and developers, who already have an account. LibrePaper asks for no permissions: it receives only the public login name.

- Instructions: [Creating an OAuth app](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app) (on GitHub: Settings, Developer settings, OAuth Apps, New OAuth App).
- Homepage URL: `https://paper.example`
- Authorization callback URL: `https://paper.example/auth/callback`
- Copy the client ID, generate a client secret, and put both in `[auth.github]`.

**Google** covers everyone without a GitHub account, and lets you open publishing to a whole email domain (for example your university's). LibrePaper asks for the account's name and email address. A Gmail address must be verified by Google; an address at another domain signs in only if that domain uses Google Workspace.

- Instructions: [Setting up OAuth 2.0](https://support.google.com/cloud/answer/6158849) (in Google Cloud Console: APIs and Services, Credentials, Create credentials, OAuth client ID, Web application). Google asks you to configure the consent screen first.
- Authorized redirect URI: `https://paper.example/auth/callback/google`
- Put the client ID and secret in `[auth.google]`:

```toml
[auth.google]
client_id = "replace"
client_secret = "replace"
```

### Permissions

Leave the files readable (0644): the containers read them through bind mounts, and the 700 directory is what keeps them private. Never `chmod 600 librepaper.toml`.

### The whole file

`librepaper.toml` as it comes in the download. The comments say what each setting does; the [advanced features](advanced.html) explain the optional ones.

```toml
# The one file to edit: set the two origins and one [auth.*] table, then docker compose up -d.
# Everything not listed here has a default; see docs/host/advanced.md.

# app is the application. docs serves published documents and must be a
# different host, because a document is code and the browser is what keeps
# it out of a reader's session. Both names need a DNS record pointing here.
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
# Alternatively: client_secret = { file = "/run/secrets/github" }

# [auth.google]
# client_id = "replace"
# client_secret = "replace"

# The bundled database, over the socket compose.yaml shares with it. For a
# database elsewhere, write the full URL here, with ?sslmode=verify-full for a managed service:
# database_url = "postgresql://user:password@db.example/librepaper?sslmode=verify-full"
[storage]
directory = "/var/lib/librepaper"
database_url = "postgresql:///librepaper?host=/var/run/postgresql&user=librepaper"

# Who may publish and comment: "any" signed-in account, a GitHub login, a
# verified Google address, or "@example.org". "anyone" also allows
# anonymous comments.
[access]
publishers = ["any"]
commenters = ["any"]

# Matches the edge network in compose.yaml; do not widen.
[proxy]
trusted_networks = ["172.29.0.0/16"]

# Private listener for your own Prometheus. Nothing in the kit scrapes it and no host port is published.
[metrics]
address = "0.0.0.0:9091"

# Optional, with origins.admin. Paste 24 random bytes in base64 (head -c 24 /dev/urandom | base64).
# [admin]
# password = "replace"

# Optional. Per-publisher storage in MiB and uploads per rolling hour.
# [limits]
# publisher_storage_mib = 50
# publisher_uploads_per_hour = 30

# Optional. Without it, documents stay until deleted.
# [retention]
# expire_after = "30d"
# expire_from = "created"
```

### Changing it later

After editing `librepaper.toml` or `resticprofile.toml`, recreate the containers that read them. Compose compares service definitions, not the bytes behind a mounted file:

```sh
docker compose up -d --force-recreate librepaper backup
```

## Deploy

Docker pulls the four images and starts the services. The database is created on the first start.

```sh
docker compose up -d
```

Once it is up, everything from [How it works](#how-it-works) is running, and the data lives where that section says.

## Check

Caddy obtains a certificate the first time a name is visited, so the first request can take a few seconds. A successful answer means the app is serving.

```sh
curl -fsS https://paper.example/ready
```

Then open `https://paper.example` in a browser and sign in. If something fails:

```sh
docker compose ps                 # all four services should be running
docker compose logs librepaper    # why the app is not answering
```

## Upgrade

```sh
cd ..   # the directory that contains librepaper/
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz --exclude='*/librepaper.toml' --exclude='*/resticprofile.toml'
cd librepaper && docker compose pull && docker compose up -d
```

The server migrates the schema at startup. The same steps apply when a release note says the download's files changed. `compose.yaml` is replaced on every upgrade: put your own changes in `compose.override.yaml`.
