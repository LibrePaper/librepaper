# Browser asset mirrors

The project is moving its Typst and LaTeX static mirrors to a dedicated OVH
Object Storage bucket. The bucket serves public browser downloads; the app
does not proxy these files. Keep the existing mirror URLs live while the new
objects, CORS response, integrity pins, and browser smoke checks are verified.

## Publish

Install AWS CLI v2 (`nix shell nixpkgs#awscli2` is convenient), then set the
OVH endpoint, region, bucket, and credentials in the environment. The endpoint
and credentials are pending; do not substitute the live Cloudflare endpoints.
Use a bucket dedicated to public immutable assets and allow anonymous
`s3:GetObject` only for the `typst/*` and `latex/*` keys. Keep bucket listing
private. For example, the public bucket policy should scope its resource to
`arn:aws:s3:::BUCKET/{typst,latex}/*` (use two resource entries in the actual
policy).

```sh
export S3_ENDPOINT='<OVH S3 endpoint URL>'
export S3_REGION='<OVH region>'
export S3_BUCKET='<dedicated public mirror bucket>'
export AWS_ACCESS_KEY_ID='<publisher key>'
export AWS_SECRET_ACCESS_KEY='<publisher secret>'
# export AWS_SESSION_TOKEN='<when using temporary credentials>'

node tools/publish-mirror.mjs --dir ../wasm-typst/mirror --prefix typst --dry-run
node tools/publish-mirror.mjs --dir ../wasm-latex/mirror --prefix latex --dry-run
node tools/publish-mirror.mjs --dir ../wasm-typst/mirror --prefix typst
node tools/publish-mirror.mjs --dir ../wasm-latex/mirror --prefix latex
```

Dry runs list the upload plan without reading or verifying payloads. Real
uploads check local hashes first, then read each uploaded object back and verify
its decoded bytes and response metadata before publishing release indexes.
Publisher credentials need both object write and read access.

The publisher uses AWS CLI signing, compresses supported text and WASM files
with gzip, and sets `Content-Encoding`, MIME type, and browser cache policy on
each object. Hash-addressed assets get immutable caching; `manifest.json` is
`no-store`, `bundles.json` is `no-cache`, and license files get a one-day
cache. Object keys stay unchanged, so hashed URLs remain canonical. Cloudflare
metadata and legacy `.br` sidecars are skipped. Publishing does not delete old
bucket objects. CORS is left untouched unless explicitly
requested with `--configure-cors`; that option permits public `GET`/`HEAD`
browser reads from any origin.

Once both mirrors are reachable over HTTPS and browser checks pass, update the
Typst asset URL and SHA-256 pin in `typst-assets.lock`, and switch the LaTeX
default mirror URL in its existing configuration. Retain the old mirrors until
those builds and checks succeed. OVH endpoint details, publishing credentials,
new URLs, and hashes remain pending live access and verification.
