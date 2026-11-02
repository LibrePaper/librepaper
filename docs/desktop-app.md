---
title: "Install desktop app"
---

LibrePaper works in any web browser. The desktop app connects to the same server, with the same account and the same projects, and adds what a browser sandbox cannot do: run your R and Python code, work with [AI agents](agents.html), pull citations from [Zotero](https://www.zotero.org), sync with [Git](git.html), keep [local backups](backups.html) and render with your own latexmk, Typst or Quarto. The [feature table](index.html#features) lists what works where.

The desktop app is not an offline editor. You can switch between the browser and the app at any time, and collaborators can use either one.

Every install except the plain macOS `.dmg` puts two commands on your PATH: `librepaper` (the [command line](cli.html)) and `librepaper-desktop` (the app). You do not need the command line to write. Once installed, open LibrePaper from your applications menu, or run `librepaper-desktop` in a terminal.

## Binaries

Download the installer from the [latest release on GitHub](https://github.com/LibrePaper/librepaper/releases/latest):

- **macOS:** `LibrePaper_<version>_universal.dmg`, for Apple Silicon and Intel Macs. Open it and drag LibrePaper to Applications. This install does not put the commands on your PATH.
- **Windows:** `LibrePaper_<version>_x64-setup.exe`, signed. It adds its folder to your user PATH, so open a new terminal to use both commands. Uninstalling removes the PATH entry.
- **Linux:** `librepaper_<version>_amd64.deb` for Debian and Ubuntu, or `librepaper-<version>-1.x86_64.rpm` for Fedora and openSUSE. Install it with your package manager.

```sh
sudo apt install ./librepaper_<version>_amd64.deb            # Debian and Ubuntu
sudo dnf install ./librepaper-<version>-1.x86_64.rpm         # Fedora
sudo zypper install ./librepaper-<version>-1.x86_64.rpm      # openSUSE
```

## Package managers

Homebrew on macOS, Scoop on Windows and Nix on x86_64 Linux install the app and keep it up to date with their own commands.

```sh
# macOS
brew install vincentarelbundock/tap/librepaper

# Windows
scoop bucket add vincentarelbundock https://github.com/vincentarelbundock/scoop-bucket
scoop install librepaper

# Nix
nix profile install github:LibrePaper/librepaper
nix run github:LibrePaper/librepaper              # or try it without installing
```

## Shell script

The release also has install scripts. They install the `librepaper` command without the desktop app, for a server or a machine where you only script documents. See the [command line](cli.html) page.

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1 | iex"
```

## Source

From a checkout of the repository, `make desktop` builds the app and the `librepaper` binary it bundles. On NixOS, it runs inside `nix-shell`. `LIBREPAPER_SERVER` points the app at another server instead of the default one.

```sh
make desktop                                   # build
desktop/target/release/librepaper-desktop      # start the app
LIBREPAPER_SERVER=https://paper.example.org/ desktop/target/release/librepaper-desktop
```
