# Deployment credentials

LibrePaper's Compose deployment uses individual files under `deploy/secrets/`.
The directory should be mode 0700 and secret files mode 0444: the directory
limits host access while the file mode lets the unprivileged container user
read an individually bind-mounted file. Docker mounts each file read-only into
only the service that needs it. `.env`
contains non-secret Compose selections and the version only. Do not create a
combined `all-secret.env`, pass values in command arguments, or print them in
deployment logs.

These files are plaintext on the host. File modes prevent ordinary users from
reading them, but do not encrypt the disk; protect the host and include the
secret directory only in encrypted off-host recovery storage.

| File | Consumer | Purpose |
| --- | --- | --- |
| `github_client_id`, `github_client_secret` | app | GitHub OAuth client |
| `google_client_id`, `google_client_secret` | app | Google OAuth client |
| `database_app_url` | app | Runtime DML and writer lease |
| `database_owner_url` | one-shot `migrate` service | Schema migration and recovery owner |
| `database_backup_url` | backup sidecar | Read-only consistent dump |
| `database_metrics_url` | setup and metrics exporter | Setup derives the exporter URI, username, and password files |
| `database_owner_password`, `database_app_password`, `database_backup_password`, `database_metrics_password` | local PostgreSQL setup | Passwords for the four scoped local roles; keep each aligned with its URL |
| `database_metrics_uri`, `database_metrics_user` | metrics exporter | Derived from `database_metrics_url` by setup |
| `postgres_bootstrap_password` | local PostgreSQL setup and recovery | Provisions local roles and supports emergency administration |
| `grafana_admin_password` | Grafana, optional | Seeds a fresh Grafana database volume |
| `restic_password` | backup sidecar | Encrypts the Restic repository |
| `aws_credentials` | backup sidecar, optional | AWS-compatible Restic repository credentials |
| `restic_known_hosts` | backup sidecar, optional | Pinned SSH hosts for SFTP repositories |
| `session.key` | app data volume | Signs session cookies and encrypts saved share secrets; included in backups |

For external PostgreSQL, provide each URL as a separate secret before running
`./setup init --database external`. The helper's stdin interface avoids
putting the value in shell history or process arguments:

```sh
printf '%s' "$DATABASE_APP_URL" | ./setup set-secret database_app_url
printf '%s' "$DATABASE_OWNER_URL" | ./setup set-secret database_owner_url
printf '%s' "$DATABASE_BACKUP_URL" | ./setup set-secret database_backup_url
printf '%s' "$DATABASE_METRICS_URL" | ./setup set-secret database_metrics_url
```

Use TLS with `sslmode=verify-full` and separate PostgreSQL roles. The app role
does not need schema ownership; the backup role must not be able to modify
data; the exporter should have `pg_monitor` only. The bootstrap credential is
used to provision local roles and should not be mounted into the app or backup
service. See the [host guide](host.md) for the deployment and external database
setup.

Rotate a local database credential by changing the role password in PostgreSQL
and atomically updating both its `database_<role>_password` file and URL file.
For an external database, update the role and URL; those local password files
are unused. For the metrics role, also update `database_metrics_uri`,
`database_metrics_user`, and `database_metrics_password` to match
`database_metrics_url`. For external PostgreSQL, `./setup check` detects
mismatches between the URL and its derived exporter files. For local
PostgreSQL, change the metrics role password and update all four files
together. Recreate the matching consumer: `librepaper` for the app URL,
`backup` for the backup URL, or the exporter for the metrics URL. A new owner
URL is needed only when running a migration. Revoke access with
`ALTER ROLE <role> NOLOGIN`, terminate that role's existing sessions, and
remove its URL and password files after its consumer has stopped.

The base setup does not provision S3 object-store credentials or mount them
into the app and backup containers. To use S3-compatible object storage,
provide protected credential files and a Compose overlay that mounts the
app's required read/write credentials and the backup service's read-only
credentials, then reference the mounted files in `[storage.s3]`. SFTP private
keys likewise need an operator-defined secret file and Compose mount;
`restic_known_hosts` alone does not provide a private key.

`deploy/setup` manages `COMPOSE_FILE` and rewrites it when you change database,
monitoring, production, or local-build selections. Keep custom Compose overlays
separate and include them explicitly, for example
`docker compose -f compose.yaml -f compose.monitoring.yaml -f custom.yaml ...`;
do not append them manually to `.env`, where a later `setup init` can replace
that selection.

The production deployment helper reads only named OAuth and Grafana values
from the SOPS-encrypted `tools/deploy/keys.yaml` and writes them to their
individual remote files. It does not decrypt database passwords into a shared
environment file; setup generates unique local role credentials. Rotate OAuth
at the provider first, then edit the encrypted value with
`sops tools/deploy/keys.yaml` and run the production deploy helper. For external
PostgreSQL, change the role password first, update the matching URL secret,
then restart its consumer. Editing SOPS and deploying a new Grafana password
does not rotate a password already stored in Grafana's existing database; use
the Grafana administrator interface to rotate that credential.

The Restic password and remote object-store/SFTP credentials are separate from
the database roles. Keep them only in resticprofile-supported files or secret
files. Do not put them in the Compose `.env` or app database URL. The session
key is created in the app data volume, not under `deploy/secrets`; include it
in the encrypted recovery copy and restore the matching file.

Keep an encrypted recovery packet outside the VPS with the matching release,
both configs, repository location and Restic password, remote storage and
OAuth provider credentials, a way to recreate database roles, and the session
key. Restrict recovery access separately from routine host access.
