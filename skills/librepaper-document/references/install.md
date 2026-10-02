# Installing the LibrePaper binary

Check what is present before installing anything:

```sh
librepaper --version
librepaper mcp --help
```

If the binary is missing, or present but without `mcp`, install or upgrade it
by following the one set of install instructions at
<https://librepaper.org/install.html> (installer scripts, Homebrew, Scoop and
Cargo). Fetch that page and use the method that matches how the existing
binary was installed, or the installer script when there is none.

The installer script installs into `~/.cargo/bin` by default. If that is not
on `PATH`, use the full binary path in every command rather than editing the
user's shell configuration.

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
