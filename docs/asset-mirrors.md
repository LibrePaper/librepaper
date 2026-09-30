# Browser asset mirrors

The Typst and LaTeX mirror publishers upload prepared directories to a
dedicated OVH S3 bucket. Build both mirrors first in the sibling
`wasm-typst` and `wasm-latex` repositories. Install Node.js, SOPS and AWS CLI v2, and
keep the encrypted publisher keys in `deploy/keys.yaml`. `make mirrors-push`
uses `nix shell nixpkgs#awscli2` when AWS CLI v2 is missing and Nix is available.

The key file supplies `OVH_S3_ENDPOINT`, `OVH_S3_REGION` (currently `bhs`),
`OVH_S3_USER`, `OVH_S3_SECRET`, and `OVH_S3_ARN`. `OVH_S3_USER` is the S3
access key ID. The exact bucket ARN may supply the bucket name; otherwise set
`S3_BUCKET` in the environment or `OVH_S3_BUCKET` in the key file. Only an ARN
of the form `arn:aws:s3:::BUCKET` is accepted. `OVH_S3_HOST` is unused. Canonical environment variables
`S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `AWS_ACCESS_KEY_ID`, and
`AWS_SECRET_ACCESS_KEY` override the corresponding mapped values.

```sh
# Check both prepared mirrors and their hashes; no credentials or network used.
make mirrors-push MIRRORS_DRY_RUN=1

# Publish both mirrors. KEYS, TYPST_MIRROR, and MIRROR can be overridden.
make mirrors-push
```

SOPS decrypts the key file into memory for the publishing process. Both local
mirrors are integrity-checked before either upload begins. The publisher reads
each uploaded object back and verifies its bytes and response metadata before
publishing release indexes. Each upload uses a public-read ACL (listing stays
private). `deploy/deploy-mirror.sh` creates the bucket as the publisher user,
sets CORS once, pushes both mirrors, and verifies public reads; `--test`
uploads two small files instead of the mirrors. Application URL pins remain
manual.

Publisher credentials need object read/write access. Allow anonymous reads only
for `typst/*` and `latex/*`; keep listing private. Uploads use gzip where suitable,
immutable caching for hashed assets, `no-store` for manifests, and `no-cache`
for bundle indexes. Existing objects are retained.

Keep the existing mirror URLs live until the OVH objects are reachable over
HTTPS and browser checks pass. Only then update the Typst asset URL and
SHA-256 pin in `typst-assets.lock` and the LaTeX default mirror URL. The
current publishing setup does not perform that cutover.
