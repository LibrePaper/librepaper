#!/bin/sh
# Install a release companion, register its launcher, and start it once.
# Startup at login is optional and can be enabled in companion settings.
set -eu
repo=LibrePaper/librepaper
BIN_DIR="${LIBREPAPER_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$BIN_DIR"
BIN_DIR=$(CDPATH='' cd -- "$BIN_DIR" && pwd)
export LIBREPAPER_BIN_DIR="$BIN_DIR"
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO- "$1"; }
else
    printf '%s\n' 'Install curl or wget, then try again.' >&2; exit 1
fi
version="${LIBREPAPER_VERSION:-latest}"
if [ "$version" = latest ]; then
    version=$(fetch "https://api.github.com/repos/$repo/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
fi
[ -n "$version" ] || { printf '%s\n' 'Could not identify a release.' >&2; exit 1; }
export LIBREPAPER_VERSION="$version"
release="https://github.com/$repo/releases/download/$version"
companion_tmp=$(mktemp -d)
trap 'rm -rf "$companion_tmp"' EXIT
fetch "$release/checksums.txt" > "$companion_tmp/checksums.txt"
verified_download() {
    asset="$1"
    fetch "$release/$asset" > "$companion_tmp/$asset"
    awk -v f="$asset" '$2 == f { print; found=1; exit } END { if (!found) exit 1 }' "$companion_tmp/checksums.txt" > "$companion_tmp/checksum.match"
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$companion_tmp" && sha256sum -c checksum.match)
    else
        (cd "$companion_tmp" && shasum -a 256 -c checksum.match)
    fi
}
case "$(uname -s)" in
    Darwin)
        case "$(uname -m)" in arm64|aarch64) arch=arm64;; x86_64|amd64) arch=amd64;; *) exit 1;; esac
        verified_download "librepaper_darwin_${arch}.app.zip"
        mkdir -p "$HOME/Applications"
        app="$HOME/Applications/LibrePaper Companion.app"
        if [ -x "$app/Contents/Resources/librepaper" ]; then "$app/Contents/Resources/librepaper" local stop; fi
        unzip -oq "$companion_tmp/librepaper_darwin_${arch}.app.zip" -d "$HOME/Applications"
        executable="$app/Contents/Resources/librepaper"
        /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"
        "$executable" local launch
        if [ "${LIBREPAPER_AUTOSTART:-0}" = 1 ]; then "$executable" local startup enable; fi
        open "$app"
        ;;
    Linux)
        verified_download install.sh
        if [ -x "$BIN_DIR/librepaper" ]; then "$BIN_DIR/librepaper" local stop; fi
        sh "$companion_tmp/install.sh"
        executable="$BIN_DIR/librepaper"
        desktop_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
        mkdir -p "$desktop_dir"
        # Desktop Entry Exec quoting has two levels: the string escape and
        # the quoted argument escape. Percent is a field-code delimiter.
        # The sed expression escapes literal dollars.
        # shellcheck disable=SC2016
        quoted=$(printf '%s' "$executable" | sed 's/\\/\\\\\\\\/g; s/"/\\\\"/g; s/`/\\\\`/g; s/\$/\\\\$/g; s/%/%%/g')
        cat > "$desktop_dir/librepaper-local.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=LibrePaper connection
Exec="$quoted" local open %u
Terminal=false
NoDisplay=true
MimeType=x-scheme-handler/librepaper;
DESKTOP
        cat > "$desktop_dir/librepaper-local-manage.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=LibrePaper companion
Comment=Local rendering settings and permissions
Exec="$quoted" local manage
Terminal=false
Categories=Office;
DESKTOP
        if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$desktop_dir"; fi
        if command -v xdg-mime >/dev/null 2>&1; then xdg-mime default librepaper-local.desktop x-scheme-handler/librepaper; fi
        "$executable" local launch
        if [ "${LIBREPAPER_AUTOSTART:-0}" = 1 ]; then "$executable" local startup enable; fi
        "$executable" local manage
        ;;
    *) printf '%s\n' 'Use the Windows companion setup from the release page.' >&2; exit 1;;
esac
printf '%s\n' 'Companion installed. Return to LibrePaper and enable local rendering.'
