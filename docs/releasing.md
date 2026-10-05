# Package release channels

See the [production and release checklist](dev/production.md) for versioning,
checks, tagging, and the separate manual VPS deployment.

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
- Homebrew uses the Linux and macOS x86_64 and ARM64 archives. Scoop uses the
  x86_64 Windows archive. Both install the released binary, which includes the
  browser application.
- User install commands live on the [install page](install.md). Update it when
  a distribution channel changes.

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
