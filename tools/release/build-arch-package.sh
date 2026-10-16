#!/usr/bin/env bash
# Builds an Arch package from the release .deb by repacking it with makepkg in an Arch container.
# Usage: build-arch-package.sh <version> <deb-path> <out-dir>
set -euo pipefail

if [[ $# != 3 ]]; then
  echo "usage: build-arch-package.sh <version> <deb-path> <out-dir>" >&2
  exit 2
fi

version="$1"
deb="$2"
out="$3"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

build="$(mktemp -d)"
trap 'rm -rf "$build"' EXIT

cp "$root/desktop/linux/arch/PKGBUILD" "$build/PKGBUILD"
cp "$root/LICENSE" "$build/LICENSE"
cp "$deb" "$build/librepaper_${version}_amd64.deb"
sed -i "s/^pkgver=.*/pkgver=${version}/" "$build/PKGBUILD"

# makepkg refuses to run as root, which is why it runs as builder.
# --nodeps skips WebKitGTK, because the package only repacks files.
docker run --rm -v "$build:/build" archlinux:base-devel bash -c '
  set -euo pipefail
  useradd -m builder
  chown -R builder /build
  cd /build
  su builder -c "makepkg --nodeps --noconfirm"
'

package="$build/librepaper-${version}-1-x86_64.pkg.tar.zst"
if [[ ! -f "$package" ]]; then
  echo "error: makepkg did not produce $(basename "$package")" >&2
  exit 1
fi
mkdir -p "$out"
cp "$package" "$out/"
echo "built $out/$(basename "$package")"
