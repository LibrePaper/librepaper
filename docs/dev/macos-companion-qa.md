# macOS Companion QA

Run this checklist on a clean macOS account or machine before a release. Native macOS testing remains required; browser tests do not cover Finder, login items, native dialogs, or Gatekeeper.

1. Download and install the Companion from the browser on a clean machine. Open it from Finder and confirm the expected first-launch Gatekeeper flow, then confirm it starts and reports its version.
2. Enable start at login, log out and back in, and confirm the Companion starts once. Quit it from Settings and confirm it stops.
3. In Settings > Rendering, use **Choose folder…** under **Project folder**. Confirm the project entrypoint updates and a live preview uses that folder. Open the chooser again and cancel; the prior entrypoint and binding should remain. In Settings > Backups, choose a backup folder and confirm its path appears in **Backup folder**; cancel a change and confirm the prior path remains. With backups authorized, start **Back up now** and confirm the status reports completion.
4. Confirm Quarto, R, and the configured ACP agent command work in Terminal. Start the Companion from Finder and again after a fresh login. In Settings > Companion, use **Check local setup** to review its tool report. Render a small Quarto document containing an R chunk and send a small task to the configured agent from the editor; confirm both reach the local process.
5. In Safari, check the loopback health request, pairing flow, and an authenticated management request in Web Inspector. Repeat in current Chrome and Firefox. Record any Safari mixed-content failure separately: omitting an unsupported `targetAddressSpace` option only handles option validation and does not repair mixed-content blocking. [WebKit issue 171934](https://bugs.webkit.org/show_bug.cgi?id=171934) remains open; the [Safari `targetAddressSpace` regression, issue 304164](https://bugs.webkit.org/show_bug.cgi?id=304164), was fixed upstream.

This checklist covers QA only. Code signing and signing credentials are outside its scope.
