---
title: "The desktop app"
---

The LibrePaper desktop app runs tools on your computer. Web browsers run every page in a sandbox to protect visitors, so a website cannot run code or read your local files. The desktop app does these things for the website, under your account.

The browser keeps writing, collaborating and reading. The features below run only in the desktop app.

## What it enables

- Computational notebooks: [Quarto](notebooks/quarto.html) and [Calepin](notebooks/calepin.html) run R, Python or Julia code inside a document, so a paper, report or slide deck computes its own tables and figures. The code runs on your computer with the tools installed there, optionally against a project folder on your disk.
- [AI agents](agents.html): start the coding agent you already have and hand it the document tools.
- [Local backups](backups.html): copy every project in your account to ZIP files in a folder you pick.
- [Zotero](https://www.zotero.org): cite from your local library. Access is read-only.
- [The CLI](cli.html): export project snapshots, and run or host a server from the same binary.

Nothing runs unasked. Folders are granted one at a time through your system's own chooser. Running a document's code is approved per document. On a shared project, editors can change the code that runs, so it is their code too.

## Install

The desktop app uses the `librepaper` binary. Pick one install method.

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

## Homebrew (macOS and Linux)

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

## Desktop app

The desktop app is built from source for now.

```sh
# From a checkout of the repository
make desktop                                   # build the app
desktop/target/release/librepaper-desktop      # start the app
```

- On NixOS, `make desktop` runs inside `nix-shell`.
- `LIBREPAPER_SERVER` points the app at another server. It replaces the default server.

```sh
# Start the app with another server
LIBREPAPER_SERVER=https://paper.example.org/ desktop/target/release/librepaper-desktop
```

## Start

```sh
librepaper --version                 # check the install
librepaper                           # start the companion in the background
librepaper install-desktop           # install a launcher that starts the companion
librepaper --at-login                # start at login too
librepaper status                    # address, tools, agents
```

On supported desktops, starting the companion shows a tray icon. Its **Settings** item opens the desktop app. If no desktop app is found, it opens your LibrePaper server in the browser. The tray appears automatically.

To stop the companion, run `librepaper stop`.

Folder selection uses your operating system's native chooser.

See [the CLI reference](cli.html#the-companion) for more commands. To build from a checkout, see [building from source](architecture/building.html).
