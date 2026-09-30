# Installing the LibrePaper binary

Check what is present before installing anything:

```sh
librepaper --version
librepaper mcp --help
```

If the binary is missing, or present but without `mcp`,
install a release with the project's installer (Linux and macOS):

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

The installer verifies the release archive's checksum and installs into
`~/.cargo/bin` by default. If that is not on `PATH`, use the full binary path
in every command rather than editing the user's shell configuration.

- `LIBREPAPER_VERSION` selects a release.
- `CARGO_INSTALL_ROOT` selects an installation root (default: `~/.cargo`).
- Windows: use the matching executable from the
  [release page](https://github.com/LibrePaper/librepaper/releases).

Run `librepaper mcp --help` afterwards. If the installed release still
does not provide the adapter, **report the version mismatch and stop.** Do
not hand-roll HTTP requests against the server, and do not try to bypass the
link's permission checks.

## Signing in

Some deployments require a signed-in identity before a link will do anything.
`librepaper login --help` describes that for the deployment in question; sign-in
is interactive, so ask the user to complete it.

Signing in supplies attribution and satisfies a deployment's sign-in policy.
It does **not** widen the link: an agent holding a read link does not gain the
owner's editing rights by signing in as them.
