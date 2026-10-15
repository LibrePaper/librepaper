# LibrePaper desktop

Build the shell:

```sh
# Writes desktop/target/release/librepaper-desktop
make desktop
```

Run it against a local server, with a debug companion binary:

```sh
# LIBREPAPER_BIN is the librepaper binary the shell starts for the companion
LIBREPAPER_SERVER=http://localhost:8080/ LIBREPAPER_BIN=target/debug/librepaper desktop/target/release/librepaper-desktop
```

- `LIBREPAPER_SERVER` replaces the server list in `desktop.json`.
- `--server <URL>` adds a server after a confirmation dialog.
- `--open <URL>` and `--settings` open a page in the running window.
