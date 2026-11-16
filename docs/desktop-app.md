---
title: "Install desktop app"
---

The desktop app connects to the same server as the browser, with the same account and projects. It adds what a browser cannot do: run your R and Python code, work with [AI agents](agents.html), pull citations from [Zotero](https://www.zotero.org), sync with [Git](git.html), keep [local backups](backups.html), and render with your own latexmk, Typst or Quarto. The [feature table](index.html#features) lists what works where.

The desktop app is not an offline editor. You can switch between the browser and the app at any time, and collaborators can use either one.

## Install the desktop app

Download the installer from the [latest release on GitHub](https://github.com/LibrePaper/librepaper/releases/latest):

- **macOS:** `LibrePaper_<version>_universal.dmg`, for Apple Silicon and Intel Macs. Open it and drag LibrePaper to Applications. This install does not put the commands on your PATH.
- **Windows:** `LibrePaper_<version>_x64-setup.exe`, signed. It adds its folder to your user PATH, so open a new terminal to use both commands. Uninstalling removes the PATH entry.
- **Linux:** `librepaper_<version>_amd64.deb` for Debian and Ubuntu, or `librepaper-<version>-1.x86_64.rpm` for Fedora and openSUSE.

```sh
sudo apt install ./librepaper_<version>_amd64.deb            # Debian and Ubuntu
sudo dnf install ./librepaper-<version>-1.x86_64.rpm         # Fedora
sudo zypper install ./librepaper-<version>-1.x86_64.rpm      # openSUSE
```

Package managers install the app and keep it up to date with their own commands:

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

On x86_64 Linux, you can also install from the [APT](https://vincentarelbundock.github.io/apt) or [DNF](https://vincentarelbundock.github.io/rpm) repository. Their pages give the setup commands. Then run `sudo apt install librepaper` or `sudo dnf install librepaper`.

Once installed, open LibrePaper from your applications menu, or run `librepaper-desktop` in a terminal. Every install except the plain macOS `.dmg` also puts the [`librepaper` command](cli.html) on your PATH. You do not need it to write.

The app checks for updates at startup and when its window regains focus, at most every six hours. It installs updates itself on macOS and on Windows installs made by the setup program. On Linux, and for Windows copies from Scoop or a portable zip, update with your package manager or from the download page.

## Other servers

The app opens app.librepaper.org by default. It works only with servers you choose, because a server can use this computer's tools through the app, such as running code or latexmk. To use a self-hosted server, click **Change server** on the sign-in page, or start the app with `librepaper-desktop --server https://paper.example.org/`.

If a link says **Open in the desktop app** and points to a server you have not chosen, the app asks whether to use that server. Choose it only if you trust it, because it then becomes your default server. If you cancel, the app says the link was not opened.

If two copies of the app are installed, for example a portable copy and an installed one, links open in whichever copy is already running.

## Build from source

From a checkout of the repository, `make desktop` builds the app and the `librepaper` binary it bundles. On NixOS, the build runs inside `nix-shell`. Set `LIBREPAPER_SERVER` to point the app at another server instead of the default one.

```sh
make desktop                                   # build
desktop/target/release/librepaper-desktop      # start the app
LIBREPAPER_SERVER=https://paper.example.org/ desktop/target/release/librepaper-desktop
```
