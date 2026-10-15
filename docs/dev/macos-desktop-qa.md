# macOS desktop app QA

Run this checklist on a clean macOS account or machine before a release. Native macOS testing remains required; browser tests do not cover Finder, native dialogs, or Gatekeeper.

1. Download and install the desktop app on a clean machine. Open it from Finder and confirm the expected first-launch Gatekeeper flow, then confirm it starts.
2. Open the desktop app from Finder, quit it, and open it again. Confirm it starts and reports its version.
3. In Settings > Rendering, use **Choose folder…** under **Project folder**. Confirm the project entrypoint updates and a live preview uses that folder. Open the chooser again and cancel; the prior entrypoint and binding should remain. In Settings > Backups, choose a backup folder and confirm its path appears in **Backup folder**; cancel a change and confirm the prior path remains. With backups authorized, start **Back up now** and confirm the status reports completion.
4. Confirm Quarto, R, and the configured ACP agent command work in Terminal. Open the desktop app from Finder and again after a fresh login. In Settings > Desktop app, use **Check local setup** to review its tool report. Render a small Quarto document containing an R chunk and send a small task to the configured agent from the editor; confirm both reach the local process.

This checklist covers QA only. Code signing and signing credentials are outside its scope.
