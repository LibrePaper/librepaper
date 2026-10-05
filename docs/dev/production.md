# Production

The official instance at librepaper.org runs `tools/deploy/docker` on an OVHcloud VPS, with DNS at Hover.

- Site (landing page and manual): `https://librepaper.org`, built by `make site` and served by Caddy as files
- App: `https://app.librepaper.org`; documents: `https://docs.librepaper.org`
- Redirected to the site: `www.librepaper.org`, `librepaper.com`, `www.librepaper.com`
- Host: OVHcloud VPS, Ubuntu 24.04, BHS, user `ubuntu`, kit in `~/librepaper`
- Shell: in zsh, run `setopt interactivecomments` first, or the `#` lines in these blocks fail as commands
- Secrets: `tools/deploy/keys.yaml` (SOPS)
  - `PRODUCTION_POSTGRES_PASSWORD`, `PRODUCTION_POSTGRES_EXPORTER_PASSWORD`, `PRODUCTION_ACME_EMAIL`
  - `PRODUCTION_GITHUB_CLIENT_ID`, `PRODUCTION_GITHUB_CLIENT_SECRET`
  - `PRODUCTION_GOOGLE_CLIENT_ID`, `PRODUCTION_GOOGLE_CLIENT_SECRET`, `PRODUCTION_ADMIN_PASSWORD`

## Release

The Rust TLS clients share the AWS-LC provider. Direct Reqwest uses 0.13, matching the Reqwest generation used by object_store 0.14; Reqwest and object_store's AWS feature select AWS-LC. SQLx explicitly selects `runtime-tokio` with `tls-rustls-aws-lc-rs` rather than the `runtime-tokio-rustls` ring alias. `tokio-tungstenite` uses rustls without choosing a provider, so `tls.rs` installs AWS-LC before its WebSocket connection only when the process has no provider already, preserving an embedding caller's choice. The all-target dependency tree has no active ring path, and workspace all-target clippy passed for this feature selection.

```sh
# Set this to the release tag matching the package version in Cargo.toml.
VERSION=vX.Y.Z
git tag "$VERSION" && git push origin "$VERSION"
gh run watch                                     # the Release workflow
gh release view "$VERSION" --json assets -q '.assets[].name' | grep linux-musl
```

- v0.0.8 does not include the metrics listener. Do not deploy it to the monitoring stack; v0.0.9 is the first release with the listener.
- v0.0.1 to v0.0.3 are Komodoc archives, and v0.0.4 to v0.0.7 never released
- Keep `/api/auth/config` and the hidden `local start`, `local stop`, `local status`, and `local agent` aliases until a removal cutoff is announced for supported external clients; no cutoff is scheduled. Preserve `local open` while installed `librepaper://` links use it.

## VPS (OVHcloud)

- Order: VPS, Ubuntu 24.04, region BHS; paste your SSH public key at checkout (otherwise the `ubuntu` password arrives by email)
- IP: the main IPv4 is static (kept across reboots and reinstalls, lost only if the VPS is deleted), so no Additional IP is needed; that product only moves an address between servers
- Find it: Control Panel, Bare Metal Cloud, Virtual private servers, the VPS, Home tab, IP section (also in the delivery email)
- Firewall: nothing to open by default; if you enable `ufw` or OVHcloud's Edge Network Firewall, allow 22, 80 and 443 (Caddy needs 80 and 443 for Let's Encrypt)

```sh
# key not pasted at checkout: install it with the password from the delivery email
ssh-copy-id ubuntu@VPS_IP

# on the VPS, once
sudo apt update && sudo apt upgrade -y
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker ubuntu   # log out and back in
```

## Harden (once)

After `ssh-copy-id` works; keep the current session open until the key login is confirmed.

```sh
# on the VPS: keys only (00- sorts before cloud-init's 50- file, and sshd keeps the first value)
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\n' | sudo tee /etc/ssh/sshd_config.d/00-keys-only.conf
sudo sshd -t && sudo systemctl restart ssh
# from your machine: must still log in with the key
ssh ubuntu@VPS_IP true
# on the VPS: security updates, should be active (running)
systemctl status unattended-upgrades --no-pager
```

## DNS (Hover)

Once the VPS has its IP, for each of librepaper.org and librepaper.com.

- Nameservers must stay `ns1.hover.com` and `ns2.hover.com`, otherwise the DNS tab has no effect
- Leave Hover's domain forwarding off: it cannot serve HTTPS, so Caddy does the redirects
- Sign in at hover.com, click the domain, open the DNS tab
- Delete the parking records `A @` and `A *` (value `216.40.34.41`): tick them, Bulk edit, Delete; the wildcard swallows `docs`. Keep `MX` and `mail` if Hover email is in use
- Add a record for each row below: Hover appends the domain to the hostname, and `@` is the bare domain; leave TTL at the default

