// Which file the preview draws, and why.
//
// The rule itself is `lib/opening.js`'s: from the open file, the file it is
// shown inside, the file the preview is locked on, or nothing at all. This
// module is where that answer is kept and moved to. When the answer changes it
// empties or repoints the preview, and it does so through the effects the
// page hands in, so that the owner can be driven without a page.
//
// The reactive part is `state`. The markup and the page read `path`, `format`,
// `reason`, `choices`, `lockedFile` and `alone` from it. `navigation` is plain
// on purpose: it is a counter that renders compare against, not a value the
// page draws.

import { includersOf, openingFile, previewTarget, rendersAlone } from "../opening.js";

/**
 * @typedef {{
 *   path: string,
 *   format: string,
 *   reason: string,
 *   choices: string[],
 *   lockedFile: string,
 *   alone: Set<string>,
 * }} PreviewTargetState
 */

/**
 * @param {{
 *   texts: () => Record<string, string>,
 *   formatOf: (path: string) => string,
 *   open: () => { textPath: string, hasFile: boolean },
 *   lockedPath: () => string,
 *   skip?: () => boolean,
 *   painted: () => { reset: (options?: { keepSynctex?: boolean, empty?: boolean }) => void },
 *   configure?: (format: string) => void,
 *   warm?: (format: string, text: string) => void,
 *   invalidate?: () => void,
 *   clearFrame?: () => void,
 *   navigateFrame?: () => void,
 *   cancelDebounce?: () => void,
 *   resetStatus?: () => void,
 *   cleared?: () => void,
 *   paint?: () => void,
 *   settle?: () => Promise<unknown>,
 *   disposed?: () => boolean,
 * }} options
 */
export function createPreviewTarget({
  // The live texts of the project, which decide what includes what.
  texts,
  // The format a file is previewed in, or "" when nothing can draw it.
  formatOf,
  // The open file: its text path, and whether any file is open at all.
  open,
  // The path of the file the preview is locked on, or "".
  lockedPath,
  // True when an update must do nothing: no session and no projection, or a
  // link holder reading a file in place of the preview.
  skip = () => false,
  // The frame's painted facts. A function so that the owner can be made
  // before the frame is.
  painted,
  // Selects the engine for a format. Empty means no format.
  configure = (_format) => {},
  // Starts loading the engine for a format.
  warm = (_format, _text) => {},
  // Drops every render in flight.
  invalidate = () => {},
  // Clears the frame's pages and their click map.
  clearFrame = () => {},
  // Reloads the frame when the kind of page changes.
  navigateFrame = () => {},
  // Cancels the typing-pause render.
  cancelDebounce = () => {},
  // Clears a render failure.
  resetStatus = () => {},
  // Clears a DOCX artifact and the diagnostics of the page.
  cleared = () => {},
  // Paints the preview.
  paint = () => {},
  // Waits for pending DOM work, such as Svelte's tick.
  settle = () => Promise.resolve(),
  // True once the page is gone and nothing may be painted.
  disposed = () => false,
}) {
  // What the preview draws. `path` is the file, `reason` says why it is that
  // one (the file itself, the file it is shown inside, the file the preview is
  // locked on, or nothing at all) and `choices` lists the files that could host
  // the open file when it has no preview of its own.
  // `lockedFile` is the file the preview is locked on, by identity so a rename
  // keeps the lock. It lasts for this visit: nothing stores it and nobody else
  // sees it.
  // `alone` is the paths that render on their own, which is what the eye in
  // Files offers to lock. It is always replaced with a new Set, never mutated.
  const state = $state({ path: "", format: "", reason: "none", choices: [], lockedFile: "", alone: new Set() });

  // Counts the changes of target. A render compares against it to know that
  // what it started for has since been replaced.
  let navigation = 0;

  // Set when the preview was emptied because there was nothing to show, so
  // that the next file to become renderable is painted: with no previous
  // target, the first target is otherwise left to the initial paint.
  let previewCleared = false;

  /// Empties the preview: no file is being followed, no format is selected and
  /// nothing is drawn. No render is attempted and no engine is warmed.
  function clear() {
    previewCleared = true;
    state.path = "";
    state.format = "";
    configure("");
    navigation += 1;
    invalidate();
    cancelDebounce();
    clearFrame();
    painted().reset();
    resetStatus();
    cleared();
  }

  // Works out which file the preview draws and moves it there. It runs when
  // the open file changes, when the lock does, when the directory does, and a
  // moment after any edit, because which file includes which is read from the
  // texts. A target that has not changed costs a comparison and nothing else.
  function update() {
    if (disposed() || skip()) return;
    const all = texts();
    if (state.lockedFile && !lockedPath()) state.lockedFile = "";
    // With a figure open there is no open text, so the preview stays on the
    // file it has. Nothing open at all means the file the project opens on.
    const opened = open();
    const openPath = opened.textPath || (opened.hasFile ? state.path : "") || openingFile(all) || "";
    const target = previewTarget({ open: openPath, locked: lockedPath(), texts: all });
    const includers = includersOf(all);
    const alone = Object.keys(all).filter((path) => rendersAlone(path, all, includers)).sort();
    if (alone.length !== state.alone.size || alone.some((path) => !state.alone.has(path))) state.alone = new Set(alone);
    if (target.reason !== state.reason) state.reason = target.reason;
    if (target.choices.join("\n") !== state.choices.join("\n")) state.choices = target.choices;
    const path = target.path || "";
    const format = path ? formatOf(path) : "";
    // Nothing the preview could follow: the open file has no renderer, or it
    // is part of a document no single file stands for, or there is no file.
    if (!format) {
      if (state.path) clear();
      return;
    }
    if (state.path === path && state.format === format) return;
    const previous = state.path || state.format || (previewCleared ? "cleared" : "");
    previewCleared = false;
    state.path = path;
    state.format = format;
    configure(format);
    warm(format, typeof all[path] === "string" ? all[path] : "");
    if (!previous) {
      // The first file the preview is given is rendered now. Waiting for the
      // typing pause would hold it for a second or more while the project
      // loads, since every hydration step restarts that timer.
      cancelDebounce();
      void settle().then(() => {
        if (!disposed() && state.path === path) paint();
      });
      return;
    }
    // Invalidate both running compiles and replayed pages, even when the
    // two selected files use the same renderer or both produce PDFs.
    navigation += 1;
    const mine = navigation;
    invalidate();
    clearFrame();
    // Only the render of the new file installs a click map.
    painted().reset();
    resetStatus();
    // The frame is reloaded only when the kind of page changes. Moving between
    // two files of the same kind keeps it, so switching files in the list does
    // not pay for a frame load before the cached page can be shown.
    navigateFrame();
    void settle().then(() => {
      if (!disposed() && mine === navigation) paint();
    });
  }

  // Locks the preview on the file with this id and follows it.
  function lock(id) {
    state.lockedFile = id;
    update();
  }

  // Lets the preview follow the open file again.
  function unlock() {
    state.lockedFile = "";
    update();
  }

  return {
    state,
    get navigation() {
      return navigation;
    },
    bump() {
      navigation += 1;
    },
    update,
    clear,
    lock,
    unlock,
  };
}
