# Isolated restore exercise

This procedure restores one remote Restic snapshot into a new Compose project,
PostgreSQL volume, and app data volume. It never uses the production project.
The migration service receives the owner URL only for `admin restore`; restore
requires an empty database and does not run migrations before loading the dump.
Use the exact LibrePaper release recorded for the snapshot.

## Prepare an isolated project

Extract the matching deployment kit into a separate recovery directory. Copy
the database role and Restic credentials from the encrypted recovery packet,
then initialize a new project and database. Never copy production `.env` or
production role URLs into the recovery project.

```sh
mkdir -m 700 -p /srv/librepaper-recovery
cd /srv/librepaper-recovery/deploy
./setup init --database local --project librepaper-recovery --version <snapshot-release>
./setup check
```

Replace the generated `restic_password` in this recovery project's `secrets/`
with the password used by the source repository. Restore the exact snapshot's
`resticprofile.toml` and any S3, SFTP, or other repository credentials from the
recovery packet. Protect these files with a mode-0700 directory and mode-0444
files. Set `LIBREPAPER_RESTICPROFILE_CONFIG_FILE=./recovery-resticprofile.toml`
for the commands below after placing that config in the project directory.
Keep the source OAuth client credentials in the encrypted recovery packet as
well; place them into the recovery project's individual provider secret files
before verifying a new login. They are not part of the database or object
snapshot.

Create `compose.recovery.yaml` so the recovery database and data volume stay
separate and only the app is reachable from localhost:

```yaml
services:
  librepaper:
    ports:
      - "127.0.0.1:18080:8080"
  migrate:
    volumes:
      - data:/var/lib/librepaper
      - ./recovery-staging:/restore:ro
  backup:
    volumes:
      - ./recovery-staging:/restore:ro
networks:
  edge:
    ipam:
      config: !override
        - subnet: 172.31.0.0/16
```

Use an unused subnet and set the recovery app config's
`proxy.trusted_networks` to the same range. Keep the recovery project's
`session.key` separate until the snapshot key is restored.

The command examples use the Compose file pair and project name explicitly:

```sh
dc() { docker compose -f compose.yaml -f compose.production.yaml \
  -f compose.recovery.yaml -p librepaper-recovery "$@"; }
mkdir -m 700 recovery-staging
dc up -d --wait postgres
```

If the matching Compose selection includes monitoring, append its file too.
Do not start the migration service at this stage.

## Fetch and restore the snapshot

Use Restic to extract the exact snapshot into `recovery-staging`. It contains
the verified database/object bundle, the app and backup configs, and the
session key. The source profile and repository credentials must be available
to the backup service for this read-only operation.

```sh
LIBREPAPER_RESTICPROFILE_CONFIG_FILE=./recovery-resticprofile.toml \
dc run --rm --no-deps --user 0 --entrypoint resticprofile backup \
  -c /etc/resticprofile/profiles.toml -n resticprofile \
  restore <snapshot-id> --target /restore
```

Inspect the extracted files. Copy the snapshot's
`etc/librepaper/librepaper.toml` to `recovery-app.toml` and
`etc/resticprofile/resticprofile.toml` to `recovery-resticprofile.toml`. Confirm
the app config points to `/run/secrets/database_url`, uses the recovery
origins/network, has `storage.directory = "/var/lib/librepaper"`, and sets
`[server] migrate = false`. The recovery `migrate` service mounts the generated
owner URL at `/run/secrets/database_url`.

Run `admin restore` into a new child directory of the data volume. The database
must be empty and the schema must come from the dump, so do not run `migrate`
before this command:

```sh
LIBREPAPER_CONFIG_FILE=./recovery-app.toml \
LIBREPAPER_RESTICPROFILE_CONFIG_FILE=./recovery-resticprofile.toml \
dc run --rm --no-deps \
  migrate admin restore --config /etc/librepaper/librepaper.toml \
  /restore/var/backups/librepaper/current /var/lib/librepaper/recovered
```

The restore verifies the backup manifest and object hashes, restores the
database in one transaction, and refuses a non-empty database or an existing
destination path.

Copy the restored objects and the matching session key to the new data-volume
root. Use the session key from this same snapshot, never a key from another
deployment or snapshot:

```sh
dc run --rm --no-deps --user 0 --entrypoint sh migrate -c '
  install -d -o 10001 -g 65534 /var/lib/librepaper/objects /var/lib/librepaper/secrets
  cp -a /var/lib/librepaper/recovered/objects/. /var/lib/librepaper/objects/
  install -o 10001 -g 65534 -m 0600 \
    /restore/var/lib/librepaper/secrets/session.key \
    /var/lib/librepaper/secrets/session.key
  chown -R 10001:65534 /var/lib/librepaper/objects
'
```

## Verify without public exposure

Start only the app on the loopback port. `--no-deps` avoids starting Caddy,
backup, or a migration job; the restored dump already contains its schema.

```sh
LIBREPAPER_CONFIG_FILE=./recovery-app.toml dc up -d --no-deps --wait librepaper
curl -fsS -H 'Host: app.example' http://127.0.0.1:18080/ready
```

Open a restored document through a local browser or an SSH tunnel, verify its
rendered content and referenced assets, and confirm an existing session/share
link works with the matching key. Check that a new login uses only recovery
OAuth credentials. Keep the production deployment untouched until these
checks pass. Record the snapshot ID, release, elapsed time, and result; apply
known post-snapshot deletions before making recovered data public.
