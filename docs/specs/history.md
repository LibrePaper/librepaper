# SPEC: remaining history work

Steps 8 and 9 are implemented: passage and file comparisons, replacement
quotations, the live merge editor, CLI diff and restore, authorized whole-version
restore, and checkpoint links. The work below remains.

## Git provenance

When `publish` or `sync` runs inside a git repository, record the current
commit and whether the working tree is dirty on the checkpoint it causes.
`--no-git` leaves these fields out. The checkpoint schema already accepts
`commit` and `dirty`; the remaining work is collecting and sending them
from both commands.

This gives authors a pointer into their local source history, including
Quarto projects. It does not make LibrePaper a git remote.

## Pinning, later

Allow an editor to set “readers see this checkpoint” from the history panel.
Readers then see that checkpoint, with a line saying a newer text exists,
while editors keep working on the live document. An editor can unpin or pin
a later checkpoint.

This is deferred until there is demand for keeping a shared draft fixed
while editing its successor.

## No retained rendering fallback

The cost-minimization policy supersedes the earlier proposal to fall back to a
stored rendering when the live source fails. Readers compile the source on
demand in the browser or through their local companion; a failure leaves the
source and diagnostics available instead of serving an older generated result.
Historical comparisons render captured source transiently in the browser and
never use or retain stored PDFs or HTML.
