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

- A change is one region. Original text that was deleted is replaced by the text now inserted. Replacing "cat" with "tabby" is one change. Either side can be empty.
- The record keeps the deleted text and two cursors, one at each end of the inserted text. The cursors follow the text as others edit around it.
- Accept removes the record. The text stays.
- Reject puts the deleted text back in place of the inserted text, then removes the record.
- Each change is decided on its own, in any order. Changes never depend on each other. The session that made a change does not matter.
- A run tag groups the changes from one agent pass. Editors can filter by run and accept all changes from a run. The tag never links changes together.

The model has these limits.

- Two editors who reject the same change at the same moment put the deleted text back twice.
- Editing inside someone else's insertion does not nest. A deletion there shrinks their text. An insertion splits it in two.
- A rejected deletion returns as new characters. Comments on the old characters may not reattach exactly.
- After a history trim, a change whose text was trimmed away is pruned.

