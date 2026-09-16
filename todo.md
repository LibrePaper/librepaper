# What is left

Written 2026-09-16. `SPEC-loro.md` §6 is the record of the migration itself;
this is only the queue of what has not been done.

## Done since the last version of this file

**§5.3's first two capabilities are built**, both entirely in the browser.

- **Rival changes stand together.** Two proposals that change the same words
  are one question with two answers, and the Changes panel now shows them as
  one contested card rather than as unrelated rows several screens apart.
  `markContention` in `web/src/lib/proposals.js`; adjacent edits and stale rows
  are deliberately not grouped.
- **A chosen set of proposals reads as prose.** Tick changes in the queue and
  "Read with N proposals" opens the two-pane `MergeEditor` with the paper as
  those proposals would leave it — read-only, on a fork, nothing persisted.

Neither needed the server. §5.3 proposed `proposal-preview` and
`proposal-preview-state` messages for the second one; they turned out to be
unnecessary, because `proposal-list` already carries every open proposal's
branch bytes. That is recorded in §5.3 rather than quietly dropped.

**The loro-codemirror fork is reconciled with upstream.** Four fixes are
prepared as a branch to send upstream; a fifth is no longer ours to report,
and a `getCursorPos` guard came back the other way.

## What is actually left

1. **The upstream pull request has not been opened.** The branch is pushed and
   the state of it is recorded in `web/vendor/loro-codemirror/README.md`, which
   is the authority here rather than this file -- including that the branch
   still carries the whole-update version of fault four and wants refreshing
   from the vendored `sync.ts` first. Opening the request needs your GitHub
   identity.

2. **`bulk` in `Changes.svelte` is half-wired.** `selectedRows`, `bulk` and
   `onbulk` are all there; the Reader has never passed `onbulk`, so the second
   half is dead. The checkbox those functions wanted now exists for previews,
   so finishing it is small -- or delete `bulk` and `onbulk` and leave the
   selection to the reading. It should not stay as it is.

3. **SPEC-security finding 1.** The document origin is derived from the `Host`
   header and never asserted, and nothing refuses to start when the deployment
   cannot support the boundary it assumes. Read the finding in
   `docs/specs/SPEC-security.md` rather than this line: what it requires is
   explicit reader and document origins with routes bound to them, not a DNS
   check, and the wording there is more careful than a summary can be. The
   spec's own order of work puts it first, and it is the one item here that
   gates anyone else using LibrePaper at all.
