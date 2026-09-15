// The project directory, as the file list shows it.
//
// This owns the directory rather than borrowing it: the files, the folders,
// which one is open, where everyone's caret is, the deployment's placement
// rules, and the figure being looked at all live here, and Reader reads them
// from `state`. What it takes in return are effects it cannot perform itself
// -- saying something to the reader, asking for a repaint, re-pointing the
// preview -- and the session the directory belongs to, which arrives later
// than this module does and is handed over with `attach`.
//
// Holding the list as state rather than deriving it is not a shortcut: what
// it would be derived from is a CRDT that changes outside Svelte's knowledge,
// so a change is an event to be observed, not a value to be recomputed.

import { checkPlacement } from "../file-manager.js";
import * as figures from "../figures.js";
import * as renderers from "../renderers.js";
import { authHeaders, uploadAsset } from "../api.js";

export function createWorkspace({
  slug,
  key = "",
  // The file named by the link this reader arrived on, if any. Opened once,
  // the first time the directory is read and the reader turns out to be able
  // to edit -- a reader has no source pane to open it in.
  arrivedFile = "",
  say = () => {},
  paint = () => {},
  // The preview follows the main file unless one was pinned, and which file
  // that is can change with any directory read.
  retarget = () => {},
  onarrived = () => {},
  upload = uploadAsset,
  gather = figures.gather,
}) {
  const state = $state({
    files: [],
    folders: [],
    openFile: "",
    peersByFile: new Map(),
    participants: [],
    // The deployment's rules, which say what a path may be and what may sit
    // at one. Fetched rather than compiled in, so a deployment that widens
    // its extension lists widens them here too.
    rules: {},
    // The figure being looked at, when the chosen file is one, and the URL
    // its bytes were fetched to. Held rather than derived because the bytes
    // it needs are fetched.
    figure: null,
    figureUrl: "",
    // Pushed by Reader when the document's permissions are known. A guard
    // rather than a source of truth: the server checks every one of these
    // again.
    canEdit: false,
  });

  let session = null;
  let arrivedOpened = false;

  /// The session whose directory this is. Called once the collaboration
  /// session exists, and again with `null` when it goes away. It does not
  /// read the directory: the first read is the caller's to schedule, because
  /// it is the one that re-points the preview and there is exactly one right
  /// moment for that in the joining sequence.
  function attach(active) {
    session = active;
  }

  /// Reading the directory into the list. Deliberately does not paint: the
  /// first call happens while the session is still being joined, before the
  /// frame has even navigated to the documents origin, and a paint sent then
  /// is a postMessage to a window that is not there yet. What paints is a
  /// *change* -- Reader's `filesChanged` -- and the first paint of all is the
  /// one the arriving text triggers, as it always was.
  function refresh() {
    if (!session) return;
    const previousFigure = state.figure;
    const previousFiles = state.files;
    state.files = session.list();
    state.folders = session.folders();
    if (previousFigure) {
      const moved = state.files.filter((file) => file.kind === "asset" && file.sha === previousFigure.sha
        && !previousFiles.some((previous) => previous.path === file.path));
      const current = state.files.find((file) => file.kind === "asset" && file.path === previousFigure.path)
        || (moved.length === 1 ? moved[0] : null);
      state.figure = current || null;
      if (current && state.openFile === previousFigure.id) state.openFile = current.id;
    }
    // A file that went away under this browser -- somebody else deleted it --
    // leaves the editor showing something that is not there any more, so it
    // falls back to the document itself.
    if (state.openFile && !state.files.some((file) => file.id === state.openFile)) state.openFile = "";
    if (!state.openFile) state.openFile = session.mainId();
    // The file the link named, if the project has one by that name. An
    // editor's source pane opens on it; a reader has no pane to open it in
    // and no list to mark it in, so for them the link is to the document.
    if (arrivedFile && state.canEdit && !arrivedOpened && state.files.length) {
      arrivedOpened = true;
      const named = state.files.find((file) => file.path === arrivedFile);
      if (named) onarrived(named);
    }
    retarget();
  }

  function refreshPeers() {
    if (!session) return;
    state.peersByFile = session.whereEveryoneIs();
    state.participants = session.participants();
  }

  /// Show a file. A figure has no editor: choosing one shows it instead. The
  /// id of an asset is its path, since its bytes are not in the shared
  /// document and there is nothing else to key it by.
  function show(file) {
    state.openFile = file.id;
    state.figure = file.kind === "asset" ? file : null;
  }

  function editable() {
    if (!state.canEdit) throw new Error("This project is read-only.");
  }

  function addText(path) {
    editable();
    const placed = checkPlacement(state.rules, { kind: "text", path }, session.list(), session.folders());
    state.openFile = session.addText(placed, "");
    paint();
  }

  /// A text dropped or chosen is read and added as a file. Its bytes are
  /// words, so they belong in the shared document rather than in the store.
  async function addDroppedText(file, path = file.name) {
    editable();
    const active = session;
    const text = await file.text();
    if (session !== active || !state.canEdit) throw new Error("The editing session changed during upload.");
    const placed = checkPlacement(state.rules, { kind: "text", path }, session.list(), session.folders());
    state.openFile = session.addText(placed, text);
    paint();
  }

  /// A figure: the bytes go to the store and the name goes into the shared
  /// document, in that order. The name is this browser's to give; the bytes
  /// are the server's to keep, under their own digest.
  ///
  /// The two are separate requests, which is why the server keeps a figure
  /// nothing refers to for an hour: between them there is a moment when the
  /// bytes are stored and nothing names them.
  async function addFigure(file, path = file.name) {
    editable();
    const active = session;
    const placed = checkPlacement(state.rules, { kind: "asset", path }, active.list(), active.folders());
    const { sha } = await upload(slug, file, key);
    // An upload yields to other editors; recheck before installing its name.
    if (session !== active || !state.canEdit) throw new Error("The editing session changed during upload.");
    checkPlacement(state.rules, { kind: "asset", path: placed }, session.list(), session.folders());
    session.putAsset(placed, sha);
    paint();
    return placed;
  }

  function addFolder(path) {
    session.addFolder(path, state.rules);
  }

  function relocate(entries, destination, rename) {
    const plan = session.relocate(entries, destination, state.rules, rename);
    if (plan.files.some((file) => file.path !== file.previousPath)) {
      say("Files moved. References in source files are not changed automatically.");
    }
  }

  function remove(entries) {
    session.removeEntries(entries);
    paint();
  }

  function duplicate(entry, path) {
    session.duplicateEntry(entry, path, state.rules);
  }

  function setMain(file) {
    if (file.kind !== "text") return;
    session.setMain(file.id);
    paint();
  }

  /// The figures a document already holds, offered to the insert dialog
  /// beside whatever the editor says is in scope.
  function assets() {
    return session.list().filter((file) => file.kind === "asset");
  }

  /// The URL a figure's bytes were fetched to, for a preview of an insertion.
  async function assetUrl(path) {
    const sha = session?.tree?.().digests?.[path];
    if (!sha) return "";
    const held = await gather(slug, { [path]: sha }, authHeaders(key));
    return held.urls[path] || "";
  }

  /// Fetch the bytes of whichever figure is being looked at. The fetch is
  /// checked against the figure still wanted on the way back: a reader
  /// clicking down a list of figures starts one of these per click.
  function watchFigure() {
    const wanted = state.figure;
    if (!wanted) {
      state.figureUrl = "";
      return;
    }
    gather(slug, { [wanted.path]: wanted.sha }, authHeaders(key))
      .then((held) => {
        if (state.figure === wanted) state.figureUrl = held.urls[wanted.path] || "";
      })
      .catch(() => {
        if (state.figure === wanted) state.figureUrl = "";
      });
  }

  /// Whether the open file is one this browser could preview on its own.
  function canPreview() {
    return state.files.some((file) => file.id === state.openFile && file.kind === "text"
      && Boolean(renderers.formatOf(file.path)));
  }

  /// The path of the open file, which is what the toolbar names.
  function openPath() {
    return state.files.find((file) => file.id === state.openFile)?.path || "";
  }

  return {
    state,
    attach,
    refresh,
    refreshPeers,
    show,
    addText,
    addDroppedText,
    addFigure,
    addFolder,
    relocate,
    remove,
    duplicate,
    setMain,
    assets,
    assetUrl,
    watchFigure,
    canPreview,
    openPath,
  };
}
