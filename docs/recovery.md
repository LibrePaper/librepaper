---
title: "Isolated restore exercise"
---

Restore one remote Restic snapshot into a new deployment on a machine that is not the VPS. The kit publishes ports 80 and 443 and claims a fixed subnet, so two copies cannot share a host.

## Restore steps

Install Docker, extract the kit, and prepare the recovery directory:

```sh
mkdir -m 700 /srv/librepaper-recovery
cd /srv/librepaper-recovery
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-deploy.tar.gz | tar xz
cp /path/to/backup/librepaper.toml librepaper/
cp /path/to/backup/resticprofile.toml librepaper/
```

Start postgres and restore the database dump:

```sh
cd librepaper
docker compose up -d postgres
docker compose exec -T postgres pg_restore -U librepaper -d librepaper -Fc /path/to/librepaper.dump
```

Restore objects and session.key, then start the app:

```sh
docker compose run --rm -u 0 backup resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile restore <snapshot-id> --target /tmp/restore
docker compose run --rm -u 0 -v /tmp/restore:/restore librepaper sh -c 'install -d -o 10001 -g 65534 /var/lib/librepaper/{objects,secrets} && cp -a /restore/var/lib/librepaper/objects/. /var/lib/librepaper/objects/ && cp /restore/var/lib/librepaper/secrets/session.key /var/lib/librepaper/secrets/ && chown -R 10001:65534 /var/lib/librepaper/objects'
docker compose up -d
docker compose exec librepaper wget -qO- http://127.0.0.1:8080/ready
```

Open the app through a local browser or SSH tunnel and verify rendered content, assets, and existing sessions.

## External backup alerts

Grafana cannot alert when the VPS is unavailable. Configure separate external dead-man checks for backup and repository-check jobs:

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

Set missed-success deadlines to 36 hours for backups and 8 days for repository checks.

## Installs from the v0.0.21 kit

Installs using the scoped-role kit with separate credentials are not converted. Back up with the sidecar, install this new kit, and restore the snapshot using the procedure above.
