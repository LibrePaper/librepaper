#!/usr/bin/env bash
# Syncs one LibrePaper project with the repository checked out here.
# action.yml sets PROJECT, LIBREPAPER_TOKEN, LIBREPAPER_VERSION, BRANCH and
# GITHUB_TOKEN (for gh). The runner sets GITHUB_EVENT_NAME.
set -euo pipefail

: "${PROJECT:?PROJECT is required}"
: "${LIBREPAPER_TOKEN:?the token is required}"
: "${LIBREPAPER_VERSION:?the version is required}"
: "${BRANCH:?the branch is required}"

# Scratch space for the CLI and its output. On every exit, the LibrePaper
# state under refs/librepaper/* goes to origin so the next runner has it.
# The state records which web versions a pull merged into the branch, so it
# goes out only when that branch went out too. A failed branch push clears
# PUBLISH_STATE. Otherwise the next runner would start from the older branch,
# never merge those versions again, and a later push could send the older
# files over the web's changes.
PUBLISH_STATE=1
WORK=$(mktemp -d)
LOG="$WORK/cli.log"
finish() {
  if [[ "$PUBLISH_STATE" -eq 1 && -n "$(git for-each-ref refs/librepaper/)" ]]; then
    git push --quiet origin '+refs/librepaper/*:refs/librepaper/*' ||
      echo "::warning::could not push refs/librepaper/* to origin"
  fi
  rm -rf "$WORK"
}
trap finish EXIT

# Download the linux x86_64 archive of the pinned release. Check it against
# the release's .sha256 file when the release has one (cargo-dist writes one).
asset="librepaper-x86_64-unknown-linux-musl.tar.xz"
base="https://github.com/LibrePaper/librepaper/releases/download/$LIBREPAPER_VERSION"
curl -fsSL -o "$WORK/$asset" "$base/$asset"
if curl -fsSL -o "$WORK/$asset.sha256" "$base/$asset.sha256"; then
  expected=$(awk 'NF { print $1; exit }' "$WORK/$asset.sha256")
  actual=$(sha256sum "$WORK/$asset" | awk '{ print $1 }')
  if [[ "$expected" != "$actual" ]]; then
    echo "::error::SHA-256 mismatch for $asset" >&2
    exit 1
  fi
else
  echo "::warning::release $LIBREPAPER_VERSION has no checksum file for $asset"
fi
# The archive holds a folder named after the target, with the binary inside.
tar -xJf "$WORK/$asset" -C "$WORK" "librepaper-x86_64-unknown-linux-musl/librepaper"
export PATH="$WORK/librepaper-x86_64-unknown-linux-musl:$PATH"

# Take the LibrePaper state from origin.
# A glob that matches nothing still succeeds, so a failure here is the network
# or the credential, not a missing state.
git fetch --quiet origin '+refs/librepaper/*:refs/librepaper/*' || {
  echo "::error::could not fetch refs/librepaper/* from origin" >&2
  exit 1
}
# Without the state every project file looks new, so the push would be refused.
# A person seeds the state once from a clone, as the README's "Before the first run" describes.
if ! git rev-parse -q --verify refs/librepaper/state >/dev/null; then
  echo "::error::this repository has no LibrePaper state yet. Seed it once from a clone on your computer, then run the workflow again:%0A  librepaper pull https://app.librepaper.org/docs/<slug>%0A  git merge --allow-unrelated-histories librepaper/main  (only when the repository and the project share no history)%0A  git push origin main 'refs/librepaper/*'" >&2
  exit 1
fi
# Merges the runner makes need an author.
git config user.name "LibrePaper"
git config user.email "noreply@librepaper.org"

# Run the CLI. Its output goes to the log and to $LOG for matching.
run_cli() {
  librepaper "$@" 2>&1 | tee "$LOG"
}

# Branch the latest web version for a person to merge, then open a pull
# request, or comment on the one that is already open.
open_sync_pr() {
  git branch -f librepaper/sync refs/librepaper/main
  git push --quiet -f origin librepaper/sync
  local number
  number=$(gh pr list --head librepaper/sync --state open --json number --jq '.[0].number // empty')
  if [[ -n "$number" ]]; then
    gh pr comment "$number" --body "The web project changed again. This branch now holds the latest web version."
  else
    gh pr create --base "$BRANCH" --head librepaper/sync \
      --title "LibrePaper changed files you also changed" \
      --body "The web project and this repository changed the same files, so the sync stopped. Merge this branch into $BRANCH in git and push. The next push sends the merged result to the web project."
  fi
}

status=0
case "${GITHUB_EVENT_NAME:-}" in
push)
  # A re-run of an older push event checks out an older commit. Pushing it
  # against the newest state would roll the web project back, so only the
  # branch tip is pushed; a newer event has done or will do the rest.
  git fetch --quiet origin "$BRANCH"
  if [[ "$(git rev-parse HEAD)" != "$(git rev-parse "origin/$BRANCH")" ]]; then
    echo "::notice::this checkout is not the tip of $BRANCH any more; nothing pushed"
    exit 0
  fi
  # The checked-out commit is the one pushed. Its subject and author name the version.
  run_cli push "$PROJECT" --message "$(git log -1 --format=%s)" \
    --author "$(git log -1 --format=%an)" || status=$?
  if [[ $status -eq 0 ]]; then
    exit 0
  fi
  # Refused: the web project changed files that this push also changed.
  if [[ $status -eq 1 ]] && grep -qi "pull first" "$LOG"; then
    run_cli pull "$PROJECT" --no-merge
    open_sync_pr
  fi
  exit "$status"
  ;;
schedule | workflow_dispatch)
  # A pull that merges moves HEAD. A pull with nothing new leaves HEAD alone.
  before=$(git rev-parse HEAD)
  run_cli pull "$PROJECT" || status=$?
  if [[ $status -ne 0 ]]; then
    if [[ $status -eq 1 ]] && grep -qi "conflict" "$LOG"; then
      # Leave main as it was, then hand the conflicting files to a person.
      if git rev-parse -q --verify MERGE_HEAD >/dev/null; then
        git merge --abort
      fi
      open_sync_pr
    fi
    exit "$status"
  fi
  if [[ "$(git rev-parse HEAD)" != "$before" ]]; then
    if ! git push origin HEAD:"$BRANCH"; then
      PUBLISH_STATE=0
      echo "::error::could not push $BRANCH; the LibrePaper state was not published and the next run fetches again" >&2
      exit 1
    fi
  fi
  exit 0
  ;;
*)
  echo "::error::this action runs on push, schedule and workflow_dispatch, not on ${GITHUB_EVENT_NAME:-no event}" >&2
  exit 1
  ;;
esac
