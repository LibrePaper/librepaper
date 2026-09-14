// One owner for history selection, mode, source endpoints, and request lifetime.
// It deliberately has no dependency on preview rendering or the live editor.
export function createHistorySource({ checkpoint, currentTree, checkpoints = () => [], preferredPath = () => "" }) {
  const state = $state({ selected: "", mode: "current", path: "", loading: false, problem: "", result: null });
  let generation = 0;
  let disposed = false;

  const snapshot = tree => ({ ...tree, texts: { ...tree.texts }, files: { ...tree.files } });
  const label = point => point.label || (point.at ? new Date(point.at).toLocaleString() : point.sha.slice(0, 7));
  async function readPoint(sha) {
    const listed = checkpoints().find(point => point.sha === sha);
    const point = { ...listed, ...(await checkpoint(sha)) };
    if (point.sha !== sha || !point.texts || typeof point.texts !== "object") {
      throw new Error("This checkpoint did not return its source files.");
    }
    return snapshot(point);
  }

  async function select(sha = state.selected, mode = state.mode, path = state.path || preferredPath()) {
    if (disposed) return;
    const mine = ++generation;
    state.selected = sha;
    state.mode = ["previous", "current", "checkpoint"].includes(mode) ? mode : "current";
    state.path = path;
    state.result = null;
    state.problem = "";
    state.loading = true;
    const chosenMode = state.mode;
    try {
      // Capture current source at selection time, before any network awaits.
      // Always read the collaborative tree, never the displayed checkpoint.
      let live = null;
      if (!sha || chosenMode === "current") {
        const available = currentTree();
        live = snapshot(available?.then ? await available : available);
      }
      if (disposed || mine !== generation) return;
      const point = sha ? await readPoint(sha) : null;
      let oldTree = point || live;
      let newTree = point || live;
      let oldLabel = point ? label(point) : "Current version";
      let newLabel = oldLabel;
      let note = "";
      if (point && chosenMode === "current") {
        newTree = live;
        newLabel = "Current version";
      } else if (point && chosenMode === "previous") {
        // A retention-reparented edge is not the previous edit. Only the
        // original parent can establish that comparison.
        const parent = point.original_parent || (!point.ancestry_gap ? point.parent : "");
        if (point.ancestry_gap && !parent) throw new Error("The previous checkpoint is no longer available.");
        if (parent) {
          try { oldTree = await readPoint(parent); }
          catch { throw new Error("The previous checkpoint is unavailable. Choose ‘vs current’ or ‘checkpoint’."); }
          oldLabel = label(oldTree);
        } else {
          note = "First checkpoint: there is no previous version to compare.";
        }
      }
      if (disposed || mine !== generation) return;
      const paths = [...new Set([
        ...Object.keys(oldTree.texts), ...Object.keys(newTree.texts),
        ...Object.keys(oldTree.files || {}), ...Object.keys(newTree.files || {}),
      ])].sort();
      state.path = paths.includes(path) ? path
        : paths.includes(point?.main || newTree.main) ? (point?.main || newTree.main)
          : paths[0] || "";
      state.result = { oldTree, newTree, oldLabel, newLabel, paths, note, diff: Boolean(sha) && chosenMode !== "checkpoint" };
    } catch (error) {
      if (!disposed && mine === generation) state.problem = error.message || "Could not load checkpoint source.";
    } finally {
      if (!disposed && mine === generation) state.loading = false;
    }
  }

  function selectFile(path) {
    if (state.result?.paths.includes(path)) state.path = path;
  }
  function close() {
    ++generation;
    state.selected = "";
    state.result = null;
    state.loading = false;
    state.problem = "";
  }
  return {
    get selected() { return state.selected; },
    get mode() { return state.mode; },
    get path() { return state.path; },
    get loading() { return state.loading; },
    get problem() { return state.problem; },
    get result() { return state.result; },
    select, selectFile, close,
    dispose() { close(); disposed = true; },
  };
}
