# loro-codemirror, forked

Upstream is <https://github.com/loro-dev/loro-codemirror>, MIT. The fork is
<https://github.com/vincentarelbundock/loro-codemirror>, and it is that
package's `src/` with four changes to it, plus a test suite of its own at
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

### Not in the fork: undo and redo as commands

There was a fifth change here, and it has moved to `web/src/lib/loro-undo.js`,
which is the only one of the five a caller can own.

`undo(view)` in the binding dispatches a `StateEffect`, and
`undoManagerStateField.update` undoes the document when it sees it. A state
field's update has to be a pure function of what it is handed -- CodeMirror
runs it while computing the new state -- so upstream defers the call with
`queueMicrotask` (their PR 19) to keep the write out of the transaction it was
asked in. That is enough to stop it re-entering: the crash it used to cause,
`Cannot destructure property 'tile' of 'o.pop(...)'` from inside the view, is
upstream's own fixed bug and not something this build can still hit.

What is left is smaller. The undo happens a microtask after the keystroke
rather than in it, and the command returns `true` whether or not there was
anything to take back, so the key never falls through to whatever is bound
behind it. Asking the manager from the command is simply better: a command runs
between transactions, which is the one place it is safe to move the document.

It is not worth a breaking change to someone else's package -- removing
`undoEffect` and `redoEffect` is what carrying it upstream would cost -- and
it does not need to be in the package at all, because this build constructs the
`UndoManager` itself and hands it to `LoroExtensions`. `lib/loro-undo.js`
keeps it in a `StateField` of ours and binds the keys at `Prec.highest`, above
the `Prec.high` `LoroExtensions` binds its own Mod-z at. Nothing in that file
refers to `web/vendor`, so it survives the switch back to the package.

## The tests

`tests/unit/loro-codemirror.mjs` drives `LoroSyncPluginValue` and
`UndoPluginValue` on a map-of-files document with a peer supplying imports,
through a stand-in for `EditorView` that builds real transactions and hands
the plugins the fields of a `ViewUpdate` they read. No DOM, so it runs under
`bun run check` with the rest of the unit tests. The `.ts` sources need
`--experimental-transform-types`, which the `check` script passes, because
they use constructor parameter properties that plain type stripping rejects.

Against upstream 0.4.0 the suite fails three of its eight cases: the import
with a map event, the undo with a map event, and the mixed update. Those three
are exactly the faults a caller cannot work around, which is the whole of what
the fork is for. Rerun it that way before trusting a change here: copy
upstream's `sync.ts` and `undo.ts` over the vendored ones, run, restore.

## Why a fork and not a patch on disk

The changes are in `src/`, which is what upstream would take, rather than in the
built `dist/` that a patch tool would edit. Keeping the source means they can
be sent upstream unchanged, and means the next person reads TypeScript rather
than bundled output.

## What to do with it

The four faults are on the fork's `fix-multi-container-events`, one commit
each plus a changeset, at `1c6f377`. That branch is what this build fetches,
so the thing the editor runs and the thing waiting to be offered upstream are
the same bytes -- which is the point of fetching rather than vendoring.

The fifth commit is not on it. It lives on `fix-undo-redo-commands`, is not
fetched by anything, and is kept only so the reasoning is not lost;
`lib/loro-undo.js` is what this build actually runs.

**The pull request has not been opened.** The branch is ready; opening it is a
decision about putting your name on the claim, and upstream is alive enough
for it to matter: an outside issue filed 2026-09-02 was fixed and released by
2026-09-13.

Fault five is not going upstream. The two undo cases that fail against
upstream's `queueMicrotask` fail on the command contract -- a synchronous
undo, and `false` when there is nothing to take back -- not on the document
ending up wrong, and with `lib/loro-undo.js` supplying the commands they pass
against upstream unchanged. Offering it would mean asking upstream to drop two
exported symbols for a difference of approach in code this build no longer
depends on them for.

When a release contains the fixes: delete `loro-codemirror.lock`, the two
tools under `web/tools`, the `loro-codemirror` and `loro-update` targets and
the `$(LCM)` prerequisite in the Makefile, and the gitignore entry; restore
the dependency in `web/package.json`; point the `LoroExtensions` imports in
`Editor.svelte` and `MergeEditor.svelte`, and the two plugin-value imports in
the test, at the package name. `lib/loro-undo.js` and everything importing it
stay as they are -- that is why the manager sits in a field of ours. Keep the
test, since it is the only thing that checks any of this.
