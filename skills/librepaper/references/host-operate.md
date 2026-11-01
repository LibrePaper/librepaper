# Operating a server

This file is for a server run with the Docker Compose kit from a release (`librepaper-deploy.tar.gz`). The kit unpacks into a `librepaper/` directory on the server, and every command below runs there. The full procedures are `docs/host/simple.md` and `docs/host/advanced.md`. Read them before inventing a step.

The operator runs anything that changes the server. Give the exact command, filled in, and let them run it.

## Status and checks

```sh
./manage status    # services, pinned version, /ready, free disk
./manage check     # validates librepaper.toml, resticprofile.toml and the Caddyfile
./manage version   # the release the kit pins
```

- `/ready` answers 200 only when the app serves requests. The home page can look fine while the app is down.
- The app has no version endpoint. `./manage version` on the server is the answer. From outside, fetch the app page and its `/assets/*.js` and search them for a string the release added.

## Upgrade

```sh
./manage upgrade            # the latest release
./manage upgrade vX.Y.Z     # a specific release
```

- It verifies the kit's attestation, backs up first when backups are configured, checks the configuration against the new release, and only then switches. If a check fails, nothing switches.
- It refuses to move to an older release.
- With backups configured, it waits for the backup and takes several minutes. A backup run already in progress makes it stop with "another process is already running this profile" and change nothing. Wait for that run to finish and upgrade again.
- A burst of 502s from Caddy during the restart is normal.
- `compose.yaml` is replaced on every upgrade. Local changes belong in `compose.override.yaml`.
- Helpers from v0.0.45 and earlier stop with `the kit pins <digest>, not vX.Y.Z`. Replace `./manage` once as `docs/host/simple.md` shows, then upgrade.

Each upgrade leaves the previous release's images on disk, about 250 MB each, and `docker image prune` keeps them. This removes every LibrePaper image except the running one, because `docker rmi` refuses an image in use:

```sh
docker images --digests --format '{{.Repository}}@{{.Digest}}' | grep '^ghcr.io/librepaper/' | xargs -r docker rmi
```

A long one-liner pasted into a terminal can wrap and run in pieces. Write it to a script file and hand over one short line to run it.

## Logs

```sh
./manage logs librepaper                                         # follows the running app
sudo journalctl -t librepaper-librepaper-1 --since "1 day ago"   # across upgrades, with the journald override
```

- Without the journald override in `docs/host/advanced.md` (Logs), every upgrade and every `./manage apply` deletes the app and backup containers' logs with the containers. Ask whether the override is in place before promising old logs.
- journalctl reads times in the server's zone, usually UTC. Convert to the operator's zone when reporting.

## Resource use

The admin console's Resources tab graphs CPU, memory, free disk, stored bytes, open documents, live connections and database connections. What each series measures is defined in `SERIES` and `sample` in `crates/librepaper-server/src/server/graphs/mod.rs`.

- "Documents resident" counts documents loaded in server memory, not documents stored. The Storage graph tracks what is stored.
- Free disk falls by about 250 MB at each upgrade because old images stay on disk. That is not user data.
- Host CPU includes every other process on the machine, including the backup sidecar.

The history behind the graphs is `state/metrics.sqlite` in the `data` volume, with one row a minute per series in `samples(series, ts, value)` and the names in `series`. Timestamps are UTC seconds. Copy it with its WAL file, or recent rows are missing:

```sh
# On the server, from the kit directory; the volume is <project>_data, usually librepaper_data
v=$(docker volume inspect -f '{{.Mountpoint}}' librepaper_data)
sudo tar -C "$v/state" -cf - metrics.sqlite metrics.sqlite-wal > metrics.tar
```

- A gap of 75 s or more is a restart only if the first row after it has a null `host_cpu_percent` (a new process has no earlier CPU ticks) and `documents_resident` resets. A gap without those is a sampler stall, for example when every database connection was checked out.
- `docker stats` under-reports the app. For its real memory, use `docker top librepaper-librepaper-1 -o pid,rss,cmd`.
- The memory budget, evictions, busy refusals and warm caches are in `/api/status`, which answers only a loopback request inside the container:

```sh
docker exec librepaper-librepaper-1 wget -qO- http://127.0.0.1:8080/api/status | jq '.memory_budget, .rooms'
```

- Counters in `/api/status` reset at each restart, so they cover the time since the last upgrade or apply.
- Prometheus `/metrics` exists only when `librepaper.toml` has a `[metrics]` section. Without it, the admin graphs are the only metrics.

## Other apps on the same server

When the server hosts other apps, never run `docker compose down` in their directories, never prune volumes, and do not change the logging driver of a Caddy or Postgres that other apps share. Each of these restarts or deletes something that is not LibrePaper's.
