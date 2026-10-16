---
title: "Web and desktop app"
---

LibrePaper works in a web browser and in the LibrePaper desktop app. The desktop app is not an offline editor. It connects to the same LibrePaper server as the website, with the same account and the same projects. Several people can edit a project at once, whether each of them uses a browser or the desktop app, and you can switch between the two at any time.

Web browsers run every page in a sandbox, so a website cannot run code or use files on your computer. The desktop app does these things for you, on your computer, under your account.

## What works where

| Feature | Web browser | Desktop app |
| --- | --- | --- |
| Write and edit projects | Yes | Yes |
| Live preview: LaTeX and Typst to PDF, Markdown and Quarto to HTML | Yes | Yes |
| Real-time collaboration with other editors, comments and track changes ([Collaborate](collaborate.html)) | Yes | Yes |
| Share links for readers, commenters and editors | Yes | Yes |
| Version history | Yes | Yes |
| Download the project or the rendered document | Yes | Yes |
| BibTeX bibliographies | Yes | Yes |
| Run R, Python or Julia code in [Quarto](notebooks/quarto.html) and [Calepin](notebooks/calepin.html) | No | Yes |
| PDF, DOCX and slides from Markdown and Quarto | No | Yes |
| [AI agents](agents.html) in the sidebar | No | Yes |
| Cite from your [Zotero](https://www.zotero.org) library (read-only) | No | Yes |
| [Local backups](backups.html) of every project | No | Yes |
| A project folder on your disk for code and data | No | Yes |

- In a browser, the desktop-only features say they need the desktop app and offer to open it.
- Nothing runs unasked. Folders are granted one at a time through your system's own chooser. Running a document's code is off whenever a document opens, and you turn it on with the Run code switch.
- On a shared project, editors can change the code that runs, so it is their code too.
- The desktop app uses the tools installed on your computer, such as R, Python, Quarto and your coding agent.

## Install the desktop app

- Windows with Scoop:

```sh
scoop bucket add vincentarelbundock https://github.com/vincentarelbundock/scoop-bucket
scoop install librepaper
```

- Windows installer: download `LibrePaper_<version>_x64-setup.exe` from the GitHub release. It is signed.
- Debian and Ubuntu: download `LibrePaper_<version>_amd64.deb` from the release, then run:

```sh
sudo apt install ./LibrePaper_<version>_amd64.deb
```

- Fedora and openSUSE: download `LibrePaper-<version>-1.x86_64.rpm` from the release, then run:

```sh
sudo dnf install ./LibrePaper-<version>-1.x86_64.rpm       # Fedora
sudo zypper install ./LibrePaper-<version>-1.x86_64.rpm    # openSUSE
```

- The Scoop package, the Debian package and the RPM package also install the [`librepaper` command line](cli.html), which hosts servers and runs scripts. You do not need it to write.
- On Linux, open LibrePaper from your applications menu, or run `librepaper-desktop` in a terminal.
- The Linux package is named `libre-paper`: remove it with `sudo apt remove libre-paper` or `sudo dnf remove libre-paper`.
- macOS: download `LibrePaper_<version>_universal.dmg` from the release, open it and drag LibrePaper to Applications. It runs on Apple Silicon and Intel Macs.
- macOS with Homebrew:

```sh
brew install --cask vincentarelbundock/tap/librepaper
```
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

On macOS and with the Windows installer, the app checks for a new version when it starts and asks before it installs it. This includes an app installed with Homebrew. Scoop and the Linux packages update the app through their own commands.

### Windows

- Ctrl+N (Cmd+N on macOS) opens another window.
- Starting the app again opens another window in the running app.
- A link to your server that opens in a new tab opens in a new window. Other links open in your browser.
- Closing the last window quits the app and stops the local service.

To build from a checkout, see [building from source](architecture/building.html).
