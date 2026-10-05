#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mock_bin="$tmp/bin"
mkdir -p "$mock_bin"

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
# and untracked credentials. It pins the same app version as its images.
kit_source="$tmp/deploy-source"
kit_output="$tmp/deploy-output"
mkdir -p "$kit_source/deploy/caddy" "$kit_source/deploy/secrets" "$kit_output"
for file in compose.yaml Dockerfile librepaper.toml setup; do
  printf 'committed fixture %s\n' "$file" > "$kit_source/deploy/$file"
done
printf 'committed caddy\n' > "$kit_source/deploy/caddy/Caddyfile"
printf 'committed operator secret\n' > "$kit_source/deploy/secrets/database_app_url"
printf 'committed operator state\n' > "$kit_source/deploy/.setup-state.json"
printf 'committed public tag placeholder\n' > "$kit_source/deploy/.env"
printf '# Backups are disabled until an operator configures them.\n' > "$kit_source/deploy/resticprofile.toml"
git -C "$kit_source" init -q
git -C "$kit_source" config user.name Fixture
git -C "$kit_source" config user.email fixture@example.invalid
git -C "$kit_source" add deploy
git -C "$kit_source" commit -qm 'release deployment files'
kit_commit=$(git -C "$kit_source" rev-parse HEAD)
printf 'dirty attacker config\n' > "$kit_source/deploy/librepaper.toml"
printf 'operator secret\n' > "$kit_source/deploy/.env"
printf 'operator secret\n' > "$kit_source/deploy/secrets/database_app_url"
printf 'untracked secret\n' > "$kit_source/deploy/leaked-backup.txt"
bash "$root/tools/release/package-deploy-kit.sh" v1.2.3 "$kit_source" "$kit_output" "$kit_commit"
kit_archive="$kit_output/librepaper-deploy-kit-v1.2.3.tar.gz"
kit_extract="$tmp/deploy-extracted"
mkdir "$kit_extract"
tar -C "$kit_extract" -xzf "$kit_archive"
[[ "$(cat "$kit_extract/deploy/.env")" == 'LIBREPAPER_VERSION=v1.2.3' ]] || fail 'deploy kit did not pin its release version'
[[ "$(cat "$kit_extract/deploy/librepaper.toml")" == 'committed fixture librepaper.toml' ]] || fail 'deploy kit used dirty config instead of committed release bytes'
[[ ! -e "$kit_extract/deploy/secrets/database_app_url" ]] || fail 'deploy kit included a secret file'
[[ ! -e "$kit_extract/deploy/.setup-state.json" ]] || fail 'deploy kit included operator setup state'
[[ ! -e "$kit_extract/deploy/leaked-backup.txt" ]] || fail 'deploy kit included an untracked file'
[[ "$(cat "$kit_extract/deploy/RELEASE")" == *'app_image=ghcr.io/librepaper/librepaper:v1.2.3'* ]] || fail 'deploy kit app image does not match release'
[[ "$(cat "$kit_extract/deploy/RELEASE")" == *'backup_image=ghcr.io/librepaper/librepaper-backup:v1.2.3'* ]] || fail 'deploy kit backup image does not match release'
(cd "$kit_output" && sha256sum -c librepaper-deploy-kit-v1.2.3.tar.gz.sha256 >/dev/null) || fail 'deploy kit checksum did not verify'

if bash "$root/tools/release/package-deploy-kit.sh" v1.2.3-rc.1 "$kit_source" "$kit_output" "$kit_commit" >/dev/null 2>&1; then
  fail 'deploy kit accepted a prerelease tag'
fi

printf '[resticprofile]\nrepository = "s3://private-bucket"\n' > "$kit_source/deploy/resticprofile.toml"
git -C "$kit_source" add deploy/resticprofile.toml
git -C "$kit_source" commit -qm 'enable restic credentials in committed profile'
enabled_commit=$(git -C "$kit_source" rev-parse HEAD)
if bash "$root/tools/release/package-deploy-kit.sh" v1.2.3 "$kit_source" "$kit_output" "$enabled_commit" >/dev/null 2>&1; then
  fail 'deploy kit accepted an enabled restic profile'
fi

echo 'release packaging mocks passed'
