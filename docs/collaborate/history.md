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

Copying a checkpoint's link preserves the share key that gave you access. Editors can
name checkpoints and restore earlier versions.

Historical comparisons render captured source as HTML, including for documents
normally viewed as PDFs. They never compile a historical PDF. When rendering
or an exact target mapping is unavailable, the interface offers a source
comparison instead. A checkpoint's actor identifies who recorded the event,
not necessarily who authored every changed passage; uncertain authorship is
shown as unknown.

Signed-in owners choose their display timezone in Account settings. A
deployment's storage limits still apply to what a project holds in total.

The changes are also listed as prose, folded away under the count, and the
files that changed open source comparisons; editors can compare two
checkpoints and bring individual changes into the live source.

The same comparisons and whole-version restore are available from the History
panel in the browser. Restore requires editor access; it records the current
version before applying the earlier directory, so both remain available.

## Track changes

Enable track changes in the Changes sidebar. Insertions, deletions, and replacements become pending revisions. Accept keeps the proposed text; reject restores the prior text when safe. Changes that depend on other revisions require attention before an independent decision. Downloads include the current proposed text.

Suggestions and proposals from assistants appear in the Changes pane. These apply their replacement on acceptance and retain their discussions.
