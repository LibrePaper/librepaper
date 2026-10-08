---
title: "Simple deployment"
---

One VPS, Docker only, one file to edit. PostgreSQL and all user files live on the same machine, in Docker volumes.

> LibrePaper is experimental. Self-host only if you can maintain its database,
> files, credentials, and recovery copies.

## What you need

- A virtual private server (for example OVHcloud, Hetzner, DigitalOcean or Linode).
- Docker Engine and Compose v2 on it.
- Ports 80 and 443 open.
- Two DNS names pointing at it: one for the app (`paper.example`) and one for published documents (`docs.paper.example`). A published document is code, so it needs its own host: the browser is what keeps it out of a reader's session.
- A GitHub OAuth app. Callback URL: `https://paper.example/auth/callback`.

## The kit

LibrePaper supplies a self-hosting kit, `librepaper-deploy.tar.gz`, attached to every GitHub release. The images it pins are that release's own version.

| File | What it is |
|---|---|
| `compose.yaml` | Four services: `postgres` (the database), `librepaper` (the app), `backup` (the restic backup sidecar, idle until configured), `caddy` (the HTTPS proxy; it obtains certificates automatically) |
| `librepaper.toml` | The one file to edit |
| `resticprofile.toml` | Backups, off by default |
| `caddy/Caddyfile` | The proxy configuration |
| `postgres/init.sql` | Creates the database and its role on first start |
| `compose.managed-db.yaml` | For a database elsewhere, see [Database elsewhere](advanced.html#database-elsewhere) |
| `README.md` | The quick start, and a link here |

## Deploy

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
$EDITOR librepaper.toml      # see the list below
docker compose up -d
curl -fsS https://paper.example/ready
```

In `librepaper.toml`, set:

- `[origins]`: `app = "https://paper.example"` and `docs = "https://docs.paper.example"`.
- `[auth.github]`: `client_id` and `client_secret`.

Permissions:

- The directory is private to you (mode 700).
- Files inside stay readable (0644) so the containers can read their bind mounts.
- Never chmod 600 `librepaper.toml`.

## Where your data lives

- PostgreSQL: the `postgres` volume.
- User files (uploads, document snapshots): `objects/` in the `data` volume, mounted at `/var/lib/librepaper`.
- `session.key`: `/var/lib/librepaper/secrets`. It signs sessions and share secrets. Keep it with your backups.

Nothing is backed up until you configure backups. See [Backups](advanced.html#backups).

## Check

```sh
docker compose ps
curl -fsS https://paper.example/ready
docker compose logs librepaper
```

Use the third command if the app does not answer.

## Upgrade

```sh
cd ..   # the directory that contains librepaper/
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz --exclude='*/librepaper.toml' --exclude='*/resticprofile.toml'
cd librepaper && docker compose pull && docker compose up -d
```

The server migrates the schema at startup. When a release note says the kit changed, this same procedure applies. `compose.yaml` is replaced on every upgrade: put your own changes in `compose.override.yaml`.

## Change the configuration

After editing `librepaper.toml` or `resticprofile.toml`, recreate the containers that read them. Compose compares service definitions, not the bytes behind a mounted file:

```sh
docker compose up -d --force-recreate librepaper backup
```

## Next

- [DNS](advanced.html#dns): the optional admin name, extra names.
- [Sign-in and access](advanced.html#sign-in-and-access): Google, who may publish and comment.
- [Backups](advanced.html#backups) and the [restore drill](advanced.html#restore-drill).
- [Object storage](advanced.html#object-storage): keep user files in an S3 bucket.
- [Health](advanced.html#health): graphs, uptime and backup alerts.
- [Secrets](advanced.html#secrets), [Database elsewhere](advanced.html#database-elsewhere), [Limits and retention](advanced.html#limits-and-retention), [Without Docker](advanced.html#without-docker).
