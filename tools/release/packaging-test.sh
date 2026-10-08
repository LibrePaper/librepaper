#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mock_bin="$tmp/bin"
mkdir -p "$mock_bin"
real_git="$(command -v git)"

cat > "$mock_bin/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail
output=""
url=""
while (($#)); do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
if [[ "$url" == *.sha256 ]]; then
  if [[ "${MOCK_BAD_CHECKSUM:-0}" == 1 ]]; then
    digest="0000000000000000000000000000000000000000000000000000000000000000"
  else
    digest="$MOCK_HASH"
  fi
  printf '%s *%s\n' "$digest" "${url##*/}" > "$output"
else
  printf '%s' "$MOCK_ARCHIVE_CONTENT" > "$output"
fi
CURL

cat > "$mock_bin/git" <<'GIT'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  clone)
    mkdir -p "$3"
    touch "$MOCK_GIT_CLONED"
    ;;
  add)
    destination="$MOCK_CAPTURE_DIR/$2"
    mkdir -p "$(dirname "$destination")"
    cp "$2" "$destination"
    ;;
  diff)
    exit 1
    ;;
  config|commit)
    ;;
  push)
    touch "$MOCK_GIT_PUSHED"
    ;;
  *)
    echo "unexpected git command: $*" >&2
    exit 1
    ;;
esac
GIT
chmod +x "$mock_bin/curl" "$mock_bin/git"

export PATH="$mock_bin:$PATH"
export MOCK_ARCHIVE_CONTENT='offline release archive fixture'
export MOCK_HASH="$(printf '%s' "$MOCK_ARCHIVE_CONTENT" | sha256sum | awk '{print $1}')"
export HOMEBREW_TAP_GITHUB_TOKEN=test-token
export SCOOP_BUCKET_GITHUB_TOKEN=test-token

fail() {
  echo "$*" >&2
  exit 1
}

reset_mock() {
  rm -f "$tmp/cloned" "$tmp/pushed"
  rm -rf "$tmp/capture"
  export MOCK_GIT_CLONED="$tmp/cloned"
  export MOCK_GIT_PUSHED="$tmp/pushed"
  export MOCK_CAPTURE_DIR="$tmp/capture"
  export MOCK_BAD_CHECKSUM=0
}

reset_mock
if LIBREPAPER_RELEASE_TAG=v1.2.3-rc.1 bash "$root/tools/release/update-homebrew-tap.sh" >/dev/null 2>&1; then
  fail 'Homebrew updater accepted a prerelease tag'
fi
[[ ! -e "$tmp/cloned" ]] || fail 'invalid tag reached git clone'

reset_mock
export MOCK_BAD_CHECKSUM=1
if LIBREPAPER_RELEASE_TAG=v1.2.3 bash "$root/tools/release/update-scoop-bucket.sh" >/dev/null 2>&1; then
  fail 'Scoop updater accepted a mismatched checksum'
fi
[[ ! -e "$tmp/cloned" ]] || fail 'checksum failure reached git clone'

reset_mock
LIBREPAPER_RELEASE_TAG=v1.2.3 bash "$root/tools/release/update-homebrew-tap.sh" >/dev/null
[[ -f "$tmp/capture/Formula/librepaper.rb" ]] || fail 'Homebrew formula was not generated'
for target in \
  x86_64-unknown-linux-musl \
  aarch64-unknown-linux-musl \
  x86_64-apple-darwin \
  aarch64-apple-darwin; do
  grep -Fq "librepaper-$target.tar.xz" "$tmp/capture/Formula/librepaper.rb" || fail "formula missing $target archive"
done
grep -Fq "sha256 \"$MOCK_HASH\"" "$tmp/capture/Formula/librepaper.rb" || fail 'formula missing verified archive hashes'
grep -Fq 'license "MIT"' "$tmp/capture/Formula/librepaper.rb" || fail 'formula missing MIT license metadata'
[[ -e "$tmp/pushed" ]] || fail 'Homebrew formula was not pushed'

reset_mock
LIBREPAPER_RELEASE_TAG=v1.2.3 bash "$root/tools/release/update-scoop-bucket.sh" >/dev/null
[[ -f "$tmp/capture/bucket/librepaper.json" ]] || fail 'Scoop manifest was not generated'
grep -Fq '"version": "1.2.3"' "$tmp/capture/bucket/librepaper.json" || fail 'manifest has wrong version'
grep -Fq '"license": "MIT"' "$tmp/capture/bucket/librepaper.json" || fail 'manifest missing MIT license metadata'
grep -Fq 'librepaper-x86_64-pc-windows-msvc.zip' "$tmp/capture/bucket/librepaper.json" || fail 'manifest missing Windows archive'
grep -Fq "\"hash\": \"$MOCK_HASH\"" "$tmp/capture/bucket/librepaper.json" || fail 'manifest missing verified archive hash'
[[ -e "$tmp/pushed" ]] || fail 'Scoop manifest was not pushed'

# A release kit uses committed bytes even when the checkout has dirty config
# and untracked credentials. It pins the release tag in its image references.
kit_source="$tmp/deploy-source"
kit_output="$tmp/deploy-output"
mkdir -p "$kit_source/deploy/caddy" "$kit_source/deploy/postgres" "$kit_output"
for file in compose.yaml compose.managed-db.yaml librepaper.toml README.md backups; do
  printf 'committed fixture %s\n' "$file" > "$kit_source/deploy/$file"
