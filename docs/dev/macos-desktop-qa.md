# macOS desktop app QA

Run this checklist on a clean macOS account or machine before a release. Native macOS testing remains required; browser tests do not cover Finder, native dialogs, or Gatekeeper.

1. Download and install the desktop app on a clean machine. Open it from Finder and confirm the expected first-launch Gatekeeper flow, then confirm it starts.
2. Open the desktop app from Finder, quit it, and open it again. Confirm it starts and reports its version.
3. Open a Quarto document, turn on **Run code**, and use the suggested folder. Confirm the line "Runs in …" appears and **Open folder** opens it. Click **Change…**, pick another folder, and cancel; the previous folder should remain. Put `data/x.csv` in the Run directory and confirm a chunk reading it renders. In Settings > Backups, choose a backup folder and confirm its path appears in **Backup folder**; cancel a change and confirm the prior path remains. With backups authorized, start **Back up now** and confirm the status reports completion.
4. Confirm Quarto and R work in Terminal. Open the desktop app from Finder and again after a fresh login. In Settings > Desktop app, use **Check local setup** to review its tool report. Render a small Quarto document containing an R chunk, send a small task to the built-in agent with **Run** on a task in the Agent panel's Tasks tab, and confirm both reach the local process.

This checklist covers QA only. Code signing and signing credentials are outside its scope.
