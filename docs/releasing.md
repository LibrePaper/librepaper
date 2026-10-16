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
  which holds the signed app and `librepaper.exe`. Homebrew uses the Linux and
  macOS x86_64 and ARM64 archives and still installs the command line.
- User install commands live on the [desktop app page](desktop-app.md). Update it when
  a distribution channel changes.

## Windows signing

- The Release workflow builds the desktop app installers on Windows (`.github/workflows/desktop-windows.yml`) and attaches them to the release: `LibrePaper_<version>_x64-setup.exe` and, for stable versions, `LibrePaper_<version>_x64_en-US.msi`.
- Tauri signs the app, the bundled `librepaper.exe`, the installers and the uninstaller with Azure Artifact Signing. A failed signature fails the release.
- The job runs in the `release` GitHub environment and signs in to Azure with OIDC. There is no client secret.
- The release also has a signed portable zip for Scoop, and `.deb` and `.rpm` packages built by `.github/workflows/desktop-linux.yml`. Prereleases ship only the `.deb`. Each desktop asset has a `.sha256` file.
- Configure these settings in the `release` environment:

| Name | Kind | Value |
| --- | --- | --- |
| `AZURE_CLIENT_ID` | Secret | Client ID of the `librepaper-release-signing` app registration |
| `AZURE_TENANT_ID` | Secret | Tenant ID of the `librepaper-release-signing` app registration |
| `AZURE_SUBSCRIPTION_ID` | Secret | Subscription that holds the signing account |
| `AZURE_SIGNING_ENDPOINT` | Variable | The account's regional endpoint, like `https://eus.codesigning.azure.net` |
| `AZURE_SIGNING_PROFILE` | Variable | The certificate profile name |
| `AZURE_SIGNING_ACCOUNT` | Variable | Optional. Defaults to `librepaper-signing` |

- The app registration needs the `Artifact Signing Certificate Profile Signer` role on the certificate profile.
- The CLI archive and the PowerShell installer are not signed.
- To check a release installer on Windows, run this in PowerShell:

```powershell
# On Windows, after downloading the installer
Get-AuthenticodeSignature .\LibrePaper_0.0.43_x64-setup.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
```

- Pull requests build unsigned installers in the "Windows build" workflow (artifact `desktop-windows-unsigned`) and the Linux packages in the "Linux desktop build" workflow (artifact `desktop-linux-packages`).

## macOS signing and notarization

- The Release workflow builds a universal macOS app on a GitHub macOS runner (`.github/workflows/desktop-macos.yml`) and attaches `LibrePaper_<version>_universal.dmg`.
- The app is compiled without secrets. The signing and notarization secrets reach only the `tauri bundle` step.
- Tauri signs the bundled `librepaper` and the app with the Developer ID Application certificate and the hardened runtime, then notarizes and staples the app. The workflow then notarizes and staples the DMG. A failed signature or a rejected notarization fails the release.
- Configure these settings in the `release` environment:

| Name | Kind | Value |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | Secret | The Developer ID Application `.p12`, base64 |
| `APPLE_CERTIFICATE_PASSWORD` | Secret | The export password of the `.p12` |
| `APPLE_API_KEY_ID` | Secret | The App Store Connect API key ID |
| `APPLE_API_ISSUER` | Secret | The issuer ID shown above the keys table |
| `APPLE_API_PRIVATE_KEY` | Secret | The contents of the downloaded `AuthKey_<id>.p8` |

- Create the API key: App Store Connect, Users and Access, Integrations, App Store Connect API, Team Keys. Add a key with the Developer role and download the `.p8` once. Then store the three values:

```sh
gh secret set APPLE_API_KEY_ID      --env release --repo LibrePaper/librepaper --body <key id>
gh secret set APPLE_API_ISSUER      --env release --repo LibrePaper/librepaper --body <issuer id>
gh secret set APPLE_API_PRIVATE_KEY --env release --repo LibrePaper/librepaper < AuthKey_<key id>.p8
```

- To check a downloaded DMG on a Mac:

```sh
xcrun stapler validate LibrePaper_<version>_universal.dmg
spctl --assess --type open --context context:primary-signature --verbose=4 LibrePaper_<version>_universal.dmg
```

## Repository secrets

Configure these Actions secrets in `LibrePaper/librepaper`:

| Secret | Use | Required access |
| --- | --- | --- |
| `HOMEBREW_TAP_GITHUB_TOKEN` | Push `Formula/librepaper.rb` to `vincentarelbundock/homebrew-tap` | Contents write |
| `SCOOP_BUCKET_GITHUB_TOKEN` | Push `bucket/librepaper.json` to `vincentarelbundock/scoop-bucket` | Contents write |

Each token is used only by its matching update workflow. Keep the Release
workflow's `GITHUB_TOKEN` permissions limited to release tasks. Update scripts
send channel tokens to Git through an HTTP authorization header; they do not
place credentials in clone URLs or output.

LibrePaper is distributed under the MIT License. Package definitions declare
`MIT`; the license text is in the repository root `LICENSE` file.
