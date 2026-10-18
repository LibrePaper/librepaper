#!/usr/bin/env bash
set -euo pipefail

usage() {
	echo "usage: package-deploy-kit.sh vMAJOR.MINOR.PATCH REPOSITORY OUTPUT_DIR COMMIT_SHA APP_DIGEST BACKUP_DIGEST" >&2
}

if [[ $# != 6 ]]; then
	usage
	exit 2
fi

tag=$1
repository=$2
output_dir=$3
commit=$4
app_digest=$5
backup_digest=$6
if [[ ! "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
	echo "expected a stable v-prefixed release tag, got: $tag" >&2
	exit 2
fi
if [[ ! "$commit" =~ ^[[:xdigit:]]{40}$ ]]; then
	echo "expected a 40-character release commit SHA" >&2
	exit 2
fi
for digest in "$app_digest" "$backup_digest"; do
	if [[ ! "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
		echo "expected an image manifest digest (sha256:...), got: $digest" >&2
		exit 2
	fi
done

# The kit is the whole deploy/ directory; check the files this script depends
# on exist in the release commit.
for file in deploy/compose.yaml deploy/manage; do
	git -C "$repository" cat-file -e "$commit:$file" 2>/dev/null || {
		echo "release commit is missing $file" >&2
		exit 2
	}
done

command -v tar >/dev/null
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$output_dir"

# The supplied Git commit determines the bytes in the kit. This excludes local
# edits and untracked files even when the packaging workflow checkout is dirty.
git -C "$repository" archive --format=tar "$commit" deploy | tar -C "$stage" -xf -

# A release commit is trusted input, but deployment data must never become a
# public release asset if an operator accidentally committed it. Kits ship
# templates only; an enabled Restic profile belongs on the host.
if [[ -f "$stage/deploy/resticprofile.toml" ]] &&
	grep -Eq '^[[:space:]]*\[resticprofile([.][^]]+)?\][[:space:]]*(#.*)?$' "$stage/deploy/resticprofile.toml"; then
	echo "refusing to package an enabled resticprofile.toml; release kits must ship backups disabled" >&2
	exit 2
fi

# Rewrite the two image tags in compose.yaml from their committed literals to the release tag.
app_tag_count=$(grep -c 'ghcr.io/librepaper/librepaper:' "$stage/deploy/compose.yaml" || true)
if [[ "$app_tag_count" != 1 ]]; then
	echo "compose.yaml must contain ghcr.io/librepaper/librepaper: exactly once" >&2
	exit 2
fi
backup_tag_count=$(grep -c 'ghcr.io/librepaper/librepaper-backup:' "$stage/deploy/compose.yaml" || true)
if [[ "$backup_tag_count" != 1 ]]; then
	echo "compose.yaml must contain ghcr.io/librepaper/librepaper-backup: exactly once" >&2
	exit 2
fi

# Replace image tags with the release tag and pin each image by its digest.
sed -i "s|ghcr.io/librepaper/librepaper:[^ \"]*|ghcr.io/librepaper/librepaper:$tag@$app_digest|g" "$stage/deploy/compose.yaml"
sed -i "s|ghcr.io/librepaper/librepaper-backup:[^ \"]*|ghcr.io/librepaper/librepaper-backup:$tag@$backup_digest|g" "$stage/deploy/compose.yaml"

# The helper in the release commit must read its own kit. Older helpers took
# the digest for the version, and a kit built from such a commit strands
# every host that runs it.
helper_version=$(bash "$stage/deploy/manage" version 2>/dev/null || true)
if [[ "$helper_version" != "$tag" ]]; then
	echo "the manage helper at $commit reads the pinned version as '$helper_version', not $tag; it cannot read a digest-pinned kit" >&2
	exit 2
fi

# Move deploy to librepaper and set permissions.
mv "$stage/deploy" "$stage/librepaper"
chmod 0700 "$stage/librepaper"
find "$stage/librepaper" -type f -exec chmod 0644 {} \;
chmod 0755 "$stage/librepaper/manage"
find "$stage/librepaper" -type d -exec chmod 0755 {} \;
chmod 0700 "$stage/librepaper"

# Create the archive with fixed name and explicit owner/group.
archive="librepaper-deploy.tar.gz"
tar -C "$stage" --owner=0 --group=0 --numeric-owner -czf "$output_dir/$archive" librepaper
