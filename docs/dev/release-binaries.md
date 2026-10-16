# Release binaries

A version tag `vX.Y.Z` runs the Release workflow (`.github/workflows/release.yml`), which cargo-dist generates. It builds the command line archives and the desktop app, signs them, and publishes one GitHub Release only after every job passes.

For the release steps, see [Releasing](../releasing.md).

## What a release ships

| Asset | Built by | Signed |
| --- | --- | --- |
| `librepaper-<target>.tar.gz` or `.zip`, `librepaper-installer.sh`, `librepaper-installer.ps1` | cargo-dist `build-local-artifacts` | No |
| `LibrePaper_<version>_x64-setup.exe` (NSIS) | `desktop-windows.yml` | Azure Artifact Signing |
| `LibrePaper_<version>_x64_en-US.msi` (stable versions only, MSI rejects prerelease versions) | `desktop-windows.yml` | Azure Artifact Signing |
| `LibrePaper_<version>_x64-portable.zip` (Scoop) | `desktop-windows.yml` | The two programs inside are signed |
| `LibrePaper_<version>_x64-setup.exe.sig` | `desktop-windows.yml` | Updater signature of the installer, read from `latest.json` |
| `LibrePaper_<version>_universal.dmg` | `desktop-macos.yml` | Developer ID, notarized and stapled |
| `LibrePaper_<version>_universal.app.tar.gz` | `desktop-macos.yml` | Updater archive of the app |
| `LibrePaper_<version>_universal.app.tar.gz.sig` | `desktop-macos.yml` | Updater signature of the archive, read from `latest.json` |
| `latest.json` | `publish-updater.yml` | No. The desktop app reads its signatures and URLs |
| `librepaper_<version>_amd64.deb` | `desktop-linux.yml` | No |
| `librepaper-<version>-1.x86_64.rpm` (stable versions only) | `desktop-linux.yml` | No |

Each desktop asset has a `.sha256` file.

The desktop jobs are cargo-dist `local-artifacts-jobs`, listed in `Cargo.toml`. A failed signature fails the whole release, and nothing is published.

After you edit the dist settings in `Cargo.toml`, regenerate `release.yml` with `dist generate` (dist 0.33.0), then check it with `dist generate --check`.

## Shared build action

The `.github/actions/build-desktop` action is used by every desktop workflow.

- It builds the `librepaper` binary and places it in `desktop/binaries/` as the Tauri sidecar named `librepaper-<target triple>`.
- It writes the Tauri config with the version.
- It runs the Tauri CLI, pinned as `@tauri-apps/cli@2.12.1`, through `bunx`.
- For `universal-apple-darwin`, it builds `aarch64-apple-darwin` and `x86_64-apple-darwin`. It keeps a sidecar for each one, then joins them with `lipo` into `librepaper-universal-apple-darwin`. Tauri needs all three.
- On Windows, the CLI is linked with a static C runtime (`-C target-feature=+crt-static`), so it runs without the Visual C++ runtime installed.

## Windows signing

- The Release workflow builds the desktop app installers on Windows (`.github/workflows/desktop-windows.yml`) and attaches them to the release: `LibrePaper_<version>_x64-setup.exe` and, for stable versions, `LibrePaper_<version>_x64_en-US.msi`.
- Tauri signs the app, the bundled `librepaper.exe`, the installers and the uninstaller with Azure Artifact Signing. A failed signature fails the release.
- The job runs in the `release` GitHub environment and signs in to Azure with OIDC. There is no client secret.
- The job also builds the portable zip for Scoop. It holds its own signed copy of the app and the signed `librepaper.exe`.
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

- The signtool program comes from the NuGet package `Microsoft.Windows.SDK.BuildTools`. The signing plugin comes from `Microsoft.ArtifactSigning.Client`. Both versions and their SHA-256 hashes are pinned in `desktop-windows.yml`.
- Before the build, the workflow signs a probe file. A broken role or endpoint fails in the first few minutes.
- After the build, the workflow unpacks the NSIS installer with 7-Zip. It checks that every program inside is signed, along with the installers and the programs in the zip.
- The `release` environment accepts only `v*` tags, by its deployment policy. Pull requests never see its secrets.

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

- Before the build, a step imports `APPLE_CERTIFICATE` into a scratch keychain. It requires a valid "Developer ID Application" identity, and it fails when the certificate expires within 30 days.
- The current certificate is "Developer ID Application: Vincent Arel-Bundock (DXDTDPS4ZV)". Its Team ID is `DXDTDPS4ZV`. It expires on 1 February 2027.

### Make the .p12 on Linux

