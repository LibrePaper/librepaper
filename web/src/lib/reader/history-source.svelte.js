// One owner for the two ends of a version comparison, the source each end
// reads, and the request lifetime of a comparison.
//
// A comparison is always a pair of endpoints, older and newer, in time order.
// An endpoint is a saved version, a moment in the editing history, the live
// project (CURRENT, always newest) or an empty project (NOTHING, before the
// first version). Clicking a version shows what that version changed: the
// previous version against it. Clicking the Current row shows the newest
// version against the project as it stands. Either side can be changed on its
// own, and `target` names the version Restore and Download act on.
//
// The live project is captured when a comparison begins, before any network
// await, and again on `reload()`. It deliberately has no dependency on preview
// rendering or the live editor.
import { createGeneration } from "./generation.js";
import { isMoment } from "../moment.js";
import { openingFile } from "../opening.js";
import { m } from "../paraglide/messages.js";
import { CURRENT, NOTHING, ordered } from "../history-endpoints.js";
import { labelOrder } from "../history.js";

/// A label is a saved version: an id that is neither a sentinel nor a moment.
const isLabel = id => Boolean(id) && id !== CURRENT && id !== NOTHING && !isMoment(id);

export function createHistorySource({ read, currentTree, labels = () => [], preferredPath = () => "", previous, newest }) {
  const state = $state({ selected: "", older: "", newer: "", path: "", loading: false, problem: "", result: null });
  const loads = createGeneration();
  let disposed = false;

  const sorted = () => [...labels()].sort(labelOrder);
  // The version just before `sha` in time, or NOTHING for the first one.
  const previousOf = async sha => {
    const list = sorted();
    const index = list.findIndex(point => point.sha === sha);
    return index > 0 ? list[index - 1].sha : NOTHING;
  };

  const snapshot = tree => ({ ...tree, texts: { ...tree.texts }, files: { ...tree.files } });
  // The live project, captured now. An async function runs its body up to its
  // first await synchronously, so `currentTree()` is called before the caller
  // awaits anything. A failure surfaces where `load` awaits it.
  function captureLive() {
    const capture = async () => {
      const available = currentTree();
      return snapshot(available?.then ? await available : available);
    };
    const live = capture();
    live.catch(() => {}); // Not unhandled while a slow `previous()` is awaited.
    return live;
  }

  async function readPoint(sha) {
    const listed = labels().find(point => point.sha === sha);
    const point = { ...listed, ...(await read(sha)) };
    if (point.sha !== sha || !point.texts || typeof point.texts !== "object") {
      throw new Error(m.history_source_label_no_source());
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

  // Starts a comparison: the endpoints and the loading state are set now, and
  // the returned predicate answers whether a later run has overtaken this one.
  function begin(selected, older, newer) {
    if (!state.path) state.path = preferredPath();
    Object.assign(state, { selected, older, newer, result: null, problem: "", loading: true });
    return loads.begin();
  }

  // The tree an endpoint stands for: the live snapshot for CURRENT, an empty
  // project for NOTHING, and the saved version otherwise.
  const treeOf = (id, live) => {
    if (id === CURRENT) return live;
    if (id === NOTHING) return { texts: {}, files: {} };
    return readPoint(id);
  };

  // Reads both endpoints and lays the result out. `live` is the capture taken
  // when this comparison began, and `pair` is what it was begun with, so a
  // reply can only fill the comparison it belongs to.
  async function load(stale, live, pair) {
    try {
      const current = await live;
      const [oldTree, newTree] = await Promise.all([
        treeOf(pair.older, current),
        treeOf(pair.newer, current),
      ]);
      if (disposed || stale()) return;
      const paths = [...new Set([
        ...Object.keys(oldTree.texts), ...Object.keys(newTree.texts),
        ...Object.keys(oldTree.files || {}), ...Object.keys(newTree.files || {}),
      ])].sort();
      const status = Object.fromEntries(paths.map(name => [name, statusOf(oldTree, newTree, name)]));
      const changed = paths.filter(name => status[name] !== "same");
      // The tree of the version the reader clicked, when it is an endpoint.
      // The Current row has none, so it lands on the live project.
      const came = pair.selected && pair.selected === pair.newer ? newTree
        : pair.selected && pair.selected === pair.older ? oldTree
          : null;
      // Land on the file the reader came for when it changed, then on any
      // file that changed: opening on an unchanged file would read as "nothing
      // changed" when the version changed another one. With no change at all,
      // keep the reader's file, then the file the project opens on.
      const landing = openingFile((came || newTree).texts);
      state.path = changed.includes(pair.path) ? pair.path
        : changed[0]
          || (paths.includes(pair.path) ? pair.path
            : paths.includes(landing) ? landing : paths[0] || "");
      state.result = {
        oldTree,
        newTree,
        older: pair.older,
        newer: pair.newer,
        paths,
        status,
        changed,
        binary: Object.fromEntries(paths.map(name =>
          [name, (entryOf(oldTree, name) || entryOf(newTree, name))?.kind === "binary"])),
        capturedAt: new Date().toISOString(),
        // Nothing to compare only when there are no versions at all: the
        // project is then shown alone.
        diff: !(pair.older === CURRENT && pair.newer === CURRENT),
      };
    } catch (error) {
      if (!disposed && !stale()) {
        state.problem = error.message || m.history_source_label_load_failed();
      }
    } finally {
      if (!disposed && !stale()) state.loading = false;
    }
  }

  // Shows a pair whose endpoints are already known.
  function show(selected, older, newer) {
    const live = captureLive();
    const stale = begin(selected, older, newer);
    return load(stale, live, { selected, older, newer, path: state.path });
  }

  // The default comparison for a version the reader clicked.
  async function view(sha) {
    if (disposed) return;
    if (sha === "") {
      // The Current row: the newest version against the project as it stands.
      if (!newest) return show("", sorted().at(-1)?.sha ?? CURRENT, CURRENT);
      // The newest version is looked up like an older one: it may not be known yet.
      const live = captureLive();
      const stale = begin("", "", CURRENT);
      let older;
      try {
        older = await newest();
      } catch (error) {
        if (!disposed && !stale()) {
          state.loading = false;
          state.problem = error.message || m.history_source_label_load_failed();
        }
        return;
      }
      if (disposed || stale()) return;
      state.older = older;
      return load(stale, live, { selected: "", older, newer: CURRENT, path: state.path });
    }
    if (isMoment(sha)) return show(sha, sha, CURRENT);
    // A label: the version before it. The lookup is awaited, so the run begins
    // first and the live project is captured before it.
    const live = captureLive();
    const stale = begin(sha, "", sha);
    let older;
    try {
      older = await (previous ?? previousOf)(sha);
    } catch (error) {
      if (!disposed && !stale()) {
        state.loading = false;
        state.problem = error.message || m.history_source_label_load_failed();
      }
      return;
    }
    if (disposed || stale()) return;
    state.older = older;
    return load(stale, live, { selected: sha, older, newer: sha, path: state.path });
  }

  // Compares two endpoints, normalised into time order. `selected` is the
  // version the reader clicked: when given it is used, otherwise it stays if it
  // is still an endpoint, and becomes `a` if it is not.
  function compare(a, b, selected) {
    const [older, newer] = ordered(a, b, labels());
    const kept = selected !== undefined ? selected
      : [older, newer].includes(state.selected) ? state.selected : a;
    return show(kept, older, newer);
  }

  // Changing one side keeps the other: the side changed is the one named.
  function setOlder(sha) {
    return compare(sha, state.newer);
  }
  function setNewer(sha) {
    return compare(sha, state.older);
  }

  // Runs the current pair again: a retry, and a fresh capture of the live project.
  function reload() {
    if (disposed) return;
    // A label whose previous version failed to load has no older side yet:
    // look that up again rather than doing nothing.
    if (isLabel(state.selected) && state.newer === state.selected && !state.older) return view(state.selected);
    // The Current row whose newest version is still unknown: look that up again.
    if (state.selected === "" && state.newer === CURRENT && !state.older) return view("");
    if (!state.older || !state.newer) return;
    return show(state.selected, state.older, state.newer);
  }

  function selectFile(path) {
    if (state.result?.paths.includes(path)) state.path = path;
  }
  function close() {
    loads.cancel();
    Object.assign(state, { selected: "", older: "", newer: "", result: null, loading: false, problem: "" });
  }
  return {
    get selected() { return state.selected; },
    get older() { return state.older; },
    get newer() { return state.newer; },
    // The version Restore and Download act on: the clicked one when it is an
    // endpoint, else the newer saved version, else the older one, else none.
    get target() {
      if (isLabel(state.selected) && [state.older, state.newer].includes(state.selected)) return state.selected;
      if (isLabel(state.newer)) return state.newer;
      if (isLabel(state.older)) return state.older;
      return "";
    },
    get path() { return state.path; },
    get loading() { return state.loading; },
    get problem() { return state.problem; },
    get result() { return state.result; },
    view, compare, setOlder, setNewer, reload, selectFile, close,
    dispose() { close(); disposed = true; },
  };
}
