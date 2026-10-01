# Production

The official instance at librepaper.org runs `tools/deploy-docker` on an OVHcloud VPS, with DNS at Hover.

- Site (landing page and manual): `https://librepaper.org`, built by `make site` and served by Caddy as files
- App: `https://app.librepaper.org`; documents: `https://docs.librepaper.org`
- Redirected to the site: `www.librepaper.org`, `librepaper.com`, `www.librepaper.com`
- Host: OVHcloud VPS, Ubuntu 24.04, BHS, user `ubuntu`, kit in `~/librepaper`
- Shell: in zsh, run `setopt interactivecomments` first, or the `#` lines in these blocks fail as commands
- Secrets: `tools/deploy-keys.yaml` (SOPS)
  - `PRODUCTION_POSTGRES_PASSWORD`, `PRODUCTION_POSTGRES_EXPORTER_PASSWORD`, `PRODUCTION_ACME_EMAIL`
  - `PRODUCTION_GITHUB_CLIENT_ID`, `PRODUCTION_GITHUB_CLIENT_SECRET`
  - `PRODUCTION_GOOGLE_CLIENT_ID`, `PRODUCTION_GOOGLE_CLIENT_SECRET`, `PRODUCTION_ADMIN_PASSWORD`

## Release

```sh
# Release the instrumented v0.0.12 build.
VERSION=v0.0.12                                  # must match crates/librepaper/Cargo.toml
git tag "$VERSION" && git push origin "$VERSION"
gh run watch                                     # the Release workflow
gh release view "$VERSION" --json assets -q '.assets[].name' | grep linux-musl
```

- v0.0.8 does not include the metrics listener. Do not deploy it to the monitoring stack; v0.0.9 is the first release with the listener.
- v0.0.1 to v0.0.3 are Komodoc archives, and v0.0.4 to v0.0.7 never released
- Windows binaries and installers are not published; current release targets cover Linux and macOS only.

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

## Secrets

```sh
# Generate each database, exporter, and Grafana password this way. Production
# passwords must use letters, digits, underscores, or hyphens only for .env safety.
openssl rand -hex 24          # without openssl: nix shell nixpkgs#openssl -c openssl rand -hex 24
sops tools/deploy-keys.yaml   # add the eight PRODUCTION_* keys
git add tools/deploy-keys.yaml && git commit -m "Add production secrets"   # values stay encrypted
```

- The Postgres password is fixed at first start; changing it later needs an `ALTER ROLE`
- The exporter password is installed into the least-privilege `librepaper_metrics` role during deploy; the command is idempotent for existing database volumes
- Grafana uses `PRODUCTION_ADMIN_PASSWORD` for its initial admin account. Grafana reads that setting only when its data volume is empty; later password changes must be made in Grafana and then saved back to SOPS

## Deploy and upgrade

Deploy the tagged release with the normal release flow. The local binary flow remains available for unreleased source builds or forks.

```sh
# deploy the tagged release
tools/deploy-production deploy v0.0.12
HOST=ubuntu@VPS_IP tools/deploy-production deploy v0.0.12   # before DNS resolves
tools/deploy-production site                                 # landing page and manual only: no release, no restart
# optional: deploy a previously built static Linux musl executable
tools/deploy-production deploy-local target/x86_64-unknown-linux-musl/release/librepaper
```

- `deploy VERSION` accepts only canonical `vMAJOR.MINOR.PATCH` tags at `v0.0.9` or later. It refuses `v0.0.8` before any remote write because that binary has no metrics endpoint. The default version comes from Cargo.toml and is subject to the same guard.
- `make site` needs bun; the site is served from `~/librepaper/site` through `compose.override.yaml`
- `deploy-local BINARY` accepts a previously built Linux musl executable. It uploads the file to a temporary name, compares SHA-256 checksums, then atomically replaces the kit binary and builds `Dockerfile.local`. Supported targets are x86_64 and aarch64. A normal release deployment resets this local-build override.
- Domain, redirects, publishers and commenters are constants at the top of `tools/deploy-production`. The official instance accepts any signed-in GitHub or Google account as a publisher. Its `.env` sets a 50 MiB per-account storage quota and a limit of 30 project creations, forks or figure uploads per rolling hour; these limits apply to release and local-binary deployments.
- The VPS keeps the kit and `.env` (mode 600) in `~/librepaper`

## Monitoring

Grafana is at `https://app.librepaper.org/admin/monitoring/`, with username `admin` and the secret in SOPS. To copy the password to a Wayland clipboard without putting it in terminal output, shell history, or process arguments:

```sh
sops --decrypt --extract '["PRODUCTION_ADMIN_PASSWORD"]' tools/deploy-keys.yaml | wl-copy
```

Paste it into Grafana's login form and clear the clipboard afterward. `wl-copy` receives the secret through a pipe; if unavailable, use an equivalent clipboard tool for your desktop. The dashboard is protected by Grafana's built-in login and is routed only on the app hostname. Prometheus, PostgreSQL exporter, and Node Exporter ports stay on the Docker internal network.

The deploy command bootstraps or updates the `librepaper_metrics` PostgreSQL role before starting the monitoring containers. It can repair an existing database volume as well as prepare a new one. The role receives `pg_monitor` only, and its password is kept in `.env` with mode 600.

To rotate the Grafana password, change it from the Grafana account page, update `PRODUCTION_ADMIN_PASSWORD` in SOPS to the same new value, then deploy. Changing only the SOPS value does not alter an existing Grafana account because `GF_SECURITY_ADMIN_PASSWORD` is an initialization setting.

Compose waits for service health during deployment. The deploy then recreates only Prometheus so it reads the newly copied scrape and alert configuration, and the verifier retries temporary Prometheus and Grafana startup failures for up to a minute. It confirms Grafana credentials work and anonymous API access is denied, ensures `/metrics`, `/api/status`, and Prometheus APIs are not public, checks that exactly the LibrePaper, Node Exporter, and PostgreSQL exporter scrape targets are up, and waits for recent LibrePaper samples, a fresh successful snapshot, and `pg_up == 1`. It also validates that the provisioned dashboard has panels.

The deployment script tests use mock SSH and HTTP commands. The persistent Grafana 503 case takes about one minute and is opt-in; run only that case with:

```sh
LIBREPAPER_TEST_SLOW_DEPLOY=1 node --test --test-name-pattern='persistent Grafana API failures' tools/tests/deploy-production.test.mjs
```

## Verify

```sh
tools/deploy-production verify    # app and OAuth redirects, monitoring, the site, documents, redirects, logs
tools/deploy-production logs      # follow
LIBREPAPER_SERVER=https://app.librepaper.org librepaper login
```

## Backups

```sh
# on the VPS
cd ~/librepaper && docker compose exec -T librepaper librepaper admin backup \
  --data-directory /var/lib/librepaper /var/backups/librepaper/$(date +%F)
```

- Not scheduled yet, and no off-machine copy yet
- Restore: `docs/host.md`, Storage
