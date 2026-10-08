---
title: "The companion"
---

Web browsers run every page in a sandbox to protect visitors: a website cannot read your local files or control your computer.

In LibrePaper it is often useful to reach past that sandbox. A Quarto document needs the R or Python installed on your machine, your AI agent lives in your terminal, and a backup belongs in a folder on your disk. The companion is one small program that does these things for the website, on your machine and under your account, only for the sites and folders you approve.

## What it enables

- Computational notebooks: [Quarto](notebooks/quarto.html) and [Calepin](notebooks/calepin.html) are two applications that allow users to run R, Python or Julia code inside a document, so a paper, report or slide deck computes its own tables and figures. The browser cannot run code. The companion runs it on your computer with the tools installed there, optionally against a project folder on your disk.
- [AI agents](agents.html): start the coding agent you already have and hand it the document tools.
- [Local backups](backups.html): copy every project in your account to ZIP files in a folder you pick.
- [Zotero](https://www.zotero.org): cite from your local library. Access is read-only.
- [The CLI](cli.html): export project snapshots, and run or host a server from the same binary.

Nothing runs unasked. A site pairs with the companion per origin and project, folders are granted one at a time through your system's own chooser, running a document's code is approved per document, and Settings shows what is connected and running. On a shared project, editors can change the code that runs, so it is their code too.

## Install

LibrePaper is one binary. Pick one install method.

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

## Start

```sh
librepaper --version                 # check the install
librepaper                           # start the companion in the background
librepaper install-desktop           # install a launcher that starts the companion
librepaper --at-login                # start at login too
librepaper status                    # address, pairings, tools, agents
```

On supported desktops, starting the companion shows a tray icon. Choose **Settings** from its menu to open the Companion section in the main LibrePaper app, where you can approve site requests, review local access, see tool status and change settings. The tray appears automatically. There is no tray preference to configure. Closing the app page does not stop the companion. Choose **Quit companion** in Settings or run `librepaper stop`. Folder selection uses your operating system's native chooser. On a headless machine, pairing approval falls back to a code shown in the terminal. Approve it with `librepaper local approve <code>` within two minutes.

See [the CLI reference](cli.html#the-companion) for more commands. To build from a checkout, see [building from source](architecture/building.html).
