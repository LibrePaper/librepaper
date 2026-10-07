# LibrePaper Deployment Kit

One VPS, Docker only, one file to edit.

1. Extract the kit: `tar xz -f librepaper-deploy.tar.gz && cd librepaper`
2. Edit `librepaper.toml`: set your two hostnames and OAuth credentials
3. Start: `docker compose up -d`
4. Check: `curl -fsS https://paper.example/ready`

For monitoring, backups, upgrades, and using a database elsewhere, see [docs/host.md](../docs/host.md).
