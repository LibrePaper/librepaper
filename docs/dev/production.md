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
VERSION=v0.0.6                                   # must match crates/librepaper/Cargo.toml
git tag "$VERSION" && git push origin "$VERSION"
gh run watch                                     # the Release workflow
gh release view "$VERSION" --json assets -q '.assets[].name' | grep linux-musl
```

- The image builds from v0.0.6 on: v0.0.1 to v0.0.3 are Komodoc archives, and v0.0.4 and v0.0.5 never released

## DNS (Hover)

Each domain, DNS tab, every record pointing at the VPS IPv4.

- Delete Hover's parking records `A @` and `A *` (the wildcard swallows `docs`); keep `MX` and `mail` if Hover email is in use
- librepaper.org: `A @`, `A docs`, `A www`
- librepaper.com: `A @`, `A www`
- No `AAAA` until `curl -6 https://example.com` works on the VPS (Let's Encrypt prefers IPv6)

```sh
# every name must answer with the VPS address before the first start
for h in librepaper.org docs.librepaper.org www.librepaper.org librepaper.com www.librepaper.com; do
  echo "$h $(dig +short "$h")"
done
```

## GitHub OAuth app

LibrePaper org, Settings, Developer settings, OAuth Apps.

- Homepage URL: `https://librepaper.org`
- Callback URL: `https://librepaper.org/auth/callback`
- Client ID and secret go in SOPS

## Secrets

```sh
openssl rand -hex 24          # PRODUCTION_POSTGRES_PASSWORD: hex, since it sits inside a postgresql:// URL
sops tools/deploy-keys.yaml   # add the four PRODUCTION_* keys
```

- The Postgres password is fixed at first start; changing it later needs an `ALTER ROLE`

## Server (once)

```sh
# on the VPS
sudo apt update && sudo apt upgrade -y
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker ubuntu   # log out and back in
```

## Deploy and upgrade

From the repository root; rerun with a new `VERSION` to upgrade.

```sh
VERSION=v0.0.6
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
