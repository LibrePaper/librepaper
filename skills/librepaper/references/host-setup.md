# Setting up a server

This file is for a new server, or for changing an existing server's `librepaper.toml`. The procedure is `docs/host/simple.md`. Hardening (SSH, firewall, Tailscale) is `docs/host/security.md`. Optional features (backups, object storage, the admin console, alerts, a database elsewhere, sharing a server with other apps) are `docs/host/advanced.md`. Follow those pages and use this file for what an agent tends to get wrong.

The operator runs commands on the server and holds every client secret. Help them fill in `librepaper.toml`, but never ask them to paste a secret into the conversation. Leave `replace` placeholders for them to fill in.

## What the server needs

- A VPS with Docker Engine and Compose v2, and ports 80 and 443 open.
- Two DNS names pointing at it: the app (`paper.example`) and published documents (`docs.paper.example`). A third (`admin.paper.example`) is optional, for the admin console.
- At least one sign-in provider registered: GitHub, Google, Microsoft Entra ID, ORCID, or any OpenID Connect provider.

```sh
# On the VPS: download the latest kit into ./librepaper (mode 700)
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
$EDITOR librepaper.toml
./manage check
docker compose up -d
./manage status
```

## Traps

- **`docs` must be a different host from `app`.** A published document runs its own scripts, and only a separate host keeps them away from a reader's sign-in. The server refuses a configuration where they share a host.
- **Leave the files readable (0644).** The containers read them through bind mounts, and the 700 directory keeps them private. `chmod 600 librepaper.toml` breaks the containers.
- **A provider table without `publishers` lets nobody from that provider publish.** Sign-in then works but nobody can create a project. `commenters` defaults to `["*"]`.
- **`publishers` entries depend on the provider.** GitHub takes logins. Google, Entra and OpenID Connect take verified addresses or `"@domain"`. ORCID takes iDs. `"*"` admits every account of that provider. An address entry trusts the provider's administrators with that address, so prefer `@domain` entries for domains the provider owns.
- **Callback URLs must match exactly:**

| Provider | Callback URL |
|---|---|
| GitHub | `https://paper.example/auth/callback` |
| Google | `https://paper.example/auth/callback/google` |
| Microsoft Entra ID | `https://paper.example/auth/callback/entra` |
| ORCID | `https://paper.example/auth/callback/orcid` |
| OpenID Connect | `https://paper.example/auth/callback/oidc-<name>` |
| Admin console | `https://admin.paper.example/auth/callback`, added to the GitHub OAuth app |

- **OpenID Connect providers must send `email_verified`.** AD FS does not and is unsupported. Each `[auth.oidc.<name>]` needs a `label`. The name is lowercase letters, digits and hyphens, up to 8 providers.
- **Never change an existing provider's issuer to a different identity provider.** Every account is keyed to its subject at that issuer. A different provider under the same name would hand old accounts to strangers. Give it a new name. `docs/host/advanced.md` (Identity providers) covers moving the same provider to a new address.
- **Local accounts do not exist.** For people without an outside account, run a self-hosted OpenID Connect provider (Authentik, Authelia, Keycloak, Pocket ID, Kanidm, Zitadel) next to LibrePaper.
- **One person through two providers has two accounts.** A session outlives a disablement at the provider until `session_lifetime` runs out (default 30 days, `1h` to `30d`).
- **The admin console needs GitHub sign-in.** A server with only Google, Entra, ORCID or OpenID Connect has no console.
- **Losing the VPS loses everything until backups are configured.** Point the operator at `docs/host/advanced.md` (Backups) once the server runs.

## Changing the configuration

```sh
cp librepaper.toml librepaper.toml.candidate
# edit librepaper.toml.candidate, then:
./manage apply   # validates, installs, recreates the app and backup containers, reloads Caddy, waits for /ready
```

- If a check fails, nothing changes.
- Without the journald override in `docs/host/advanced.md` (Logs), `./manage apply` deletes the app and backup containers' logs with the containers. Save any logs needed first.
- Every limit and retention setting, with its default, is listed commented out at the end of `librepaper.toml`. Uncomment a line to change it.

## Limits to look at when users complain

| Symptom | Setting in `librepaper.toml` | Default |
|---|---|---|
| One account's uploads refused, storage full | `[limits] publisher_storage_mib` | 50 |
| Uploads refused for everyone | `[limits] deployment_storage_mib` | 5120 |
| Too many projects or figures in an hour | `[limits] publisher_uploads_per_hour` | 30 |
| Busy refusals, documents evicted, connections capped | `[limits] memory_budget_mib` (also sets 8 connections per MiB) | 512 |
| A large push or a long edit history refused | `[limits] log_quota_mib` | 32 |
| Documents or accounts disappearing | `[retention] expire_after`, `erase_inactive_after` | unset |

Check `/api/status` and the admin console before raising a limit ([host-operate.md](host-operate.md)). Raising `memory_budget_mib` beyond the VPS's real memory trades refusals for swapping.
