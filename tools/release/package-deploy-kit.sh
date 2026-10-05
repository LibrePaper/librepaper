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
for required in deploy/compose.yaml deploy/Dockerfile deploy/caddy/Caddyfile deploy/librepaper.toml deploy/setup; do
	git -C "$repository" cat-file -e "$commit:$required" 2>/dev/null || {
		echo "release commit is missing $required" >&2
		exit 2
	}
done

command -v tar >/dev/null
command -v sha256sum >/dev/null
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/deploy" "$output_dir"

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
rm -rf -- "$stage/deploy/secrets" "$stage/deploy/backups" "$stage/deploy/backup-metrics"
rm -f -- \
	"$stage/deploy/.env" \
	"$stage/deploy"/.env.* \
	"$stage/deploy/.setup-state.json" \
	"$stage/deploy"/*.candidate \
	"$stage/deploy"/*.tmp \
	"$stage/deploy"/librepaper \
	"$stage/deploy"/librepaper.exe \
	"$stage/deploy"/resticprofile.toml.local

# The only environment file in the archive contains the public release tag.
# Compose uses it to build/pull the same app release as the matching images.
printf 'LIBREPAPER_VERSION=%s\n' "$tag" > "$stage/deploy/.env"
chmod 0644 "$stage/deploy/.env"
cat > "$stage/deploy/RELEASE" <<EOF
tag=$tag
commit=$commit
app_image=ghcr.io/librepaper/librepaper:$tag
backup_image=ghcr.io/librepaper/librepaper-backup:$tag
EOF
chmod 0644 "$stage/deploy/RELEASE"

archive="librepaper-deploy-kit-${tag}.tar.gz"
tar -C "$stage" -czf "$output_dir/$archive" deploy
(cd "$output_dir" && sha256sum "$archive" > "$archive.sha256")
