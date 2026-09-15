# loro-codemirror, forked

Upstream is <https://github.com/loro-dev/loro-codemirror> at 0.3.3, MIT, and the
LICENSE beside this file is theirs. This is that package's `src/` with one
change.

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


Each change carries a comment at the point of it saying what upstream does and
why it is wrong here.

## Why a fork and not a patch on disk

The change is in `src/`, which is what upstream would take, rather than in the
built `dist/` that a patch tool would edit. Keeping the source means the fix can
be sent upstream unchanged, and means the next person reads TypeScript rather
than bundled output.

## What to do with it

Send it upstream. Until it lands, this is what the editor imports — see
`web/src/components/Editor.svelte`. Note that upstream has not published since
October 2025 while `loro-prosemirror` has moved in that time, so a reply may be
slow; that is the reason for forking rather than waiting.

When a release contains the fix: delete this directory, restore the dependency
in `web/package.json`, and point the two imports in `Editor.svelte` back at the
package name.
