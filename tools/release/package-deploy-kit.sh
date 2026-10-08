#!/usr/bin/env bash
set -euo pipefail

usage() {
	echo "usage: package-deploy-kit.sh vMAJOR.MINOR.PATCH REPOSITORY OUTPUT_DIR COMMIT_SHA" >&2
}

if [[ $# != 4 ]]; then
	usage
	exit 2
fi

tag=$1
repository=$2
output_dir=$3
commit=$4
if [[ ! "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
	echo "expected a stable v-prefixed release tag, got: $tag" >&2
	exit 2
fi
if [[ ! "$commit" =~ ^[[:xdigit:]]{40}$ ]]; then
	echo "expected a 40-character release commit SHA" >&2
	exit 2
fi

# Verify all required kit files exist in the release commit.
required_files=(
	deploy/compose.yaml
	deploy/compose.managed-db.yaml
	deploy/librepaper.toml
	deploy/resticprofile.toml
	deploy/postgres/init.sql
	deploy/caddy/Caddyfile
	deploy/README.md
	deploy/backups
)
for file in "${required_files[@]}"; do
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
git -C "$repository" archive --format=tar "$commit" "${required_files[@]}" | tar -C "$stage" -xf -

# A release commit is trusted input, but deployment data must never become a
# public release asset if an operator accidentally committed it. Kits ship
# templates only; an enabled Restic profile belongs on the host.
if [[ -f "$stage/deploy/resticprofile.toml" ]] &&
	grep -Eq '^[[:space:]]*\[resticprofile([.][^]]+)?\][[:space:]]*(#.*)?$' "$stage/deploy/resticprofile.toml"; then
	echo "refusing to package an enabled resticprofile.toml; release kits must ship backups disabled" >&2
	exit 2
fi

# Remove operator files and directories even though git archive only ships tracked files.
rm -rf -- "$stage/deploy/secrets"
rm -f -- \
	"$stage/deploy/.env" \
	"$stage/deploy"/.env.* \
	"$stage/deploy/.setup-state.json" \
	"$stage/deploy"/*.candidate \
	"$stage/deploy"/*.tmp \
	"$stage/deploy"/librepaper \
	"$stage/deploy"/librepaper.exe \
	"$stage/deploy"/resticprofile.toml.local

# Rewrite the two image tags in compose.yaml from their committed literals to the release tag.
if [[ ! -f "$stage/deploy/compose.yaml" ]]; then
	echo "release commit is missing compose.yaml" >&2
	exit 2
fi
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

# Replace image tags with the release tag.
sed -i "s|ghcr.io/librepaper/librepaper:[^ \"]*|ghcr.io/librepaper/librepaper:$tag|g" "$stage/deploy/compose.yaml"
sed -i "s|ghcr.io/librepaper/librepaper-backup:[^ \"]*|ghcr.io/librepaper/librepaper-backup:$tag|g" "$stage/deploy/compose.yaml"

# Move deploy to librepaper and set permissions.
mv "$stage/deploy" "$stage/librepaper"
chmod 0700 "$stage/librepaper"
find "$stage/librepaper" -type f -exec chmod 0644 {} \;
chmod 0755 "$stage/librepaper/backups"
find "$stage/librepaper" -type d -exec chmod 0755 {} \;
chmod 0700 "$stage/librepaper"

# Create the archive with fixed name and explicit owner/group.
archive="librepaper-deploy.tar.gz"
tar -C "$stage" --owner=0 --group=0 --numeric-owner -czf "$output_dir/$archive" librepaper
