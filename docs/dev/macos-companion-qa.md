# macOS Companion QA

Run this checklist on a clean macOS account or machine before a release. Native macOS testing remains required; browser tests do not cover Finder, login items, native dialogs, or Gatekeeper.

1. Download and install the Companion from the browser on a clean machine. Open it from Finder and confirm the expected first-launch Gatekeeper flow, then confirm it starts and reports its version.
2. Enable start at login, log out and back in, and confirm the Companion starts once. Quit it from Settings and confirm it stops.
3. Connect from LibrePaper. Approve the native folder chooser and confirm the selected folder appears in Settings. Repeat and cancel the chooser; the previous selection should remain. Select a backup folder, enable backups, and confirm a manual backup completes.
4. Start the Companion from Finder with Quarto, R, and the configured ACP agent available on `PATH`. Confirm Settings discovers each tool and a small Quarto render and ACP request reach the local process.
5. In Safari, check the loopback health request, pairing flow, and an authenticated management request in Web Inspector. Repeat in current Chrome and Firefox. Record any Safari mixed-content failure separately: omitting an unsupported `targetAddressSpace` option only handles option validation and does not repair mixed-content blocking. [WebKit issue 171934](https://bugs.webkit.org/show_bug.cgi?id=171934) remains open; the [Safari `targetAddressSpace` regression, issue 304164](https://bugs.webkit.org/show_bug.cgi?id=304164), was fixed upstream.

This checklist covers QA only. Code signing and signing credentials are outside its scope.
