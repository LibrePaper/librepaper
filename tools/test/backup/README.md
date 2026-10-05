# Backup drill

Proves that a backup taken by `admin backup`, encrypted with age and restored
by `admin restore` into an empty database matches the source row for row and
byte for byte, and replays to the same document heads and labeled versions.
The source is synthetic: five tutorial projects, each compacted to a snapshot
and then edited twice.

```sh
tools/test/suite backup   # everything: database, key, drill, replay check
```

The sidecar integration suite uses the same synthetic five-project fixture,
then starts the locally built backup image with disposable PostgreSQL and a
temporary filesystem repository. It covers disabled/enabled startup, a real
profile-merged backup and check, failed export metrics, after-backup retention,
and restore into a fresh database. The supervisor builds the image first:

```sh
BACKUP_TEST_IMAGE=librepaper-backup-test:75ew \
tools/test/suite backup-sidecar
```

`BACKUP_TEST_IMAGE` defaults to the coordinator's `librepaper-backup-test:75ew`
when exported. Docker containers and temporary data are run-scoped. Set
`BACKUP_SIDECAR_KEEP=1` to retain the fixture for inspection. This suite does
not yet exercise SFTP, S3-compatible, or REST repositories.

## Running it by hand

- `LIBREPAPER_BIN`: a release `librepaper`
- `LIBREPAPER_SOURCE_URL`, `LIBREPAPER_RESTORE_URL`: two empty databases,
  named exactly `drill_source` and `drill_restored`
- `AGE_RECIPIENT`, `AGE_IDENTITY`: an age public key and the path of its identity
- `BACKUP_DRILL_DIR` (optional): an empty `/tmp/librepaper-backup-drill.*`
  directory to work in; the caller removes it
- `BACKUP_DRILL_KEEP=1` (optional): keep the source and restored data directories

```sh
tools/test/backup/drill.sh
# then, with the data directories it printed:
BACKUP_DRILL_SOURCE_URL=... BACKUP_DRILL_RESTORE_URL=... \
BACKUP_DRILL_SOURCE_DATA=.../source-data BACKUP_DRILL_RESTORE_DATA=.../restored-data \
cargo test --release -p librepaper --test backup_drill_verify -- --ignored --nocapture
```

Without matching PostgreSQL 17 clients, symlink `postgres-tool.sh` as `psql`,
`pg_dump` and `pg_restore` somewhere on `PATH`; it runs them in Docker.

## Caveats

- A recovery-path check, not a measurement of recovery time, cost or size.
- Deleting staging files is not secure erasure on SSDs or snapshotting filesystems.
- It installs no backup scheduler, retention policy or key management.
