#!/bin/sh
# Install komodoc: fetch the release binary for this machine and put it on the
# PATH. Nothing else is needed -- the tool is one static file.
#
#   curl -fsSL https://raw.githubusercontent.com/vincentarelbundock/komodoc/main/install.sh | sh
#
# Environment:
#   KOMODOC_VERSION   version to install, e.g. v0.0.1 (default: latest)
#   KOMODOC_BIN_DIR   where to put the binary (default: ~/.local/bin)
set -eu

REPO="vincentarelbundock/komodoc"
VERSION="${KOMODOC_VERSION:-latest}"
BIN_DIR="${KOMODOC_BIN_DIR:-$HOME/.local/bin}"

die() { printf 'install: %s\n' "$*" >&2; exit 1; }

# The release archives are named for the platform they were built for.
case "$(uname -s)" in
	Linux)   os=linux ;;
	Darwin)  os=darwin ;;
	*)       die "unsupported operating system $(uname -s); see github.com/$REPO/releases" ;;
esac
case "$(uname -m)" in
	x86_64|amd64)   arch=amd64 ;;
	arm64|aarch64)  arch=arm64 ;;
	*)              die "unsupported architecture $(uname -m); see github.com/$REPO/releases" ;;
esac

if command -v curl >/dev/null 2>&1; then
	fetch() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
	fetch() { wget -qO- "$1"; }
else
	die "neither curl nor wget is available"
fi

# "latest" redirects to the newest tag, whose name is the last path segment.
if [ "$VERSION" = latest ]; then
	VERSION=$(fetch "https://api.github.com/repos/$REPO/releases/latest" |
		sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
	[ -n "$VERSION" ] || die "could not determine the latest version"
fi

archive="komodoc_${os}_${arch}.tar.gz"
url="https://github.com/$REPO/releases/download/$VERSION/$archive"
checksums_url="https://github.com/$REPO/releases/download/$VERSION/checksums.txt"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

printf 'install: downloading komodoc %s (%s/%s)\n' "$VERSION" "$os" "$arch" >&2
# The archive is saved under its release name, not a generic temp name, so the
# line pulled out of checksums.txt below names a file that is actually there.
fetch "$url" > "$tmp/$archive" || die "download failed: $url"
fetch "$checksums_url" > "$tmp/checksums.txt" || die "download failed: $checksums_url"

# checksums.txt has one "<sha256>  <filename>" line per archive; pull out
# ours rather than trusting the whole file, so sha256sum/shasum only ever
# checks the one file this run downloaded.
line=$(awk -v f="$archive" '$2 == f { print; exit }' "$tmp/checksums.txt")
[ -n "$line" ] || die "no checksum for $archive in checksums.txt"
printf '%s\n' "$line" > "$tmp/checksums.txt.match"

if command -v sha256sum >/dev/null 2>&1; then
	verify() { (cd "$tmp" && sha256sum -c checksums.txt.match) >/dev/null 2>&1; }
elif command -v shasum >/dev/null 2>&1; then
	verify() { (cd "$tmp" && shasum -a 256 -c checksums.txt.match) >/dev/null 2>&1; }
else
	die "neither sha256sum nor shasum is available to verify the download"
fi
verify || die "checksum mismatch for $archive; the download may be corrupt or tampered with"

tar -xzf "$tmp/$archive" -C "$tmp" || die "the download was not a valid archive"
[ -f "$tmp/komodoc" ] || die "the archive did not contain a komodoc binary"

mkdir -p "$BIN_DIR"
mv "$tmp/komodoc" "$BIN_DIR/komodoc"
chmod +x "$BIN_DIR/komodoc"

printf 'install: komodoc %s -> %s/komodoc\n' "$VERSION" "$BIN_DIR" >&2

case ":$PATH:" in
	*":$BIN_DIR:"*) ;;
	*) printf 'install: %s is not on your PATH. Add it:\n\n    export PATH="%s:$PATH"\n\n' "$BIN_DIR" "$BIN_DIR" >&2 ;;
esac
