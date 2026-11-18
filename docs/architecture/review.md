---
title: "Comments and track changes"
---

## Comments

Each comment keeps three things.

| | what it is | lifetime |
| --- | --- | --- |
| Original anchor | what the reviewer selected: a checkpoint, a file, a UTF-16 range and the quoted words, or the whole document | in Postgres, written once, never modified |
| Anchor record | where the passage is now | in the document's `anchors` root, kept in step by the editing session |
| Derived status | exact, modified, pending deletion, deleted or unresolved | computed on each read, never stored |

Every anchor is a range, not a point. The anchor record holds the file, two Loro cursors at the ends of the passage, and the quoted words. Edits move the cursors, so a record follows its text and grows or shrinks with it. The quoted words never change, so a passage that was edited and then moved still reads as modified. Re-attaching never searches the text for words.

**Status.** Exact means the passage is where it was. Modified means it is there but edited. Pending deletion means it sits inside a tracked deletion nobody has decided. Deleted means it is gone. Unresolved means nothing can be said, such as a missing file or a comment with no record yet. A comment about the whole document has no passage, so it has no record and no status. Editors and suggesters get status from the server, and each editing session recomputes it after every change. Readers and commenters get neither the anchor nor the status. Their marks are painted from the words the page showed.

**Deletions and undo.** A deletion that covers a whole passage detaches its record. The record keeps its quote and the passage's length, and the editing session remembers the text it deleted. When an insertion brings exactly that text back, the comments inside it re-attach. This covers cut and paste, drag and drop, and undo and redo, within a file or across files. Rejecting a tracked deletion puts its comments back by fragment and offset. Text typed again by hand, pasted from another session, or left by a merge does not re-attach. The comment keeps its quote until an editor selects the passage and chooses "Attach to selection".

**Lifecycle.** A commenter's comment and its record are written in one transaction. Deleting a comment deletes its record. Erasing an account anonymizes the comment and keeps the record. When a room opens, each passage comment without a record gets one. A comment whose original state can no longer be rebuilt, such as one older than a history trim, gets none and reads as unresolved. Before a history trim takes its snapshot, the server heals every record whose cursor sits on deleted text, and marks a collapsed passage detached.

**Locating.** When a comment is made, the server maps the rendered page to source by flattening both with syntax blanked and whitespace collapsed, then finds the selection. A preview selection may carry a line hint from the renderer's `data-sourcepos`, which names a file and a few lines; the search looks there first and falls back to the whole document. The words still decide the range. It refuses to locate a passage that reads identically in several places. Locating happens once, at creation. The browser keeps each file's resolved offsets for the session, so an edit resolves no cursor. In a spike, a keystroke at 1,000 anchors cost about 0.24 ms through that index. Resolving every cursor on each keystroke would have cost 26 to 85 ms.

**Limits.** A passage split and moved in parts keeps the part that stayed. Two sessions re-pinning one record at once keep one answer. A merge that collapses a record is written detached by the next editing session. The move memory lasts for one editing session, or one agent patch on the server, so a reload or a separate patch forgets it. After a trim, a comment's original state cannot be shown.

Comments use concepts from the W3C Web Annotation Data Model, adapted to source anchors. The CLI's `librepaper export` writes a project snapshot of source and assets, not annotations. There is no comments export to JSON-LD or other formats in the CLI.

To save comments, choose **File > Download > Comments**. It writes every thread of the project as Markdown, with its quoted passage, its replies, and a note on resolved comments and on passages no longer in the document.

## Track changes

A tracked edit goes into the text at once, like any other edit, and the document also records it as a pending change. The editor, read links, agents and exports all show the text with every pending change in it.

A change is one author's edit of one region: the text it inserted and the original text it deleted. Replacing "cat" with "tabby" is one change, and either side can be empty. A change record keeps two cursors, one at each end of the inserted text, and they follow the text as others edit around it.

Deleted text is kept as fragments. Each fragment has its own cursor on the character that followed it, and a rank. Fragments that resolve to one point form a gap, in rank order, so several authors deleting in the same place come back in document order. No record ever moves, so two people editing near one point at the same moment never overwrite each other.

Accepting a change removes its record and leaves the text in place. Rejecting it removes the inserted text and puts its fragments back into their gaps. Other changes' fragments in those gaps become gaps of their own, so the text comes back in its original order whichever change is decided first. Changes never depend on each other, and the session that made a change does not matter.

Rows are derived from the records at read time and are never repaired by a write. A fragment whose change was accepted while it was written is read as a change of its own, so a reader sees the same rows as the author. Edits made with tracking off, in the editor or by an agent's patch on the server, go through the same planner. Pending spans shrink, move or split around them, and nothing is recorded for them. A run tag groups the changes from one agent pass, so editors can filter by run and accept them together. The tag never links changes.

Pending changes appear in two places, both read straight from the records:

- The code pane marks inserted text and shows deleted text struck through.
- The Changes panel lists every change with its deleted and inserted text, for editors and readers.

The rendered preview does not paint changes. The outputs are too varied (HTML from Markdown, Typst and LaTeX, and PDF from Typst and LaTeX), and placing a change on the page depends on word matching, which is a heuristic. Math, tables, code and macros have no prose to match, a deletion has no text left to mark, and a PDF text layer cannot hold a struck-through span. Comments keep their marks on the page because a comment that lands a little off is still readable. A change mark on the wrong word would mislead a reviewer.

The model has these limits:

- Two editors who reject the same change at the same moment put the deleted text back twice.
- Editing inside someone else's insertion does not nest. A deletion there shrinks their text, and an insertion splits it in two.
- A rejected deletion returns as new characters.
- After a history trim, a change whose text was trimmed away is pruned.
- Two people editing the same point at the same moment can leave the deleted fragments there in either order. Nothing is lost.
- A change accepted or rejected while its author is still typing into it, in a session that has not synced, keeps the text typed in the meantime, but that text is no longer tracked.
