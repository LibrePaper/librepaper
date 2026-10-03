---
title: "History and revisions"
---

## What is kept

Editing history and named versions are retained by default while a document
exists. An owner can explicitly trim older history and versions to reduce
storage use. Trimming permanently removes that recovery information; it cannot
be restored from the live document. See [storage and history](../architecture/document.html#storage-and-history).

Versions are the moments somebody asked for: a checkpoint you name in the
History panel, a restore, or an accepted proposal. Each
of those records the whole directory, so a chapter and the file that
includes it can never come back out of step. A comment on the live draft is
anchored to the moment it was made rather than to a version, so a round of
review does not leave a copy of the paper behind for every remark.

Copying a checkpoint's link preserves the share key that gave you access. Restore requires editor access; it records the current version before applying the earlier directory, so both remain available.

## Track changes

Changes that depend on other revisions require attention before an independent decision. Downloads include the current proposed text. Agent suggestions arrive as track changes to accept or reject.
