---
title: "Install desktop app"
---

LibrePaper works in any web browser. The desktop app connects to the same server, with the same account and the same projects, and adds what a browser sandbox cannot do: run your R and Python code, work with [AI agents](agents.html), pull citations from [Zotero](https://www.zotero.org), sync with [Git](git.html), keep [local backups](backups.html) and render with your own latexmk, Typst or Quarto. The [feature table](index.html#features) lists what works where.

The desktop app is not an offline editor. You can switch between the browser and the app at any time, and collaborators can use either one.

## Install the desktop app

Download the installer from the [latest release on GitHub](https://github.com/LibrePaper/librepaper/releases/latest):

- **macOS:** `LibrePaper_<version>_universal.dmg`, for Apple Silicon and Intel Macs.
- **Windows:** `LibrePaper_<version>_x64-setup.exe`, signed.
- **Linux:** `librepaper_<version>_amd64.deb` for Debian and Ubuntu, or `librepaper-<version>-1.x86_64.rpm` for Fedora and openSUSE.

Package managers are covered below. Every package except the plain macOS `.dmg` puts two commands on your PATH: `librepaper` (the [command line](cli.html)) and `librepaper-desktop` (the app). You do not need the command line to write.

**macOS**

- With Homebrew:

```sh
brew install vincentarelbundock/tap/librepaper
```

- Or download `LibrePaper_<version>_universal.dmg` from the [latest release](https://github.com/LibrePaper/librepaper/releases/latest), open it and drag LibrePaper to Applications. It runs on Apple Silicon and Intel Macs. This install does not put the commands on your PATH.

**Windows**

- With Scoop:

```sh
scoop bucket add vincentarelbundock https://github.com/vincentarelbundock/scoop-bucket
scoop install librepaper
```

- Or download `LibrePaper_<version>_x64-setup.exe` from the [latest release](https://github.com/LibrePaper/librepaper/releases/latest). It is signed. It adds its folder to your user PATH, so open a new terminal to use both commands. Uninstalling removes the PATH entry.

**Linux**

- Debian and Ubuntu: download `librepaper_<version>_amd64.deb` from the [latest release](https://github.com/LibrePaper/librepaper/releases/latest), then run:

```sh
sudo apt install ./librepaper_<version>_amd64.deb
sudo apt remove librepaper                        # remove it
```

- Fedora: download `librepaper-<version>-1.x86_64.rpm` from the [latest release](https://github.com/LibrePaper/librepaper/releases/latest), then run:

```sh
sudo dnf install ./librepaper-<version>-1.x86_64.rpm
sudo dnf remove librepaper                        # remove it
```

- openSUSE: use the same `.rpm` file, then run:

```sh
sudo zypper install ./librepaper-<version>-1.x86_64.rpm
```

- Nix (x86_64 Linux only):

```sh
nix profile install github:LibrePaper/librepaper    # install
nix run github:LibrePaper/librepaper                # or try it without installing
```

- Open LibrePaper from your applications menu, or run `librepaper-desktop` in a terminal.

- To build from source:

```sh
# From a checkout of the repository
make desktop                                   # build the app and the librepaper binary it bundles
desktop/target/release/librepaper-desktop      # start the app
```

- On NixOS, `make desktop` runs inside `nix-shell`.
- `LIBREPAPER_SERVER` points the app at another server. It replaces the default server.

```sh
# Start the app with another server
LIBREPAPER_SERVER=https://paper.example.org/ desktop/target/release/librepaper-desktop
```
