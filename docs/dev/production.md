# Production and release flow

Production is a Docker Compose deployment on the OVHcloud VPS. Releasing a
version publishes downloadable artifacts; it does **not** deploy the VPS. The
VPS deploy and the landing-page-only update are operator-run commands.

- Public site: `https://librepaper.org`; app: `https://app.librepaper.org`;
  documents: `https://docs.librepaper.org`.
- `www.librepaper.org`, `librepaper.com`, and `www.librepaper.com` redirect to
  the public site. Caddy needs the app, documents, and site DNS names pointed
  at the VPS before the first deployment.

## Release a version

1. Set `[workspace.package].version` in `Cargo.toml` and update the workspace
   package entries in `Cargo.lock`. The release tag must be
   `v<version>` (for example, version `X.Y.Z` uses tag `vX.Y.Z`).
2. Run the checks described in [Building and testing](../architecture/building.md),
   including `make check` and `tools/dev/db test`. If the release changes
   browser assets, publish them first; see [asset mirrors](asset-mirrors.md).
   Commit and push the release commit, then confirm CI is green for it. The
   Release workflow builds on tag pushes but does not wait for CI.
3. From a clean checkout of that release commit, push the matching tag:

   ```sh
   VERSION="v$(sed -n 's/^version = \"\(.*\)\"/\1/p' Cargo.toml | head -n 1)"
   git tag "$VERSION"
   git push origin "$VERSION"
   gh run list --workflow release.yml --limit 5
   gh run watch RUN_ID --exit-status
   ```

   Select the Release run whose tag is `$VERSION`.

4. The Release workflow uses cargo-dist to build platform archives, installers,
   and SHA-256 files, then creates the GitHub Release. Confirm the Linux
   x86_64 asset exists before deploying:

   ```sh
   gh release view "$VERSION" --json assets -q '.assets[].name'
   ```

5. After a successful stable release, the Homebrew and Scoop workflows update
   their repositories automatically. They verify each downloaded archive
   against its `.sha256` file before publishing. They can also be run manually
   for a stable tag. See [package release details](../releasing.md).

## Deploy the VPS

- Run from the repository root in a clean checkout of the selected tag.
  Migration files, production config, and site content come from the local tree.
- Needs SSH access, Docker on the VPS, local Bun/Node.js for `make site`,
  `sops` access to `tools/deploy/keys.yaml`, and working production DNS.
- The script writes secrets to a mode-600 `.env` on the VPS.

```sh
# Deploy a published release. VERSION defaults to the Cargo.toml version.
tools/deploy/production deploy "$VERSION"

# Override the SSH destination; production DNS validation still runs.
HOST=ubuntu@VPS_IP tools/deploy/production deploy "$VERSION"

# Check the live app and monitoring, site, document host, redirect, and logs.
tools/deploy/production verify
tools/deploy/production logs
```

### One-time host setup

- Host: OVHcloud VPS, Ubuntu 24.04, SSH user `ubuntu`; deployment kit and
  operator `.env` live in `~/librepaper`.
- Point these six A records to the VPS IPv4: `librepaper.org`,
  `app.librepaper.org`, `docs.librepaper.org`, `www.librepaper.org`,
  `librepaper.com`, and `www.librepaper.com`. Keep Hover forwarding off; Caddy
  serves HTTPS and redirects.
- Allow inbound ports 22, 80, and 443. Caddy needs 80 and 443 for certificates.
- GitHub OAuth callback:
  `https://app.librepaper.org/auth/callback`; Google OAuth callback:
  `https://app.librepaper.org/auth/callback/google`.
- Required SOPS keys: `PRODUCTION_GITHUB_CLIENT_ID`,
  `PRODUCTION_GITHUB_CLIENT_SECRET`, `PRODUCTION_GOOGLE_CLIENT_ID`,
  `PRODUCTION_GOOGLE_CLIENT_SECRET`, and `PRODUCTION_ADMIN_PASSWORD`.
- Grafana password in `.env` may contain only ASCII letters, digits, `_`, and
  `-`. Generate it with `openssl rand -hex 32`. Grafana reads
  its password from `.env` only when its data volume is empty; rotate it in
  Grafana, then update SOPS.

### Deployment sequence

- `deploy` accepts canonical `vMAJOR.MINOR.PATCH` releases from `v0.0.9` onward.
  The lower bound is retained because `v0.0.8` lacks the metrics listener
  required by this deployment.
- The script checks that the x86_64 Linux musl release asset exists, builds the
  candidate Docker image from that release, and validates the staged production
  TOML with the candidate binary. The Dockerfile checks the release archive
  against its published SHA-256 file.
- Before candidate validation, the script syncs the deployment kit and built
  site, writes `.env` (COMPOSE_FILE, LIBREPAPER_VERSION, OAuth pairs, Grafana
  password) and a candidate config on the VPS, and ensures the monitoring
  database role exists. A failed candidate check leaves the running app and
  active config in place, but these staged files and role setup may already
  have changed.
