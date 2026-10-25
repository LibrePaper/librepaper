# Local projects

Status: building as Kata pphs, 2026-10-25. Builds on `kata/rj7r-folder-mirror`, which contains the local render pipeline (`SPEC-local-render.md`) and the granted folder mirror.

## Goal

In the desktop app, a folder on disk opens as a project. There is no web project, no sign-in, no socket, and nothing is stored on the server, in the browser, or in the folder. Edits are saved to the files. Changes made by other programs appear in the editor, the way VS Code reloads a file. LibrePaper does not sync a local project anywhere. People sync and collaborate with their own tools: Git, Dropbox, iCloud, Syncthing.

## Decisions

- **The folder is the project.** The Loro document is built in memory from the files when the project opens and is thrown away when it closes. It is an editing buffer, never persisted.
- **No sync of our own.** Nothing about a local project leaves the computer.
- **Outside changes reload.** A file changed on disk replaces the editor's copy when the editor has no unsaved edits to it. Otherwise the person chooses: keep mine, or load the disk version.
- **Polling, not a watcher library.** The desktop app stats the project's files every second and reads a file once its size and modification time have held still for one poll. The same code runs on Linux, macOS, Windows and network drives.
- **Server features are off.** Sharing, comments, suggestions, chat, the agent, server history, publishing, templates and the Git panel are hidden. The Git panel is server-bound today; Git on a local project is the person's own Git client for now.
- **The page still comes from the server origin.** The desktop app loads its pages from the configured server, and the preview frame is served from its docs origin. A local project therefore needs that server reachable, or the shell cached by the service worker, exactly like any page today.

## Routing

A local project opens at `/reader.html?local=<id>`, which the server already serves. `<id>` is the desktop app's binding id for the folder. The Reader reads `local` from the query string before it derives a slug from the path, and in local mode its project id is `local-<id>` everywhere a project id is passed (renders, build settings, the preview cache key, `scopeLocalApp`).

## Desktop app (sidecar)

All routes sit under the existing `lp-local` service, are authenticated like every other route, and are scoped to the request origin.

### Folder projects

A folder project is a binding with `kind: "project"`, `project: "local-<id>"`, no fixed entrypoint, and the folder root. Existing bindings get `kind: "render"` by default. A project binding passes `validate_scoped_entrypoint` for any safe relative entrypoint inside its root, so renders may build any file.

- `POST projects/open`: shows the native folder chooser ("Open a folder as a LibrePaper project"). Creates the binding, or returns the existing one for the same root and origin. Answers `{ id, project, name, root_name }`, where `name` is the folder's base name. Answers 409 when a chooser is already open, 204 when the person cancels.
- `GET projects`: the folder projects for this origin, most recently opened first: `[{ id, project, name, opened_at, present }]`. `present` is false when the folder is gone.
- `DELETE projects/<id>`: forgets the folder. Files are untouched.
- Opening a project through `GET projects/<id>/tree` updates `opened_at`.

### Reading

`GET projects/<id>/tree` answers `{ files: [{ path, size, sha256, mtime }], skipped: [{ path, reason }] }`.
- It walks the root, skipping `.git`, any hidden file or folder, `node_modules`, `renv`, `__pycache__`, `.quarto`, `_freeze`, symlinks, and files over the upload size bound. At most 5000 files; past that it answers 413.
- It records every listed file's digest in the binding's written record, so the folder mirror treats every file of the project as managed: a deletion in the editor trashes it, and a rename moves it.
- `GET projects/<id>/files/<path>`: the bytes of one file inside the root, checked the same way.

### Writing

`PUT folder` (the `rj7r` mirror) works for project bindings unchanged: edits, renames, deletions to the trash, the `.local` copy when a file changed on disk since LibrePaper last wrote it.

### Outside changes

`GET projects/<id>/changes?since=<cursor>` is a long poll, held up to 25 seconds. It answers `{ cursor, changes: [{ path, kind: "changed" | "added" | "removed", sha256?, size? }] }` as soon as there is something to report. The first call, with no `since`, answers at once with the current cursor and no changes.
- A background poller per open project stats the tree (with the same skip rules) every second.
- A file is reported after its size and modification time held still across one poll, with its new digest.
- A change whose digest equals the written record's digest for that path is LibrePaper's own write and is not reported.
- The poller stops when no `changes` request has arrived for 60 seconds.

### Server

`/api/config` gains `docs_origin`, so a page with no server document can find the preview frame's origin.

## Web

### Session

- `createProjectSession` gains a `standalone: true` option. A standalone session is `joined` once its persistence has hydrated, and `pending` follows the persistence adapter rather than server acknowledgement.
- `web/src/lib/reader/local-project.js` (new) builds the session for a folder: it reads the tree and file bytes, adds each text file and asset to a fresh Loro document through the session's own file operations, and exposes the same `start`, `close`, `send`, `syncStatus` and `sendLive` the Reader's collaboration object has, with no socket. Text and asset are told apart with the rules from `/api/config` (`text_extensions`, `asset_extensions`); other files are left out of the project and never touched.
- Edits reach disk through the existing folder sync (`syncFolder` and `createFolderSync`), with the folder's binding id.
- Figures: in local mode `gatherFigures` and the workspace's `upload`/`gather` read and write through the desktop app instead of `/api/documents/<slug>/assets`.

### Outside changes

A loop calls `projects/<id>/changes`. For each change:
- `added`: the file joins the project.
- `removed`: the file leaves the project.
- `changed`: when the editor's text for that path equals the text last read from or written to disk, the new text replaces it through `textPatches` (in `lib/text-patches.js`), so the cursor and the rest of the document stay put. When the editor holds unsaved edits to that file, a notice asks "chapter.tex changed on disk." with two actions, "Keep mine" (the next sync writes the editor's text) and "Load from disk".
- Assets that changed are replaced by their new bytes.

The cursor moves past a batch only after every change in it was applied. When a file cannot be read, the loop keeps the old cursor, says so once, waits and asks again: the desktop app answers the same changes after a cursor until it is given a newer one, and the changes already applied are skipped. After three polls in a row that end on an unreadable file, the loop reads the whole tree instead, as it does after a `reset`. A reset whose tree read fails keeps the old cursor and tries again.

### Reader local mode

One derived `local` flag:
- `mayEdit` is true, the document's title is the folder name, and `source_format` comes from the open file.
- `tabsFor` hides collaboration, agent, history, share and git.
- `mayChat`, `canPublish` and `canSeeSharing` are false; save-as-template, `rememberProject`, the preview cache's IndexedDB and link-key handling are skipped.
- The connection note and Offline badge do not appear.
- Run code and local renders work as in a web project, with the folder's binding.

### Landing page

In the desktop app only, signed in or not:
- "Open folder…" beside New project calls `projects/open` and opens `/reader.html?local=<id>`.
- "Recent folders" lists `GET projects`, each row opening its project, a missing folder shown as missing, and a "Forget" action.

## Out of scope

- A Git panel that works on the folder itself.
- Publishing a local project to the server.
- Opening the same folder in two windows. The second opening focuses nothing special; both windows work on the same files and see each other's saves as outside changes.

## Done

- `make test` green, the browser suite green.
- Unit tests for: the standalone session, the tree-to-document build, the outside-change rules (clean replace, unsaved conflict, added, removed), the sidecar tree skip rules and 5000 cap, echo suppression, the long poll, `kind: "project"` entrypoint freedom, `projects/open` reuse of an existing root.
- A Node integration test against the real binary: open a project binding, read the tree, write through `PUT folder`, change a file on disk and see it in `changes`.
