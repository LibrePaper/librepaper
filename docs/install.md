---
title: "Install"
---

Install `librepaper` to run the companion, use the CLI, or host a server. It is one binary; pick one method.

## Installer script

```sh
# macOS and Linux, x86_64 and aarch64
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

```powershell
# Windows, x86_64
powershell -ExecutionPolicy Bypass -c "irm https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1 | iex"
```

- Installs to `~/.cargo/bin` and adds it to your PATH. Open a new terminal afterwards.
- `LIBREPAPER_INSTALL_DIR` chooses another directory.
- `LIBREPAPER_NO_MODIFY_PATH=1` leaves your PATH alone.
- To pin a version, replace `releases/latest/download/` with `releases/download/<tag>/`.
- Update later with `librepaper-update`.

## Homebrew (macOS)

```sh
# macOS and Linux
brew install vincentarelbundock/tap/librepaper
```

## Scoop (Windows)

```powershell
# Windows
scoop bucket add vincentarelbundock https://github.com/vincentarelbundock/scoop-bucket
scoop install librepaper
```

## Cargo

```sh
# Any platform with a Rust toolchain; builds from source
cargo install librepaper
```

## Start the companion

```sh
librepaper --version                 # check the install
librepaper                           # start the companion in the background
librepaper desktop                   # open its private dashboard
librepaper install-desktop           # install an optional app launcher
librepaper --at-login                # start at login too
librepaper status                    # address, pairings, tools, agents
```

The dashboard lets you approve site requests, review local access, see tool status and change settings. Closing its browser tab does not stop the companion; choose **Quit companion** in the dashboard or run `librepaper stop`. The optional tray icon depends on desktop-environment support. Folder selection uses your operating system's native chooser. On a headless machine, pairing approval falls back to a code shown in the terminal; approve it with `librepaper local approve <code>` within five minutes.

See [the CLI reference](cli.html#the-companion) for more commands. To build from a checkout, see [building from source](architecture/building.html).
