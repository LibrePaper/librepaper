#!/usr/bin/env bash
set -euo pipefail

dispatch_tag="${DISPATCH_TAG:-}"
release_sha="${RELEASE_SHA:-}"
run_head_branch="${RUN_HEAD_BRANCH:-}"
output="${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"

source "$(dirname "${BASH_SOURCE[0]}")/release-updater-common.sh"

skip() {
  echo "$1"
  printf 'proceed=false\n' >> "${output}"
}

if [[ -n "${dispatch_tag}" ]]; then
  tag="${dispatch_tag}"
else
  tag="${run_head_branch#refs/tags/}"
fi

if ! release_updater_validate_tag "${tag}" >/dev/null; then
  skip "No stable v-prefixed semver release tag found; skipping."
  exit 0
fi

# The checkout stays on github.workflow_sha for trusted helper code. Fetch only
# the candidate tag so old releases need not contain current updater scripts.
if ! git fetch --no-tags origin "+refs/tags/${tag}:refs/tags/${tag}"; then
  skip "Release tag ${tag} is missing; skipping."
  exit 0
fi

tag_sha="$(git rev-parse "refs/tags/${tag}^{commit}" 2>/dev/null || true)"
if [[ -z "${tag_sha}" ]]; then
  skip "Release tag ${tag} does not resolve to a commit; skipping."
elif [[ -n "${dispatch_tag}" ]]; then
	printf 'tag=%s\nsha=%s\nproceed=true\n' "${tag}" "${tag_sha}" >> "${output}"
elif [[ -z "${release_sha}" || "${tag_sha}" != "${release_sha}" ]]; then
  skip "Release tag does not resolve to the triggering commit; skipping."
else
	printf 'tag=%s\nsha=%s\nproceed=true\n' "${tag}" "${tag_sha}" >> "${output}"
fi
