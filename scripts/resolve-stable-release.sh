#!/usr/bin/env bash
set -euo pipefail

dispatch_tag="${DISPATCH_TAG:-}"
release_sha="${RELEASE_SHA:-}"
run_head_branch="${RUN_HEAD_BRANCH:-}"

if [[ -n "${dispatch_tag}" ]]; then
  tag="${dispatch_tag}"
  sha="$(git rev-parse "refs/tags/${tag}^{commit}" 2>/dev/null || true)"
else
  sha="${release_sha}"
  tag="${run_head_branch#refs/tags/}"
fi

if ! source "$(dirname "${BASH_SOURCE[0]}")/release-updater-common.sh"; then
  exit 1
fi
if ! release_updater_validate_tag "${tag}" >/dev/null; then
  echo "No stable v-prefixed semver release tag found; skipping."
  printf 'proceed=false\n' >> "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
elif [[ -z "${sha}" || "$(git rev-parse "refs/tags/${tag}^{commit}" 2>/dev/null || true)" != "${sha}" ]]; then
  echo "Release tag does not resolve to the triggering commit; skipping."
  printf 'proceed=false\n' >> "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
else
  printf 'tag=%s\nproceed=true\n' "${tag}" >> "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
fi
