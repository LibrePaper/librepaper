// One owner for the two ends of a version comparison, the source each end
// reads, and the request lifetime of a comparison.
//
// A comparison is always a pair of endpoints, older and newer, in time order.
// An endpoint is a saved version, a moment in the editing history, the live
// project (CURRENT, always newest) or an empty project (NOTHING, before the
// first version). Clicking a version compares it as the `compareWith` setting
// says, below. Clicking the Latest row shows the newest version against the
// project as it stands. Either side can be changed on its own, from a selection
// mode that picks the replacement, and `target` names the version Restore and
// Download act on.
//
// What a click on a version compares it with is the `compareWith` setting. It
// sticks until it is changed:
// - "previous" (the default): the version before it, against the version.
// - "latest": the version, against the project as it stands.
// - "pair": two versions picked in turn. The next two clicks choose them.
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

// `previous` answers the version before a saved version, as a sha, NOTHING or
// CURRENT. Without it, the order of the labels answers.
export function createHistorySource({ read, currentTree, labels = () => [], preferredPath = () => "", newest, previous }) {
  // `mode` is what a click does. "normal" shows the version. "select-older" and
  // "select-newer" replace one endpoint (a pencil). "select-first" and
  // "select-second" pick the two versions of the "pair" setting.
  // `cancelled` is the endpoint whose picking was last cancelled, to return focus to.
  const state = $state({
    selected: "", older: "", newer: "", path: "", loading: false, problem: "", result: null,
    mode: "normal", cancelled: "", compareWith: "previous",
  });
  const loads = createGeneration();
  let disposed = false;

  const sorted = () => [...labels()].sort(labelOrder);

  // The version before a saved version: the option's answer, else the one
  // listed before it. The first version is compared with an empty project, and
  // an unlisted version has no known position, so it is compared with the live one.
  function previousOf(sha) {
    if (previous) return previous(sha);
    const list = sorted();
    const index = list.findIndex(point => point.sha === sha);
    if (index === -1) return CURRENT;
    return index > 0 ? list[index - 1].sha : NOTHING;
  }

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
    live.catch(() => {}); // Not unhandled when the comparison fails before awaiting it.
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
      // The Latest row has none, so it lands on the live project.
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

  // Shows a comparison whose older side is looked up first, by `find`: the
  // version before a clicked one, or the newest version. The lookup may not be
  // known yet, so a failure is shown as a problem, and a retry is `reload()`.
  async function showAfter(selected, newer, find) {
    const live = captureLive();
    const stale = begin(selected, "", newer);
    let found;
    try {
      found = await find();
    } catch (error) {
      if (!disposed && !stale()) {
        state.loading = false;
        state.problem = error.message || m.history_source_label_load_failed();
      }
      return;
    }
    if (disposed || stale()) return;
    const [older, later] = ordered(found, newer, labels());
    state.older = older;
    state.newer = later;
    return load(stale, live, { selected, older, newer: later, path: state.path });
  }

  // The default comparison for a version the reader clicked. What it compares
  // with depends on the `compareWith` setting.
  async function view(sha) {
    if (disposed) return;
    // The pair setting: the click is a pick, not a comparison.
    if (state.compareWith === "pair") {
      state.mode = "select-first";
      return choose(sha);
    }
    if (sha === "") {
      // The Latest row: the newest version against the project as it stands.
      // Without `newest` the last listed version stands in for it.
      if (!newest) return show("", sorted().at(-1)?.sha ?? CURRENT, CURRENT);
      // The newest version is looked up like an older one: it may not be known yet.
      return showAfter("", CURRENT, newest);
    }
    if (isMoment(sha)) return show(sha, sha, CURRENT);
    // The version before it, against the version itself.
    if (state.compareWith === "previous") return showAfter(sha, sha, () => previousOf(sha));
    // The latest setting: the version, against the project as it stands.
    return compare(sha, CURRENT, sha);
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

  // Starts picking the older or newer endpoint. Asking again for the endpoint
  // already being picked cancels it. The comparison itself is not touched.
  function beginSelect(which) {
    if (state.mode === `select-${which}`) return cancelSelect();
    Object.assign(state, { mode: `select-${which}`, cancelled: "" });
  }
  // Stops picking, keeping the comparison, and remembers which endpoint it was for.
  function cancelSelect() {
    if (state.mode === "normal") return;
    const which = state.mode === "select-older" ? "older" : "newer";
    Object.assign(state, { mode: "normal", cancelled: which });
  }
  // Answers a version (a sha, "" for the live project, or CURRENT). Outside
  // selection, it is a plain view. While picking a pair, the first answer is
  // compared with the live project, and the second completes the pair.
  function choose(id) {
    const target = id || CURRENT;
    if (state.mode === "normal") return view(target === CURRENT ? "" : target);
    if (state.mode === "select-first") {
      Object.assign(state, { mode: "select-second", cancelled: "" });
      return compare(target, CURRENT, target === CURRENT ? "" : target);
    }
    const which = state.mode === "select-older" ? "older" : "newer";
    const replaced = state[which];
    const other = which === "older" ? state.newer : state.older;
    Object.assign(state, { mode: "normal", cancelled: "" });
    // The endpoint already in place, on either side, changes nothing.
    if (target === replaced || target === other) return;
    const [a, b] = which === "older" ? [target, other] : [other, target];
    // A choice that changed the pair makes the setting "pair".
    state.compareWith = "pair";
    // The Latest row is selected as "", as view("") selects it.
    return compare(a, b, target === CURRENT ? "" : target);
  }

  // Changes what a click compares with. A version that is open is shown again
  // on the new basis: `anchor` names the version to show, else the selected one
  // when it is a saved version. The pair setting starts a pick instead.
  function setCompareWith(basis, anchor) {
    if (disposed) return;
    if (basis === state.compareWith && anchor === undefined && basis !== "pair") return;
    state.compareWith = basis;
    if (basis === "pair") {
      Object.assign(state, { mode: "select-first", cancelled: "" });
      return;
    }
    Object.assign(state, { mode: "normal", cancelled: "" });
    if (anchor !== undefined) return view(anchor);
    if (isLabel(state.selected)) return view(state.selected);
    // Otherwise (the Latest row, a moment, nothing open) the comparison is unchanged.
  }

  // Runs the current pair again: a retry, and a fresh capture of the live project.
  function reload() {
    if (disposed) return;
    // An older side still unknown, from a failed lookup: look it up again.
    if (state.newer && !state.older) return view(state.selected);
    if (!state.older || !state.newer) return;
    return show(state.selected, state.older, state.newer);
  }

  function selectFile(path) {
    if (state.result?.paths.includes(path)) state.path = path;
  }
  function close() {
    loads.cancel();
    Object.assign(state, {
      selected: "", older: "", newer: "", result: null, loading: false, problem: "",
      mode: "normal", cancelled: "",
    });
  }
  return {
    get selected() { return state.selected; },
    get older() { return state.older; },
    get newer() { return state.newer; },
    get mode() { return state.mode; },
    get cancelled() { return state.cancelled; },
    // What a click on a version compares it with: "previous", "latest" or "pair".
    get compareWith() { return state.compareWith; },
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
    view, compare, setOlder, setNewer, setCompareWith, beginSelect, cancelSelect, choose, reload, selectFile, close,
    dispose() { close(); disposed = true; },
  };
}
