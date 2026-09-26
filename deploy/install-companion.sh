#!/bin/sh
# Compatibility entry point for saved companion-install commands. Cargo Dist
# owns archive selection, verification, and installation.
set -eu

repo=LibrePaper/librepaper
version=${LIBREPAPER_VERSION:-}
die() { printf 'librepaper companion installer: %s\n' "$*" >&2; exit 1; }

if command -v curl >/dev/null 2>&1; then
	fetch() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
	fetch() { wget -qO- "$1"; }
else
	die 'neither curl nor wget is available'
fi

if [ -n "${LIBREPAPER_BIN_DIR:-}" ]; then
	export LIBREPAPER_INSTALL_DIR="$LIBREPAPER_BIN_DIR"
fi
installer=librepaper-installer.sh
if [ -z "$version" ] || [ "$version" = latest ]; then
	unset LIBREPAPER_VERSION
	url="https://github.com/$repo/releases/latest/download/$installer"
else
	case "$version" in
		*[!A-Za-z0-9._-]*) die "invalid release version: $version" ;;
	esac
	export LIBREPAPER_VERSION="$version"
	url="https://github.com/$repo/releases/download/$version/$installer"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
fetch "$url" > "$tmp/$installer" || die "could not download $url"
sh "$tmp/$installer" "$@" || exit $?
