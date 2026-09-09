# Resume the refactor

State at the 2026-09-08 handoff. All twelve tracks in SPEC-refactor.md
are merged to `live-markdown-editor`; see STATUS.md for the commit-level
index and each `NN-*.md` for evidence and limitations. Nothing from the
roadmap is left on a branch.

## How integration was done

The first attempt at this refactor kept its branches only in temporary
clones under `/tmp`, and they were lost. This time every branch is a ref in
the main repository (`git branch | grep refactor/`) with a worktree under
`/tmp/librepaper-refactor-<track>` and its own Cargo target directory under
`/tmp/librepaper-target-<track>`; delete the worktrees and target directories
once the branches are merged (`git worktree remove`, then `rm -rf` the
target directories), but keep the branch refs until the merge is confirmed.

`refactor/integration` (worktree `/tmp/librepaper-integration`) is where
branches were merged, conflicts resolved, and the full suite run. The main
checkout was never used for merges because the user was committing to
`live-markdown-editor` concurrently; the main branch only ever moved by
`git merge --ff-only refactor/integration`, after merging any new tip of
`live-markdown-editor` into the integration branch first.

Building needs `web/dist` (README copy, docs images, `bun run build` in web,
`make wasm`); worktrees symlink it to the root copy, and that symlink is not
matched by `.gitignore`, so never `git add -A` in a worktree.

## Remaining follow-ups outside the roadmap's scope

- Type `CatalogError::Conflict` so `CatalogError::refusal` no longer reads
  message text (one function, pinned by a test; 112 construction sites).
- Give `storage/backup.rs` a `Catalog` snapshot/verify API so it stops opening
  its own SQLite connection outside the execution boundary and shutdown.
- Paginate `catalog_entries` and `documents()`, which the track 1 inventory
  flags as unbounded reads.
- Distributed backup ownership (conditional claims, fencing, stale-owner
  recovery) remains a separate proposal; single-authority deployments are
  what track 12 supports.
- `set_main_file` and `add_text` refusals during publication are typed now,
  but the room fenced by the encoded-size backstop stays read-only until it
  is reopened; a transient fault there needs a reopen path.
