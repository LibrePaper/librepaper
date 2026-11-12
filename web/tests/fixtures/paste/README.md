# Paste fixtures

Two kinds of fixture live here.

- The top-level `*.html` files (`word.html`, `google-docs.html`,
  `libreoffice.html`, `browser.html`) are hand-written imitations of each app's
  clipboard HTML. They are a stable baseline for the converter, not a record of
  what those apps really produce.
- `captured/` holds real clipboards, saved by `web/tools/paste-capture.mjs`.
  Never edit these by hand. Re-capture instead.

`source/` holds the sample document (`sample.md`, a fictional grant application
with no real people) and `build.sh`, which renders it to `sample.docx`,
`sample.odt` and `sample.html`. The outputs are committed, so you do not need
pandoc to capture. After editing `sample.md`, run `source/build.sh` (needs
pandoc) and commit the regenerated files.

## Capture procedure

Each capture is one app and one browser. Use the name `<app>-<browser>`, in
lowercase with hyphens, for example `google-docs-chrome`.

1. Open `source/sample.docx` in Google Docs: upload it to Google Drive, then
   open it with Google Docs.
2. Open the same `sample.docx` in Word online.
3. Open `source/sample.odt` in LibreOffice Writer.
4. Open `source/sample.html` in a browser (the `web-page` capture).
5. In each document, select all (Ctrl+A, or Cmd+A on macOS) and copy.
6. Run the script right after copying, from the repository root:

   ```sh
   node web/tools/paste-capture.mjs google-docs-chrome --app "Google Docs" --browser "Chrome 142"
   ```

   Use `--note "..."` for anything unusual, such as a dialog you dismissed.
   The script refuses to overwrite an existing capture unless you pass
   `--force`.

Desktop Word must be captured on Windows or macOS, because Word does not run on
Linux. The macOS path of the script reads the clipboard with `osascript` and
`pbpaste`, so `node web/tools/paste-capture.mjs word-desktop-mac --app "Word"`
works from a Mac terminal. The capture's `platform` and `session` fields in the
`.json` record which machine made it.

On Linux, the script uses `wl-paste` on Wayland (`WAYLAND_DISPLAY` set) and
`xclip` on X11.

## Goldens

Each capture gets expected outputs next to it, as
`captured/<name>.expected.tex`, `.typ`, `.md` and `.org`. Write them from the
converter's current output:

```sh
cd web && UPDATE_PASTE_GOLDENS=1 node --test --experimental-transform-types tests/unit/paste-captured.mjs
```

Then review the new files with `git diff` before committing. A golden that
looks wrong is a converter bug to fix, not a golden to accept.
