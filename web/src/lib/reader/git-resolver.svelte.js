// Resolves the conflicts a pull found. Nothing is written into the project
// until every path is resolved and Apply succeeds. The session is kept by
// sync.js, so closing this keeps it.
import { applyResolution, saveResolutions } from "../gitsync/sync.js";

// The workspace owns the choices and the save lifetime. Its view code only
// reads state and calls these methods.
export function createGitResolver() {
  // Per path: { kind: "text" | "binary", base, ours, theirs (text or null), baseId, oursId, theirsId }.
  // "ours" is LibrePaper's copy and "theirs" the repository's.
  // Per path, as applyResolution takes it: { keep: "ours" | "theirs" } | { delete: true } | { text }.
  const state = $state({
    active: false,
    loading: false,
    busy: false,
    error: "",
    items: [],
    resolutions: {},
    selected: "",
    revision: 0,
    // Bumped by every load, so a merge view built from the previous texts is
    // built again.
    loads: 0,
  });

  let ctx = null;
  let onresult = null;
  let current = null;
  let loaded = null;
  let timer = null;
  // Saves run one at a time. A request made meanwhile only marks that the
  // latest choices still need saving, so an older save never lands after a
  // newer one. `held` stops new saves while Apply runs and after it succeeded,
  // so no save recreates a session that Apply removed.
  let inflight = null;
  let wanted = false;
  let held = false;
  let failed = false;

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

  // The first path that still needs a choice, else the first path.
  function firstOpen(items, resolutions) {
    return items.find((item) => !resolutions[item.path])?.path ?? items[0]?.path ?? "";
  }

  async function load(saved) {
    state.loading = true;
    state.error = "";
    try {
      const conflicts = saved.conflicts || [];
      const ids = [...new Set(conflicts.flatMap((item) => [item.base, item.ours, item.theirs]).filter(Boolean))];
      const blobs = ids.length ? await ctx.git.blobs(ids) : new Map();
      const next = [];
      const chosen = {};
      for (const item of conflicts) {
        const texts = [item.base, item.ours, item.theirs].map((id) => (id ? decode(blobs.get(id)) : null));
        const binary = item.kind === "asset" || texts.some((text) => text === undefined);
        next.push({
          path: item.path,
          kind: binary ? "binary" : "text",
          baseId: item.base,
          oursId: item.ours,
          theirsId: item.theirs,
          base: texts[0],
          ours: texts[1],
          theirs: texts[2],
        });
        const kept = saved.resolutions?.[item.path] ?? saved.carried?.[item.path];
        if (kept) chosen[item.path] = kept;
      }
      state.items = next;
      state.resolutions = chosen;
      state.loads += 1;
      // A reload keeps the file on screen when it is still conflicting.
      if (!next.some((item) => item.path === state.selected)) state.selected = firstOpen(next, chosen);
    } catch (failure) {
      state.error = failure?.message || "Could not read the conflicting files.";
    } finally {
      state.loading = false;
    }
  }

  // Opening the session that is already loaded only shows the workspace again.
  async function open(nextCtx, session, callback) {
    ctx = nextCtx;
    onresult = callback;
    state.active = true;
    if (session === loaded) return;
    loaded = session;
    current = session;
    held = false;
    state.selected = "";
    await load(session);
  }

  const select = (path) => {
    state.selected = path;
  };

  function cancelSave() {
    if (timer) clearTimeout(timer);
    timer = null;
  }

  async function drain() {
    try {
      while (wanted && !held && current) {
        wanted = false;
        try {
          await saveResolutions(ctx, current, state.resolutions);
          failed = false;
        } catch (failure) {
          // The choices stay on screen; the next change tries again.
          failed = true;
          state.error = `Could not save your choices: ${failure?.message || "unknown error"}`;
        }
      }
    } finally {
      inflight = null;
    }
  }

  /// Stores the choices on the saved session so a closed window or a restart
  /// keeps them. Resolves when no save is outstanding.
  async function persist() {
    cancelSave();
    if (!current || held) return;
    wanted = true;
    inflight ??= drain();
    await inflight;
  }

  const choose = (path, value) => {
    state.resolutions = { ...state.resolutions, [path]: value };
    // A whole-file choice replaces the Result text, so the merge view reloads it.
    if (value.keep || value.delete) state.revision += 1;
    persist();
  };
  // Typing saves after a short pause. It does not reload the merge view,
  // which already shows the text.
  const edit = (path, text) => {
    state.resolutions = { ...state.resolutions, [path]: { text } };
    cancelSave();
    timer = setTimeout(persist, 500);
  };
  // The Result follows the choice: typed text, then the chosen side, then LibrePaper's copy before any choice.
  const resultText = (item) => {
    const chosen = state.resolutions[item.path];
    if (chosen?.text !== undefined) return chosen.text;
    if (chosen?.delete) return "";
    if (chosen?.keep === "theirs") return item.theirs ?? "";
    return item.ours ?? "";
  };
  const deleteVsModify = (item) => (item.oursId == null) !== (item.theirsId == null);
  const keepSide = (item) => (item.oursId == null ? "theirs" : "ours");
  const allResolved = () => state.items.length > 0 && state.items.every((item) => state.resolutions[item.path]);

  // Flushes a pending save, so closing keeps the latest typing.
  function close() {
    if (timer) persist();
    state.active = false;
  }

  async function apply() {
    if (state.busy || !allResolved()) return;
    state.busy = true;
    state.error = "";
    try {
      // A pending autosave is flushed, not dropped: what is applied is what is saved.
      // An outstanding save is awaited first, and none starts while Apply runs.
      cancelSave();
      await persist();
      held = true;
      if (failed) {
        held = false;
        return;
      }
      const result = await applyResolution(ctx, current, state.resolutions);
      if (result.status === "refused") {
        held = false;
        // The saved session and the choices stay, and so does this window.
        if (result.session) current = result.session;
        state.error = result.refusals?.map((item) => item.reason).join(" ") || "The resolution could not be applied.";
        return;
      }
      if (result.status === "conflict" && result.unresolved?.length) {
        held = false;
        state.error = `These files still need a choice: ${result.unresolved.join(", ")}`;
        return;
      }
      if (result.status === "conflict") {
        // The repository moved and the merge was made again.
        held = false;
        current = result.session;
        loaded = result.session;
        await load(result.session);
        state.error = "The repository changed, so the conflicts were found again. Check the choices and apply again.";
        return;
      }
      state.active = false;
      onresult?.(result);
    } catch (failure) {
      held = false;
      state.error = failure?.message || "Could not apply the resolution.";
    } finally {
      state.busy = false;
    }
  }

  function dispose() {
    cancelSave();
  }

  return { state, open, select, choose, edit, apply, close, dispose, resultText, deleteVsModify, keepSide, allResolved };
}
