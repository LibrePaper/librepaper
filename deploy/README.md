# LibrePaper deployment kit

One VPS, Docker only, one file to edit.

```sh
tar xzf librepaper-deploy.tar.gz && cd librepaper
$EDITOR librepaper.toml      # the two hostnames, a GitHub OAuth client id and secret
docker compose up -d
curl -fsS https://paper.example/ready
```

Backups: `./backups help` lists, inspects and restores snapshots.
Upgrades: https://librepaper.org/host/simple.html#upgrade
Backups, monitoring, object storage and a database elsewhere: https://librepaper.org/host/advanced.html
