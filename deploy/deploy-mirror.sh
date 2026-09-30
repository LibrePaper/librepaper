#!/usr/bin/env bash
set -euo pipefail

# Deploy browser asset mirrors to OVH S3.
#
# One-time setup:
# 1. Public Cloud > Object Storage > Users: create a user, download S3 credentials.
# 2. Do NOT create the bucket in the console; this script creates it so the user owns it.
# 3. sops deploy/keys.yaml: add OVH_S3_ENDPOINT, OVH_S3_REGION, OVH_S3_USER, OVH_S3_SECRET, OVH_S3_ARN.
# 4. make wasm && make mirrors: fetch the pinned browser wasm modules into web/wasm and build
#    the LaTeX mirror (in wasm-latex).
# 5. Publish every GitHub release assets.lock pins (a draft fails the preflight check).
# 6. deploy/deploy-mirror.sh --test: verify bucket, CORS, and connectivity (uploads two small test files).
# 7. deploy/deploy-mirror.sh: publish all mirrors.
# 8. Afterwards (manual): point DEFAULT_ASSET_MIRROR (crates/librepaper/src/config.rs) at https://<bucket>.s3.<region>.io.cloud.ovh.net/,
#    and pin the published LaTeX release (latex/<sha256>/) and wasm modules in assets.lock.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_MODE=0

case "${1:-}" in
  --test)
    TEST_MODE=1
    ;;
  "")
    TEST_MODE=0
    ;;
  *)
    printf '%s\n' "Usage: $0 [--test]" >&2
    exit 1
    ;;
esac

cd "$REPO_ROOT"

# Every pin must name a public GitHub release: `make wasm` downloads the modules
# anonymously, and the LaTeX release's source link is its licence obligation. A
# draft release answers 404 to anyone signed out, so check before uploading.
public() { [ "$(curl -sIL -o /dev/null -w '%{http_code}' "$1")" = 200 ] || { printf '%s\n' "Error: $2 is not publicly downloadable (a draft release?): $1" >&2; exit 1; }; }
while read -r module repo tag _; do
  [ "$module" = latex ] && continue
  public "https://github.com/LibrePaper/$repo/releases/download/$tag/$module" "$repo $tag"
done < <(grep -v '^#' assets.lock | grep .)
LATEX_ID=$(awk '$1 == "latex" { print $4 }' assets.lock)
LATEX_RELEASE="${MIRROR:-../wasm-latex/mirror}/$LATEX_ID/release.json"
[ -f "$LATEX_RELEASE" ] || { printf '%s\n' "Error: $LATEX_RELEASE missing; run make mirror in wasm-latex" >&2; exit 1; }
public "$(node -p 'require(process.argv[1]).source.corresponding_source.url' "$(realpath "$LATEX_RELEASE")")" "wasm-latex source for $LATEX_ID"

# Decrypt and read keys.
KEYS="${KEYS:-deploy/keys.yaml}"
key() { sops --decrypt --extract "[\"$1\"]" "$KEYS"; }
OVH_S3_ENDPOINT=$(key OVH_S3_ENDPOINT)
OVH_S3_REGION=$(key OVH_S3_REGION)
OVH_S3_USER=$(key OVH_S3_USER)
OVH_S3_SECRET=$(key OVH_S3_SECRET)
OVH_S3_ARN=$(key OVH_S3_ARN)

# Derive bucket from ARN: arn:aws:s3:::bucket -> bucket
S3_BUCKET="${S3_BUCKET:-${OVH_S3_ARN#arn:aws:s3:::}}"
if [ -z "$S3_BUCKET" ] || [ "$S3_BUCKET" = "$OVH_S3_ARN" ]; then
  printf '%s\n' "Error: could not derive bucket from OVH_S3_ARN or S3_BUCKET is not set" >&2
  exit 1
fi

# Export AWS credentials (read-only here, not echoed).
export AWS_ACCESS_KEY_ID="$OVH_S3_USER"
export AWS_SECRET_ACCESS_KEY="$OVH_S3_SECRET"

