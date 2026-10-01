---
title: "Local backups"
---

LibrePaper Companion can keep a local ZIP backup of every project available to
the signed-in account, including shared projects. Open a project, choose
**Backup** in the top bar, then open **Backups** in Settings. Connect the
companion if needed, choose a destination folder, and enable automatic backups.
The companion must be running on this computer; it continues the schedule
while browser tabs are closed.

The interval can be 1, 5, 15, 30 or 60 minutes (5 minutes by default). Choose
**Back up now** to start a run immediately. If the companion asks for a CLI
login, run this in a terminal, using the server you have open:

```sh
librepaper login --server https://app.librepaper.org
```

Replace the example address with your LibrePaper server. The companion asks
the server for all projects available to that account and writes one ZIP per
project. It keeps these files in a server and account subfolder inside the
destination you selected. Each ZIP contains project source and assets;
comments, suggestions, review annotations and edit history are not included.
File names use the project's stable ID, so renaming a project does not change
which project the archive belongs to. Each project has one latest ZIP; this
feature does not keep a history of older ZIPs. The companion replaces that ZIP
when it backs up newer content.

These are one-way copies. Editing or deleting a ZIP does not change a project
on LibrePaper, and restoring a ZIP requires importing it as a project. Turning
backups off stops future scheduled runs; a backup already in progress may
finish. Disabling backups or deleting a project does not delete ZIPs already on
disk. Remove those files yourself if you no longer want them.

The selected folder is chosen by the companion's native folder picker and
stays on this computer. The browser sends the signed-in account ID to the
paired companion; the companion authenticates to the server with the CLI login
and checks that both identities match. The browser does not receive the local
folder's absolute path.
