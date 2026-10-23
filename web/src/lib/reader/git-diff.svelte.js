// Compares the project with its Git remote. It is read only: nothing is
// written into the project. The view only reads state and calls these methods.
import { compareRemote } from "../gitsync/sync.js";

export function createGitDiff() {
  // Per file, as compareRemote lists it: { path, change, ours, theirs } with blob ids.
  // The loaded item holds the texts: { path, change, kind: "text" | "binary", ours, theirs }.
  const state = $state({
    active: false,
    loading: false,
    error: "",
    label: "",
    branch: "",
    files: [],
    same: 0,
    unsynced: [],
    selected: "",
    item: null,
    itemLoading: false,
  });

  let ctx = null;
  // Loaded items by path. Plain data, not state: the view reads only the selected item.
  let cache = new Map();
  // Bumped by every select, refresh and close, so a load that finishes late is ignored.
  let generation = 0;

  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

  /// The text of a blob, null for no blob, undefined for bytes that are not text.
  function decode(bytes) {
    if (bytes == null) return null;
    if (bytes.includes(0)) return undefined;
    try {
      return decoder.decode(bytes);
    } catch {
      return undefined;
    }
  }

  async function select(path) {
    const run = ++generation;
    state.selected = path;
    state.error = "";
    const file = state.files.find((entry) => entry.path === path);
    if (!file) {
      state.item = null;
      state.itemLoading = false;
      return;
    }
    const cached = cache.get(path);
    if (cached) {
      state.item = cached;
      state.itemLoading = false;
      return;
    }
    state.item = null;
    state.itemLoading = true;
    try {
      const ids = [file.ours, file.theirs].filter(Boolean);
      const blobs = ids.length ? await ctx.git.blobs(ids) : new Map();
      const texts = [file.ours, file.theirs].map((id) => (id ? decode(blobs.get(id)) : null));
      const binary = texts.some((text) => text === undefined);
      const item = {
        path,
        change: file.change,
        kind: binary ? "binary" : "text",
        ours: binary ? null : texts[0],
        theirs: binary ? null : texts[1],
      };
      if (run !== generation) return;
      cache.set(path, item);
      state.item = item;
    } catch (failure) {
      if (run !== generation) return;
      state.error = failure?.message || "Could not read the file.";
    } finally {
      if (run === generation) state.itemLoading = false;
    }
  }

  async function refresh(preferred = state.selected) {
    if (!ctx) return;
    generation += 1;
    state.loading = true;
    state.error = "";
    state.item = null;
    state.itemLoading = false;
    cache = new Map();
    try {
      const result = await compareRemote(ctx);
      if (result.status === "refused") {
        state.error = result.refusals?.map((item) => item.reason).join(" ") || "The project could not be compared.";
        state.files = [];
        state.same = 0;
        state.unsynced = [];
        state.selected = "";
        return;
      }
      state.files = result.files;
      state.same = result.same;
      state.unsynced = result.unsynced;
      const first = result.files.find((file) => file.change === "different") ?? result.files[0];
      state.loading = false;
      await select(preferred || first?.path || "");
    } catch (failure) {
      state.error = failure?.message || "The project could not be compared.";
    } finally {
      state.loading = false;
    }
  }

  async function open(nextCtx, { label, branch }) {
    ctx = nextCtx;
    state.active = true;
    state.label = label;
    state.branch = branch;
    await refresh("");
  }

  /// Shows one file. Opens the comparison first when it is closed or was for
  /// another remote or branch; otherwise only the file changes.
  async function show(nextCtx, { label, branch }, path) {
    if (!state.active || state.label !== label || state.branch !== branch) {
      ctx = nextCtx;
      state.active = true;
      state.label = label;
      state.branch = branch;
      await refresh(path);
      return;
    }
    ctx = nextCtx;
    await select(path);
  }

  function close() {
    generation += 1;
    state.active = false;
    state.item = null;
    state.itemLoading = false;
  }

  return { state, open, select, close, refresh, show };
}
