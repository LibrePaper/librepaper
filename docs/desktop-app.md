---
title: "Web and desktop app"
---

LibrePaper works in a web browser and in the LibrePaper desktop app. The desktop app is not an offline editor. It connects to the same LibrePaper server as the website, with the same account and the same projects. Several people can edit a project at once, whether each of them uses a browser or the desktop app, and you can switch between the two at any time.

Web browsers run every page in a sandbox, so a website cannot run code or use files on your computer. The desktop app does these things for you, on your computer, under your account.

## What works where

| Feature | Web browser | Desktop app |
| --- | --- | --- |
| Write and edit projects | ✅ | ✅ |
| Live preview: LaTeX and Typst to PDF, Markdown and Quarto to HTML | ✅ | ✅ |
| Real-time collaboration with other editors, comments and track changes ([Collaborate](collaborate.html)) | ✅ | ✅ |
| Share links for readers, commenters and editors | ✅ | ✅ |
| Version history | ✅ | ✅ |
| Download the project or the rendered document | ✅ | ✅ |
| BibTeX bibliographies | ✅ | ✅ |
| Run R, Python or Julia code in [Quarto](notebooks/quarto.html) and [Calepin](notebooks/calepin.html) | ❌ | ✅ |
| PDF, DOCX and slides from Markdown and Quarto | ❌ | ✅ |
| [AI agents](agents.html) in the sidebar | ❌ | ✅ |
| Cite from your [Zotero](https://www.zotero.org) library (read-only) | ❌ | ✅ |
| [Local backups](backups.html) of every project | ❌ | ✅ |
| A project folder on your disk for code and data | ❌ | ✅ |

- In a browser, the desktop-only features say they need the desktop app and offer to open it.
- Nothing runs unasked. Folders are granted one at a time through your system's own chooser. Running a document's code is off whenever a document opens, and you turn it on with the Run code switch.
- On a shared project, editors can change the code that runs, so it is their code too.
- The desktop app uses the tools installed on your computer, such as R, Python, Quarto and your coding agent.
- The window's title bar follows the same Theme setting (Settings > Appearance), and System follows the operating system.

The server you connect to decides what the desktop app runs. A document's code runs only when you turn on Run code, and the app still trusts its server. A compromised server could run code on your computer through the same tools. Connect the app only to a server you trust.

## Install the desktop app

Every package except the plain macOS `.dmg` puts two commands on your PATH: `librepaper` (the [command line](cli.html)) and `librepaper-desktop` (the app). You do not need the command line to write.

**macOS**

- With Homebrew:

```sh
brew install vincentarelbundock/tap/librepaper
```

- Or download `LibrePaper_<version>_universal.dmg` from the release, open it and drag LibrePaper to Applications. It runs on Apple Silicon and Intel Macs. This install does not put the commands on your PATH.

**Windows**

- With Scoop:

```sh
scoop bucket add vincentarelbundock https://github.com/vincentarelbundock/scoop-bucket
scoop install librepaper
```

- Or download `LibrePaper_<version>_x64-setup.exe` from the release. It is signed. It adds its folder to your user PATH, so open a new terminal to use both commands. Uninstalling removes the PATH entry.

**Linux**

- Debian and Ubuntu: download `librepaper_<version>_amd64.deb` from the release, then run:

```sh
sudo apt install ./librepaper_<version>_amd64.deb
sudo apt remove librepaper                        # remove it
```

- Fedora: download `librepaper-<version>-1.x86_64.rpm` from the release, then run:

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

### Updates

On macOS and with the Windows installer, the app checks for a new version when it starts and asks before it installs it. This includes an app installed with Homebrew. Scoop, the Linux packages and Nix update the app through their own commands.

### Windows

- Ctrl+N (Cmd+N on macOS) opens another window.
- Ctrl+W (Cmd+W on macOS) closes the window.
- Ctrl+, (Cmd+, on macOS) opens Settings.
- Starting the app again opens another window in the running app.
- A link to your server that opens in a new tab opens in a new window. Other links open in your browser.
- Closing the last window quits the app and stops the local service.

### Help

On macOS, the Help menu copies diagnostics, opens the issue tracker and checks for updates. On every system, Settings, Desktop app, Diagnostics copies the same report.

To build from a checkout, see [building from source](architecture/building.html).
