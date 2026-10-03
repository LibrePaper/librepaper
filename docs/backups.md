---
title: "Local backups"
---

Turn backups on in Settings and pick a folder. Interval: 1, 5, 15, 30, or 60 minutes (default 5). Companion must be running; start with `librepaper`.

Each ZIP contains project source and assets only (no comments, suggestions, history). Backups use project stable ID; renaming does not change archive names. Only the latest ZIP per project is kept; older versions are replaced.

Unchanged projects keep their existing ZIP. Changed projects reuse unchanged assets from the previous ZIP before swapping in the new archive. Only server-saved changes are included.

One-way copies: editing or deleting a ZIP does not affect LibrePaper. Restore requires importing as a new project. Disabling backups or deleting a project does not remove ZIPs already on disk.
