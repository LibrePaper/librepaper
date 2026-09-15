# What is left

Written 2026-09-15, after finishing the work the previous version of this file
described. `SPEC-loro.md` §6 is the record of the migration itself; this is
only the queue of what has not been done.

## Done since the last version of this file

**Phase 2's gate passes.** `editor-browser.mjs` passes whole, and so does
`insert-browser.mjs`. Three separate faults were in the way, none of them
about Loro, all recorded in §6 Phase 2: the undo command moved the document
from inside a CodeMirror state field; `MergeEditor` built its `UndoManager`
from a text rather than from the document; and the browser tests held two wasm
modules, so a version vector handed from one to the other was a pointer into
the wrong memory.

Two more came out with them, and neither had anything to do with the
substrate:

- **One Ctrl-Z after creating a file deleted the file.** The undo manager is
  one per document and merges everything within a second, so creating a file
  and typing the first word into it were one undo step. Directory changes now
  commit under `DIRECTORY_ORIGIN` and the manager is told to leave them alone.
- **An insertion touching the open file wrote to the document and not to the
  editor.** `applyInsertResult` keyed its per-file plans on the `LoroText`
  handle, and `session.textOf` returns a fresh wrapper each call, so the file
  on screen was filed twice; the second copy bypassed CodeMirror. Enabling a
  Quarto table of contents put the frontmatter in the file and showed none of
  it.

**The review interface exists.** `proposal-marks.js` is wired into
`Editor.svelte` and draws what a proposal would add and take out, in the text
it would change. The Reader computes each open proposal's hunks (§5.2 — both
sides compute, neither sends) and hands them to the Changes panel as one card
per hunk, which is the unit a decision names. A stale hunk is shown and cannot
be answered.

**`Changes.svelte` is about proposals.** The 22 references to the deleted
revision mechanism are gone, along with the undo-a-decision menu item that was
wired to a callback nothing passed. It has a browser test now —
`changes-browser.mjs` — which it never had while it was the panel this all
depended on.

**Tracked editing works again.** The Track changes switch opens a branch and
the editor binds to it; switching it off flushes and leaves the proposal with
the server. `proposals.js` had three protocol faults that made this dead code
regardless: it never read the id back from `proposal-opened`, its updates
omitted `proposal_id` and `tip`, and `decide` used camelCase keys the server
does not read and refused to run at all unless the reviewer happened to have a
proposal of their own open.

## What is actually left

1. **The client→server proposal round trip has no automated coverage.** Every
   piece is tested apart — hunk grouping against `hunks.rs`, the marks, the
   panel, the server's own `room/proposals.rs` tests — and nothing exercises
   open → update → decide → resolve across a socket. That gap is exactly the
   shape of the one Phase 2 kept falling into, and it needs Postgres, so it
   belongs with the `#[ignore]`d Postgres tests rather than in `make test`.

2. **`proposal-open` carries a base the server ignores.** §5.1 says the
   message carries the branch's base frontier; `room.open_proposal` forks at
   its own `state_frontiers()` instead. The two agree in a quiet room and
   diverge under concurrent edits, which would put somebody else's words
   inside a proposal. Either honour the client's base or drop it from the
   message and say why.

3. **Three §9 follow-ups**, still deliberately deferred: evaluate `LoroTree`
   to collapse `files` + `paths`; reconsider checkpoint density now that
   contract 6 makes every intermediate state reachable; and delete
   `max_encoded_snapshot_bytes`'s speculative-encode machinery.

4. **The fork has not been sent upstream.** `web/vendor/loro-codemirror/` now
   carries five fixes, and its README says what to delete when a release
   contains them. Pushing to someone else's repository is yours to decide.
