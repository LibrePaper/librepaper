// The history panel's data and comparisons.
//
// Reader owns the document pane and checkpoint navigation. This controller
// owns the history manifest, comparison choices, passage/file diffs, and the
// async lifetimes behind them. Its callers provide the live session and the
// small pieces of UI that must remain in Reader.
import * as historyApi from "../history.js";
import * as passagesApi from "../passages.js";
import { read, write } from "../storage.js";

const emptyLive = () => ({ session: null, text: null });

export function createHistoryController({
  slug,
  headers = () => ({}),
  baselineKey = `librepaper-history-baseline:${slug}`,
  readBaseline = () => read(baselineKey, ""),
  rememberBaseline = (sha) => write(baselineKey, sha),
  comments = () => [],
  live = emptyLive,
  viewing = () => null,
  sourceFormat = () => "markdown",
  mayEdit = () => false,
  editing = () => false,
  onRedlines = () => {},
  onMerge = async () => {},
  history = historyApi,
  passages = passagesApi,
} = {}) {
  const state = $state({
    checkpoints: [],
    problem: "",
    baseline: null,
    target: null,
    changes: null,
    changedPaths: [],
    redlines: false,
    fileDiff: null,
  });

  let disposed = false;
  let loadGeneration = 0;
  let baselineGeneration = 0;
  let diffGeneration = 0;
  let fileDiffGeneration = 0;

  const alive = () => !disposed;
  const current = (generation, kind) => {
    if (!alive()) return false;
    if (kind === "load") return generation === loadGeneration;
    if (kind === "baseline") return generation === baselineGeneration;
    if (kind === "diff") return generation === diffGeneration;
    if (kind === "file") return generation === fileDiffGeneration;
    return false;
  };

  function notifyRedlines() {
    if (alive()) onRedlines();
  }

  function invalidateFileDiff() {
    fileDiffGeneration += 1;
    state.fileDiff = null;
  }

  function clearComparison() {
    state.changes = null;
    state.changedPaths = [];
    notifyRedlines();
    invalidateFileDiff();
  }

  function listedPoint(sha) {
    return state.checkpoints.find((point) => point.sha === sha) || null;
  }

  async function load() {
    const generation = ++loadGeneration;
    try {
      const points = await history.load(slug, headers());
      if (!current(generation, "load")) return;
      state.checkpoints = points;
      state.problem = "";
      if (!state.baseline && points.length) {
        const remembered = readBaseline();
        const own = [...(comments() || [])].reverse().find((comment) =>
          comment.mine && comment.revision && points.some((point) => point.sha === comment.revision),
        );
        const sha = points.some((point) => point.sha === remembered)
          ? remembered
          : own?.revision || points[0].sha;
        await chooseBaseline(sha);
      }
    } catch (error) {
      if (current(generation, "load")) state.problem = error.message || "the history could not be read";
    }
  }

  async function chooseBaseline(sha) {
    const generation = ++baselineGeneration;
    const listed = listedPoint(sha);
    if (!listed) return;
    try {
      const point = listed.texts ? listed : await history.checkpoint(slug, sha, headers());
      if (!current(generation, "baseline")) return;
      state.baseline = point;
      if (state.target?.sha === sha) state.target = null;
      onMerge(null);
      clearComparison();
      state.problem = "";
      rememberBaseline(sha);
      await computeChanges(point);
    } catch (error) {
      if (current(generation, "baseline")) state.problem = error.message || "that checkpoint could not be read";
    }
  }

  async function chooseTarget(sha) {
    const generation = ++baselineGeneration;
    onMerge(null);
    invalidateFileDiff();
    if (!sha) {
      state.target = null;
      state.changes = null;
      state.changedPaths = [];
      notifyRedlines();
      await computeChanges();
      return;
    }
    const listed = listedPoint(sha);
    if (!listed) return;
    try {
      const point = listed.texts ? listed : await history.checkpoint(slug, sha, headers());
      if (!current(generation, "baseline")) return;
      state.target = point;
      clearComparison();
      state.problem = "";
      await computeChanges();
    } catch (error) {
      if (current(generation, "baseline")) state.problem = error.message || "that checkpoint could not be read";
    }
  }

  async function computeChanges(point = state.baseline) {
    const liveState = live() || emptyLive();
    const session = liveState.session;
    const liveText = liveState.text;
    if (!point || !session || liveText === null || (!state.target && viewing())) return;

    const baselineGenerationAtStart = baselineGeneration;
    const target = state.target;
    const request = ++diffGeneration;
    try {
      const oldVisible = await passages.textAt(slug, point.sha, headers());
      const targetVisible = target
        ? await passages.textAt(slug, target.sha, headers())
        : liveText;
      if (!current(request, "diff") || baselineGenerationAtStart !== baselineGeneration || target !== state.target) return;
      if (live().session !== session || (!target && live().text !== liveText)) return;
      if (typeof oldVisible !== "string" || typeof targetVisible !== "string") {
        state.changes = [];
        state.problem = "Changes are unavailable for this checkpoint.";
        notifyRedlines();
        return;
      }

      const edits = await history.wordDiff(oldVisible, targetVisible, sourceFormat());
      if (!current(request, "diff") || baselineGenerationAtStart !== baselineGeneration || target !== state.target) return;
      if (live().session !== session || (!target && live().text !== liveText)) return;

      let shift = 0;
      state.changes = history.hunks(oldVisible, targetVisible, edits).map((hunk) => {
        const newAt = hunk.at + shift;
        shift += (hunk.insert || "").length - (hunk.delete || 0);
        return {
          ...hunk,
          path: point.main || "document",
          new: hunk.insert || "",
          exact: hunk.insert || "",
          position: newAt,
          prefix: hunk.currentBefore || "",
          suffix: hunk.currentAfter || "",
          contextBefore: hunk.insert ? (hunk.currentBefore || hunk.before || "") : (hunk.before || ""),
          contextAfter: hunk.insert ? (hunk.currentAfter || hunk.after || "") : (hunk.after || ""),
        };
      });

      const paths = new Set();
      const baselineTexts = point.texts || {};
      const targetTree = target ? target : session.tree();
      const targetTexts = targetTree.texts || {};
      for (const path of new Set([...Object.keys(baselineTexts), ...Object.keys(targetTexts)])) {
        if ((baselineTexts[path] || "") !== (targetTexts[path] || "")) paths.add(path);
      }
      const oldFiles = point.files || {};
      const newFiles = targetTree.files || {};
      for (const path of new Set([...Object.keys(oldFiles), ...Object.keys(newFiles)])) {
        const oldEntry = oldFiles[path] || null;
        const newEntry = newFiles[path] || null;
        if (!oldEntry || !newEntry || oldEntry.kind !== newEntry.kind) {
          paths.add(path);
        } else if (oldEntry.kind === "asset" && oldEntry.sha !== newEntry.sha) {
          paths.add(path);
        } else if (oldEntry.kind === "text" && (baselineTexts[path] || "") !== (targetTexts[path] || "")) {
          paths.add(path);
        }
      }
      state.changedPaths = [...paths].sort();
      state.problem = "";
      notifyRedlines();
    } catch (error) {
      if (current(request, "diff") && baselineGenerationAtStart === baselineGeneration) {
        state.changes = [];
        state.problem = error.message || "Changes are unavailable for this checkpoint.";
        notifyRedlines();
      }
    }
  }

  async function openCheckpointFile(point, path) {
    if (!state.checkpoints.some((candidate) => candidate.sha === point.parent)) {
      state.problem = "The previous checkpoint is no longer available for this comparison.";
      return;
    }
    await chooseBaseline(point.parent);
    if (!alive() || state.baseline?.sha !== point.parent) return;
    await chooseTarget(point.sha);
    if (alive() && state.target?.sha === point.sha) await openFileDiff(path);
  }

  async function openFileDiff(path) {
    const base = state.baseline;
    const liveState = live() || emptyLive();
    const session = liveState.session;
    if (!base || !session) return;
    const request = ++fileDiffGeneration;
    const target = state.target;
    const targetTree = target || session.tree();
    const oldText = base.texts?.[path];
    const newText = targetTree.texts?.[path];
    onMerge(null);
    state.fileDiff = { path, loading: true };
    try {
      const edits = await history.wordDiff(oldText ?? "", newText ?? "");
      if (!current(request, "file") || base !== state.baseline || target !== state.target || session !== (live()?.session || null)) return;
      state.fileDiff = {
        path,
        old: oldText,
        new: newText,
        hunks: history.hunks(oldText ?? "", newText ?? "", edits),
        oldEntry: base.files?.[path],
        newEntry: targetTree.files?.[path],
      };
      if (!mayEdit() || !editing() || newText === undefined) return;
      if (!current(request, "file") || base !== state.baseline || target !== state.target || session !== (live()?.session || null)) return;
      const id = session.idOf(path);
      state.fileDiff = null;
      await onMerge({
        path,
        oldText: oldText ?? "",
        newText,
        liveText: target ? null : id ? session.textOf(id) : null,
        awareness: target ? null : session.awareness,
        editable: !target && Boolean(id),
        targetLabel: target?.label || (target ? history.shortSha(target.sha) : "Live document"),
      });
    } catch (error) {
      if (current(request, "file")) state.fileDiff = { path, problem: error.message || "This comparison is unavailable." };
    }
  }

  function setRedlines(on) {
    state.redlines = Boolean(on);
    notifyRedlines();
  }

  function closeFileDiff() {
    invalidateFileDiff();
    onMerge(null);
  }

  // Checkpoint navigation belongs to Reader, but a navigation invalidates a
  // live-document comparison that was waiting on the old visible page.
  function invalidateChanges() {
    diffGeneration += 1;
    if (!state.target) state.changes = null;
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    loadGeneration += 1;
    baselineGeneration += 1;
    diffGeneration += 1;
    fileDiffGeneration += 1;
  }

  return {
    get checkpoints() { return state.checkpoints; },
    get problem() { return state.problem; },
    get baseline() { return state.baseline; },
    get target() { return state.target; },
    get changes() { return state.changes; },
    get changedPaths() { return state.changedPaths; },
    get redlines() { return state.redlines; },
    get fileDiff() { return state.fileDiff; },
    load,
    chooseBaseline,
    chooseTarget,
    computeChanges,
    openCheckpointFile,
    openFileDiff,
    setRedlines,
    closeFileDiff,
    invalidateChanges,
    dispose,
  };
}
