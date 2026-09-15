// The lifecycle of one engine's local live preview: start it against the
// local app, keep its workspace in sync as the document changes, poll its
// rendered page and its own status, and tear it down. Quarto's own preview
// and a Typst document previewed through Calepin are the same shape of job
// to `web/src/lib/companion/client.js` (`startLocalPreview`, `stopLocalPreview`,
// `localPreviewStatus`, `localPreviewPage`, `syncWorkspace`) -- this module is
// the one lifecycle both engines run through, parameterized by `engine`.
//
// Every dependency reaching outside this module -- the bridge client, the
// timer, disposal -- arrives through the constructor so
// `web/tests/integration/local-preview.mjs` can drive the whole thing under Node with no
// browser and no real clock.

import { createGeneration } from "./generation.js";

const POLL_FAST_MS = 500;
const POLL_SLOW_MS = 1000;
const STATUS_POLL_MS = 5000;

// The live page's own bytes are named, for the PDF case, by the etag the
// bridge served them under -- quoted, and possibly weak (`W/"..."`) -- while
// `framePreview.publish` wants a bare digest, the same shape a compiled PDF's
// own `sha` already is.
function etagToSha(etag) {
  return String(etag || "").replace(/^W\//, "").replace(/^"/, "").replace(/"$/, "");
}

export function createLocalPreview({
  local,
  publish,
  say,
  treeNow,
  entrypointOf,
  optionsOf,
  jobOf = () => ({}),
  engine,
  label = engine,
  isDisposed = () => false,
  // The preview starting is the one change with a consequence beyond
  // drawing it: a managed preview taking the pane invalidates whatever this
  // browser was rendering into it.
  onStartingChange,
  onEnded,
  now = () => Date.now(),
  setTimer = (fn, ms) => setTimeout(fn, ms),
  clearTimer = (timer) => clearTimeout(timer),
} = {}) {
  void now; // reserved for a future backoff; every cadence today is fixed.

  // What the page draws. Owned here rather than mirrored into the page
  // through callbacks: these are facts about a preview, the preview is this
  // module, and a mirror is one more thing that can disagree.
  const state = $state({
    session: null, // the bridge's { id, url, ... }, or null while idle
    starting: false,
    rendering: false,
    // The last thing that went wrong, or "" once a new attempt clears it.
    error: "",
  });
  let pageTimer = null;
  let pageEtag = null;
  let statusTimer = null;
  let syncBusy = false;
  let syncQueued = false;
  let errorShown = false;
  const lifecycle = createGeneration();
  let pendingStart = null;
  let pendingStop = null;
  let target = null;
  const reconciliations = createGeneration();

  function setRendering(value) {
    state.rendering = !!value;
  }

  function report(message, isProblem) {
    if (message) say?.(message, isProblem);
  }

  function stopPagePoll() {
    clearTimer(pageTimer);
    pageTimer = null;
    pageEtag = null;
    setRendering(false);
  }

  function stopStatusPoll() {
    clearTimer(statusTimer);
    statusTimer = null;
  }

  // Polls the live page's own rendered bytes while a preview runs, and
  // publishes each new one the moment it arrives. A 304 (nothing new) or a
  // 404 (`NotRendered`, nothing rendered yet) costs one small request and
  // changes nothing on screen but the render-state flag. Re-arms itself as a
  // timeout so it can poll faster (500ms) while the engine is known to be
  // re-rendering, and back off to 1000ms once it settles.
  function startPagePoll(id) {
    stopPagePoll();
    const tick = () => {
      Promise.resolve(local.localPreviewPage(id, { etag: pageEtag }))
        .then((page) => {
          if (isDisposed() || state.session?.id !== id) return;
          setRendering(page?.rendering);
          if (page?.kind === "html") {
            pageEtag = page.etag;
            publish({ kind: "html", html: page.html, presentation: "document" });
          } else if (page?.kind === "pdf") {
            pageEtag = page.etag;
            publish({ kind: "pdf", sha: etagToSha(page.etag), bytes: page.bytes });
          }
        })
        .catch((error) => {
          // A network hiccup is not a reason to stop polling; the status
          // poll is what decides the preview is dead. "Not rendered yet"
          // still carries a render-state flag worth reflecting.
          if (isDisposed() || state.session?.id !== id) return;
          if (error?.name === "NotRendered") setRendering(error.rendering);
        })
        .finally(() => {
          if (isDisposed() || state.session?.id !== id || pageTimer === null) return;
          pageTimer = setTimer(tick, state.rendering ? POLL_FAST_MS : POLL_SLOW_MS);
        });
    };
    pageTimer = setTimer(tick, 0);
  }

  // Any `state` other than "running" -- the process exited, whether it
  // finished cleanly or failed -- ends the session: the poller stops, the
  // bridge is told to drop it, the last log line is reported, and the
  // caller's `onEnded` gets a chance to repaint its own fallback preview.
  function endSession(message, isProblem) {
    const active = state.session;
    if (!active) return;
    state.session = null;
    stopPagePoll();
    stopStatusPoll();
    void local.stopLocalPreview(active.id).catch(() => {});
    report(message, isProblem);
    state.error = message || "";
    onEnded?.();
  }

  function startStatusPoll(id) {
    stopStatusPoll();
    const tick = () => {
      Promise.resolve(local.localPreviewStatus(id))
        .then((status) => {
          if (isDisposed() || state.session?.id !== id) return;
          if (status?.state && status.state !== "running") {
            const line = String(status.log_tail || "").trim().split("\n").filter(Boolean).pop();
            endSession(line || `${label} preview ended`, true);
            return;
          }
          if (state.session?.id === id) statusTimer = setTimer(tick, STATUS_POLL_MS);
        })
        .catch((error) => {
          if (isDisposed() || state.session?.id !== id) return;
          endSession(`${label} preview ended: ${error.message}`, true);
        });
    };
    statusTimer = setTimer(tick, STATUS_POLL_MS);
  }

  function setStarting(value) {
    value = !!value;
    if (state.starting === value) return;
    state.starting = value;
    onStartingChange?.(state.starting);
  }

  async function start() {
    const stopped = lifecycle.mark();
    if (pendingStop) await pendingStop;
    if (pendingStart) await pendingStart;
    if (stopped() || state.session || isDisposed()) return;
    pendingStart = runStart();
    try { await pendingStart; }
    finally { pendingStart = null; }
  }

  async function runStart() {
    if (state.session || state.starting || isDisposed()) return;
    const stopped = lifecycle.mark();
    const cancelled = () => isDisposed() || stopped();
    state.error = "";
    setStarting(true);
    try {
      const tree = await Promise.resolve(treeNow());
      await local.syncWorkspace({ tree });
      if (cancelled()) return;
      const options = { entrypoint: entrypointOf(tree), ...optionsOf(tree) };
      const started = await local.startLocalPreview({ engine, job: jobOf(tree), tree, options });
      if (cancelled()) {
        await local.stopLocalPreview(started.id).catch(() => {});
        return;
      }
      state.session = started;
      errorShown = false;
      startPagePoll(started.id);
      startStatusPoll(started.id);
      if (syncQueued) {
        syncQueued = false;
        void sync();
      }
    } catch (error) {
      if (!cancelled()) {
        state.error = error.message || `${label} preview unavailable`;
        report(state.error, true);
        errorShown = true;
      }
    } finally {
      setStarting(false);
    }
  }

  async function stop() {
    lifecycle.cancel();
    syncQueued = false;
    const active = state.session;
    if (!active) { await Promise.all([pendingStart, pendingStop]); return; }
    state.session = null;
    stopPagePoll();
    stopStatusPoll();
    const stopped = local.stopLocalPreview(active.id).catch(() => {});
    pendingStop = stopped;
    await stopped;
    if (pendingStop === stopped) pendingStop = null;
    await pendingStart;
  }

  // Serialized: at most one sync in flight, and a source change arriving
  // mid-sync is coalesced into a single trailing retry rather than queued
  // one-for-one. The engine's own file watcher does the rest once the
  // workspace has the new bytes.
  async function sync() {
    if (state.starting && !state.session) { syncQueued = true; return; }
    if (!state.session || isDisposed()) return;
    if (syncBusy) {
      syncQueued = true;
      return;
    }
    syncBusy = true;
    try {
      await local.syncWorkspace({ tree: await Promise.resolve(treeNow()) });
    } catch (error) {
      if (!errorShown) {
        state.error = error.message || `${label} preview could not sync`;
        report(state.error, true);
        errorShown = true;
      }
      await stop();
      onEnded?.();
    } finally {
      syncBusy = false;
      if (syncQueued) {
        syncQueued = false;
        void sync();
      }
    }
  }

  // Starts the moment `active` is true and nothing is running or starting
  // yet, and tears down -- reporting the fallback through `onEnded` -- the
  // moment it stops holding. The whole of what a caller's own `$effect` needs
  // to drive automatic start/stop.
  async function reconcile(active) {
    const stale = reconciliations.begin();
    const next = active || null;
    if (target !== next) {
      target = next;
      const wasActive = state.session || state.starting;
      if (wasActive || pendingStop) await stop();
      if (stale()) return;
      if (!active && wasActive) { onEnded?.(); return; }
    }
    if (active) {
      await start();
      return;
    }
    if (state.session || state.starting || pendingStop) {
      await stop();
      onEnded?.();
    }
  }

  return {
    start,
    stop,
    sync,
    reconcile,
    state,
    get rendering() {
      return state.rendering;
    },
    get running() {
      return !!state.session;
    },
    get starting() {
      return state.starting;
    },
    get id() {
      return state.session?.id ?? null;
    },
  };
}