# Choose aws command: prefer aws if available, else nix shell.
if command -v aws >/dev/null 2>&1; then
  aws_cmd="aws"
else
  aws_cmd="nix shell nixpkgs#awscli2 -c aws"
fi

# Ensure bucket exists.
if ! $aws_cmd s3api head-bucket --endpoint-url "$OVH_S3_ENDPOINT" --region "$OVH_S3_REGION" --bucket "$S3_BUCKET" 2>/dev/null; then
  printf '%s\n' "Creating bucket $S3_BUCKET..."
  $aws_cmd s3 mb "s3://$S3_BUCKET" --endpoint-url "$OVH_S3_ENDPOINT" --region "$OVH_S3_REGION"
fi

# Apply CORS: allow GET/HEAD from any origin.
printf '%s\n' "Configuring CORS..."
CORS_CONFIG='{"CORSRules":[{"AllowedOrigins":["*"],"AllowedMethods":["GET","HEAD"],"AllowedHeaders":["*"],"MaxAgeSeconds":3600}]}'
$aws_cmd s3api put-bucket-cors --endpoint-url "$OVH_S3_ENDPOINT" --region "$OVH_S3_REGION" --bucket "$S3_BUCKET" --cors-configuration "$CORS_CONFIG"

if [ "$TEST_MODE" = 1 ]; then
  # Test mode: upload two small test files.
  printf '%s\n' "Uploading test files..."
  MIRROR="${MIRROR:-../wasm-latex/mirror}"

  # A missing file fails the run instead of being skipped.
  $aws_cmd s3api put-object --endpoint-url "$OVH_S3_ENDPOINT" --region "$OVH_S3_REGION" \
    --bucket "$S3_BUCKET" --key "test/markdown.wasm" --body "web/wasm/markdown.wasm" \
    --content-type "application/wasm" --acl public-read
  $aws_cmd s3api put-object --endpoint-url "$OVH_S3_ENDPOINT" --region "$OVH_S3_REGION" \
    --bucket "$S3_BUCKET" --key "test/_headers" --body "$MIRROR/_headers" \
    --content-type "text/plain" --acl public-read
  VERIFY_KEY="test/markdown.wasm"
else
  # Normal mode: run make mirrors-push.
  printf '%s\n' "Publishing mirrors..."
  make mirrors-push
  # The LaTeX release the build pins: the `latex` row of assets.lock names its
  # directory, latex/<sha256>/. Everything on the mirror is immutable.
  VERIFY_KEY="latex/${LATEX_ID}/release.json"
fi

# Verify: fetch public URL with Origin header; expect 200 + access-control-allow-origin.
printf '%s\n' "Verifying deployment..."
VERIFY_URL="https://$S3_BUCKET.s3.$OVH_S3_REGION.io.cloud.ovh.net/$VERIFY_KEY"
HEADERS=$(curl -s -o /dev/null -D - -H "Origin: https://example.com" "$VERIFY_URL")
HTTP_CODE=$(printf "%s" "$HEADERS" | head -1 | cut -d" " -f2)

if [ "$HTTP_CODE" != "200" ]; then
  printf '%s\n' "Error: verification failed (HTTP $HTTP_CODE)" >&2
  exit 1
fi

if ! printf '%s' "$HEADERS" | grep -qi "access-control-allow-origin"; then
  printf '%s\n' "Error: CORS header missing in response" >&2
  exit 1
fi

# Verify that bucket root listing is not public.
LIST_RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" "https://$S3_BUCKET.s3.$OVH_S3_REGION.io.cloud.ovh.net/")
if [ "$LIST_RESPONSE" = "200" ]; then
  printf '%s\n' "Error: bucket listing is public (should be private)" >&2
  exit 1
fi

printf '%s\n' "Success: mirrors deployed to s3://$S3_BUCKET/ with CORS enabled."
