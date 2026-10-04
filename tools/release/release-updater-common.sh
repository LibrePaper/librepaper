#!/usr/bin/env bash
# Shared release validation and publication primitives for package updaters.

release_updater_validate_tag() {
  local tag="${1:-}"
  if [[ ! "${tag}" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    echo "expected a stable v-prefixed semver release tag, got: ${tag:-<empty>}" >&2
    return 1
  fi
  printf '%s' "${tag}"
}

# Fetch an archive and its sidecar into a caller-owned temporary directory,
# rejecting malformed or mismatched checksums before package metadata is made.
release_updater_fetch_verified_asset() {
  local tag="$1" asset="$2" directory="$3"
  local url="https://github.com/LibrePaper/librepaper/releases/download/${tag}"
  local archive="${directory}/${asset}" checksum="${directory}/${asset}.sha256"
  local declared actual
  curl -fsSL "${url}/${asset}" -o "${archive}"
  curl -fsSL "${url}/${asset}.sha256" -o "${checksum}"
  declared="$(awk 'NR == 1 { print $1 }' "${checksum}")"
  if ! [[ "${declared}" =~ ^[[:xdigit:]]{64}$ ]]; then
    echo "invalid SHA-256 checksum for ${asset}" >&2
    return 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${archive}" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "${archive}" | awk '{print $1}')"
  fi
  if [[ "${actual,,}" != "${declared,,}" ]]; then
    echo "SHA-256 mismatch for ${asset}" >&2
    return 1
  fi
  printf '%s' "${actual}"
}

# Keep credentials out of clone URLs and process arguments.
release_updater_configure_github_auth() {
  local token="$1"
  export GIT_CONFIG_COUNT=1
  export GIT_CONFIG_KEY_0=http.https://github.com/.extraheader
  export GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $(printf 'x-access-token:%s' "${token}" | base64 | tr -d '\n')"
}

release_updater_publish_file() {
  local path="$1" no_change="$2" commit_message="$3" remote_ref="$4"
  git add "${path}"
  if git diff --cached --quiet; then
    echo "${no_change}"
    return 0
  fi
  git config user.name "github-actions[bot]"
  git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
  git commit -m "${commit_message}"
  git push origin "HEAD:${remote_ref}"
}
