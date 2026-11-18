---
title: "Simple deployment"
notice: "LibrePaper is experimental. Self-host only if you can maintain its database, files, credentials, and recovery copies."
---

This setup runs on one VPS with Docker. You edit one file, `librepaper.toml`. Backups, object storage, alerts and the rest are in [Advanced features](advanced.html).

## LibrePaper

### How it works

Four containers run on the VPS:

| Service | What it does |
|---|---|
| Caddy | Answers on ports 80 and 443, gets HTTPS certificates, and passes requests to the app. |
| LibrePaper | The app: documents, sharing, comments, publishing, sign-in. |
| PostgreSQL | The database. |
| Backup | Copies the database and files off the VPS once a day, once you configure it. |

Your data lives in two Docker volumes on the VPS's disk:

- **`postgres`** holds accounts, sharing, comments, and each document's edit history.
- **`data`** holds uploaded figures, compacted document snapshots, and `secrets/session.key`, which signs sign-ins and share links.

Everything is on one machine, so losing the VPS loses it all until [backups](advanced.html#backups) are set up.

### What you need

- A virtual private server, for example from OVHcloud, Hetzner, DigitalOcean or Linode.
- Docker Engine and Compose v2 on the server.
- Ports 80 and 443 open.
- Two DNS names pointing at the server: one for the app (`paper.example`) and one for published documents (`docs.paper.example`).
- At least one sign-in provider. See [Sign-in](#sign-in).

## Install

Run this on the VPS, over SSH:

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
```

This unpacks a `librepaper/` directory, private to you (mode 700). It holds the Compose file, `librepaper.toml`, the backup and Caddy configuration, and `./manage`, the helper used below. The two LibrePaper images are pinned by digest, so an upgrade runs the images the release was built from. PostgreSQL and Caddy are pinned by version tag.

## Configure

Set everything in `librepaper.toml`. You need your two origins and one sign-in provider. Everything else has a working default.

Keep the file readable (0644). The containers read it through bind mounts, and the 700 directory keeps it private. Never `chmod 600 librepaper.toml`.

```sh
$EDITOR librepaper.toml
```

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

# Optional. The operator console (graphs, accounts, moderation) at this name.
# People sign in with GitHub; list who may in [admin] below.
# One more DNS record pointing here; Caddy obtains its certificate on first visit.
# admin = "https://admin.paper.example"

# Sign-in needs at least one provider. Callback URLs:
# GitHub  https://paper.example/auth/callback
# Google  https://paper.example/auth/callback/google
# Microsoft  https://paper.example/auth/callback/entra
# ORCID  https://paper.example/auth/callback/orcid
# Other OIDC https://paper.example/auth/callback/oidc-<name>
#
# publishers: "*" admits any account of that provider. Otherwise list GitHub
# logins, verified addresses or @domains (Google, Entra, OIDC), or ORCID iDs.
# A provider without publishers lets nobody from it publish. commenters
# default to "*".
[auth.github]
client_id = "replace"
client_secret = "replace"
publishers = ["*"]
commenters = ["*"]

# [auth.google]
# client_id = "replace"
# client_secret = "replace"
# publishers = ["*"]
# commenters = ["*"]

# [auth.entra]
# tenant = "00000000-0000-0000-0000-000000000000"   # Directory (tenant) ID, a GUID
# client_id = "replace"                             # Application (client) ID
# client_secret = "replace"
# label = "Microsoft"                               # the button text
# publishers = ["*"]
# commenters = ["*"]

# [auth.oidc.example]
# issuer = "https://sso.example.org/realms/staff"   # the provider's issuer URL
# client_id = "replace"
# client_secret = "replace"
# label = "Example SSO"                             # the button text
# publishers = ["*"]
# commenters = ["*"]

# [auth.orcid]
# client_id = "APP-XXXXXXXXXXXXXXXX"                # the ORCID API client ID
# client_secret = "replace"
# publishers = ["0000-0002-1825-0097"]              # ORCID iDs, or "*"
# commenters = ["*"]

# [auth]
# session_lifetime = "24h"   # 1h to 30d. Absent, a browser session lasts 30 days.

# The bundled database, over the socket compose.yaml shares with it. For a
# database elsewhere, write the full URL here, with ?sslmode=verify-full for a managed service:
# database_url = "postgresql://user:password@db.example/librepaper?sslmode=verify-full"
[storage]
directory = "/var/lib/librepaper"
database_url = "postgresql:///librepaper?host=/var/run/postgresql&user=librepaper"
# database_connections = 20         # PostgreSQL connection pool size

# Optional. Anonymous access, per link. A link works without sign-in only when
# its owner chose "Anyone with the link" and its switch here is true. All off by default.
# [access]
# anonymous_reads    = false  # read links may be opened without signing in
# anonymous_comments = false  # comment links may be used without signing in
# anonymous_suggests = false  # reserved: suggest links always need sign-in for now
# anonymous_edits    = false  # reserved: edit links always need sign-in for now
# agent_connectors   = true   # owners may connect a chat app as a connector (on by default)

# Matches the edge network in compose.yaml; do not widen.
[proxy]
trusted_networks = ["172.29.0.0/16"]

# Optional, with origins.admin and [auth.github]. Numeric GitHub user IDs that
# may sign in (gh api users/<login> --jq .id). Also add
# https://admin.paper.example/auth/callback to the GitHub OAuth app's callback URLs.
# [admin]
# github_ids = [123456]

# Optional. Every limit, with its default; uncomment a line to change it.
# [limits]
# Per account:
# publisher_storage_mib = 50         # storage per account, in MiB
# publisher_uploads_per_hour = 30    # projects, forks and figure uploads per rolling hour
# Whole server:
# deployment_storage_mib = 5120      # storage for all accounts together; uploads are refused when full
# memory_budget_mib = 512            # memory for loading documents; also sets the live connection cap (8 per MiB)
# log_quota_mib = 32                 # edit history one document holds before compaction; scale with memory_budget_mib
# pending_mib = 64                   # unsaved edits held in memory
# pending_scratch_mib = 161          # unsaved edits spilled to disk; the default follows log_quota_mib
# session_peer_queue_frames = 256    # messages queued per editing connection before it is dropped

# Optional. Without it, documents and accounts stay until deleted.
# [retention]
# expire_after = "30d"
# expire_from = "updated"            # or "created"
# erase_inactive_after = "365d"      # erase accounts with no sign-in for this long, with their documents

# Optional. Links the sign-in page shows as this server's terms of service and privacy policy.
# [legal]
# terms_url = "https://example.org/terms.html"
# privacy_url = "https://example.org/privacy.html"
```

### Update configuration

Edit a copy, then apply it. `./manage apply` checks the copy before anything changes:

```sh
cp librepaper.toml librepaper.toml.candidate
# edit librepaper.toml.candidate, then:
./manage apply
```

- `apply` validates the candidate, installs it as `librepaper.toml`, recreates the app and backup containers, reloads Caddy and waits for `/ready`.
- If a check fails, nothing changes.
- You can also edit `librepaper.toml` directly and run `./manage apply`.

## Sign-in

LibrePaper has no passwords of its own. People sign in with GitHub, Google, Microsoft Entra ID, ORCID or any OpenID Connect provider, and the server never sees a password. Register LibrePaper once with each provider, and copy the client ID and client secret into that provider's table in `librepaper.toml`. Enable as many providers as you need. At least one is required.

### Who may publish and comment

Each provider table sets `publishers` and `commenters`:

- `publishers` lists the accounts that may publish. A provider table without it lets nobody from that provider publish. Each entry is one of:
  - `"*"`, any account of that provider.
  - A GitHub login.
  - A verified email address, or `"@example.org"` for a whole domain. This works for Google, Entra and custom OpenID Connect providers.
  - An ORCID iD, such as `"0000-0002-1825-0097"`, for ORCID.
- `commenters` takes the same values. It defaults to `["*"]`, any account of that provider.
- `publishers` also decides who may get suggest or edit access through a link or an agent token. Other accounts get comment access.
- An email or `@domain` entry admits whoever the provider verifies as owning that address. Trusting a provider with an address means trusting its administrators with it, so prefer `@domain` entries for domains the provider owns.

A browser session lasts 30 days. To shorten it, set `session_lifetime` in `[auth]`, from `1h` to `30d`. Sessions do not renew on use. A CLI token never outlives the browser session that approved it.

### Providers

**GitHub.** LibrePaper requests the `user:email` scope. It reads the account's login and its primary verified email address. The address is stored as a contact address and never used to grant access. The admin console requests no scopes.

- Create the app at [Creating an OAuth app](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app): Settings, Developer settings, OAuth Apps, New OAuth App.
- Homepage URL: `https://paper.example`. Authorization callback URL: `https://paper.example/auth/callback`.
- Put the client ID and a generated client secret in `[auth.github]`.

**Google.** LibrePaper requests the `openid`, `email` and `profile` scopes. A Gmail address must be verified by Google. An address at another domain signs in only if that domain uses Google Workspace.

- Create the credentials in [Setting up OAuth 2.0](https://support.google.com/cloud/answer/6158849). In Google Cloud Console, go to APIs and Services, Credentials, Create credentials, OAuth client ID, Web application. Google asks you to configure the consent screen first.
- Authorized redirect URI: `https://paper.example/auth/callback/google`.
- Put the client ID and secret in `[auth.google]`.

**Microsoft Entra ID.** Sign-in admits the members and guests of one Microsoft Entra tenant. Register a single-tenant web app with the redirect URI `https://paper.example/auth/callback/entra`, following [Register an application](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app). Put the tenant ID (a GUID, not a domain name), the application ID and a client secret in `[auth.entra]`. Members removed from the tenant keep access until their session ends, so set `session_lifetime` to shorten that.

**ORCID.** LibrePaper requests only the `openid` scope and receives the ORCID iD and the name on the ORCID record. ORCID sends no email address, so an account's handle is its iD.

- Register a free public API client at [ORCID developer tools](https://orcid.org/developer-tools). The redirect URI is `https://paper.example/auth/callback/orcid`. ORCID requires HTTPS redirect URIs.
- Put the client ID (it starts with `APP-`) and the client secret in `[auth.orcid]`, as in the file above.

**Other OpenID Connect providers.** Any provider with an OpenID Connect discovery document works. Add one `[auth.oidc.<name>]` table per provider, up to 8. The name starts with a letter and uses lowercase letters, digits and hyphens, up to 24 characters.

- Register LibrePaper as a client with the callback URL `https://paper.example/auth/callback/oidc-<name>`.
- `issuer` and `label` (the button text) are required. `scopes` defaults to `openid email profile`.
- The provider must send `email_verified` and mark addresses as verified. AD FS does not, so it is not supported.

Keycloak and GitLab, for example:

```toml
[auth.oidc.keycloak]
issuer = "https://sso.example.org/realms/staff"   # the realm's issuer, no trailing path
client_id = "librepaper"
client_secret = "replace"
label = "Example SSO"

[auth.oidc.gitlab]
issuer = "https://gitlab.com"                     # or your own GitLab
client_id = "replace"
client_secret = "replace"
label = "GitLab"
# scopes = "openid email profile"                 # the default
```

**Accounts on your own server.** To sign people in without an outside service, run a self-hosted OpenID Connect provider beside LibrePaper, such as [Authentik](https://goauthentik.io), [Authelia](https://www.authelia.com), [Keycloak](https://www.keycloak.org), [Pocket ID](https://pocket-id.org), [Kanidm](https://kanidm.com) or [Zitadel](https://zitadel.com). You create and remove accounts in that provider. Add it as an `[auth.oidc.<name>]` table, and check that it sends `email_verified: true` for your users.

**Known limits.**

- A person who signs in through two providers has two accounts, each with its own projects.
- A session outlives a disablement at the provider until `session_lifetime` runs out.

## Deploy

```sh
docker compose up -d
```

Docker pulls the images and starts the services. The database is created on the first start. `./manage status` shows the result.

## Check

```sh
./manage status    # services, pinned version, /ready, free disk
./manage check     # validates librepaper.toml, resticprofile.toml and the Caddyfile; warns if a name does not resolve
```

Caddy gets a certificate the first time a name is visited, so the first request can take a few seconds. Open `https://paper.example` in a browser and sign in. If something fails:

```sh
./manage logs librepaper
```

## Upgrade

```sh
./manage upgrade            # the latest release
./manage upgrade v0.0.25    # a specific release
```

- It verifies the downloaded kit's attestation with cosign, run from a pinned Docker image, before it runs anything from the kit. Releases before this check have no bundle file. For one of those, `gh attestation download librepaper-deploy.tar.gz --repo LibrePaper/librepaper` fetches the bundle. Verify the kit as in [Upgrades](security.html#upgrades), then pass it with `LIBREPAPER_KIT`, or add `--unverified`.
- It backs up first when backups are configured. `--no-backup` skips this.
- It checks your configuration with the new release before switching. If a check fails, nothing switches.
- It keeps your `.toml` files and `caddy/local.d/`. It replaces `compose.yaml` on every upgrade, so put your own changes in `compose.override.yaml`.
- It refuses to move to an older release.
- It leaves the previous release's images on disk, about 250 MB per upgrade. The server migrates the schema at startup.
- `[access] anonymous_comments = true` only lets owners mark comment links "Anyone with the link". Existing links still need sign-in. See [Anonymous access](advanced.html#anonymous-access).

Helpers from v0.0.45 and earlier cannot read the digest-pinned kit. They stop with `the kit pins <digest>, not vX.Y.Z` and change nothing. Replace the helper once, then upgrade as usual. Later helpers hand the upgrade to the kit's own helper by themselves.

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xzO librepaper/manage > manage.new &&
  chmod 755 manage.new && mv -f manage.new manage
```

To free the space old releases take, remove every LibrePaper image except the running one:

```sh
docker images --digests --format '{{.Repository}}@{{.Digest}}' | grep '^ghcr.io/librepaper/' | xargs -r docker rmi
```

`docker rmi` refuses an image that a container still uses, so the running release is never removed. Old releases are not needed afterwards, because `./manage upgrade` refuses to move back to one.
