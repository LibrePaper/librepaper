// The history panel's data and comparisons.
//
// Reader owns the document pane and checkpoint navigation. This controller
// owns the history manifest, comparison choices, passage/file diffs, and the
// async lifetimes behind them. Its callers provide the live session and the
// small pieces of UI that must remain in Reader.
import * as historyApi from "../history.js";
import * as passagesApi from "../passages.js";
import { read, write } from "../storage.js";
import { attribution } from "../redlines.js";
import { provenIntervals, refineSourceAttribution, MAX_PROVENANCE_INTERVALS } from "../provenance.js";
import { snapshotDigest as digestTree } from "../tree-digest.js";
import { projectText } from "../diff-display.js";
import { compareProjections, retireComparisons } from "../compare-projections.js";

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
  // Reader may paint a captured-current target while the baseline is still
  // rendering. Historical targets are already shown by their navigation path.
  onTargetReady = () => {},
  onMerge = async () => {},
  history = historyApi,
  passages = passagesApi,
} = {}) {
  const state = $state({
    checkpoints: [],
    durability: null,
    problem: "",
    baseline: null,
    target: null,
    changes: null,
    projection: null,
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
  let pendingBaseline = 0;

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
    state.projection = null;
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
    let liveState = live() || emptyLive();
    const session = liveState.session;
    if (!session) return null;
    // A historical URL can render before the initial collaborative state
    // arrives. Never freeze the temporary empty Yjs document as "Current".
    const deadline = Date.now() + 10_000;
    while (session.joined === false && alive() && live().session === session && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    if (!alive() || live().session !== session) return null;
    if (session.joined === false) throw new Error("The current document is still connecting. Retry comparison once it is loaded.");
    liveState = live() || emptyLive();
    const sourceTree = liveState.tree || session.tree?.() || {};
    const texts = { ...(sourceTree.texts || {}) };
    const files = Object.fromEntries(Object.entries(sourceTree.files || {}).map(([path, file]) => [path, { ...file }]));
    let tree = {
      ...sourceTree,
      texts,
      files,
      digests: { ...(sourceTree.digests || {}) },
      settings: sourceTree.settings ? { ...sourceTree.settings } : sourceTree.settings,
    };
    if (passages.captureTree) tree = await passages.captureTree(slug, tree, headers());
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
    // Do not carry a previous response's boundary through a refresh whose
    // result has not been revalidated yet.
    state.durability = null;
    try {
      // Prefer the metadata-bearing adapter, while accepting the historical
      // list-only seam used by embedded readers and older integrations.
      const loaded = typeof history.loadWithStatus === "function"
        ? await history.loadWithStatus(slug, headers())
        : await history.load(slug, headers());
      const points = Array.isArray(loaded) ? loaded : (loaded?.checkpoints || []);
      if (!current(generation, "load")) return;
      state.checkpoints = Array.isArray(points) ? points : [];
      state.durability = Array.isArray(loaded) ? null : (loaded?.durability || null);
      state.problem = "";
      if (!state.baseline && !pendingBaseline && state.checkpoints.length) {
        const remembered = readBaseline();
        const own = [...(comments() || [])].reverse().find((comment) =>
          comment.mine && comment.revision && state.checkpoints.some((point) => point.sha === comment.revision),
        );
        const sha = state.checkpoints.some((point) => point.sha === remembered)
          ? remembered
          : own?.revision || state.checkpoints[0].sha;
        await chooseBaseline(sha, { compute: false });
      }
    } catch (error) {
      if (current(generation, "load")) {
        // A failed refresh is not evidence that the previous save state still
        // applies; keep the old rows for navigation but make status unknown.
        state.durability = null;
        state.problem = error.message || "the history could not be read";
      }
    }
  }

  // The shared body of `chooseBaseline`/`chooseTarget`: look up the listed
  // checkpoint, fetch its full text when the manifest carries only a summary,
  // revalidate against the generation counters the caller captured before the
  // fetch, and hand the point to `use`.
  //
  // `use` runs inside the `try`, which is where each caller's work sat before
  // this was extracted, and that placement is load-bearing twice over: a
  // failure in `computeChanges` is still reported as `state.problem` rather
  // than escaping as an unhandled rejection, and the caller's first statement
  // still runs in the same tick as the staleness check above it, so nothing
  // can invalidate the point in between.
  async function withComparisonPoint(sha, generation, manifestGeneration, use) {
    const listed = listedPoint(sha);
    if (!listed) return;
    try {
      const point = listed.texts
        ? listed
        : {
          ...(await history.checkpoint(slug, sha, headers())),
          // The endpoint contains source files; the manifest carries the
          // retention/provenance edge metadata. Keep both on one comparison
          // point without letting an endpoint actor fill missing evidence.
          original_parent: listed.original_parent,
          ancestry_gap: listed.ancestry_gap,
          authorship: listed.authorship || { kind: "unknown" },
        };
      // An identical background refresh does not cancel a user selection.
      // Removal or any changed point metadata still invalidates the fetched
      // endpoint, including changed retention/provenance boundaries.
      if (!current(generation, "baseline") || (manifestGeneration !== loadGeneration
        && JSON.stringify(listedPoint(sha)) !== JSON.stringify(listed))) return;
      await use(point);
    } catch (error) {
      if (current(generation, "baseline") && manifestGeneration === loadGeneration) state.problem = error.message || "that checkpoint could not be read";
    }
  }

  async function chooseBaseline(sha, { compute = true } = {}) {
    if (disposed) return;
    const generation = ++baselineGeneration;
    const manifestGeneration = loadGeneration;
    pendingBaseline = generation;
    try {
      await withComparisonPoint(sha, generation, manifestGeneration, async (point) => {
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
        if (compute) await computeChanges(point);
      });
    } finally {
      if (pendingBaseline === generation) pendingBaseline = 0;
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
    await withComparisonPoint(sha, generation, manifestGeneration, async (point) => {
      state.target = point;
      clearComparison();
      state.problem = "";
      await computeChanges();
    });
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
    const pending = chooseBaseline(before.sha, { compute: false });
    const generation = baselineGeneration;
    await pending;
    if (!current(generation, "baseline") || state.baseline?.sha !== before.sha) return;
    await chooseTarget(sha);
  }

  // Shared by `compareWithCurrent`/`refreshCurrent`: capture a live snapshot
  // as the new comparison target, revalidating against `baselineGeneration`
  // both before and after the (async) capture, since edits -- or a fresher
  // call to `chooseBaseline`/`chooseTarget` -- may have landed while it was
  // pending. Returns `false` exactly where the two callers used to bail out
  // early, leaving them nothing to do but return in turn. `enterComparingCurrent`
  // lets `compareWithCurrent` set `state.comparingCurrent = true` at the same
  // point in the sequence -- right after `state.target` is assigned, before
  // `state.newerEdits` is cleared -- that it did before extraction.
  async function captureLiveTarget({ enterComparingCurrent = false } = {}) {
    const captureGeneration = baselineGeneration;
    let snapshot;
    try { snapshot = await liveSnapshot(); }
    catch (error) {
      if (alive() && captureGeneration === baselineGeneration) state.problem = error.message || "Could not capture the current document";
      return false;
    }
    if (!alive() || captureGeneration !== baselineGeneration) return false;
    if (!snapshot) return false;
    ++baselineGeneration;
    state.target = snapshot;
    if (enterComparingCurrent) state.comparingCurrent = true;
    state.newerEdits = false;
    // Edits may have landed while the digest/render capture was pending.
    noteLiveChange();
    clearComparison();
    return true;
  }

  // Enter a stable comparison against the current document.  The target is
  // copied now; later edits are reported, but never folded into the diff.
  async function compareWithCurrent(sha) {
    if (disposed || !sha || indexOf(sha) < 0) return;
    const pending = chooseBaseline(sha, { compute: false });
    const generation = baselineGeneration;
    await pending;
    if (!current(generation, "baseline") || state.baseline?.sha !== sha) return;
    if (!(await captureLiveTarget({ enterComparingCurrent: true }))) return;
    state.problem = "";
    await computeChanges(state.baseline);
  }

  async function refreshCurrent() {
    if (disposed || !state.comparingCurrent || !state.baseline) return;
    if (!(await captureLiveTarget())) return;
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
    if (!point || !session || (!state.target && (viewing() || liveText === null))) return;
    if (!state.target && passages.projectionTree) {
      if (!(await captureLiveTarget({ enterComparingCurrent: true }))) return;
      return computeChanges(point);
    }

    const baselineGenerationAtStart = baselineGeneration;
    const target = state.target;
    const request = ++diffGeneration;
    const stillCurrent = () => current(request, "diff") && baselineGenerationAtStart === baselineGeneration
      && target === state.target && live().session === session && (target || live().text === liveText);
    updateChangedPaths(point, target, session);
    try {
      // Target first: a single-slot LaTeXML worker must not postpone the
      // clean selected preview behind the baseline or intermediate versions.
      let targetProjection;
      if (target?._current && passages.projectionTree) {
        targetProjection = await passages.projectionTree(target, target.label || "Current version", { slug, headers: headers() });
      } else if (target && passages.projectionAt) {
        targetProjection = await passages.projectionAt(slug, target.sha, headers());
      } else if (!target && passages.projectionTree && liveState.tree) {
        // The live endpoint must use the same semantic projection as the
        // baseline. Clone and capture its tree first so an asset replacement
        // during rendering cannot mix with the source snapshot.
        const liveTree = {
          ...liveState.tree,
          texts: { ...(liveState.tree.texts || {}) },
          files: Object.fromEntries(Object.entries(liveState.tree.files || {}).map(([path, file]) => [path, { ...file }])),
          digests: { ...(liveState.tree.digests || {}) },
        };
        const captured = passages.captureTree
          ? await passages.captureTree(slug, liveTree, headers())
          : liveTree;
        targetProjection = await passages.projectionTree(captured, "Current version", { slug, headers: headers() });
      } else {
        // Compatibility fallback for the source-only/unit-test seam. The
        // production passages API has projectionTree and never reaches this
        // raw-source comparison path.
        targetProjection = projectText(target
          ? await passages.textAt(slug, target.sha, headers(), target._current ? { history: { checkpoint: async () => target } } : {})
          : liveText);
      }
      if (!stillCurrent()) return;
      if (target?._current) {
        try {
          await onTargetReady(target, targetProjection);
        } catch {
          // A presentation observer cannot make a valid comparison fail.
        }
      }
      if (!stillCurrent()) return;
      const baselineProjection = passages.projectionAt
        ? await passages.projectionAt(slug, point.sha, headers())
        : projectText(await passages.textAt(slug, point.sha, headers()));
      if (!stillCurrent()) return;
      if (!baselineProjection?.complete || !targetProjection?.complete) {
        throw new Error("Some rendered content cannot be compared reliably. Use the file-level source comparison.");
      }
      const displayHunks = await compareProjections(baselineProjection, targetProjection);
      if (!stillCurrent()) return;
      const oldVisible = baselineProjection.text;
      const targetVisible = targetProjection.text;
      state.projection = targetProjection;

      const displayContext = history.hunks(
        oldVisible,
        targetVisible,
        displayHunks.map((hunk) => ({ at: hunk.at, delete: hunk.old.length, insert: hunk.insert || "" })),
      );
      state.changes = displayHunks.map((hunk, index) => {
        const newAt = Number(hunk.position) || 0;
        const context = displayContext[index] || {};
        return {
          ...hunk,
          delete: hunk.old.length,
          path: point.main || "document",
          new: hunk.insert || "",
          exact: hunk.insert || "",
          position: newAt,
          prefix: context.currentBefore || context.before || "",
          suffix: context.currentAfter || context.after || "",
          contextBefore: hunk.insert ? (context.currentBefore || context.before || "") : (context.before || ""),
          contextAfter: hunk.insert ? (context.currentAfter || context.after || "") : (context.after || ""),
        };
      });

      state.problem = "";
      notifyRedlines();

      // Only explicit interval evidence can establish range authorship.
      // Never render intermediate versions or use a checkpoint actor to fill
      // a pruned/unknown provenance interval.
      const who = target?._current ? "" : attribution(state.checkpoints, point.sha, target?.sha);
      for (const hunk of state.changes) hunk.who = who;

      // Refine only from exact source diffs. This runs after the endpoint
      // projections are painted and never asks a renderer for an
      // intermediate checkpoint. If the backend has no explicit interval
      // evidence (the normal legacy case), provenIntervals returns null and
      // the initial conservative range attribution remains unchanged.
      if (target && !target._current && passages.sourceProvenance) {
        const baselineAt = indexOf(point.sha);
        const targetAt = indexOf(target.sha);
        const range = baselineAt >= 0 && targetAt > baselineAt
          ? state.checkpoints.slice(baselineAt + 1, targetAt + 1)
          : [];
        if (range.length && range.length <= MAX_PROVENANCE_INTERVALS
          && provenIntervals(state.checkpoints, point.sha, target.sha)) {
          void (async () => {
            try {
              const intervals = await passages.sourceProvenance({
                slug, baseline: point, target, checkpoints: range,
                headers: headers(), changedPaths: state.changedPaths,
                maxIntervals: MAX_PROVENANCE_INTERVALS,
              });
              if (!stillCurrent() || !Array.isArray(intervals)) return;
              const refined = refineSourceAttribution({
                checkpoints: state.checkpoints, baselineSha: point.sha,
                targetSha: target.sha, intervals, hunks: state.changes,
              });
              if (!stillCurrent() || !Array.isArray(state.changes) || !refined.refined) return;
              for (const [index, hunk] of refined.hunks.entries()) {
                if (Object.prototype.hasOwnProperty.call(hunk, "who")) state.changes[index].who = hunk.who;
              }
              notifyRedlines();
            } catch {
              // Source-only refinement is optional detail. Endpoint redlines
              // and their conservative range attribution stay usable.
            }
          })();
        }
      }
    } catch (error) {
      if (current(request, "diff") && baselineGenerationAtStart === baselineGeneration) {
        state.changes = null;
        state.projection = null;
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
    retireComparisons();
    loadGeneration += 1;
    baselineGeneration += 1;
    diffGeneration += 1;
    fileDiffGeneration += 1;
  }

  return {
    get checkpoints() { return state.checkpoints; },
    get durability() { return state.durability; },
    get problem() { return state.problem; },
    get baseline() { return state.baseline; },
    get target() { return state.target; },
    get capturedCurrent() { return state.target?._current ? state.target : null; },
    get comparingCurrent() { return state.comparingCurrent; },
    get newerEdits() {
      return state.newerEdits;
    },
    get changes() { return state.changes; },
    get projection() { return state.projection; },
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
