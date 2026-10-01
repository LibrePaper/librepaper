# Production

The official instance at librepaper.org runs `tools/deploy-docker` on an OVHcloud VPS, with DNS at Hover.

- Site (landing page and manual): `https://librepaper.org`, built by `make site` and served by Caddy as files
- App: `https://app.librepaper.org`; documents: `https://docs.librepaper.org`
- Redirected to the site: `www.librepaper.org`, `librepaper.com`, `www.librepaper.com`
- Host: OVHcloud VPS, Ubuntu 24.04, BHS, user `ubuntu`, kit in `~/librepaper`
- Shell: in zsh, run `setopt interactivecomments` first, or the `#` lines in these blocks fail as commands
- Secrets: `tools/deploy-keys.yaml` (SOPS)
  - `PRODUCTION_POSTGRES_PASSWORD`, `PRODUCTION_ACME_EMAIL`
  - `PRODUCTION_GITHUB_CLIENT_ID`, `PRODUCTION_GITHUB_CLIENT_SECRET`
  - `PRODUCTION_GOOGLE_CLIENT_ID`, `PRODUCTION_GOOGLE_CLIENT_SECRET`

## Release

```sh
VERSION=v0.0.8                                   # must match crates/librepaper/Cargo.toml
git tag "$VERSION" && git push origin "$VERSION"
gh run watch                                     # the Release workflow
gh release view "$VERSION" --json assets -q '.assets[].name' | grep linux-musl
```

- The image builds from v0.0.8 on: v0.0.1 to v0.0.3 are Komodoc archives, and v0.0.4 to v0.0.7 never released
- No Windows build since v0.0.8: the PowerShell installer in the README, docs/start.md and the settings page 404s until `x86_64-pc-windows-msvc` is back in the dist targets

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

Create a Google OAuth client for a web application. Its authorized redirect URI must be exactly `https://app.librepaper.org/auth/callback/google`; Google requires the redirect URI used by the app to match an authorized URI. This callback is separate from GitHub's `https://app.librepaper.org/auth/callback`. See Google's [web server OAuth guide](https://developers.google.com/identity/protocols/oauth2/web-server).

- Store the client ID and client secret as `PRODUCTION_GOOGLE_CLIENT_ID` and `PRODUCTION_GOOGLE_CLIENT_SECRET` in `tools/deploy-keys.yaml` (SOPS), alongside the four existing production keys
- `tools/deploy-production deploy` requires all six keys and writes the Google values as `LIBREPAPER_GOOGLE_CLIENT_ID` and `LIBREPAPER_GOOGLE_CLIENT_SECRET` into the VPS `.env`
- `tools/deploy-production verify` checks that both `/auth/login/github` and `/auth/login/google` return an HTTP 302 to the matching provider's authorization endpoint

## Secrets

```sh
# PRODUCTION_POSTGRES_PASSWORD: hex, since it sits inside a postgresql:// URL
openssl rand -hex 24          # without openssl: nix shell nixpkgs#openssl -c openssl rand -hex 24
sops tools/deploy-keys.yaml   # add the six PRODUCTION_* keys, including both Google client values
git add tools/deploy-keys.yaml && git commit -m "Add production secrets"   # values stay encrypted
```

- The Postgres password is fixed at first start; changing it later needs an `ALTER ROLE`

## Deploy and upgrade

From the repository root; rerun to upgrade after a release.

```sh
# builds the site, checks the release, copies the kit, writes .env from SOPS, builds, starts, then verifies
tools/deploy-production deploy                               # the Cargo.toml version, or: deploy v0.0.8
HOST=ubuntu@VPS_IP tools/deploy-production deploy            # before DNS resolves
tools/deploy-production site                                 # landing page and manual only: no release, no restart
```

- `make site` needs bun; the site is served from `~/librepaper/site` through `compose.override.yaml`
- Domain, redirects, publishers and commenters are constants at the top of `tools/deploy-production`
- The VPS keeps the kit and `.env` (mode 600) in `~/librepaper`

## Verify

```sh
tools/deploy-production verify    # app health, the site, the documents host, the librepaper.com redirect, the last 50 log lines
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
