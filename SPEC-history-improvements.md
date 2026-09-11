# History panel improvements

Status: proposed

## Purpose

Make it easy to find an earlier draft, read it, understand what changed, and
restore it. The panel should remain useful after hundreds of checkpoints.
Browsing history must make the viewed version and the comparison baseline clear.

Google Docs provides the interaction reference: a chronological sidebar with
expandable groups, named versions, a historical preview in the document pane,
and an option to show changes. The checkpoint intervals below are LibrePaper
design choices, not claims about Google's implementation.

## Existing behavior

`web/src/components/History.svelte` already provides a timeline, naming,
checkpoint links, restore controls, inline change controls, and change navigation.
`web/src/lib/reader/history.svelte.js` manages comparison endpoints.
`web/src/lib/history.js` groups checkpoints by day and folds consecutive unnamed
checkpoints by the same author, preserving the first and last entries.

Build on these capabilities. This specification changes presentation and makes
checkpoint and comparison behavior explicit. It complements the remaining work
in `docs/specs/history.md`; git provenance, reader pinning, and compile-failure
fallback remain separate work.

## Checkpoint creation

Checkpoint creation and timeline grouping have separate rules. Grouping changes
what is initially visible; it must not delete or merge stored checkpoints.

Use these initial checkpoint thresholds:

- Create a checkpoint after 30 seconds without document edits, if content has
  changed since the last checkpoint.
- During continuous editing, create a checkpoint no later than five minutes
  after the first uncheckpointed edit. Further edits must not postpone it.
- Preserve immediate checkpoints for meaningful actions such as publishing,
  syncing changed content, restoring a version, and explicitly naming the
  current draft. Preserve existing event-triggered checkpoint behavior unless
  a separate change is justified.
- Preserve checkpointing when the last editor leaves, subject to the existing
  connection and persistence lifecycle.
- Avoid duplicate automatic checkpoints when document content has not changed.
  Naming an existing checkpoint updates its label without duplicating content.

Apply timing to document activity across collaborators, not independently to
each browser. Any checkpoint resets the pending interval for the content it
captures. Checkpoint timing must not delay normal durable saving of edits.

These thresholds should be constants that can be tuned without changing the
panel's behavior. Audit existing checkpoint triggers before implementation.

## Timeline presentation

Show the current document at the top, followed by checkpoints newest first.
Group dates in the viewer's local timezone, using Today, Yesterday, and explicit
dates for older entries.

Within each day, collapse routine activity into editing sessions:

- A gap of at least 15 minutes between adjacent checkpoints starts a new group.
- An author change starts a new group, retaining the existing attribution model.
- A named checkpoint or meaningful milestone is always visible and breaks a
  routine group. Publication and restoration are milestones; background sync
  and rendering alone need not be.
- Only collapse groups of three or more checkpoints. Show smaller groups directly.

A collapsed session shows its time range, author, and checkpoint count, for
example `09:15–10:30 · Vincent · 8 versions`. Its disclosure arrow expands every
checkpoint in that group. Clicking the session label previews its newest
checkpoint. Expansion must also support collapsing the group again.

Every checkpoint returned by the history service remains reachable through
expansion. This does not introduce a new storage-retention guarantee or override
existing server retention policies.

Checkpoint rows show the version name when present, timestamp, and author.
Milestones may have a short reason such as Published or Restored. Raw hashes and
routine checkpoint reasons belong in details or tooltips, not primary labels.
Use “version” in user-facing text; “checkpoint” remains the implementation term.

Add a “Named versions only” filter. Keep Current version visible while filtering.
If the selected version is filtered out, preserve its preview and identify it in
the preview header. Show an explanatory empty state when no versions are named.

## Selecting and displaying a version

Opening history keeps the current document visible until a historical version
is selected. Clicking a checkpoint displays that version in the main document
pane, read-only, with its normal layout and formatting.

The preview header contains:

- Back to current.
- The selected version's name and full timestamp.
- Restore this version, for users authorized to restore.

Historical previews stay fixed while collaborators edit the current document.
New checkpoints may appear in the timeline without changing the selection,
comparison endpoints, or the user's scroll position. A direct checkpoint link
opens the corresponding preview and reveals its row, expanding its group.

Returning to current restores the current document and its normal editing
behavior. Preserve the user's prior position where practical. Loading failures
must be explicit and must not present current content as the requested old version.

## Showing changes

Provide a “Show changes” toggle, initially enabled. Retain its value while the
panel remains open. Turning it off shows a clean historical document.

Selecting a checkpoint normally compares its immediate predecessor with that
checkpoint. Explicitly reset this default baseline on each ordinary selection;
do not silently retain a baseline from a previously selected version. The first
checkpoint has no predecessor: show its clean contents and “First version.”