done
printf 'ghcr.io/librepaper/librepaper:v0.0.21\n' >> "$kit_source/deploy/compose.yaml"
printf 'ghcr.io/librepaper/librepaper-backup:v0.0.21\n' >> "$kit_source/deploy/compose.yaml"
printf 'committed caddy\n' > "$kit_source/deploy/caddy/Caddyfile"
printf '# Backups are disabled until an operator configures them.\n' > "$kit_source/deploy/resticprofile.toml"
printf 'committed init\n' > "$kit_source/deploy/postgres/init.sql"
"$real_git" -C "$kit_source" init -q
"$real_git" -C "$kit_source" config user.name Fixture
"$real_git" -C "$kit_source" config user.email fixture@example.invalid
"$real_git" -C "$kit_source" add deploy
"$real_git" -C "$kit_source" commit -qm 'release deployment files'
kit_commit=$("$real_git" -C "$kit_source" rev-parse HEAD)

# Plant unwanted files that should be excluded: setup
printf 'excluded\n' > "$kit_source/deploy/setup"
"$real_git" -C "$kit_source" add deploy/setup
"$real_git" -C "$kit_source" commit -qm 'add files that should be excluded'
excluded_commit=$("$real_git" -C "$kit_source" rev-parse HEAD)

# Dirty files should not be included in archive
printf 'dirty attacker config\n' > "$kit_source/deploy/librepaper.toml"
printf 'operator secret\n' > "$kit_source/deploy/.env"
printf 'untracked secret\n' > "$kit_source/deploy/leaked-backup.txt"

# Test with the first commit (clean files, no excluded setup yet)
PATH="${real_git%/*}:$PATH" bash "$root/tools/release/package-deploy-kit.sh" v1.2.3 "$kit_source" "$kit_output" "$kit_commit"
kit_archive="$kit_output/librepaper-deploy.tar.gz"
kit_extract="$tmp/deploy-extracted"
mkdir "$kit_extract"
tar -C "$kit_extract" -xzf "$kit_archive"

# Verify top directory is librepaper/ with mode 700
[[ -d "$kit_extract/librepaper" ]] || fail 'archive did not extract to librepaper/ directory'
top_mode=$(stat -c '%a' "$kit_extract/librepaper" 2>/dev/null || stat -f '%OLp' "$kit_extract/librepaper" | tail -c 4)
[[ "$top_mode" == "0700" || "$top_mode" == "700" ]] || fail "librepaper/ directory has mode $top_mode, expected 0700"

# Verify no .env, no RELEASE, no .sha256 files
[[ ! -e "$kit_extract/librepaper/.env" ]] || fail 'archive included .env file'
[[ ! -e "$kit_extract/librepaper/RELEASE" ]] || fail 'archive included RELEASE file'
[[ ! -e "$kit_output/librepaper-deploy.tar.gz.sha256" ]] || fail 'archive generated .sha256 file'

# Verify committed fixture was included, not dirty version
[[ "$(cat "$kit_extract/librepaper/librepaper.toml")" == 'committed fixture librepaper.toml' ]] || fail 'deploy kit used dirty config instead of committed release bytes'

# Verify image tags were rewritten to the release tag
compose_content=$(cat "$kit_extract/librepaper/compose.yaml")
[[ "$compose_content" == *'ghcr.io/librepaper/librepaper:v1.2.3'* ]] || fail 'deploy kit did not rewrite app image tag'
[[ "$compose_content" == *'ghcr.io/librepaper/librepaper-backup:v1.2.3'* ]] || fail 'deploy kit did not rewrite backup image tag'
[[ "$compose_content" != *'v0.0.21'* ]] || fail 'deploy kit did not replace original version tag'

# The backup helper ships and stays executable
[[ -x "$kit_extract/librepaper/backups" ]] || fail 'archive lost the backups helper or its executable bit'

# Verify only allowed files are present
[[ ! -e "$kit_extract/librepaper/secrets" ]] || fail 'archive included secrets directory'
[[ ! -e "$kit_extract/librepaper/.setup-state.json" ]] || fail 'archive included operator setup state'
[[ ! -e "$kit_extract/librepaper/leaked-backup.txt" ]] || fail 'archive included untracked file'

# Now test with excluded_commit to verify setup is excluded
rm -rf "$kit_extract" "$kit_output"
mkdir "$kit_extract" "$kit_output"
PATH="${real_git%/*}:$PATH" bash "$root/tools/release/package-deploy-kit.sh" v1.2.3 "$kit_source" "$kit_output" "$excluded_commit"
tar -C "$kit_extract" -xzf "$kit_output/librepaper-deploy.tar.gz"
[[ ! -e "$kit_extract/librepaper/setup" ]] || fail 'archive included deploy/setup'

# Reject prerelease tags
if PATH="${real_git%/*}:$PATH" bash "$root/tools/release/package-deploy-kit.sh" v1.2.3-rc.1 "$kit_source" "$kit_output" "$kit_commit" >/dev/null 2>&1; then
  fail 'deploy kit accepted a prerelease tag'
fi

# Reject enabled resticprofile.toml
printf '[resticprofile]\nrepository = "s3://private-bucket"\n' > "$kit_source/deploy/resticprofile.toml"
"$real_git" -C "$kit_source" add deploy/resticprofile.toml
"$real_git" -C "$kit_source" commit -qm 'enable restic credentials in committed profile'
enabled_restic_commit=$("$real_git" -C "$kit_source" rev-parse HEAD)
if PATH="${real_git%/*}:$PATH" bash "$root/tools/release/package-deploy-kit.sh" v1.2.3 "$kit_source" "$kit_output" "$enabled_restic_commit" >/dev/null 2>&1; then
  fail 'deploy kit accepted an enabled restic profile'
fi

echo 'release packaging mocks passed'
