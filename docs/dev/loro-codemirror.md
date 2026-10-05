# The pinned loro-codemirror fork

- Origin: MIT-licensed [upstream](https://github.com/loro-dev/loro-codemirror),
  consumed through [our fork](https://github.com/vincentarelbundock/loro-codemirror).
- Fork fixes: multi-container sync/undo, mixed transaction ordering, presence
  refresh, unresolved cursors, and initialization dropping the first edit.
- Local undo/redo commands live in [`loro-undo.js`](../../web/src/lib/loro-undo.js).

The fork source is not committed here. [`web/loro-codemirror.lock`](../../web/loro-codemirror.lock)
pins a full commit SHA and SHA-256 digests for each required source file.
[`tools/assets/pins fetch`](../../tools/assets/pins) verifies those bytes and
writes them to ignored build output at `web/vendor/loro-codemirror/`, which the
editor and tests import.

## Update the fork pin

1. Make and push the binding changes to the fork.
2. From the LibrePaper repository, update the lock with the pushed full commit
   SHA, then fetch the verified source:

   ```sh
   tools/assets/pins update loro <full-40-character-commit-sha>
   tools/assets/pins fetch
   ```

3. Review the `web/loro-codemirror.lock` diff, then run the focused binding
   suite:

   ```sh
   cd web
   node --experimental-transform-types --test tests/unit/loro-codemirror.mjs
   ```

The update command reads each file at the chosen commit and records its digest;
the fetch command later refuses files that differ from those recorded digests.
Never pin a branch or tag: only a full commit SHA fixes the source revision.

The focused suite exercises the fetched plugin source. Keep it with the lock
and fetch tooling. Retire the fork only after a package release contains the
needed fixes and the editor and tests have moved to that package; update the
fetch and build dependencies together.
