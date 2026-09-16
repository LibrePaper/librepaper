# loro-codemirror, forked

Upstream is <https://github.com/loro-dev/loro-codemirror>, MIT, and the LICENSE
beside this file is theirs. This is that package's `src/` with five changes to
it.

Based on 0.3.3 and reconciled with upstream `main` (0.4.0, unpublished) on
2026-09-16. Two things came back from that comparison and are in here now:
`getCursorPos` is guarded in `awareness.ts` and `ephemeral.ts` (it returns
undefined for a cursor it cannot resolve, and the unguarded `.offset` threw
from inside presence rendering), and upstream now marks its own init dispatch
with `loroSyncAnnotation` instead of the `isInitDispatch` flag 0.3.3 used.

## Why it is forked

The binding cannot keep an editor in step with a document that holds more than
one text, and every fault is inside a private loop, so none of it can be worked
around by a caller. A LibrePaper document is a map of files, so this is not an
edge case here -- it is every update.

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

Two bugs in the same six lines.

* **`return` where `continue` belongs.** An import touching anything besides
  this editor's own text abandons the whole batch. A LibrePaper document is a
  map of files, so almost every real update carries a map event beside the text
  one — and the text change was dropped with it.
* **The dispatch is inside the loop** while `changes` and `pos` are declared
  outside it, so two events for one text apply the accumulated list twice. The
  second application lands past the end of the document, which surfaces as
  `RangeError: Invalid position N in document of length M` from CodeMirror
  rather than as anything that names the plugin.

The fix is to skip rather than return, and to dispatch once for the batch.

### Three, in `undo.ts`: the same loop again

`UndoPluginValue` walks an undo's events exactly as `sync.ts` walked an
import's, with the same `return` in place of `continue`. So undoing anything in
a document with more than one text never reached the view: the document undid
and the view did not, leaving them out of step by exactly the text that had been
taken back. The next edit was then dispatched at a position past the end of the
view.

Fixed the same way.

### Four, in `sync.ts`: only the first transaction was asked

`update()` decides whether a change came from this plugin by inspecting
`update.transactions[0]`. A `ViewUpdate` can carry several, so a change the
plugin had already written to the document, arriving behind another
transaction, was written a second time.

Every transaction is now asked.

### Five, in `undo.ts`: the undo happened inside a state field

`undoManagerStateField.update` called `UndoManager.undo()`. A state field's
update has to be a pure function of what it is handed — CodeMirror runs it
while it is computing the new state, before that state exists — and
`UndoManager.undo()` writes to the document. Loro delivers the resulting event
synchronously, so `UndoPluginValue`'s subscriber called `view.dispatch` from
inside the dispatch that was still being computed.

The inner transaction then updated the view against a state the outer one was
about to replace, and the two ended up apart by exactly the text that had been
undone. What a user saw was a crash from deep inside CodeMirror's view —
`Cannot destructure property 'tile' of 'o.pop(...)'`, thrown while walking a
tile tree whose length no longer matched its document — with nothing in it
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

## Why a fork and not a patch on disk

The changes are in `src/`, which is what upstream would take, rather than in the
built `dist/` that a patch tool would edit. Keeping the source means they can
be sent upstream unchanged, and means the next person reads TypeScript rather
than bundled output.

## What to do with it

Send them upstream. Until they land, this is what the editor imports — see
`web/src/components/Editor.svelte`. Note that upstream has not *published* since
October 2025, though `main` has moved to an unreleased 0.4.0, so a reply may be
slow; that is the reason for forking rather than waiting.

Four of the five go upstream, and a branch is prepared with one commit each:
faults one, two, three and four, all still present on `main` as of 2026-09-16.

The fifth does not. Upstream now wraps `value.undo()` in `queueMicrotask`
inside the state field, which gets the write out of the update the same way
moving it to a command does. That makes it a difference of approach rather than
a bug to report, and removing `undoEffect`/`redoEffect` — which only follows
from our version — would be an API break to ask for on top of it. Keep both
here; raise them separately, if at all.

When a release contains the fixes: delete this directory, restore the dependency
in `web/package.json`, and point the imports in `Editor.svelte` and
`MergeEditor.svelte` back at the package name.
