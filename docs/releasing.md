# Releasing packages

LibrePaper publishes the `librepaper` crate to crates.io and updates the
Homebrew formula and Scoop manifest after a successful stable release. The
Homebrew formula uses the four prebuilt cargo-dist archives for Intel and ARM
macOS and Linux. Scoop uses the prebuilt 64-bit Windows archive. These
installers use the release binary, which already contains the built browser
application.

## Repository secrets

Configure these Actions secrets in `LibrePaper/librepaper`:

| Secret | Use | Required access |
| --- | --- | --- |
| `CARGO_REGISTRY_TOKEN` | Publish the one `librepaper` crate to crates.io | crates.io publish token for `librepaper` |
| `HOMEBREW_TAP_GITHUB_TOKEN` | Push `Formula/librepaper.rb` to `vincentarelbundock/homebrew-tap` | Contents write access to that repository |
| `SCOOP_BUCKET_GITHUB_TOKEN` | Push `bucket/librepaper.json` to `vincentarelbundock/scoop-bucket` | Contents write access to that repository |

Keep the release workflow's repository `GITHUB_TOKEN` permissions limited to
the release tasks it needs. The two tap tokens are used only by their matching
update workflows. The update scripts pass them to Git through an HTTP
authorization header kept in the process environment; credentials do not go
in clone URLs or output.

## Publishing to crates.io

The repository is a workspace of internal sub-crates. crates.io gets one
package, `librepaper`, generated from the workspace by `tools/flatten/flatten`.

- Version: `[workspace.package]` in the root `Cargo.toml`; the tag check reads it.
- Output: `target/flat/`, never committed.
- The publish workflow builds the browser app, flattens, and publishes from `target/flat/` with `CARGO_REGISTRY_TOKEN`.
- Prepare the browser assets first (the executable embeds them):

```sh
tools/pins fetch                  # pinned renderers and loro-codemirror
(cd web && bun install --frozen-lockfile && bun run build)
```

Dry run:

```sh
tools/flatten/flatten lint        # rules the sub-crates follow
tools/flatten/flatten build       # generate target/flat/
(cd target/flat && cargo publish --dry-run --locked --allow-dirty)
```

After a release, smoke test the published crate:

```sh
cargo install librepaper --locked   # on a clean machine
librepaper --version
```

## Stable releases

Push a stable tag in `vMAJOR.MINOR.PATCH` form, such as `v0.1.0`. The update
workflows run only after the `Release` workflow succeeds for a tag push. They
ignore pull request completions and prerelease tags. A manual run accepts a
stable tag and checks out that tag before resolving and publishing its
artifacts.

Each channel updater downloads its release archives and cargo-dist `.sha256`
files, checks that each digest is a 64-character SHA-256 value, and compares
it with a fresh hash of the downloaded archive. A missing or mismatched
checksum stops that updater without changing its package repository.

## Install commands

The user-facing commands for every channel live on the [install page](install.md) only. Update that page when a channel changes.

LibrePaper is distributed under the MIT License. The package manifest and the
generated Homebrew and Scoop definitions declare `MIT`; the license text is in
the repository's root `LICENSE` file.
