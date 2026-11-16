---
title: "Local backups"
---

Open **Settings > Backups** in the [desktop app](desktop-app.html). Choose a **Backup folder**, then turn on **Automatic backups**. The switch stays off until a folder is chosen. Turning it on also signs the desktop app in to your account if it is not already signed in. Set **Frequency** to 1, 5, 15, 30 or 60 minutes. The default is 5. **Back up now** starts a run.

Backups run only while the desktop app is open.

Each project is saved as one ZIP file in a `librepaper-backups` folder inside the folder you chose. Projects shared with you are included. A ZIP holds the project's source and assets, with pending tracked changes in the text. It holds no comments or history. The file name starts with the project's title and ends with its ID, so renaming a project writes a new ZIP and removes the old one.

Only the latest ZIP for each project is kept. An unchanged project keeps its ZIP. A changed project reuses its unchanged assets from the previous ZIP. Only changes saved to the server are included.

The copies are one-way. Editing or deleting a ZIP does not affect LibrePaper. To restore a project, import its ZIP as a new project with **New project > Upload files**. Turning backups off or deleting a project leaves existing ZIPs on disk.
