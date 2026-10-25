---
title: "Comments and track changes"
---

## Comments

Each comment keeps three things.

| | what it is | lifetime |
| --- | --- | --- |
| Original anchor | what the reviewer selected: a checkpoint, a file, a UTF-16 range and the quoted words | in Postgres, written once, never modified |
| Anchor record | where the passage is now | in the document, kept in step by the editing session |
| Derived status | whether the passage is exact, modified, pending a decision, deleted or unresolved | read on each request, never stored |

The original anchor is a checkpoint plus a UTF-16 range or the whole document. Every anchor is a range, not a point.

The anchor record is one map per comment in the document's `anchors` root. It holds the file, two Loro cursors at the ends of the passage, and the quoted words. Edits move the cursors, so an attached record follows its text and grows or shrinks with it. The quoted words do not change with edits, so a passage that was edited and then moved still reads as modified. Re-attaching never searches the text for words.

A deletion that covers a whole passage detaches its record. The record keeps its quote and the passage's length. A tracked deletion also records the fragment that holds the text and the offset inside it. The editing session remembers the text it deleted whole. When an insertion brings exactly that text back, the comments inside it re-attach. This covers cut and paste, drag and drop, and undo and redo, within one file or across files. Rejecting a tracked deletion puts the comments inside it back by fragment and offset.

Text typed in again by hand, pasted from another session, or left behind by a merge of two people's edits does not re-attach by itself. The comment keeps its quote and shows it until an editor selects the passage and chooses "Attach to selection".

Exact means the passage is where it was. Modified means it is there but edited. Pending deletion means it sits inside a tracked deletion that no one has decided. Deleted means it is gone. Unresolved means nothing can be said either way, such as a missing file or a comment with no record yet. Status is derived each time it is read and never stored. Editors get it from the server, and each editing session derives it again after every change. Readers and commenters get neither the anchor nor the status. Their marks are painted from the words the page showed. A comment about the whole document has no passage, so it has no record and no status.

A commenter's comment is written with its record in one transaction. Deleting a comment deletes its record. Erasing an account anonymizes the comment and keeps the record. When a room opens, each passage comment without a record gets one, which covers comments made before records existed. A comment whose original state can no longer be rebuilt, such as one from before a history trim, gets none and reads as unresolved.

Before a history trim takes its snapshot, the server heals every record whose cursor sits on deleted text. A record whose passage has collapsed is marked detached.

The server bridges rendered page to source by flattening both with syntax blanked and whitespace collapsed. It looks for selection in the flattened source, mapping positions back. It refuses to locate if a passage reads identically in several places. Locating happens once, when the comment is made.

The browser keeps each file's resolved offsets for the session, so an edit resolves no cursor. In a spike, a keystroke at 1,000 anchors cost about 0.24 ms through that index. Resolving every cursor on each keystroke would have cost 26 to 85 ms.

The comment model has these limits. A passage split and moved in parts keeps the part that stayed. Two sessions that re-pin one record at once keep one answer. A merge that collapses a record is written detached by the next editing session. The move memory lasts for one editing session, or for one agent patch on the server, so a reload or a separate patch forgets it. After a trim, the original state of a comment cannot be shown.

Comments use concepts from the W3C Web Annotation Data Model, adapted to the
document's source anchors. The CLI's `librepaper export DOCUMENT DIRECTORY`
command exports a project snapshot of source and assets, not annotations. The
current CLI has no comments-to-Markdown or JSON-LD export.

File > Download > Comments saves every thread of the project as Markdown, with
its quoted passage, its replies and a note for resolved comments and for
passages that are no longer in the document.

## Track changes

Track changes are part of the shared document. A tracked edit goes into the text at once, like any other edit. The document also records it as a pending change. The editor, read links, agents and exports all show the text with every pending change in it.

- A change is one author's edit of one region: the text it inserted and the original text it deleted. Replacing "cat" with "tabby" is one change. Either side can be empty.
- A change record keeps two cursors, one at each end of the inserted text. The cursors follow the text as others edit around it.
- Deleted text is kept as fragments. A fragment is one piece of deleted text with its own cursor on the character that followed it and a rank. The fragments that resolve to one point form a gap, in rank order, so several authors deleting around the same place come back in document order. No record is ever moved, so two people editing near one point at the same moment never overwrite each other.
- Accept removes the record. The text stays.
- Reject removes the inserted text and puts the change's fragments back where their gaps are. Other changes' fragments in those gaps become gaps of their own on either side of the restored text, so whatever is decided first, the text comes back in the order it had.
- Each change is decided on its own, in any order. Changes never depend on each other. The session that made a change does not matter.
- Every row is derived from the records at read time, never repaired by a write. A fragment whose change was accepted while it was written is read as a change of its own, so a reader who never edits sees the same rows as the author.
- Edits made with tracking off, in the editor or by an agent's patch on the server, go through the same planner. Pending spans shrink, move or split around them, and nothing is recorded for them.
- A run tag groups the changes from one agent pass. Editors can filter by run and accept all changes from a run. The tag never links changes together.

Pending changes are shown in two places, and both read the records directly, so both are exact.

- The code pane marks inserted text and shows deleted text struck through where it was taken out.
- The Changes panel lists every change with its deleted and inserted text, for editors and for readers.

The rendered preview does not paint changes. This is out of scope. The outputs are too diverse: HTML from Markdown, Typst and LaTeX, and PDF from Typst and LaTeX. Placing a change on the page goes through the same word matching comments use, which is a heuristic. Math, tables, code and macros have no prose to match, a deletion has no text left to mark, and a PDF text layer cannot hold a struck-through span. Comments keep their marks on the page because a comment that lands a little off is still readable. A change mark that is missing or on the wrong word misleads a reviewer.

The model has these limits.

- Two editors who reject the same change at the same moment put the deleted text back twice.
- Editing inside someone else's insertion does not nest. A deletion there shrinks their text. An insertion splits it in two.
- A rejected deletion returns as new characters, and it puts the comments inside it back by fragment and offset.
- After a history trim, a change whose text was trimmed away is pruned.
- Two people editing the very same point at the same moment can leave the deleted fragments there in either order. Nothing is lost.
- A change accepted or rejected while its author is still typing into it in a session that has not synced keeps the text typed in the meantime, but that text is no longer tracked.

