# loro-codemirror, forked

Upstream is <https://github.com/loro-dev/loro-codemirror>, MIT. The fork is
<https://github.com/vincentarelbundock/loro-codemirror>, and it is that
package's `src/` with five changes to it, plus a test suite of its own at
`web/tests/unit/loro-codemirror.mjs`.

**The source is not in this repository.** `make loro-codemirror` fetches it
from the fork at the commit `loro-codemirror.lock` pins, verifies every file
against the digest recorded there, and writes it to `web/vendor/loro-codemirror`
-- which is build output and gitignored. It was vendored here until 2026-09-16,
which meant the same source existed twice and the two drifted: the fork sat
three fixes behind for a while, and nothing noticed until the fork's own test
suite was run against both. One copy, pinned, verified.

To change the binding: push to the fork, `make loro-update COMMIT=<full sha>`,
review the lock diff, and run the tests. The pin is a full commit sha, never a
branch or a tag, because a pin that can move on its own is not a pin.

Based on 0.3.3 and reconciled with upstream `main` (0.4.0) on 2026-09-16. Two
things came back from that comparison: `getCursorPos` is guarded in
`awareness.ts` and `ephemeral.ts` (it returns undefined for a cursor it cannot
resolve, and the unguarded `.offset` threw from inside presence rendering), and
the init dispatch is marked with `loroSyncAnnotation` instead of the
`isInitDispatch` flag 0.3.3 used. That flag was set before the equality check
and cleared by whatever update came next, so when the editor was created with
the document's text already in it, which is how `Editor.svelte` creates one,
the flag stayed up and the first edit was dropped. Upstream's issue 24.

## Why it is forked

The binding cannot keep an editor in step with a document that holds more than
one container, and every fault is inside a private loop, so none of it can be
worked around by a caller. A LibrePaper document is a map of files, so this is
not an edge case here -- it is every update.

On an imported change, upstream walks the batch like this:

```ts
for (let { diff, target } of e.events) {
    if (diff.type !== "text") return;      // not `continue`
    if (target !== text.id) return;        // not `continue`
    ...
    this.view.dispatch({ changes, ... });  // inside the loop
}
```

### One and two, in `sync.ts`: the import loop

Two faults in the same six lines.

* **`return` where `continue` belongs.** An import touching anything besides
  this editor's own text abandons the whole batch. Loro emits one event per
  container, with the map's event ahead of the text's, so in a document that
  is a map of files almost every real update is dropped.
* **The dispatch is inside the loop** while `changes` and `pos` are declared
  outside it, so a second matching event would apply the accumulated list
  twice. Loro emits one event per container per batch (checked with
  `importBatch` across two peers as well), so this does not happen in
  practice; the dispatch is moved out of the loop because that is the shape
  that is right, not because a failure was seen.

The fix is to skip rather than return, and to dispatch once for the batch.

### Three, in `undo.ts`: the same loop again

`UndoPluginValue` walks an undo's events exactly as `sync.ts` walked an
import's, with the same `return` in place of `continue`. An undo step that
touches the map as well as the text (typing into a file just created, say)
puts the map event first, so the undo never reached the view: the document
undid and the view did not, leaving them out of step by exactly the text that
had been taken back. The next edit was then dispatched at a position past the
end of the view.

Fixed the same way.

### Four, in `sync.ts`: the update was judged as a whole

`update()` decides whether a change came from this plugin by inspecting
`update.transactions[0]`. A `ViewUpdate` can carry several transactions --
`EditorView.dispatch(tr1, tr2)` makes one -- and upstream decides once for
the whole update from the first. With one of the plugin's own writes and one
user edit in the same update, that either copies the write into the document a
second time, or throws the edit away, depending on which came first.

`update()` now walks the transactions in order and writes only the ones that
are not its own. Each transaction's changes are in the coordinates of the
document it started from, and that is what the text holds once the ones before
it have been written or skipped, so the two stay in step. What this does not
cover is a user edit dispatched *ahead* of an import's write in the same
update: the import's positions were computed against a text that did not yet
hold the edit. That is a concurrent-edit race rather than a batching fault, and
nothing in this binding maps positions across it. Imports arrive from the
network, on their own task, so it is not a sequence CodeMirror produces.

