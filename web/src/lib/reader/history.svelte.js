// The history panel's data and comparisons.
//
// Reader owns the document pane and checkpoint navigation. This controller
// owns the history manifest, comparison choices, passage/file diffs, and the
// async lifetimes behind them. Its callers provide the live session and the
// small pieces of UI that must remain in Reader.
import * as historyApi from "../history.js";
import * as passagesApi from "../passages.js";
import { read, write } from "../storage.js";
import { attribution, attributeChain } from "../redlines.js";
import { snapshotDigest as digestTree } from "../tree-digest.js";

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
  snapshotDigest = digestTree,
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
    // Painted in the document from the start: the document is where the
    // changes are, and the panel only says which range is being shown.
    redlines: true,
    fileDiff: null,
    // An explicit comparison has a frozen copy of the live tree as its
    // target.  Keeping this separate from `target` is useful to callers that
    // need to distinguish a checkpoint from the captured current document.
    comparingCurrent: false,
    newerEdits: false,
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

  const treeSignature = (tree) => JSON.stringify({
    main: tree?.main || "", texts: tree?.texts || {}, files: tree?.files || {},
    digests: tree?.digests || {}, settings: tree?.settings || {},
  });
  async function liveSnapshot() {
    const liveState = live() || emptyLive();
    const session = liveState.session;
    if (!session) return null;
    const sourceTree = liveState.tree || session.tree?.() || {};
    const texts = { ...(sourceTree.texts || {}) };
    const files = Object.fromEntries(Object.entries(sourceTree.files || {}).map(([path, file]) => [path, { ...file }]));
    const tree = {
      ...sourceTree,
      texts,
      files,
      digests: { ...(sourceTree.digests || {}) },
      settings: sourceTree.settings ? { ...sourceTree.settings } : sourceTree.settings,
    };
    const at = new Date().toISOString();
    const sha = await snapshotDigest(tree);
    const snapshot = {
      ...tree,
      sha,
      at,
      label: "Current version",
      _current: true,
      _signature: treeSignature(tree),
      texts,
      files,
    };
    return snapshot;
  }

  function currentSnapshotChanged(point = state.target) {
    if (!point?._current) return false;
    const now = live() || emptyLive();
    const tree = now.tree || now.session?.tree?.() || {};
    return point._signature !== treeSignature(tree);
  }

  // Reader calls this when its live source changes. Keeping mutation out of
  // the `newerEdits` getter makes that getter safe to consume from `$derived`.
  function noteLiveChange() {
    if (state.comparingCurrent && currentSnapshotChanged()) state.newerEdits = true;
  }

  const indexOf = (sha) => state.checkpoints.findIndex((point) => point.sha === sha);

  async function load() {
    if (disposed) return;
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
    if (disposed) return;
    const generation = ++baselineGeneration;
    const manifestGeneration = loadGeneration;
    const listed = listedPoint(sha);
    if (!listed) return;
    try {
      const point = listed.texts ? listed : await history.checkpoint(slug, sha, headers());
      if (!current(generation, "baseline") || manifestGeneration !== loadGeneration) return;
      state.baseline = point;
      state.comparingCurrent = false;
      state.newerEdits = false;
      // The range reads forward from the baseline. A compare point at or
      // before it is no longer one, so the comparison runs to the live
      // document instead.
      if (state.target && indexOf(state.target.sha) <= indexOf(sha)) state.target = null;
      onMerge(null);
      clearComparison();
      state.problem = "";
      rememberBaseline(sha);
      await computeChanges(point);
    } catch (error) {
      if (current(generation, "baseline") && manifestGeneration === loadGeneration) state.problem = error.message || "that checkpoint could not be read";
    }
  }

  async function chooseTarget(sha) {
    if (disposed) return;
    const generation = ++baselineGeneration;
    const manifestGeneration = loadGeneration;
    onMerge(null);
    invalidateFileDiff();
    state.comparingCurrent = false;
    state.newerEdits = false;
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
      if (!current(generation, "baseline") || manifestGeneration !== loadGeneration) return;
      state.target = point;
      clearComparison();
      state.problem = "";
      await computeChanges();
    } catch (error) {
      if (current(generation, "baseline") && manifestGeneration === loadGeneration) state.problem = error.message || "that checkpoint could not be read";
    }
  }

  // What changed up to a checkpoint, as the timeline asks it: the checkpoint
  // becomes the compare point, and when it is not after the baseline -- the
  // reader clicked an older row -- the baseline moves to the checkpoint just
  // before it, so the range shows what that checkpoint itself changed. The
  // first checkpoint has nothing before it and compares with itself, which
  // is an empty change list rather than an error.
  async function compareTo(sha) {
    if (disposed) return;
    if (!sha) {
      await chooseTarget("");
      return;
    }
    const at = indexOf(sha);
    if (at < 0) return;
    // Ordinary selection always means this checkpoint versus its immediate
    // predecessor.  It must not inherit a baseline chosen by a prior action.
    const before = state.checkpoints[Math.max(0, at - 1)];
    const pending = chooseBaseline(before.sha);
    const generation = baselineGeneration;
    await pending;
    if (!current(generation, "baseline") || state.baseline?.sha !== before.sha) return;
    await chooseTarget(sha);
  }

  // Enter a stable comparison against the current document.  The target is
  // copied now; later edits are reported, but never folded into the diff.
  async function compareWithCurrent(sha) {
    if (disposed || !sha || indexOf(sha) < 0) return;
    const pending = chooseBaseline(sha);
    const generation = baselineGeneration;
    await pending;
    if (!current(generation, "baseline") || state.baseline?.sha !== sha) return;
    const captureGeneration = baselineGeneration;
    const snapshot = await liveSnapshot();
    if (!alive() || captureGeneration !== baselineGeneration) return;
    if (!snapshot) return;
    ++baselineGeneration;
    state.target = snapshot;
    state.comparingCurrent = true;
    state.newerEdits = false;
    // Edits may have landed while the digest/render capture was pending.
    noteLiveChange();
    clearComparison();
    state.problem = "";
    await computeChanges(state.baseline);
  }

  async function refreshCurrent() {
    if (disposed || !state.comparingCurrent || !state.baseline) return;
    const captureGeneration = baselineGeneration;
    const snapshot = await liveSnapshot();
    if (!alive() || captureGeneration !== baselineGeneration) return;
    if (!snapshot) return;
    ++baselineGeneration;
    state.target = snapshot;
    state.newerEdits = false;
    noteLiveChange();
    clearComparison();
    await computeChanges(state.baseline);
  }

  function updateChangedPaths(point, target, session) {
    const paths = new Set();
    const baselineTexts = point.texts || {};
    const targetTree = target || session.tree();
    const targetTexts = targetTree.texts || {};
    for (const path of new Set([...Object.keys(baselineTexts), ...Object.keys(targetTexts)])) {
      if ((baselineTexts[path] || "") !== (targetTexts[path] || "")) paths.add(path);
    }
    const oldFiles = point.files || {};
    const newFiles = targetTree.files || {};
    for (const path of new Set([...Object.keys(oldFiles), ...Object.keys(newFiles)])) {
      const oldEntry = oldFiles[path] || null;
      const newEntry = newFiles[path] || null;
      if (!oldEntry || !newEntry || oldEntry.kind !== newEntry.kind) paths.add(path);
      else if (oldEntry.kind === "asset" && oldEntry.sha !== newEntry.sha) paths.add(path);
      else if (oldEntry.kind === "text" && (baselineTexts[path] || "") !== (targetTexts[path] || "")) paths.add(path);
    }
    state.changedPaths = [...paths].sort();
  }

  async function computeChanges(point = state.baseline) {
    if (disposed) return;
    const liveState = live() || emptyLive();
    const session = liveState.session;
    const liveText = liveState.text;
    if (!point || !session || liveText === null || (!state.target && viewing())) return;

    const baselineGenerationAtStart = baselineGeneration;
    const target = state.target;
    const request = ++diffGeneration;
    try {
      const oldVisible = await passages.textAt(slug, point.sha, headers());
      const targetVisible = target?._current
        ? await passages.textAt(slug, target.sha, headers(), { history: { checkpoint: async () => target } })
        : target
        ? await passages.textAt(slug, target.sha, headers())
        : liveText;
      if (!current(request, "diff") || baselineGenerationAtStart !== baselineGeneration || target !== state.target) return;
      if (live().session !== session || (!target && live().text !== liveText)) return;
      updateChangedPaths(point, target, session);
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

      state.problem = "";
      notifyRedlines();

      // Per-author colouring needs the diff of each step between
      // checkpoints, not just the range as a whole -- otherwise a paragraph
      // two people edited in turn reads as one person's work. Chaining more
      // than a dozen diffs to colour a redline is not worth the requests;
      // beyond that, hunks carry no `who` and `itemsFor` falls back to the
      // range-level name from `attribution()`. This runs after the redlines
      // above are already painted and published, as a refinement: a late
      // result here only ever upgrades a still-current comparison's hunks
      // with a `who`, never anything else, so it is safe to just skip when
      // the comparison has moved on rather than unwind the whole function.
      const baselineAt = indexOf(point.sha);
      const endAt = target ? indexOf(target.sha) : state.checkpoints.length - 1;
      const rangeCheckpoints = endAt >= baselineAt ? state.checkpoints.slice(baselineAt + 1, endAt + 1) : [];
      if (rangeCheckpoints.length && rangeCheckpoints.length <= 12) {
        try {
          const stepTexts = await Promise.all(
            rangeCheckpoints.map((checkpoint) => passages.textAt(slug, checkpoint.sha, headers())),
          );
          const points = [oldVisible, ...stepTexts];
          const authorsChain = rangeCheckpoints.map((checkpoint) => checkpoint.by || "");
          if (!target) {
            // The range runs to the live document, which has no checkpoint
            // of its own yet -- its author is unattributed, same as
            // `attribution()`'s existing treatment of uncommitted edits.
            points.push(targetVisible);
            authorsChain.push("");
          }
          const steps = [];
          for (let index = 0; index < points.length - 1; index += 1) {
            const stepEdits = await history.wordDiff(points[index], points[index + 1], sourceFormat());
            steps.push({ by: authorsChain[index], hunks: history.hunks(points[index], points[index + 1], stepEdits) });
          }
          const stillCurrent = current(request, "diff") && baselineGenerationAtStart === baselineGeneration
            && target === state.target && live().session === session && (target || live().text === liveText);
          if (stillCurrent && Array.isArray(state.changes)) {
            const attributed = attributeChain(
              steps,
              state.changes,
              attribution(state.checkpoints, point.sha, target?.sha || null),
            );
            // Written onto the hunks in place: a new array would tell the
            // panel the comparison changed and reset the reader's place in it.
            for (const [index, hunk] of attributed.entries()) {
              if (state.changes[index]) state.changes[index].who = hunk.who;
            }
            notifyRedlines();
          }
        } catch {
          // Leave hunks without `who`; itemsFor falls back to the range-level name.
        }
      }
    } catch (error) {
      if (current(request, "diff") && baselineGenerationAtStart === baselineGeneration) {
        state.changes = [];
        state.problem = error.message || "Changes are unavailable for this checkpoint.";
        notifyRedlines();
      }
    }
  }

  async function openCheckpointFile(point, path) {
    if (disposed) return;
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
    if (disposed) return;
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
      const stillCurrent = () => current(request, "file") && base === state.baseline
        && target === state.target && session === (live()?.session || null) && mayEdit() && editing();
      await onMerge({
        path,
        oldText: oldText ?? "",
        newText,
        liveText: target ? null : id ? session.textOf(id) : null,
        awareness: target ? null : session.awareness,
        editable: !target && Boolean(id),
        targetLabel: target?.label || (target ? history.shortSha(target.sha) : "Live document"),
      }, stillCurrent);
      if (stillCurrent()) state.fileDiff = null;
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
    baselineGeneration += 1;
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
    get capturedCurrent() { return state.target?._current ? state.target : null; },
    get comparingCurrent() { return state.comparingCurrent; },
    get newerEdits() {
      return state.newerEdits;
    },
    get changes() { return state.changes; },
    get changedPaths() { return state.changedPaths; },
    get redlines() { return state.redlines; },
    get fileDiff() { return state.fileDiff; },
    load,
    chooseBaseline,
    chooseTarget,
    compareTo,
    compareWithCurrent,
    refreshCurrent,
    noteLiveChange,
    computeChanges,
    openCheckpointFile,
    openFileDiff,
    setRedlines,
    closeFileDiff,
    invalidateChanges,
    dispose,
  };
}
