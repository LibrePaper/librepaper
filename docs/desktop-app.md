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

## How the desktop app works

### Nothing else to start

The desktop app starts its own background local service when it opens. You do not start, stop or pair the local service.

### Trust

Nothing runs unasked. Folders are granted one at a time through your system's own chooser, and a document's code runs only after you turn on the Run code switch. On a shared project, editors can change that code, so it is their code too. The app still trusts the server it connects to: a compromised server could run code on your computer through the same tools. Connect the app only to a server you trust.

### Updates

On macOS and with the Windows installer, the app checks for a new version when it starts and asks before it installs it. This includes an app installed with Homebrew. Scoop, the Linux packages and Nix update the app through their own commands.

### Windows

- Ctrl+N (Cmd+N on macOS) opens another window.
- Ctrl+W (Cmd+W on macOS) closes the window.
- Ctrl+, (Cmd+, on macOS) opens Settings.
- Starting the app again shows the windows hidden in the tray, or opens another window when one is already showing.
- A link to your server that opens in a new tab opens in a new window. Other links open in your browser.
- Closing the last window keeps the app running in the system tray (the menu bar on macOS). To quit instead, set Settings, General, When the window closes to Quit LibrePaper.
- The tray menu shows the window again, opens a new window or quits the app.
- Quit in the tray (or the File menu) stops the local service.
- On Linux, the tray needs AppIndicator support (on GNOME, the AppIndicator extension). Without it, closing the last window always quits the app.

### Help

On macOS, the Help menu copies diagnostics, opens the issue tracker and checks for updates. On every system, Settings, General, Diagnostics copies the same report.

If a window stays blank, quit the app and start it from a terminal with `librepaper-desktop --debug` (on macOS the binary is inside the app bundle, under Contents/MacOS). The web inspector opens with the window, and on Linux it is also served at `http://127.0.0.1:9222/` for any browser. The desktop log, whose path is in the diagnostics report, records when the part of the app that draws the page crashes. Send both with your report.

To build from a checkout, see [building from source](architecture/building.html).