When changes are enabled:

- Mark additions and deletions within the rendered prose where supported.
- Distinguish additions and deletions through underline/strikethrough or other
  non-color cues. Author colors supplement these distinctions.
- State the baseline using a readable name or timestamp, such as
  “Compared with Sep 10, 10:30.”
- Provide previous/next change controls and a position such as “Change 2 of 7.”
- Keep file details and source comparisons available as secondary views.
- Show an explicit no-changes state when the comparison is empty.

For a session summary, the preview and default comparison use its newest
checkpoint and that checkpoint's immediate predecessor. Do not imply that this
comparison covers the entire session.

## Explicit comparison with current

Provide a separate “Compare with current” action for a historical version.
This enters comparison mode with the selected historical version as the baseline
and the current document as the target. Identify both endpoints prominently.

Capture the current target when entering this mode so the comparison stays
stable during review. If current content changes, show “Newer edits available”
with an explicit refresh action. Refreshing updates the target and recomputes
the comparison. It must not happen silently.

Keep existing arbitrary-range comparison capabilities available as secondary
controls. Every comparison must label its baseline and target. Ordinary version
selection exits explicit comparison mode and restores the default predecessor
comparison.

## Format-specific rendering

For Markdown and other supported rendered-text formats, show the historical
document with inline word changes where the renderer supports them.

For Typst and LaTeX, display the stored PDF associated with the selected
checkpoint. Do not introduce server-side compilation. When reliable inline
change marks are unavailable, offer the existing source/file comparison and
explain that rendered change highlighting is unavailable for this format.

If a checkpoint has no historical PDF, state that its preview is unavailable
and offer its source where supported. Never substitute a newer PDF without
identifying it. Historical assets must correspond to the selected checkpoint.

## Naming and restoration

Allow authorized editors to name and rename versions from a row menu. Naming
the current draft first captures pending content if necessary. Named versions
remain visible outside collapsed sessions.

Restoring is separate from selecting or comparing. Identify the selected version
in the restore confirmation, preserve pending current work through the existing
restore workflow, and append the restoration to history. Earlier versions must
remain available under existing retention rules. On success, return to current
and show the new restoration entry. On failure, retain the preview and show the
error without implying that restoration succeeded.

Preserve existing server-side history, naming, and restore authorization.

## Layout sketch

```text
← Back to current    Viewing Sep 10, 10:42    Restore this version

┌──────────────────────────────────────┬─────────────────────────┐
│                                      │ History                 │
│ Document as it was at 10:42           │ □ Named versions only   │
│                                      │                         │
│ Inline changes when enabled          │ Current version         │
│                                      │                         │
│                                      │ Today                   │
│                                      │ ● Submitted draft       │
│                                      │   10:42 · Vincent       │
│                                      │ ▸ 09:15–10:30           │
│                                      │   Vincent · 8 versions  │
│                                      │                         │
│                                      │ ☑ Show changes          │
│                                      │ Compared with 10:30     │
│                                      │ Change 2 of 7     ↑ ↓   │
└──────────────────────────────────────┴─────────────────────────┘
```

Use existing panel styling. Keep selection, expansion, filters, comparison,
and change navigation keyboard-accessible, with visible focus and accessible
labels. Selected rows must remain identifiable without relying on color.

## Acceptance criteria

1. Hundreds of routine checkpoints produce a compact timeline while every
   returned checkpoint remains accessible through expansion.
2. Two same-author editing periods separated by at least 15 minutes appear as
   separate groups. Named versions and milestones never disappear inside groups.
3. Ordinary checkpoint selection always previews the selected version and
   compares it with its immediate predecessor, regardless of prior selection.
4. Turning off Show changes produces a clean historical preview. The first
   version and empty comparisons have clear states.
5. Incoming edits cannot change a historical preview or an explicit comparison
   until the user requests a refreshed current target.
6. Checkpoint scheduling covers pauses and continuous editing without emitting
   duplicate automatic checkpoints for unchanged content.
7. Restoration appends history, preserves recoverability of the preceding current
   state under existing retention rules, and respects server authorization.
8. Historical PDF previews use the selected checkpoint's artifact or report its
   absence. Unsupported rendered diffs have an explicit source-view alternative.
9. Grouping, baseline selection, scheduling boundaries, and concurrent-edit
   behavior receive focused automated coverage. Verify the main browsing flow
   and keyboard controls in the browser.

## References

- [Google Docs: Find what's changed in a file](https://support.google.com/docs/answer/190843?hl=en)
- [Illustrated Google Docs version-history walkthrough](https://zapier.com/blog/google-docs-revision-history/)
