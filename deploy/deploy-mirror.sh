#!/usr/bin/env bash
set -euo pipefail

# Deploy browser asset mirrors to OVH S3.
#
# One-time setup:
# 1. Public Cloud > Object Storage > Users: create a user, download S3 credentials.
# 2. Do NOT create the bucket in the console; this script creates it so the user owns it.
# 3. sops deploy/keys.yaml: add OVH_S3_ENDPOINT, OVH_S3_REGION, OVH_S3_USER, OVH_S3_SECRET, OVH_S3_ARN.
# 4. make mirrors: build the Typst and LaTeX mirrors (in wasm-typst and wasm-latex).
# 5. deploy/deploy-mirror.sh --test: verify bucket, CORS, and connectivity (uploads two small test files).
# 6. deploy/deploy-mirror.sh: publish all mirrors.
# 7. Afterwards (manual): point DEFAULT_LATEX_MIRROR and typst-assets.lock at https://<bucket>.s3.<region>.io.cloud.ovh.net/.

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
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

# Decrypt and read keys.
KEYS="${KEYS:-deploy/keys.yaml}"
SECRETS_JSON=$(sops --decrypt --output-type json "$KEYS")

# Extract OVH credentials and derive bucket.
OVH_S3_ENDPOINT=$(printf '%s' "$SECRETS_JSON" | grep -o '"OVH_S3_ENDPOINT":"[^"]*' | cut -d'"' -f4)
OVH_S3_REGION=$(printf '%s' "$SECRETS_JSON" | grep -o '"OVH_S3_REGION":"[^"]*' | cut -d'"' -f4)
OVH_S3_USER=$(printf '%s' "$SECRETS_JSON" | grep -o '"OVH_S3_USER":"[^"]*' | cut -d'"' -f4)
OVH_S3_SECRET=$(printf '%s' "$SECRETS_JSON" | grep -o '"OVH_S3_SECRET":"[^"]*' | cut -d'"' -f4)
OVH_S3_ARN=$(printf '%s' "$SECRETS_JSON" | grep -o '"OVH_S3_ARN":"[^"]*' | cut -d'"' -f4)

# Derive bucket from ARN: arn:aws:s3:::bucket -> bucket
S3_BUCKET="${S3_BUCKET:-$(printf '%s' "$OVH_S3_ARN" | sed 's|^arn:aws:s3:::\([a-z0-9][a-z0-9.-]*[a-z0-9]\)$|\1|')}"
if [ -z "$S3_BUCKET" ]; then
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
  TYPST_MIRROR="${TYPST_MIRROR:-../wasm-typst/mirror}"
  MIRROR="${MIRROR:-../wasm-latex/mirror}"

  if [ -f "$TYPST_MIRROR/LICENSE" ]; then
    $aws_cmd s3api put-object --endpoint-url "$OVH_S3_ENDPOINT" --region "$OVH_S3_REGION" \
      --bucket "$S3_BUCKET" --key "test/LICENSE" --body "$TYPST_MIRROR/LICENSE" \
      --content-type "text/plain" --acl public-read
  fi

  if [ -f "$MIRROR/_headers" ]; then
    $aws_cmd s3api put-object --endpoint-url "$OVH_S3_ENDPOINT" --region "$OVH_S3_REGION" \
      --bucket "$S3_BUCKET" --key "test/_headers" --body "$MIRROR/_headers" \
      --content-type "text/plain" --acl public-read
  fi

  VERIFY_KEY="test/LICENSE"
else
  # Normal mode: run make mirrors-push.
  printf '%s\n' "Publishing mirrors..."
  make mirrors-push
  VERIFY_KEY="latex/manifest.json"
fi

# Verify: fetch public URL with Origin header; expect 200 + access-control-allow-origin.
printf '%s\n' "Verifying deployment..."
VERIFY_URL="https://$S3_BUCKET.s3.$OVH_S3_REGION.io.cloud.ovh.net/$VERIFY_KEY"
RESPONSE=$(curl -s -w "\n%{http_code}" -H "Origin: https://example.com" "$VERIFY_URL")
HTTP_CODE=$(printf '%s' "$RESPONSE" | tail -1)
HEADERS=$(printf '%s' "$RESPONSE" | head -n-1)

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
