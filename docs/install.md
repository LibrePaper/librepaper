---
title: "Install"
---

Reading, commenting and publishing happen in the browser and need no install. Install `librepaper` to run the companion (local Quarto, Typst and TeX tools, coding agents, backups), to use the CLI, or to host a server. It is one binary; pick one method.

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
librepaper --at-login                # also start it every time you log in
librepaper status                    # address, pairings, tools and agents found
```

Then open *Settings*, *Local app* in LibrePaper and click **Connect**. No install method adds a desktop shortcut or registers a `librepaper://` link handler: you start the companion from a terminal.

More commands are in [the CLI reference](cli.html#the-companion). To build from a checkout, see [building from source](architecture.html#building-from-source).
