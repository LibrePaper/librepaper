# Installing the local companion

Cargo Dist publishes the platform archives and generated installer entry
points with each tagged release:

- Linux and macOS: `librepaper-installer.sh`
- Windows: `librepaper-installer.ps1`

Linux and macOS:

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1 | iex"
```

The generated installers select the matching release archive. The old
`deploy/install.sh` and `install-companion.*` commands remain compatibility
wrappers and forward to those generated installers. For those wrappers,
`LIBREPAPER_VERSION` pins a release; the old shell variable `LIBREPAPER_BIN_DIR` maps to
`LIBREPAPER_INSTALL_DIR`.

The installer places the executable on PATH. In a fresh terminal, run:

```sh
librepaper local settings
```

This opens the companion settings so you can configure and start the local
service. Installation does not create a desktop shortcut or register the
`librepaper://` link handler. Start at login is configured from companion
settings when available. Quarto, R/Python, and TeX must be installed
separately.

Cargo Dist also installs `librepaper-update` to update LibrePaper. It does not
update separately installed tools.

## Release transition note

The generated asset names become available after the first Cargo Dist release
tag. The currently published `v0.0.3` predates these assets.

Release configuration lives in `Cargo.toml`. After changing it or
`.github/dist-build-setup.yml`, regenerate the workflow with `dist generate`
using the pinned Cargo Dist version, then run `dist generate --check` and
`dist plan`. The release tag must match the application crate's version.
