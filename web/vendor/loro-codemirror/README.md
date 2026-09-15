# loro-codemirror, forked

Upstream is <https://github.com/loro-dev/loro-codemirror> at 0.3.3, MIT, and the
LICENSE beside this file is theirs. This is that package's `src/` with one
change.

## Why it is forked

The sync plugin cannot keep several editors on one document in step, and the
fault is inside a private loop, so it cannot be worked around by a caller.

On an imported change, upstream walks the batch like this:

```ts
for (let { diff, target } of e.events) {
    if (diff.type !== "text") return;      // not `continue`
    if (target !== text.id) return;        // not `continue`
    ...
    this.view.dispatch({ changes, ... });  // inside the loop
}
```

Two bugs:

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
`sync.ts` carries a comment saying so at the point of the change.

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
