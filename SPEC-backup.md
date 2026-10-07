# Automated backup sidecar

**Status:** Implemented and tested in the task worktree, 2026-10-05. Based on
tree `a7dc3b7a` and resticprofile `0.33.1`; pending review and merge.

## Decisions

- Add a Docker `backup` sidecar for scheduled exports, snapshots, retention,
  pruning and repository checks. It uses resticprofile `0.33.1` for scheduling,
  locking and restic operations.
- Keep independent operator files: `librepaper.toml` for the app and
  `resticprofile.toml` for resticprofile. The named `[resticprofile]` table is
  merged into a kit-owned base profile; a comments-only backup file disables
  backups.
- The app parser does not understand backup settings or report their presence.
  It rejects misplaced `[resticprofile]` and legacy `[backup]` tables safely.
- No Docker socket, expiration policy or deletion ledger. If backups stop,
  snapshots remain until backups resume or an operator deletes them; alerts
  report the stoppage.
- The kit has no Alertmanager: local alerts are dashboard-only. Operators must
  configure external notification delivery.

## Existing behavior

- `admin backup` exports a verified snapshot-consistent `pg_dump` and referenced
  objects while holding the blob lifecycle lock. Restore requires an empty DB
  and new destination, verifies hashes, and writes objects under `objects/`.
- The kit lacks `pg_dump`; add PostgreSQL 17 client tools. Fix the current
  `fork_at on shallow docs` replay failure and pass `tools/test/suite backup`
  before release.
- Back up both operator configs, `secrets/session.key`, the current export and
  referenced objects: without the session key, sessions are invalidated and the
  app cannot reveal existing share URLs; URLs already held still work.
- Erasure revokes sessions immediately and purges owned documents after 7 days.
  Comments, replies and labels on others' documents remain as “Deleted user.”
  In-flight erasures resume after restore. Remove the incorrect “configurable”
  claim in `docs/privacy.md`.

## Image, Compose and production deployment

- Add `postgresql17-client` to `app-runtime`. Add a `backup` target from it
  with restic, OpenSSH client, Python 3, Supercronic and pinned resticprofile
  `0.33.1`; copy in the profile/helper. End with `FROM app-runtime AS app` so
  plain builds select the app. Install rclone if supporting `rclone:` remotes.
- Keep the app user/runtime unchanged. Run backup as UID 10001 with private
  writable paths and `HEALTHCHECK NONE` (no inherited HTTP check).
- Compose builds explicit app and backup targets. Backup shares `pgsocket`,
  read-write `data`, `backups`, read-only app and backup configs;
  it gets the app env and DB override and starts after app health. Mount private
  tmpfs at `/run/librepaper-backup` (`uid=10001,gid=65534,mode=0700`).
- Production passes matching source/version/build args to both images, builds
  and checks both, and recreates both. Preserve an existing remote
  `resticprofile.toml` with its credentials; install the empty template only
  when that file is absent. Stop backup for schema upgrades; restart only after
  app health.

## Startup, jobs and locks

- Startup uses `set -eu`. Python `tomllib` parses the complete
  `/etc/resticprofile/resticprofile.toml`; a named `[resticprofile]` table
  enables backups, while comments-only default disables them. Accept
  whitespace, quoted table syntax and implicit parent tables. Reject malformed
  TOML and wrong types without logging values.
- On absent table, write `enabled=0` and idle without a scheduler. On present
  table, persist an enabled sentinel before profile validation, so invalid
  enabled configuration remains alertable. Validate the selected profile
  without printing resolved secrets; `show` checks merged config, not
  credentials or backend connectivity.
- Generate only the selected profile's crontab with
  `resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile schedule`,
  then `exec /usr/bin/supercronic /run/librepaper-backup/crontab` as UID 10001. Any
  parsing, validation or scheduling error exits nonzero.
- A single sidecar owns the private tmpfs and local lock. It may clear a stale
  local lock at startup only under this invariant. Manual backup, check and
  maintenance use the same profile via `docker compose exec backup`; never
  bypass the lock. Before operator recovery, stop the sidecar and all manual
  jobs. Inspect remote restic locks and confirm no active owner before using
  restic's recovery procedure; never blindly unlock.
- Schedule daily backup and Monday 06:00 check away from the backup window;
  both wait up to 2h on the shared lock. Give each independent dead-man URLs
  and freshness deadlines; schedule changes require matching alert changes.
- Clean staging on normal exit and remove stale staging under lock before export.

## Base profile and operator configuration

- `/etc/resticprofile/profiles.toml` includes only
  `/etc/resticprofile/resticprofile.toml`.
- In resticprofile `0.33.1`, included properties override base properties;
  sections merge and lists replace as a whole.
- The base owns export source, hooks, lock/status paths and fixed operations;
  docs prohibit overriding them.

```toml
includes = ["/etc/resticprofile/resticprofile.toml"]

[global]
scheduler = "crontab:-:/run/librepaper-backup/crontab"

[resticprofile]
initialize = true
lock = "/run/librepaper-backup/lock"
cache-dir = "/var/backups/librepaper/cache"
status-file = "/var/backups/librepaper/status.json"

[resticprofile.backup]
schedule = "daily"
schedule-lock-wait = "2h"
schedule-permission = "user"
extended-status = true
host = "librepaper"
tag = ["librepaper"]
source = ["/var/backups/librepaper/current", "/etc/librepaper/librepaper.toml", "/etc/resticprofile/resticprofile.toml", "/var/lib/librepaper/secrets"]
run-before = """
set -eu
rm -rf /var/backups/librepaper/current
librepaper admin backup --config /etc/librepaper/librepaper.toml /var/backups/librepaper/current
"""
run-finally = "rm -rf /var/backups/librepaper/current"

[resticprofile.retention]
after-backup = true
keep-within = "48h"
keep-daily = 14
keep-weekly = 11
prune = true
max-unused = "0"

[resticprofile.check]
schedule = "Mon 06:00"
schedule-lock-wait = "2h"
schedule-permission = "user"
read-data-subset = "10%"
```

