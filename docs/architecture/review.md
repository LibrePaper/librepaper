---
title: "Comments and track changes"
---

## Comments

Three things are kept about each comment:

| | what it is | lifetime |
| --- | --- | --- |
| Original anchor | what the reviewer selected in the document as it stood | written once, never modified |
| Derived attachment | where that passage is now | cache, recomputed on every edit |
| Presentation context | rendered quotation | display evidence, never identity |

The original anchor is a checkpoint plus a UTF-16 range or the whole document. Every anchor is a range, not a point.

The server bridges rendered page to source by flattening both with syntax blanked and whitespace collapsed. It looks for selection in the flattened source, mapping positions back. It refuses to locate if a passage reads identically in several places.

Cursors follow characters through insertions and deletions. Resolution falls back to searching for quoted words if cursors are unusable. Two equally good candidates produce `ambiguous`. Replacement cursors are stored in the cache, never in the anchor. Results are attached at position, deleted, or ambiguous.

Marks are painted in the browser. Source identity and current attachment come from the server.

Comments use concepts from the W3C Web Annotation Data Model, adapted to the
document's source anchors. The CLI's `librepaper export DOCUMENT DIRECTORY`
command exports a project snapshot of source and assets, not annotations. The
current CLI has no comments-to-Markdown or JSON-LD export.

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
- A rejected deletion returns as new characters. Comments on the old characters may not reattach exactly.
- After a history trim, a change whose text was trimmed away is pruned.
- Two people editing the very same point at the same moment can leave the deleted fragments there in either order. Nothing is lost.

