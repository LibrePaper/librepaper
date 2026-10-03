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

The original anchor is a checkpoint plus a UTF-16 range or the whole document. Every anchor is a range (no point annotations).

The server bridges rendered page to source by flattening both with syntax blanked and whitespace collapsed. Selection is looked for in the flattened source, mapping positions back. It refuses to locate if a passage reads identically in several places.

Cursors follow characters through insertions and deletions. Resolution falls back to searching for quoted words if cursors are unusable. Two equally good candidates produce `ambiguous`. Replacement cursors are stored in cache, never anchor. Results are attached at position, deleted, or ambiguous.

Marks are painted in the browser; source identity and current attachment come from the server.

Comments follow the W3C Web Annotation Data Model (reshaping, not translation). `librepaper export DOCUMENT` writes them in Markdown or JSON.

## Track changes

A tracked change is a branch: a fork of the document at a known point. Everything awaiting a decision is one object:
- typing with Track changes on (usually one hunk)
- a suggestion from a comment thread (one hunk)
- an agent run (many hunks)

All appear in the Changes pane. The stored object is called a proposal. A branch is stored as a blob of operations alongside a Postgres row (not a room, no sockets, nothing inside the shared document).

A hunk is a maximal run of non-retain operations. Replacing "cat" with "tabby" is one decision, not two. Character-level raw differences are grouped: two changes separated by eight or fewer retained units are one hunk. A decision names a hunk by branch, base, tip and index.

Accepting a whole branch imports it. Accepting part of one: (1) import the branch, (2) compute inverse difference, (3) keep only rejected hunks, (4) apply as reviewer. Result is the accepted subset with accepted text attributed to author and removal to reviewer. Steps 1-4 are atomic; intermediate state is never persisted.

Two branches touching the same passage are grouped into one card. Adjacent edits are not grouped.
