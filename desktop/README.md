# LibrePaper desktop

Build the app. The build also bundles the CLI:

```sh
# Builds dist/librepaper, copies it to desktop/binaries/ as the sidecar,
# then writes desktop/target/release/librepaper-desktop
make desktop
```

Run it against a local server, with a debug build of the CLI:

```sh
# LIBREPAPER_BIN overrides the librepaper binary the shell starts
LIBREPAPER_SERVER=http://localhost:8080/ LIBREPAPER_BIN=target/debug/librepaper desktop/target/release/librepaper-desktop
```

- `LIBREPAPER_SERVER` replaces the server list in `desktop.json`.
- `LIBREPAPER_BIN` replaces the bundled CLI for one run.
- `--server <URL>` adds a server after a confirmation dialog.
- `--open <URL>` and `--settings` open a page in the running window.
- `librepaper://open?url=<URL>` opens a page the same way as `--open`.
