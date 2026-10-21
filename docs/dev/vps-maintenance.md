# VPS maintenance

The official instance runs on one OVH VPS (Ubuntu 26.04) with the Docker kit
in `deploy/`. This page covers what an upgrade costs in downtime and how to
keep the host itself up to date. Keep each `ssh` command on one line: a pasted
command that wraps runs only up to the break.

## LibrePaper upgrades

`tools/deploy/production deploy` runs `./manage upgrade` on the host.

- The backup, image pulls and config checks run while the old app still serves.
- The app restarts once. `manage` force-recreates `librepaper` and `backup`,
  then a plain `docker compose up` touches only services whose pins changed.
- Caddy holds requests during the restart and retries the connection for up
  to 30 s (`lb_try_duration` in `deploy/caddy/Caddyfile`), so a request waits
  instead of getting a 502. Open WebSockets drop and the browser reconnects.
- A release that bumps the Postgres or Caddy pin restarts those too. A
  Postgres restart makes the app lose its writer lease and exit, and
  `restart: unless-stopped` brings it back.

Two app instances never overlap. The server holds a deployment-wide advisory
lock (`claim_writer` in `crates/librepaper-engine/src/storage/postgres/ownership.rs`)
and migrates at startup, so a blue/green switch would need a lock handover,
backward-compatible migrations and a drain of unflushed edits. One short
restart is the design.

## Operating system updates

Security updates install nightly through `unattended-upgrades`.

```sh
# both lines should end in "1"
ssh ubuntu@librepaper.org 'cat /etc/apt/apt.conf.d/20auto-upgrades'
# what the last nightly run installed
ssh ubuntu@librepaper.org 'sudo tail -4 /var/log/unattended-upgrades/unattended-upgrades.log'
```

A manual upgrade:

```sh
ssh ubuntu@librepaper.org 'sudo apt update && apt list --upgradable'
ssh ubuntu@librepaper.org 'sudo apt full-upgrade -y'
```

- Most packages cause no downtime, because the containers do not use the
  host's libraries.
- `docker-ce` and `containerd.io` restart the Docker daemon. Live restore
  keeps the containers running through that.
- A new kernel or `libc6` takes effect only after a reboot.
- needrestart may restart services after an upgrade. `sudo needrestart -r l`
  lists them without restarting anything.

## Docker live restore

Enabled on production on 2026-10-21. `/etc/docker/daemon.json` holds:

```json
{
  "live-restore": true
}
```

```sh
# apply after editing: reload, not restart, so no container stops
ssh ubuntu@librepaper.org 'sudo dockerd --validate && sudo systemctl reload docker'
# prints true
ssh ubuntu@librepaper.org 'docker info -f "{{.LiveRestoreEnabled}}"'
```

Docker supports live restore across patch and minor releases. An upgrade that
skips a major version may still restart the containers.

## Reboots

```sh
# the file exists when a reboot is pending and lists the packages that need it
ssh ubuntu@librepaper.org 'cat /var/run/reboot-required.pkgs'
ssh ubuntu@librepaper.org 'sudo reboot'
# about a minute later, from this checkout
tools/deploy/production verify
```

- A reboot takes about a minute. Docker starts at boot and
  `restart: unless-stopped` brings every container back.
- Tried on production on 2026-10-21: the reboot was very fast, and users with
  the app open saw only a moment of unusual latency, not an error.
- The VPS also runs `jev-human-validation-web`, which goes down for the same
  minute.
- Reboot right after a LibrePaper deploy so the two outages land together.
  Once a month is enough.
- Canonical Livepatch, free with Ubuntu Pro for up to five personal machines,
  applies most kernel security fixes without a reboot: `sudo pro attach`, then
  `sudo pro enable livepatch`.
