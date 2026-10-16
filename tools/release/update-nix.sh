#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/release-updater-common.sh"

tag="$(release_updater_validate_tag "${LIBREPAPER_RELEASE_TAG:-}")"

version="${tag#v}"
asset="librepaper_${version}_amd64.deb"
url="https://github.com/LibrePaper/librepaper/releases/download/${tag}/${asset}"
tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

hex="$(release_updater_fetch_verified_asset "${tag}" "${asset}" "${tmpdir}")"
# python3 is used instead of xxd, which is not always installed.
hash="sha256-$(python3 -c 'import base64, sys; print(base64.b64encode(bytes.fromhex(sys.argv[1])).decode())' "${hex}")"

# Pull first so the commit sits on top of the latest main.
git pull --rebase origin main

cat > nix/source.json <<JSON
{
  "version": "${version}",
  "url": "${url}",
  "hash": "${hash}"
}
JSON

release_updater_publish_file nix/source.json \
  "Nix source already up to date" "Nix: pin ${tag}" main
