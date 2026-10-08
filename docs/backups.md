---
title: "Local backups"
---

Turn backups on in Settings and pick a folder. Set the interval to 1, 5, 15, 30, or 60 minutes. The default is 5. The Companion must be running. Start it with `librepaper`.

Each ZIP contains only project source and assets. It holds no comments, suggestions, or history. Backups use the project's stable ID. Renaming does not change archive names. Only the latest ZIP per project is kept. Older versions are replaced.

Unchanged projects keep their existing ZIP. Changed projects reuse unchanged assets from the previous ZIP before swapping in the new archive. Only server-saved changes are included.

The copies are one-way. Editing or deleting a ZIP does not affect LibrePaper. Restore requires importing as a new project. Disabling backups or deleting a project does not remove ZIPs already on disk.
