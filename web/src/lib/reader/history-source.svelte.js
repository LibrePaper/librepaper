// One owner for history selection, the source endpoints behind it, and the
// request lifetime of a comparison.
//
// There is one comparison: the selected version against the project as it
// stands, captured when the comparison opens and again on request. It
// deliberately has no dependency on preview rendering or the live editor.
import { createGeneration } from "./generation.js";

export function createHistorySource({ checkpoint, currentTree, checkpoints = () => [], preferredPath = () => "" }) {
  const state = $state({ selected: "", path: "", loading: false, problem: "", result: null });
  const selections = createGeneration();
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

  // What a file is in a tree, without deciding yet whether it changed: the
  // text it holds, or the digest of the bytes it is. A path a tree does not
  // have answers `null`, which is what "added" and "removed" are read from.
  function entryOf(tree, path) {
    const file = tree.files?.[path];
    if (file && file.kind !== "text") return { kind: "binary", sha: file.sha || "" };
    const text = tree.texts?.[path];
    if (typeof text === "string") return { kind: "text", text };
    return file ? { kind: "text", text: "" } : null;
  }

  function statusOf(oldTree, newTree, path) {
    const before = entryOf(oldTree, path);
    const after = entryOf(newTree, path);
    if (!before && !after) return "same";
    if (!before) return "added";
    if (!after) return "removed";
    if (before.kind !== after.kind) return "changed";
    return (before.kind === "binary" ? before.sha === after.sha : before.text === after.text)
      ? "same"
      : "changed";
  }

  async function select(sha = state.selected, path = state.path || preferredPath()) {
    if (disposed) return;
    const stale = selections.begin();
    state.selected = sha;
    state.path = path;
    state.result = null;
    state.problem = "";
    state.loading = true;
    try {
      // Capture the current source at selection time, before any network
      // awaits, and always from the collaborative tree.
      const available = currentTree();
      const live = snapshot(available?.then ? await available : available);
      if (disposed || stale()) return;
      const point = sha ? await readPoint(sha) : null;
      if (disposed || stale()) return;
      const oldTree = point || live;
      const newTree = live;
      const paths = [...new Set([
        ...Object.keys(oldTree.texts), ...Object.keys(newTree.texts),
        ...Object.keys(oldTree.files || {}), ...Object.keys(newTree.files || {}),
      ])].sort();
      const status = Object.fromEntries(paths.map(name => [name, statusOf(oldTree, newTree, name)]));
      const changed = paths.filter(name => status[name] !== "same");
      // Land on a file the reader came for, then on one that actually
      // differs, and only then on the document itself.
      state.path = paths.includes(path) ? path
        : changed[0]
          || (paths.includes(point?.main || newTree.main) ? (point?.main || newTree.main) : paths[0] || "");
      state.result = {
        oldTree,
        newTree,
        oldLabel: point ? label(point) : "Current version",
        newLabel: "Current version",
        paths,
        status,
        changed,
        binary: Object.fromEntries(paths.map(name =>
          [name, (entryOf(oldTree, name) || entryOf(newTree, name))?.kind === "binary"])),
        capturedAt: new Date().toISOString(),
        diff: Boolean(sha),
      };
    } catch (error) {
      if (!disposed && !stale()) state.problem = error.message || "Could not load checkpoint source.";
    } finally {
      if (!disposed && !stale()) state.loading = false;
    }
  }

  function selectFile(path) {
    if (state.result?.paths.includes(path)) state.path = path;
  }
  function close() {
    selections.cancel();
    state.selected = "";
    state.result = null;
    state.loading = false;
    state.problem = "";
  }
  return {
    get selected() { return state.selected; },
    get path() { return state.path; },
    get loading() { return state.loading; },
    get problem() { return state.problem; },
    get result() { return state.result; },
    select, selectFile, close,
    dispose() { close(); disposed = true; },
  };
}
