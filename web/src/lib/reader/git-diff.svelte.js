// Compares one changed file of a folder project's repository: `HEAD` on the
// left, the working tree on the right. It is read only: nothing is written
// into the folder. The Git panel picks the file; the editor area shows this
// state. The view only reads state and calls these methods.
import * as routes from "../desktop/git.js";

/** @typedef {{ active: boolean, loading: boolean, error: string, path: string, kind: string, from: string, head: string | null, work: string | null, binary: boolean }} GitDiffState */

/** @param {{ id: string, api?: typeof routes }} options */
export function createGitDiff({ id, api = routes }) {
  /** @type {GitDiffState} */
  let current = $state.raw({ active: false, loading: false, error: "", path: "", kind: "", from: "", head: null, work: null, binary: false });
  // Bumped by every show, refresh and close, so a read that finishes late is ignored.
  let generation = 0;
  let closed = false;

  const set = (/** @type {Partial<GitDiffState>} */ patch) => {
    if (!closed) current = { ...current, ...patch };
  };

  /// Reads `path` and shows it. `kind` is how the change list calls it (added,
  /// modified, deleted, renamed or conflicted), which the bar says. `from` is
  /// the old path of a renamed file, and its `HEAD` side is read at that path.
  async function show(/** @type {string} */ path, /** @type {string} */ kind = "", /** @type {string} */ from = "") {
    const request = ++generation;
    set({ active: true, loading: true, error: "", path, kind, from, head: null, work: null, binary: false });
    try {
      const answer = await api.gitDiff(id, path, from);
      if (request !== generation) return;
      set({ loading: false, head: answer?.head ?? null, work: answer?.work ?? null, binary: Boolean(answer?.binary) });
    } catch (error) {
      if (request === generation) set({ loading: false, error: error?.message || "The desktop app could not read that file." });
    }
  }

  /// Reads the file on show again, for after an edit.
  const refresh = () => (current.active ? show(current.path, current.kind, current.from) : Promise.resolve());

  function close() {
    generation += 1;
    set({ active: false, loading: false, error: "", head: null, work: null, binary: false });
  }

  function dispose() {
    closed = true;
  }

  return {
    get state() { return current; },
    show,
    refresh,
    close,
    dispose,
  };
}
