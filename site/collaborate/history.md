---
title: "History and revisions"
---

## What is kept

Everything. Editing is recorded as a graph of operations, and that graph is
retained in full, so any state the document has ever been in can be
reproduced from it. Nothing is written on a timer and nothing is thinned as
it ages.

Versions are the moments somebody asked for: a checkpoint you name in the
History panel, a restore, an accepted proposal, a commit from the CLI. Each
of those records the whole directory, so a chapter and the file that
includes it can never come back out of step. A comment on the live draft is
anchored to the moment it was made rather than to a version, so a round of
review no longer leaves a copy of the paper behind for every remark.

In the reader, the history button opens the same list beside the document,
newest first, with the live document as the top row. Picking a moment shows
the document as it was then, with what changed since the baseline struck
through and underlined in place, the way a version history does it; the head
of the panel counts the changes and steps through them. A row's compare
action makes that checkpoint the baseline, and a bracket down the timeline
shows the range. "Back to now" returns to the live document. Copying a
checkpoint's link preserves the share key that gave you access. Editors can
name checkpoints and restore earlier versions.

Historical comparisons render captured source as HTML, including for documents
normally viewed as PDFs. They never compile a historical PDF. When rendering
or an exact target mapping is unavailable, the interface offers a source
comparison instead. A checkpoint's actor identifies who recorded the event,
not necessarily who authored every changed passage; uncertain authorship is
shown as unknown.

Signed-in owners choose their display timezone in Storage settings. A
deployment's storage limits still apply to what a project holds in total.

The changes are also listed as prose, folded away under the count, and the
files that changed open source comparisons; editors can compare two
checkpoints and bring individual changes into the live source.

The same comparisons and whole-version restore are available from the History
panel in the browser. Restore requires editor access; it records the current
version before applying the earlier directory, so both remain available.

## Track changes

Open the **Changes** sidebar and enable **Track changes** at the top. Edit the
source normally: insertions, deletions, and replacements become pending
revisions. The setting applies to your edits in this document and stays on
when you close the sidebar. Turning it off leaves pending revisions available
for review.

Use the Changes pane to inspect revisions, filter the queue, and accept or
reject individual or selected changes. Accept keeps the proposed text;
reject restores the prior text when it can do so safely. Changes that depend
on other revisions require attention before an independent decision.
**Show markup** controls the display without disabling tracking. Downloads
include the current proposed text.

Existing suggestions and proposals from assistants remain in the Changes
pane as legacy proposals. These apply their replacement on acceptance, and
retain their discussions. Checkpoint comparisons remain available in History.
