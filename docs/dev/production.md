# Production

The official instance at librepaper.org runs `tools/deploy-docker` on an OVHcloud VPS, with DNS at Hover.

- Reader: `https://librepaper.org`; documents: `https://docs.librepaper.org`
- Redirected to the reader: `www.librepaper.org`, `librepaper.com`, `www.librepaper.com` (the server answers 421 to any other host)
- Host: OVHcloud VPS, Ubuntu 24.04, BHS, user `ubuntu`, kit in `~/librepaper`
- Secrets: `tools/deploy-keys.yaml` (SOPS)
  - `PRODUCTION_POSTGRES_PASSWORD`, `PRODUCTION_ACME_EMAIL`
  - `PRODUCTION_GITHUB_CLIENT_ID`, `PRODUCTION_GITHUB_CLIENT_SECRET`

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
# on the VPS, once
sudo apt update && sudo apt upgrade -y
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker ubuntu   # log out and back in
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
for h in librepaper.org docs.librepaper.org www.librepaper.org librepaper.com www.librepaper.com; do
  echo "$h $(dig +short "$h")"
done
```

## GitHub OAuth app

LibrePaper org, Settings, Developer settings, OAuth Apps.

- Homepage URL: `https://librepaper.org`
- Callback URL: `https://librepaper.org/auth/callback`, the only one
- Wildcard matching: off (it would also accept `docs.librepaper.org`, which serves published documents, so a document could catch sign-in codes)
- Device flow: off (`librepaper login` uses the server's own device flow, not GitHub's)
- Client ID and secret go in SOPS

## Secrets

```sh
openssl rand -hex 24          # PRODUCTION_POSTGRES_PASSWORD: hex, since it sits inside a postgresql:// URL
sops tools/deploy-keys.yaml   # add the four PRODUCTION_* keys
```

- The Postgres password is fixed at first start; changing it later needs an `ALTER ROLE`

## Deploy and upgrade

From the repository root; rerun with a new `VERSION` to upgrade.

```sh
VERSION=v0.0.8
HOST=ubuntu@VPS_IP
key() { sops --decrypt --extract "[\"$1\"]" tools/deploy-keys.yaml; }

# the kit, then the production redirects (rsync resets the Caddyfile, so append every time)
rsync -a tools/deploy-docker/ "$HOST:librepaper/"
ssh "$HOST" 'cat >> librepaper/Caddyfile' <<'EOF'

www.{$DOMAIN}, librepaper.com, www.librepaper.com {
	redir https://{$DOMAIN}{uri} permanent
}
EOF

# .env from SOPS, written on the VPS only
ssh "$HOST" 'umask 077; cat > librepaper/.env' <<EOF
LIBREPAPER_VERSION=$VERSION
DOMAIN=librepaper.org
ACME_EMAIL=$(key PRODUCTION_ACME_EMAIL)
POSTGRES_PASSWORD=$(key PRODUCTION_POSTGRES_PASSWORD)
LIBREPAPER_PUBLISHERS=vincentarelbundock
LIBREPAPER_COMMENTERS=anyone
LIBREPAPER_GITHUB_CLIENT_ID=$(key PRODUCTION_GITHUB_CLIENT_ID)
LIBREPAPER_GITHUB_CLIENT_SECRET=$(key PRODUCTION_GITHUB_CLIENT_SECRET)
EOF

ssh "$HOST" 'cd librepaper && docker compose up -d --build'
```

## Verify

```sh
curl -s https://librepaper.org/health
curl -sI https://librepaper.com | head -3              # 301 to https://librepaper.org/
ssh "$HOST" 'cd librepaper && docker compose logs --tail 50'
LIBREPAPER_SERVER=https://librepaper.org librepaper login
```

## Backups

```sh
# on the VPS
cd ~/librepaper && docker compose exec -T librepaper librepaper admin backup \
  --data-directory /var/lib/librepaper /var/backups/librepaper/$(date +%F)
```

- Not scheduled yet, and no off-machine copy yet
- Restore: `docs/host.md`, Storage