- Before starting the app, the script checks applied SQLx migration versions
  and checksums against the local migration files. A mismatch stops deployment
  unless the tree contains one version-1 squashed migration; see the schema
  comparison and re-pin procedure below.
- After preflight, the script installs the checked config, recreates the app
  container, starts/waits for the stack, recreates Prometheus to load its
  current config, prunes replaced Docker images, then verifies the deployment.
- Every app deploy also rebuilds `docs/_site` with `make site` and uploads it.
  `tools/deploy/production site` rebuilds and uploads only the landing page and
  manual; it does not build a release, restart containers, or deploy the app.

### Unreleased local candidate

Use `deploy-local` only for a previously built Linux musl executable, a fork,
or a build that is not in a GitHub Release:

```sh
tools/deploy/production deploy-local target/x86_64-unknown-linux-musl/release/librepaper
```

- Uses a previously built musl binary for the host architecture: x86_64 or aarch64.
- Verifies the uploaded SHA-256, builds the Docker candidate, checks TOML
  support and migrations, then restarts the app.
- Keeps the checked binary for future Docker rebuilds. A normal `deploy`
  restores the release-image build configuration.

## Migration squash

Keep migrations additive while the deployment is in normal use. Squashing is
only appropriate while there is one deployment to preserve and the history
needs to be collapsed. It is a coordinated release operation:

1. Deploy a release that contains every migration to be combined.
2. Combine the SQL into `crates/librepaper-engine/migrations/postgres/0001_catalog.sql`,
   remove the superseded files, then commit and release that change.
3. Deploy that release. The deploy script detects the single version-1 file,
   runs it against a scratch database, and compares its schema with production.
   If they match, it re-pins `_sqlx_migrations` to version 1, logs the previous
   versions in `~/librepaper/migrations-squash.log`, attempts a fresh backup,
   and continues the deploy.

- The re-pin changes migration metadata before the app restarts. Do not restart
  or deploy another image between the re-pin and release deploy.
- The backup attempt does not block deployment. If it fails, take a manual
  backup as soon as possible and investigate the failure.
- `squash-migrations` runs this preflight manually; normally `deploy` handles
  it automatically. Keep migrations additive once a second deployment exists.

## Backups and monitoring

Backups are not scheduled or copied off the VPS by this deployment tooling.
Take and copy them regularly, and test restores; see [Storage and backup](../host.md#storage-and-backup).

For a manual production backup, run on the VPS:

```sh
cd ~/librepaper
docker compose exec -T librepaper librepaper admin backup \
  --config /etc/librepaper/config.toml \
  "/var/backups/librepaper/$(date +%F)"
```

### Monitoring

Enable monitoring by exporting `COMPOSE_FILE`:

```sh
export COMPOSE_FILE=compose.yaml:compose.monitoring.yaml
docker compose up -d
docker compose exec -T postgres sh -s < monitoring-user.sh
```

Create the metrics role only on first setup; the script is idempotent.

Grafana:
- URL: `https://app.librepaper.org/admin/monitoring/`, user `admin`
- Initial password from `PRODUCTION_ADMIN_PASSWORD` (only on empty volume)
- Rotate in Grafana (Administration, Users and access, Users, admin), then update SOPS
- Prometheus keeps 30 days, capped at 8 GB (not a filesystem quota; leave headroom)

`verify` checks authenticated Grafana access, private endpoints, all three
Prometheus targets, recent LibrePaper samples, a fresh metrics snapshot, and
PostgreSQL health.

## Configuration

Server and admin commands use one TOML file. The authoritative production
configuration is in [tools/deploy/production.toml](../../tools/deploy/production.toml).

For S3-compatible storage, edit the existing `[storage]` table and enable
the commented `[storage.s3]` example; do not create a duplicate table.
Credentials come only from its references; ambient AWS credentials are
not used.

## Moderation

Moderation uses the server config. Database access is the operator
authorization boundary. Every command needs an actor and reason; the
`moderation_audit` table records actor, timestamp, action, target, and reason.

| Command | Effect |
| --- | --- |
| `block-account` | Revokes sessions and denies access and writes. |
| `hide-project` | Keeps data but blocks document, asset, source, history, export, and socket access. |

Open sockets recheck authorization every two seconds; frames already in flight
may still arrive.

```sh
librepaper admin moderate block-account github-handle --config /etc/librepaper/config.toml \
  --actor "on-call@example.org" --reason "automated abuse investigation"
librepaper admin moderate hide-project abusive-project --config /etc/librepaper/config.toml \
  --actor "on-call@example.org" --reason "contains abusive material"
```

## Deployment files

Compose and proxy settings are in [`tools/deploy/docker/`](../../tools/deploy/docker/).
The deploy script stages and validates the TOML before activating it. Update
the configuration files and this runbook together when the production topology
or deploy sequence changes.
