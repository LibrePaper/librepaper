# Companion installers

Release tags publish the platform archives plus installer entry points:

- Linux/macOS: `install-companion.sh` (curl or wget)
- Windows: `install-companion.cmd` or `install-companion.ps1`
- macOS: `librepaper_darwin_<arch>.app.zip`

Installers verify downloaded archives against the release checksums, register `librepaper://connect`, launch the companion, and leave login
startup opt-in. The macOS app is ad-hoc signed; public distribution with Gatekeeper trust
requires Developer ID signing and notarization in the release environment.

The desktop settings entry invokes `librepaper local manage`.

On Linux, save `install-companion.sh` and run `sh install-companion.sh` once.
The installer adds a LibrePaper companion entry to the application menu.
On macOS, unzip the matching app archive, move the app to `~/Applications`,
and open it. On Windows, open `install-companion.cmd`; it downloads the
matching PowerShell installer and creates a companion settings shortcut.

Use **Start at login** in companion settings to avoid launching it manually.
The editor's **Open companion** button also launches a stopped installation.
Quarto, R/Python, and TeX are detected, not installed or updated by these installers.

For an update, quit the running companion before replacing the macOS app.
The scripted installers stop the previous companion before replacing its binary.
Permissions live separately from the application and survive updates.