### Five, in `undo.ts`: the undo happened inside a state field

`undoManagerStateField.update` called `UndoManager.undo()`. A state field's
update has to be a pure function of what it is handed -- CodeMirror runs it
while it is computing the new state, before that state exists -- and
`UndoManager.undo()` writes to the document. Loro delivers the resulting event
synchronously, so `UndoPluginValue`'s subscriber called `view.dispatch` from
inside the dispatch that was still being computed.

The inner transaction then updated the view against a state the outer one was
about to replace, and the two ended up apart by exactly the text that had been
undone. What a user saw was a crash from deep inside CodeMirror's view --
`Cannot destructure property 'tile' of 'o.pop(...)'`, thrown while walking a
tile tree whose length no longer matched its document -- with nothing in it
naming Loro, undo, or this package.

`undo()` and `redo()` now ask the manager directly. They are commands, so they
run between transactions, which is the one place it is safe to move the
document; the change comes back as a dispatch of its own. They also return
`false` when there is nothing to undo, so the key falls through, which is what
a CodeMirror command is expected to do.

With the field no longer acting on them, `undoEffect` and `redoEffect` would
be exports that compile, run and do nothing, so they are removed rather than
left as a trap. Anyone dispatching one should call `undo(view)` or
`redo(view)`, which is what the keymap already did.

Each change carries a comment at the point of it saying what upstream does and
why it is wrong here.

## The tests

`tests/unit/loro-codemirror.mjs` drives `LoroSyncPluginValue` and
`UndoPluginValue` on a map-of-files document with a peer supplying imports,
through a stand-in for `EditorView` that builds real transactions and hands
the plugins the fields of a `ViewUpdate` they read. No DOM, so it runs under
`bun run check` with the rest of the unit tests. The `.ts` sources need
`--experimental-transform-types`, which the `check` script passes, because
they use constructor parameter properties that plain type stripping rejects.

Against upstream 0.4.0 the suite fails four of its eight cases (the import
with a map event, both undo cases, and the mixed update). Against the
vendored copy as of the 2026-09-16 reconcile it fails five, the extra one
being the dropped first edit. Rerun it that way before trusting a change
here: copy the other version over `sync.ts` and `undo.ts`, run, restore.

## Why a fork and not a patch on disk

The changes are in `src/`, which is what upstream would take, rather than in the
built `dist/` that a patch tool would edit. Keeping the source means they can
be sent upstream unchanged, and means the next person reads TypeScript rather
than bundled output.

## What to do with it

All five faults are on the fork's `fix-multi-container-events`, one commit
each plus a changeset, at `ddbc6e7`. That branch is what this build fetches,
so the thing the editor runs and the thing waiting to be offered upstream are
the same bytes -- which is the point of fetching rather than vendoring.

**The pull request has not been opened.** The branch is ready; opening it is a
decision about putting your name on the claim, and upstream is alive enough
for it to matter: an outside issue filed 2026-09-02 was fixed and released by
2026-09-13.

Fault five goes upstream after all. It was held back on the reasoning that
upstream's `queueMicrotask` inside the state field gets the write out of the
update the same way moving it to a command does, making it a difference of
approach rather than a bug. The test suite here disproves that: against the
branch carrying upstream's `queueMicrotask`, both undo cases fail. Deferring
the write is not sufficient. The commit says so, and flags the removal of
`undoEffect`/`redoEffect` as the breaking change it is.

When a release contains the fixes: delete `loro-codemirror.lock`, the two
tools under `web/tools`, the `loro-codemirror` and `loro-update` targets and
the `$(LCM)` prerequisite in the Makefile, and the gitignore entry; restore
the dependency in `web/package.json`; point the imports in `Editor.svelte` and
`MergeEditor.svelte` back at the package name; and keep the test, pointed at
the package, since it is the only thing that checks any of this.
