// The lifecycle of one engine's local live preview: start it against the
// local app, keep its workspace in sync as the document changes, poll its
// rendered page and its own status, and tear it down. Quarto's own preview
// and a Typst document previewed through Calepin are the same shape of job
// to `web/src/lib/latex/local.js` (`startLocalPreview`, `stopLocalPreview`,
// `localPreviewStatus`, `localPreviewPage`, `syncWorkspace`) -- this module is
// the one lifecycle both engines run through, parameterized by `engine`.
//
// Every dependency reaching outside this module -- the bridge client, the
// timer, disposal -- arrives through the constructor so
// `web/checks/local-preview.mjs` can drive the whole thing under Node with no
// browser and no real clock.

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
  onRunningChange,
  onRenderingChange,
  onStartingChange,
  onEnded,
  now = () => Date.now(),
  setTimer = (fn, ms) => setTimeout(fn, ms),
  clearTimer = (timer) => clearTimeout(timer),
} = {}) {
  void now; // reserved for a future backoff; every cadence today is fixed.

  let session = null; // the bridge's { id, url, ... }, or null while idle
  let starting = false;
  let rendering = false;
  let pageTimer = null;
  let pageEtag = null;
  let statusTimer = null;
  let syncBusy = false;
  let syncQueued = false;
  let errorShown = false;

  function setRendering(value) {
    value = !!value;
    if (rendering === value) return;
    rendering = value;
    onRenderingChange?.(rendering);
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
          if (isDisposed() || session?.id !== id) return;
          setRendering(page?.rendering);
          if (page?.kind === "html") {
            pageEtag = page.etag;
            publish({ kind: "html", html: page.html });
          } else if (page?.kind === "pdf") {
            pageEtag = page.etag;
            publish({ kind: "pdf", sha: etagToSha(page.etag), bytes: page.bytes });
          }
        })
        .catch((error) => {
          // A network hiccup is not a reason to stop polling; the status
          // poll is what decides the preview is dead. "Not rendered yet"
          // still carries a render-state flag worth reflecting.
          if (isDisposed() || session?.id !== id) return;
          if (error?.name === "NotRendered") setRendering(error.rendering);
        })
        .finally(() => {
          if (isDisposed() || session?.id !== id || pageTimer === null) return;
          pageTimer = setTimer(tick, rendering ? POLL_FAST_MS : POLL_SLOW_MS);
        });
    };
    pageTimer = setTimer(tick, 0);
  }

  // Any `state` other than "running" -- the process exited, whether it
  // finished cleanly or failed -- ends the session: the poller stops, the
  // bridge is told to drop it, the last log line is reported, and the
  // caller's `onEnded` gets a chance to repaint its own fallback preview.
  function endSession(message, isProblem) {
    const active = session;
    if (!active) return;
    session = null;
    stopPagePoll();
    stopStatusPoll();
    onRunningChange?.(null);
    void local.stopLocalPreview(active.id).catch(() => {});
    report(message, isProblem);
    onEnded?.();
  }

  function startStatusPoll(id) {
    stopStatusPoll();
    const tick = () => {
      Promise.resolve(local.localPreviewStatus(id))
        .then((status) => {
          if (isDisposed() || session?.id !== id) return;
          if (status?.state && status.state !== "running") {
            const line = String(status.log_tail || "").trim().split("\n").filter(Boolean).pop();
            endSession(line || `${label} preview ended`, true);
            return;
          }
          if (session?.id === id) statusTimer = setTimer(tick, STATUS_POLL_MS);
        })
        .catch((error) => {
          if (isDisposed() || session?.id !== id) return;
          endSession(`${label} preview ended: ${error.message}`, true);
        });
    };
    statusTimer = setTimer(tick, STATUS_POLL_MS);
  }

  function setStarting(value) {
    value = !!value;
    if (starting === value) return;
    starting = value;
    onStartingChange?.(starting);
  }

  async function start() {
    if (session || starting || isDisposed()) return;
    setStarting(true);
    try {
      const tree = treeNow();
      await local.syncWorkspace({ tree });
      if (isDisposed()) return;
      const options = { entrypoint: entrypointOf(tree), ...optionsOf(tree) };
      const started = await local.startLocalPreview({ engine, job: jobOf(tree), tree, options });
      if (isDisposed()) {
        void local.stopLocalPreview(started.id).catch(() => {});
        return;
      }
      session = started;
      errorShown = false;
      onRunningChange?.(session);
      startPagePoll(started.id);
      startStatusPoll(started.id);
    } catch (error) {
      if (!errorShown) {
        report(error.message || `${label} preview unavailable`, true);
        errorShown = true;
      }
    } finally {
      setStarting(false);
    }
  }

  async function stop() {
    const active = session;
    if (!active) return;
    session = null;
    stopPagePoll();
    stopStatusPoll();
    onRunningChange?.(null);
    await local.stopLocalPreview(active.id).catch(() => {});
  }

  // Serialized: at most one sync in flight, and a source change arriving
  // mid-sync is coalesced into a single trailing retry rather than queued
  // one-for-one. The engine's own file watcher does the rest once the
  // workspace has the new bytes.
  async function sync() {
    if (!session || isDisposed()) return;
    if (syncBusy) {
      syncQueued = true;
      return;
    }
    syncBusy = true;
    try {
      await local.syncWorkspace({ tree: treeNow() });
    } catch (error) {
      if (!errorShown) {
        report(error.message || `${label} preview could not sync`, true);
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
    if (active) {
      await start();
      return;
    }
    if (session) {
      await stop();
      onEnded?.();
    }
  }

  return {
    start,
    stop,
    sync,
    reconcile,
    get rendering() {
      return rendering;
    },
    get running() {
      return !!session;
    },
    get starting() {
      return starting;
    },
    get id() {
      return session?.id ?? null;
    },
  };
}
