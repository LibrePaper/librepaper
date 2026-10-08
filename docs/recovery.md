---
title: "Isolated restore exercise"
---

Restore one remote Restic snapshot into a new deployment on a machine that is not the VPS. The kit publishes ports 80 and 443 and claims a fixed subnet, so two copies cannot share a host.

## Restore steps

```sh
# On a machine that is not the VPS. Docker is the only requirement.
mkdir -m 700 librepaper-recovery && cd librepaper-recovery
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cd librepaper
cp /path/to/your/resticprofile.toml .   # the repository and RESTIC_PASSWORD are all restore needs
# Keep the kit's own librepaper.toml: it points at the empty local database. Your copy may point at a managed database, or at secret files this kit does not mount.
mkdir -m 700 staging
docker compose up -d --wait postgres                                 # an empty database; do not start the app yet
docker compose run --rm --no-deps --user 0 -v ./staging:/restore --entrypoint resticprofile backup \
  -c /etc/resticprofile/profiles.toml -n resticprofile restore <snapshot-id> --target /restore
docker compose run --rm --no-deps --user 0 -v ./staging:/restore:ro librepaper \
  admin restore --config /etc/librepaper/librepaper.toml /restore/var/backups/librepaper/current /var/lib/librepaper/recovered
docker compose run --rm --no-deps --user 0 -v ./staging:/restore:ro --entrypoint sh librepaper -c '
  install -d -o 10001 -g 65534 /var/lib/librepaper/objects /var/lib/librepaper/secrets
  cp -a /var/lib/librepaper/recovered/objects/. /var/lib/librepaper/objects/
  install -o 10001 -g 65534 -m 0600 /restore/var/lib/librepaper/secrets/session.key /var/lib/librepaper/secrets/session.key
  chown -R 10001:65534 /var/lib/librepaper/objects'
docker compose up -d --wait
docker compose exec librepaper wget -qO- http://127.0.0.1:8080/ready
```

`admin restore` verifies the manifest and object hashes and loads the database in one transaction; it refuses a non-empty database or an existing destination. Use the session key from the same snapshot, never one from another deployment.

Open the app through a local browser or SSH tunnel and verify rendered content, assets, and existing sessions.

## External backup alerts

Nothing on the VPS can report that the VPS is down, so dead-man checks live outside it. Configure Healthchecks for backup and repository-check jobs:

```toml
[[resticprofile.backup.send-before]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>/start"
[[resticprofile.backup.send-after]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>"
[[resticprofile.backup.send-after-fail]]
method = "HEAD"
url = "https://hc-ping.com/<backup-id>/fail"

[[resticprofile.check.send-before]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>/start"
[[resticprofile.check.send-after]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>"
[[resticprofile.check.send-after-fail]]
method = "HEAD"
url = "https://hc-ping.com/<check-id>/fail"
```

In Healthchecks, give the backup check a Simple schedule of 1 day with 12 hours grace, and the repository check 7 days with 1 day grace: an alert after 36 hours without a successful backup, or 8 days without a check, and at once on a failed run. The account's weekly report, sent on Mondays, lists every check's status. Uptime monitoring is in [host.md](host.html#alerts).
