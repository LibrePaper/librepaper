---
title: "Local backups"
---

LibrePaper Companion can keep a local ZIP backup of every project available to
the signed-in account, including shared projects. Open a project, choose
**Backup** in the top bar, then open **Backups** in Settings. Connect the
companion if needed, authorize it with your signed-in browser account, choose a
destination folder, and enable automatic backups. Authorization uses the
browser's existing sign-in; no separate CLI login is needed. Start the local
companion with `librepaper` if it is not already running.
The companion must be running on this computer; it continues the schedule
while browser tabs are closed.

The interval can be 1, 5, 15, 30 or 60 minutes (5 minutes by default). Choose
**Back up now** to start a run immediately. The companion asks the server for
all projects available to that account and writes one ZIP per project. It keeps
these files in a server and account subfolder inside the destination you
selected. Each ZIP contains project source and assets;
comments, suggestions, review annotations and edit history are not included.
File names use the project's stable ID, so renaming a project does not change
which project the archive belongs to. Each project has one latest ZIP; this
feature does not keep a history of older ZIPs. The companion replaces that ZIP
when it backs up newer content.

Unchanged projects keep their existing ZIP. For a changed project, the companion
reuses unchanged assets from the previous ZIP and builds a replacement archive
before swapping it into place. Renaming a project updates its ZIP filename on
the next run; the old filename is removed only after the replacement is saved.
Only changes already saved to the server are included.

These are one-way copies. Editing or deleting a ZIP does not change a project
on LibrePaper, and restoring a ZIP requires importing it as a project. Turning
backups off stops future scheduled runs; a backup already in progress may
finish. Disabling backups or deleting a project does not delete ZIPs already on
disk. Remove those files yourself if you no longer want them.

The selected folder is chosen by the companion's native folder picker and
stays on this computer. Settings displays the full selected path in a
read-only field; the browser cannot edit or submit a path. The browser sends
the signed-in account ID to the paired companion, which uses the browser's
explicit device-code approval to access projects for that account.
