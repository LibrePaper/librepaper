# Releasing

Releasing a version publishes downloadable artifacts. It does not deploy
librepaper.org; that is an operator command described in the private runbook
(`tools/deploy/runbook`).

## Release a version

1. Set the version with `tools/release/version X.Y.Z`. It updates
   `Cargo.toml`, the workspace entries in `Cargo.lock` and the image pins in
   `deploy/compose.yaml`; a test fails when the pins and the version differ.
   The release tag must be `vX.Y.Z`.
2. Run the checks described in [Building and testing](architecture/building.md),
   including `make check` and `tools/dev/db test`. If the release changes
   browser assets, publish them first; see [asset mirrors](dev/asset-mirrors.md).
   Commit and push the release commit, then confirm CI is green for it. The
   Release workflow builds on tag pushes but does not wait for CI.
3. From a clean checkout of that release commit, push the matching tag:

   ```sh
   VERSION="v$(sed -n 's/^version = \"\(.*\)\"/\1/p' Cargo.toml | head -n 1)"
   git tag "$VERSION"
   git push origin "$VERSION"
   gh run list --workflow release.yml --limit 5
   gh run watch RUN_ID --exit-status
   ```

   Select the Release run whose tag is `$VERSION`.

4. The Release workflow uses cargo-dist to build platform archives, installers,
   and SHA-256 files, then creates the GitHub Release. Confirm the Linux
   x86_64 asset exists:

   ```sh
   gh release view "$VERSION" --json assets -q '.assets[].name'
   ```

5. After a successful stable release, the Homebrew, Scoop and Docker image
   workflows run automatically; see below.

## Automated updates

- A successful stable tag release triggers the Homebrew and Scoop update
  workflows. Pull request runs, failed releases, prerelease tags, and releases
  from other repositories are ignored.
- Each updater runs the helper script from the workflow revision, then resolves
  the stable tag and downloads that release's artifacts. A manual run accepts a
  stable tag and uses the same artifact checks.
- Before publishing, each updater checks the downloaded archive against the
  release's cargo-dist `.sha256` file. A missing or invalid checksum stops the
  update without changing the package repository.
- Scoop installs the desktop app from `LibrePaper_<version>_x64-portable.zip`,
  which holds the signed app and `librepaper.exe`. Homebrew gets a formula for
  the command line (Linux and macOS archives) and a cask,
  `brew install --cask vincentarelbundock/tap/librepaper`, for the notarized
  macOS app from `LibrePaper_<version>_universal.dmg`.
- User install commands live on the [desktop app page](desktop-app.md). Update it when
  a distribution channel changes.

## Desktop app signing

The desktop app is signed on Windows and signed and notarized on macOS. Settings, credentials, test releases and troubleshooting are in [Release binaries](dev/release-binaries.md).

## Repository secrets

Configure these Actions secrets in `LibrePaper/librepaper`:

| Secret | Use | Required access |
| --- | --- | --- |
| `HOMEBREW_TAP_GITHUB_TOKEN` | Push `Formula/librepaper.rb` to `vincentarelbundock/homebrew-tap` | Contents write |
| `SCOOP_BUCKET_GITHUB_TOKEN` | Push `bucket/librepaper.json` to `vincentarelbundock/scoop-bucket` | Contents write |
| `AUR_SSH_PRIVATE_KEY` | Push `PKGBUILD` and `.SRCINFO` to `aur.archlinux.org/librepaper-bin` | SSH key registered on the AUR account |

Each token is used only by its matching update workflow. Keep the Release
workflow's `GITHUB_TOKEN` permissions limited to release tasks. Update scripts
send channel tokens to Git through an HTTP authorization header; they do not
place credentials in clone URLs or output.

LibrePaper is distributed under the MIT License. Package definitions declare
`MIT`; the license text is in the repository root `LICENSE` file.
