# Installing the LibrePaper binary

Check what is present before installing anything:

```sh
librepaper --version
librepaper agent --help
```

If the binary is missing, or present but without the `agent` subcommands,
install a release with the project's installer (Linux and macOS):

```sh
curl -fsSL https://raw.githubusercontent.com/LibrePaper/librepaper/main/deploy/install.sh -o /tmp/librepaper-install.sh
sh /tmp/librepaper-install.sh
```

The installer verifies the release archive's checksum and installs into
`~/.local/bin` by default. If that is not on `PATH`, use the full binary path
in every command rather than editing the user's shell configuration.

- `LIBREPAPER_VERSION` selects a release.
- `LIBREPAPER_BIN_DIR` selects an installation directory.
- Windows: use the matching executable from the
  [release page](https://github.com/LibrePaper/librepaper/releases).

Run `librepaper agent --help` again afterwards. If the installed release still
does not provide the commands, **report the version mismatch and stop.** Do
not hand-roll HTTP requests against the server, and do not try to bypass the
link's permission checks.

## Signing in

Some deployments require a signed-in identity before a link will do anything.
`librepaper login --help` describes that for the deployment in question; sign-in
is interactive, so ask the user to complete it.

Signing in supplies attribution and satisfies a deployment's sign-in policy.
It does **not** widen the link: an agent holding a read link does not gain the
owner's editing rights by signing in as them.