- The success hook runs only after export, snapshot and retention/prune succeed.
- Keep `max-unused = "0"` under retention: after-backup runs forget/prune as
  the retention operation. The optional `[resticprofile.prune]` is for manual
  prune settings, not this policy.
- Operators may set repository, credentials, schedules, retention and
  notifications in `resticprofile.toml`. Protect both config files and keep
  independent off-host recovery copies. `resticprofile show` may expose
  secrets; use only in the operator's terminal.
- Local repositories need durable storage outside staging/container layers.
  SFTP needs mounted private key and `known_hosts` files. Document required
  mounts alongside supported backends.

## Status, alerts and retention

- Status is resticprofile's `status-file` (`/var/backups/librepaper/status.json`)
  plus the external Healthchecks pings. There are no metrics, no textfile, no
  node-exporter.
- Use independent dead-man checks: 36 hours for backup and 8 days for the weekly
  check. Hook failures publish job failure; failures outside hooks, including
  scheduler lock timeout, are caught by the missed-success deadline.
- `backups` needs room for one full export including the database dump. Export
  holds the blob lifecycle lock.
- While backups run, keep snapshots from the last 48 hours, then 14 daily and
  11 weekly. Gaps can extend ages; failed retention/pruning delays removal.
  Successful prune retains zero unused data. Versioned buckets may keep old
  pack versions; configure noncurrent-version expiration (B2: keep only last
  version). Do not promise fixed 75/90-day deletion windows.

## Restore procedure

1. Stop app and backup. Preserve the original app, database, data volume and
   remote repository until recovery is verified. Restore with the matching
   LibrePaper release; upgrade schema only afterward.
2. Create an empty recovery database and a new named data volume. Set the
   recovery database URL explicitly; remove or replace inherited
   `LIBREPAPER_DATABASE_URL` so it cannot point to production.
3. Restore the selected snapshot to a temporary directory. Inspect its path
   hierarchy and copy both backed-up configs. Keep
   `storage.directory = "/var/lib/librepaper"` in the app config, set the
   recovery DB URL, and use the matching backup config to access snapshots.
4. Run `admin restore` against the matching release, empty database and a
   nonexistent child path such as `/var/lib/librepaper/recovered` in the new
   volume. The volume root already exists. Install `recovered/objects` and
   restored `secrets/` at the volume root with UID 10001 ownership.
5. For S3, upload and verify every manifest object under the same keys in a
   **new recovery bucket**, preserving the original bucket. Point recovery at
   that bucket, or explicitly switch to filesystem storage and remove old S3
   settings. Never start against an empty or unverified store.
6. Start recovery, re-run known deletions requested after the snapshot, and
   verify health/documents while preserving the original deployment. Record
   release, snapshot age, elapsed time and outcome in drills; claim no recovery
   time until measured.

## Implementation boundaries

- Image and runtime: `deploy/Dockerfile`; app/sidecar wiring:
  `deploy/compose.yaml`; matching production rollout and upgrade order:
  `tools/deploy/production`.
- Config and status: `deploy/librepaper.toml`, `deploy/resticprofile.toml`,
  `tools/deploy/production.toml`,
  `deploy/backup/profiles.toml` and sidecar entrypoint.
- Parser: `crates/librepaper-base/src/config/mod.rs` and
  `crates/librepaper/src/cli/server_config.rs`; remove `BackupPolicy`, reject
  misplaced `[resticprofile]` and legacy `[backup]` at app startup.
- Tests/docs: `tools/test/backup/`, `tools/test/suite`, `docs/host.md`,
  `docs/cli.md`, `docs/dev/cost-policy.md`, `docs/privacy.md` and
  `docs/dev/privacy-operators.md`.
- Pass acceptance before release; first fix the PostgreSQL client/replay issue.

## Acceptance

- Sidecar configuration selection covers absent `[resticprofile]` (disabled),
  valid TOML variants, malformed input and wrong types without logging values.
  The app parser rejects misplaced `[resticprofile]` and legacy `[backup]`.
  Test shipped-version merge/show.
- Exercise actual after-backup retention/pruning with `max-unused = "0"`.
  Export, snapshot, retention, checks, hooks and independent dead-man results
  must report correctly; failed exports create no snapshots and failures never publish job success.
- Cover absent/missing metrics, never-run jobs, first-enabled deadline across
  restart, scheduler timeout/collision, abrupt termination and staging cleanup.
  Never remove an active local lock; metrics contain no identifiers/secrets.
- Confirm UID 10001, private writable paths, no HTTP health check, plain build
  defaulting to app, and `compose up --wait` with backup disabled/enabled.
- Confirm production builds/recreates both images, reloads replaced config and
  pauses backup across schema upgrades. Restore drills use a new DB/volume,
  matching release, correct ownership and verified new-bucket S3 keys; preserve
  original deployment and re-run known later deletions.
- `tools/test/suite backup` passes. Exercise local, SFTP, S3-compatible and rest
  targets with their required mounts and credentials.

## References

- [resticprofile](https://creativeprojects.github.io/resticprofile/) 0.33.1:
  includes/merge, scheduler, hooks, metrics, extended status, non-root
  Supercronic.
- [restic forget and prune](https://restic.readthedocs.io/en/stable/060_forget.html)
- Engine backup/maintenance/worker code; auth deployment key; PostgreSQL
  erasure repository; `deploy/`; production deploy; backup test README.
