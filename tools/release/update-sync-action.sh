#!/usr/bin/env bash
# Publish tools/sync-action/ to LibrePaper/sync-action after a stable release.
set -euo pipefail

: "${SYNC_ACTION_GITHUB_TOKEN:?SYNC_ACTION_GITHUB_TOKEN is required}"
source "$(dirname "${BASH_SOURCE[0]}")/release-updater-common.sh"

tag="$(release_updater_validate_tag "${LIBREPAPER_RELEASE_TAG:-${GITHUB_REF_NAME:-}}")"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

# The Action downloads this asset, so the release must have it before the
# Action's default version points at the tag.
release_updater_fetch_verified_asset "${tag}" librepaper-x86_64-unknown-linux-musl.tar.xz "${tmpdir}" >/dev/null

# The checkout is the workflow revision, not the tag. Read the Action files as
# they were at the release.
files=(action.yml sync.sh workflow.yml README.md)
git fetch --depth=1 origin "refs/tags/${tag}:refs/tags/${tag}"
mkdir -p "${tmpdir}/source"
for file in "${files[@]}"; do
  git show "${tag}:tools/sync-action/${file}" > "${tmpdir}/source/${file}"
done

# The default of the version input is the first default: line under version:.
pin="$(awk '/^  version:/ { in_version = 1; next } in_version && /^  [^ #]/ { exit } in_version && /^    default:/ { print $2; exit }' "${tmpdir}/source/action.yml")"
if [[ "${pin}" != "${tag}" ]]; then
  echo "action.yml pins ${pin:-<none>} but the release is ${tag}; run tools/release/version" >&2
  exit 1
fi

release_updater_configure_github_auth "${SYNC_ACTION_GITHUB_TOKEN}"
action_dir="${tmpdir}/sync-action"
git clone https://github.com/LibrePaper/sync-action.git "${action_dir}"
cd "${action_dir}"

# Leave LICENSE and anything else in the repository alone.
for file in "${files[@]}"; do
  cp "${tmpdir}/source/${file}" "${file}"
done
git add "${files[@]}"
if git diff --cached --quiet; then
  echo "sync-action already up to date"
  exit 0
fi

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git commit -m "Sync from LibrePaper/librepaper ${tag}"

# Each publish gets an immutable v1.<minor>.0 tag, and the floating v1 moves to
# it. The floating major only ever moves within v1. A breaking change to the
# Action's inputs needs a person to cut v2.
# The highest v1 minor, whatever order the tags come in. -1 when there is none.
minor="$(git tag -l 'v1.*' | awk -F. '/^v1\.[0-9]+\.[0-9]+$/ && (!seen || $2 + 0 > max) { max = $2 + 0; seen = 1 } END { print (seen ? max : -1) }')"
next="v1.$((minor + 1)).0"
git tag "${next}"
git tag -f v1
git push origin HEAD:main "refs/tags/${next}"
git push --force origin refs/tags/v1
