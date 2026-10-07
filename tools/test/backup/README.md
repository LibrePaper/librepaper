# Backup drill

Proves that a backup taken by `admin backup`, encrypted with age and restored
by `admin restore` into an empty database recovers the PostgreSQL rows, object
bytes, and the original `session.key`. The drill signs in against the source
before backup, then starts the restored app with a restricted runtime role
and confirms that the original session cookie still authenticates to the
recovered account. It also replays to the same document heads and labeled
versions. The source is synthetic: five tutorial projects, each compacted to
a snapshot and then edited twice.

```sh
tools/test/suite backup   # everything: database, key, drill, replay check
```

The sidecar integration suite uses the same synthetic five-project fixture,
then starts the locally built backup image with disposable PostgreSQL and
temporary repositories. It connects to PostgreSQL with a generated SELECT-only
backup role, covers disabled/enabled startup, a real profile-merged backup and
check, failed backup recovery, after-backup retention, and restore into a fresh
database. The restore is run without the source backup role, then the test
starts the restored app with a restricted DML role and verifies the original
signed-in session. Status is reported by resticprofile's status.json file and
Healthchecks hooks. By default it also runs SFTP, MinIO S3-compatible, and
authenticated REST backends using generated keys and synthetic credentials.
Build the image first:

```sh
make web
docker build -f deploy/Dockerfile --build-arg SOURCE=checkout \
  --target backup -t librepaper-backup-test:local .
BACKUP_TEST_IMAGE=librepaper-backup-test:local \
tools/test/suite backup-sidecar
```

`BACKUP_TEST_IMAGE` defaults to `librepaper-backup-test:local`.
Override helper images with `BACKUP_TEST_SFTP_IMAGE`,
`BACKUP_TEST_MINIO_IMAGE`, and `BACKUP_TEST_REST_IMAGE`. Set
`BACKUP_TEST_BACKENDS=0` for a local-only run. Docker containers and temporary
data are run-scoped. Set `BACKUP_SIDECAR_KEEP=1` to retain the fixture for
inspection.

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