librepaper.org:

| Type | Hostname | IP address | Name |
|---|---|---|---|
| A | `@` | `VPS_IP` | `librepaper.org` |
| A | `app` | `VPS_IP` | `app.librepaper.org` |
| A | `docs` | `VPS_IP` | `docs.librepaper.org` |
| A | `www` | `VPS_IP` | `www.librepaper.org` |

librepaper.com:

| Type | Hostname | IP address | Name |
|---|---|---|---|
| A | `@` | `VPS_IP` | `librepaper.com` |
| A | `www` | `VPS_IP` | `www.librepaper.com` |

- No `AAAA` until `curl -6 https://example.com` works on the VPS (Let's Encrypt prefers IPv6)
- New records usually resolve within minutes; Hover quotes up to 48 hours

```sh
# nameservers: ns1.hover.com and ns2.hover.com for both domains
dig +short NS librepaper.org; dig +short NS librepaper.com
# every name must answer with the VPS address before the first start
for h in librepaper.org app.librepaper.org docs.librepaper.org www.librepaper.org librepaper.com www.librepaper.com; do
  echo "$h $(dig +short "$h")"
done
```

## GitHub OAuth app

LibrePaper org, Settings, Developer settings, OAuth Apps.

- Homepage URL: `https://librepaper.org`
- Callback URL: `https://app.librepaper.org/auth/callback`, the only one
- Wildcard matching: off (only that exact URL should ever receive a sign-in code)
- Device flow: off (`librepaper login` uses the server's own device flow, not GitHub's)
- Client ID and secret go in SOPS

## Google OAuth client

In Google Auth Platform, open **Clients**, create a **Web application** client, and add this exact Authorized redirect URI:

`https://app.librepaper.org/auth/callback/google`

Store the client ID and secret in SOPS as `PRODUCTION_GOOGLE_CLIENT_ID` and `PRODUCTION_GOOGLE_CLIENT_SECRET`. Both `deploy` and `deploy-local` map these to `LIBREPAPER_GOOGLE_CLIENT_ID` and `LIBREPAPER_GOOGLE_CLIENT_SECRET`; `verify` checks the authorization redirects for both Google and GitHub. See Google's [OAuth 2.0 guide for web server applications](https://developers.google.com/identity/protocols/oauth2/web-server).

The official TOML file is [tools/deploy/production.toml](../../tools/deploy/production.toml).
It records the three public origins, `/var/lib/librepaper`, PostgreSQL,
filesystem objects, the current 50 MiB/30-per-hour admission limits, the
internal metrics listener, and the trusted Docker edge subnet. Credentials are
referenced by name from `.env`; the TOML file contains no secret values. The
deployment stages this file and runs the candidate image's `admin config
check` before replacing `config.toml` or restarting the application.

## Secrets

```sh
# Generate each database, exporter, and Grafana password this way. Production
# passwords must use letters, digits, underscores, or hyphens only for .env safety.
openssl rand -hex 24          # without openssl: nix shell nixpkgs#openssl -c openssl rand -hex 24
sops tools/deploy/keys.yaml   # add the eight PRODUCTION_* keys
git add tools/deploy/keys.yaml && git commit -m "Add production secrets"   # values stay encrypted
```

- The Postgres password is fixed at first start; changing it later needs an `ALTER ROLE`
- The exporter password is installed into the least-privilege `librepaper_metrics` role during deploy; the command is idempotent for existing database volumes
- Grafana uses `PRODUCTION_ADMIN_PASSWORD` for its initial admin account. Grafana reads that setting only when its data volume is empty; later password changes must be made in Grafana and then saved back to SOPS

## Deploy and upgrade

Deploy the tagged release with the normal release flow. The local binary flow remains available for unreleased source builds or forks.

```sh
# deploy the tagged release
tools/deploy/production deploy "$VERSION"
HOST=ubuntu@VPS_IP tools/deploy/production deploy "$VERSION"   # before DNS resolves
tools/deploy/production site                                 # landing page and manual only: no release, no restart
# optional: deploy a previously built static Linux musl executable
tools/deploy/production deploy-local target/x86_64-unknown-linux-musl/release/librepaper
```

- `deploy VERSION` accepts only canonical `vMAJOR.MINOR.PATCH` tags at `v0.0.9` or later. It refuses `v0.0.8` before any remote write because that binary has no metrics endpoint. The default version comes from Cargo.toml and is subject to the same guard.
- The candidate binary must include TOML configuration support. Older releases fail the remote `admin config check` preflight; the active config is left in place and the app is not restarted. Until a TOML-capable release is published, use `deploy-local` with a built binary that includes this CLI.
- After validation, deploy replaces the active TOML file, recreates only the LibrePaper container so it opens the new file, then brings up the rest of the stack without force-recreating the database or other services.
- `make site` needs bun; the site is served from `~/librepaper/site` through `compose.override.yaml`
- `deploy-local BINARY` accepts a previously built Linux musl executable. It uploads the file to a temporary candidate, compares SHA-256 checksums, builds the kit's Dockerfile with `SOURCE=local`, and checks TOML support before migration work or application restart. The checked candidate remains in the kit as the Docker build input; a normal release deployment resets this local-build override. Supported targets are x86_64 and aarch64.
- Domain and redirects are constants at the top of `tools/deploy/production`. The official config allows any signed-in GitHub or Google account to publish and comment. It sets a 50 MiB per-account storage quota and a limit of 30 project creations, forks or figure uploads per rolling hour; these limits apply to release and local-binary deployments.
- The VPS keeps the kit and `.env` (mode 600) in `~/librepaper`
- The VPS `.env` sets `COMPOSE_FILE` to list `compose.yaml`, `compose.monitoring.yaml` and `compose.override.yaml`

## Squash migrations

One deployment exists, so the files under `crates/librepaper-engine/migrations/postgres/` can be hand-merged into a single `0001_catalog.sql` whenever the history gets noisy. The server refuses to start when an applied version in `_sqlx_migrations` has no file or a different checksum, so production's table is re-pinned in the same deploy that ships the squashed file.

```sh
# 1. deploy the release that holds every migration you are about to merge
# 2. merge the files into 0001_catalog.sql, delete the others, commit, tag, release
# 3. deploy: the preflight sees one local file against several applied rows,
#    proves the file rebuilds production's schema (pg_dump of both, on the VPS),
#    pins the table to version 1 with the file's SHA-384, logs the old versions
#    to ~/librepaper/migrations-squash.log, takes a backup, then starts containers
tools/deploy/production deploy "$VERSION"
# the same step on its own; only right before a deploy, since a restart in between fails
tools/deploy/production squash-migrations
```

- The preflight runs on every deploy: matching rows pass, a fresh database passes, any other mismatch stops before containers are touched
- A non-empty schema diff stops the deploy with nothing changed; fix the merged file and release again
- Backups record the migration version and restore refuses a mismatch, so backups from before the squash no longer restore; the step takes a fresh one
- Once a second deployment exists, stop squashing and go back to additive migrations

## Monitoring

Grafana is at `https://app.librepaper.org/admin/monitoring/`, with username `admin` and the secret in SOPS. To copy the password to a Wayland clipboard without putting it in terminal output, shell history, or process arguments:

```sh
sops --decrypt --extract '["PRODUCTION_ADMIN_PASSWORD"]' tools/deploy/keys.yaml | wl-copy
```

Paste it into Grafana's login form and clear the clipboard afterward. `wl-copy` receives the secret through a pipe; if unavailable, use an equivalent clipboard tool for your desktop. Access rules are in [Self-hosting](../host.md#docker-compose).

The deploy command bootstraps or updates the `librepaper_metrics` PostgreSQL role before starting the monitoring containers. It can repair an existing database volume as well as prepare a new one. The role receives `pg_monitor` only, and its password is kept in `.env` with mode 600.

To rotate the Grafana password, follow [Self-hosting](../host.md#docker-compose), with `PRODUCTION_ADMIN_PASSWORD` in SOPS as the `.env` value, then deploy.

Compose waits for service health during deployment. The deploy then recreates only Prometheus so it reads the newly copied scrape and alert configuration, and the verifier retries temporary Prometheus and Grafana startup failures for up to a minute. It confirms Grafana credentials work and anonymous API access is denied, ensures `/metrics`, `/api/status`, and Prometheus APIs are not public, checks that exactly the LibrePaper, Node Exporter, and PostgreSQL exporter scrape targets are up, and waits for recent LibrePaper samples, a fresh successful snapshot, and `pg_up == 1`. It also validates that the provisioned dashboard has panels.

The deployment script tests use mock SSH and HTTP commands. The persistent Grafana 503 case takes about one minute and is opt-in; run only that case with:

```sh
LIBREPAPER_TEST_SLOW_DEPLOY=1 node --test --test-name-pattern='persistent Grafana API failures' tools/deploy/production.test.mjs
```

## Verify

```sh
tools/deploy/production verify    # app and OAuth redirects, monitoring, the site, documents, redirects, logs
tools/deploy/production logs      # follow
LIBREPAPER_SERVER=https://app.librepaper.org librepaper login
```

## Backups

```sh
# on the VPS
cd ~/librepaper && docker compose exec -T librepaper librepaper admin backup \
  --config /etc/librepaper/config.toml /var/backups/librepaper/$(date +%F)
```

- Not scheduled yet, and no off-machine copy yet
- Restore: `docs/host.md`, Storage