```sh
# Key and certificate signing request, uploaded at developer.apple.com (Certificates, Developer ID Application)
openssl req -new -newkey rsa:2048 -nodes -keyout librepaper-apple.key -subj "/CN=LibrePaper/C=CA" -out librepaper-apple.csr
# Convert the downloaded developerID_application.cer to PEM
openssl x509 -inform DER -in developerID_application.cer -out librepaper-apple.pem
# -legacy is required: the macOS keychain rejects the OpenSSL 3 default (SHA-256 MAC)
openssl pkcs12 -export -legacy -inkey librepaper-apple.key -in librepaper-apple.pem -out librepaper-apple.p12
# Store the base64 certificate and its password in the release environment
openssl base64 -A -in librepaper-apple.p12 | gh secret set APPLE_CERTIFICATE --env release --repo LibrePaper/librepaper
gh secret set APPLE_CERTIFICATE_PASSWORD --env release --repo LibrePaper/librepaper
```

Keep the key and the `.p12` out of the repository. Delete the local copies you no longer need.

## Linux packages

- `desktop-linux.yml` builds the `.deb` and `.rpm` packages on `ubuntu-22.04`. The `.deb` runs on Ubuntu 22.04 and newer.
- The packages are not signed.
- The package name is `librepaper`, because `desktop/tauri.linux.conf.json` sets the Linux product name to lowercase. The menu entry still reads LibrePaper.
- Pull requests build both packages in the "Linux desktop build" workflow (`linux-desktop-build.yml`, artifact `desktop-linux-packages`). They build unsigned Windows installers in the "Windows build" workflow (artifact `desktop-windows-unsigned`).
- No pull request builds the macOS app. The first macOS test of a change is a release tag.

## Test a release without publishing a stable version

Use a prerelease tag such as `v0.0.43-rc.1` from a test branch. Prerelease tags do not trigger the Homebrew, Scoop or Docker updates.

`tools/release/version` accepts only stable versions. Edit the version by hand in `Cargo.toml` and in the workspace entries of `Cargo.lock`.

```sh
# In a worktree on a test branch that includes the change
git commit -am "Release 0.0.43-rc.1 (signing test)"  # Commit the version bump and the change
git push origin HEAD                                  # Push the test branch
git tag v0.0.43-rc.1 && git push origin v0.0.43-rc.1  # Tag the commit, which starts the Release workflow
# Follow the run
gh run list --workflow release.yml --limit 3          # Find the run for the tag
gh run watch RUN_ID --exit-status                     # Wait for the run and fail if it fails
# Rerun only the failed jobs after fixing a secret (the tag's code is reused)
gh run rerun RUN_ID --failed
```

- A code fix needs a new tag.
- A secret fix needs only a rerun of the failed jobs. Wait until the whole run has finished before you rerun it.
- Delete test prereleases and tags you no longer need with `gh release delete vX --cleanup-tag`.

## What users see

- On Windows, a new certificate has no SmartScreen reputation yet. SmartScreen treats every new publisher this way. It has not been seen in a test yet. Browser downloads may show "Windows protected your PC" with the publisher name until reputation builds. Users click More info, then Run anyway.
- Scoop downloads carry no web mark, so users see no prompt.
- On macOS, the notarized DMG shows only the standard "downloaded from the Internet" prompt.
- On macOS, Gatekeeper blocks the unsigned `librepaper` binary from a CLI archive that a browser downloaded. Users must allow it in System Settings, Privacy & Security. The install script and Homebrew are not affected.
- On Linux, there is no prompt.

## Desktop updates

- The app checks `latest.json` from the latest stable release when it starts. It checks on macOS and on Windows NSIS installs only.
- Prereleases never reach `releases/latest`, so they never update installed apps.
- Scoop and Linux packages are updated by their package managers. The app does not update them, and it does not update MSI installs.
- The Homebrew cask sets `auto_updates true`, so Homebrew leaves updates to the app.
- The app checks each signature against the public key in `desktop/tauri.conf.json`.
- The updater key is the secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the `release` environment.
- The maintainer keeps the private key and its password in `~/.config/librepaper-release/`. Back them up somewhere safe. If they are lost, installed apps can never update again.
- Set `LIBREPAPER_UPDATE_URL` to point a running app at another `latest.json`, for testing.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Release fails with a missing updater signature | TAURI_SIGNING_PRIVATE_KEY is not set or createUpdaterArtifacts did not reach the bundle step | Check the release environment secrets |
| signtool fails on the first release after setting up Azure | The role assignment had not propagated | Wait, then rerun the failed jobs |
| The check reports `target\release\librepaper-desktop.exe` as NotSigned | Tauri signs the copy it bundles, not the build output | The zip takes its own signed copy (already handled in the workflow) |
| `MAC verification failed during PKCS12 import (wrong password?)` | The `.p12` uses the OpenSSL 3 default format | Export it again with `-legacy` |
| `resource path binaries/librepaper-aarch64-apple-darwin doesn't exist` | The universal build lacked a per-architecture sidecar | Fixed in the build action |
| The app fails with `VCRUNTIME140.dll` missing on Windows | The CLI was linked against the dynamic C runtime | Fixed with the static C runtime |
| Defender flags an unsigned installer as `Trojan:Win32/Bearfoos.B!ml` | A machine-learning false positive on an unsigned pull request build | Rebuild. Signed release builds have not been flagged so far |
