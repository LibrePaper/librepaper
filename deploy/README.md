# LibrePaper deployment kit

This archive is version-matched to the app and backup images published for the
same stable release tag. Verify the adjacent `.sha256` file before extracting:

```sh
sha256sum -c librepaper-deploy-kit-v<release>.tar.gz.sha256
tar -xzf librepaper-deploy-kit-v<release>.tar.gz
cd deploy
```

The kit contains no operator credentials or host-specific configuration.

Requirements: Docker Engine with Compose 2.24.4+, Python 3.11+, `openssl`, and
`curl`. Configure DNS for the app and docs origins and allow inbound ports 80
and 443; the site origin is optional. See the full [self-hosting guide](https://github.com/LibrePaper/librepaper/blob/main/docs/host.md)
and [credential guidance](https://github.com/LibrePaper/librepaper/blob/main/docs/credentials.md)
before exposing the service.

For a local PostgreSQL install, edit `librepaper.toml`, place OAuth values in
individual files under a mode-0700 `secrets/` directory, then run:

```sh
./setup init --database local
./setup check
docker compose up -d --wait
```

Create `secrets/grafana_admin_password` and use
`./setup init --monitoring` to enable the optional monitoring overlay. Configure
a remote Restic repository in `resticprofile.toml`; backups are disabled until
that file has a valid `[resticprofile]` table. Confirm the first remote backup
and complete a restore exercise before using the server for important data.

For a managed PostgreSQL service, place separate runtime, owner, backup, and
metrics URLs in their `database_*_url` secret files, require TLS certificate
verification, and initialize with `./setup init --database external`.

The supported topology uses one app writer and one backup scheduler per
database. See the self-hosting guide for role privileges, upgrade and rollback
steps, key recovery, and limits on horizontal scaling.
