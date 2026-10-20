// One owner for history selection, the source endpoints behind it, and the
// request lifetime of a comparison.
//
// Two comparisons. Alone, the selected version is compared with the project
// as it stands, captured when the comparison opens and again on request. With
// a base version set, the base is the older side and the selected version is
// the newer one, or the project as it stands when nothing is selected. It
// deliberately has no dependency on preview rendering or the live editor.
import { createGeneration } from "./generation.js";
import { isMoment } from "../moment.js";

/// A selection is a label's id, and nothing else: the timeline lists
/// versions, and a minute nobody saved is not one.

export function createHistorySource({ read, currentTree, labels = () => [], preferredPath = () => "" }) {
  const state = $state({ selected: "", against: "", path: "", loading: false, problem: "", gone: false, result: null });
  const selections = createGeneration();
  let disposed = false;

  const snapshot = tree => ({ ...tree, texts: { ...tree.texts }, files: { ...tree.files } });
  const label = point => point.label
    || (point.at ? new Date(point.at).toLocaleString() : "")
    || (isMoment(point.sha) ? "An earlier moment" : "")
    || (point.sha ? point.sha.slice(0, 7) : "An earlier moment");

  async function readPoint(sha) {
    const listed = labels().find(point => point.sha === sha);
    const point = { ...listed, ...(await read(sha)) };
    if (point.sha !== sha || !point.texts || typeof point.texts !== "object") {
      throw new Error("This label did not return its source files.");
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
    if (sha && sha === state.against) state.against = "";
    const stale = selections.begin();
    state.selected = sha;
    state.path = path;
    state.result = null;
    state.problem = "";
    state.gone = false;
    state.loading = true;
    try {
      // Capture the current source at selection time, before any network
      // awaits, and always from the collaborative tree.
      const available = currentTree();
      const live = snapshot(available?.then ? await available : available);
      if (disposed || stale()) return;
      const base = state.against;
      const [before, after] = base
        ? await Promise.all([readPoint(base), sha ? readPoint(sha) : null])
        : [null, sha ? await readPoint(sha) : null];
      if (disposed || stale()) return;
      const oldPoint = base ? before : after;
      const newPoint = base ? after : null;
      const oldTree = oldPoint || live;
      const newTree = newPoint || live;
      const paths = [...new Set([
        ...Object.keys(oldTree.texts), ...Object.keys(newTree.texts),
        ...Object.keys(oldTree.files || {}), ...Object.keys(newTree.files || {}),
      ])].sort();
      const status = Object.fromEntries(paths.map(name => [name, statusOf(oldTree, newTree, name)]));
      const changed = paths.filter(name => status[name] !== "same");
      // Land on a file the reader came for, then on one that actually
      // differs, and only then on the document itself.
      const main = (after || before)?.main || newTree.main;
      state.path = paths.includes(path) ? path
        : changed[0]
          || (paths.includes(main) ? main : paths[0] || "");
      state.result = {
        oldTree,
        newTree,
        oldLabel: oldPoint ? label(oldPoint) : "Current version",
        newLabel: newPoint ? label(newPoint) : "Current version",
        against: base,
        paths,
        status,
        changed,
        binary: Object.fromEntries(paths.map(name =>
          [name, (entryOf(oldTree, name) || entryOf(newTree, name))?.kind === "binary"])),
        capturedAt: new Date().toISOString(),
        diff: Boolean(sha || base),
      };
    } catch (error) {
      if (!disposed && !stale()) {
        state.problem = error.message || "Could not load label source.";
        state.gone = Boolean(error.gone);
      }
    } finally {
      if (!disposed && !stale()) state.loading = false;
    }
  }

  function selectFile(path) {
    if (state.result?.paths.includes(path)) state.path = path;
  }
  function compareWith(sha) {
    state.against = sha && sha !== state.selected ? sha : "";
    return select(state.selected);
  }
  function swap() {
    if (state.against && state.selected) {
      [state.selected, state.against] = [state.against, state.selected];
    } else if (state.against) {
      state.selected = state.against;
      state.against = "";
    } else {
      return;
    }
    return select(state.selected);
  }
  function clearBase() {
    state.against = "";
  }
  function close() {
    selections.cancel();
    state.selected = "";
    state.against = "";
    state.result = null;
    state.loading = false;
    state.problem = "";
    state.gone = false;
  }
  return {
    get selected() { return state.selected; },
    get against() { return state.against; },
    get path() { return state.path; },
    get loading() { return state.loading; },
    get problem() { return state.problem; },
    /// The last comparison's moment was trimmed away, which the server says with a 410.
    get gone() { return state.gone; },
    get result() { return state.result; },
    select, selectFile, compareWith, swap, clearBase, close,
    dispose() { close(); disposed = true; },
  };
}
