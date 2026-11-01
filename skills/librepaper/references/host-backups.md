# Backups and restore drills

The kit's backup sidecar streams the database and every stored file into restic as one archive. Setup, schedule, retention, bucket keys and alerts are in `docs/host/advanced.md` under Backups. Read it before changing anything.

## Never print the profile

`resticprofile.toml` holds the restic password and the bucket keys. Never open, print or copy its contents into the conversation. Losing the restic password loses every backup.

## Inspect

```sh
./manage backup list          # every snapshot: id, time, size
./manage backup show latest   # when it ran, migration version, stored files, database size
./manage backup now           # a backup now; also pings Healthchecks
```

- `backup show` prints in seconds, because the manifest is the first entry of the archive.
- `backup now` takes several minutes. An agent's shell may time out first, so run it in the background and read its output for the exit code and the prune lines.
- A backup records the migration version, and `admin restore` refuses a backup whose version differs from the binary's. Take a fresh backup after an upgrade that changes migrations.

## Restore drill

Restore the latest snapshot into a fresh copy of the kit on another machine, usually a laptop, to prove the backups work.

```sh
# On the other machine, in a fresh copy of the same release's kit, with a copy of resticprofile.toml
./manage backup restore latest   # or a snapshot id
```

Before the restore:

1. Ports 80 and 443 must be free on that machine. The copy's Caddy needs them.
2. The kit's `compose.yaml` sets `name: librepaper`, so renaming the folder does not isolate the drill. Leftover `librepaper_*` volumes from an earlier drill are reused, and `admin restore` refuses a non-empty database. Remove them first (`docker volume ls | grep ^librepaper_`).

A good result:

- `restore latest` ends with `ready`.
- Counts in the copy match the server: accounts, documents and document_assets in Postgres, and the number of files under `/var/lib/librepaper/objects` in the app container equals the manifest's stored files.

```sh
docker compose exec -T postgres psql -U librepaper librepaper -c "SELECT (SELECT count(*) FROM accounts), (SELECT count(*) FROM documents), (SELECT count(*) FROM document_assets)"
```

Cleanup: the copy holds real user data. Run `docker compose down -v` in the kit folder, then delete the folder, which removes the copied `resticprofile.toml`.

Kits older than v0.0.37 started the backup service after a restore, and that service then backed the copy up into the production repository and applied retention there. With such a kit, stop it at once with `docker stop librepaper-backup-1`.

## When a backup alert fires

A Healthchecks "failure signal" means a run started and failed. "No ping" means runs stopped.

```sh
docker compose ps backup
docker compose logs --since 6h --tail 200 backup
./manage backup list | tail -5
```

- The log is long. Search it for `exit status`, `lacked`, `Error`, `WARNING` and `snapshot`.
- `librepaper-backup-export: error: backup object ...` means a GET on the objects bucket failed. One failed object aborts the whole run.
- A stored file missing from object storage does not stop the backup. It is left out, logged as `WARNING`, listed under `missing` in the manifest, and the run counts as failed until the file is back or no longer referenced.
- A single `403 SignatureDoesNotMatch` from S3 between successful runs with the same container is the provider, not the keys. Retry before suspecting the keys.
- Once the cause is known, `./manage backup now` confirms the fix and pings Healthchecks.
