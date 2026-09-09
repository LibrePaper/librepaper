// Fetches server renderings and owns the delayed publication of locally
// compiled bytes. Generations discard obsolete responses and disposal clears
// timers; a late response cannot replace a newer checkpoint or source.

export function createRenderingStore({
  api,
  getViewing,
  getSourceGeneration,
  getNavigationGeneration,
  getRenderedSha = () => null,
  getPreview = () => null,
  deliver,
  onRendering,
  onMissing,
  schedulePreview,
  pollDelay = 30_000,
  quietDelay = 60_000,
  now = () => new Date().toISOString(),
  setTimer = globalThis.setTimeout,
  clearTimer = globalThis.clearTimeout,
}) {
  let requestGeneration = 0;
  let cacheGeneration = 0;
  let checked = false;
  let hasRendering = false;
  let held = null;
  let quietTimer = null;
  let pollTimer = null;
  let disposed = false;

  const clearPoll = () => {
    clearTimer(pollTimer);
    pollTimer = null;
  };

  async function latest() {
    const response = await api.latest().catch(() => null);
    return response?.ok ? response.json().catch(() => null) : null;
  }

  async function paint() {
    if (disposed) return;
    const mine = ++requestGeneration;
    const viewing = getViewing();
    const found = viewing
      ? { sha: viewing.sha, at: viewing.at, current: true }
      : await latest();
    if (disposed || mine !== requestGeneration || (!viewing && !found)) return;
    const requestedSha = found?.sha || null;
    hasRendering = Boolean(requestedSha);
    onRendering(requestedSha ? found : null);
    checked = true;
    clearPoll();
    if (!viewing && (!found || !found.current)) schedulePoll();
    if (!requestedSha || requestedSha === getRenderedSha()) {
      return;
    }
    const preview = getPreview();
    if (preview?.kind === "pdf" && preview.sha === requestedSha) {
      deliver(preview);
      return;
    }
    const response = await api.rendering(requestedSha).catch(() => null);
    const bytes = response?.ok ? await response.arrayBuffer().catch(() => null) : null;
    if (disposed || mine !== requestGeneration) return;
    if (!bytes) {
      if (viewing) onMissing({ ...found, current: false, missing: true });
      return;
    }
    deliver({ kind: "pdf", sha: requestedSha, bytes: new Uint8Array(bytes) });
  }

  function schedulePoll() {
    if (disposed) return;
    clearPoll();
    pollTimer = setTimer(() => {
      pollTimer = null;
      if (!disposed) schedulePreview();
    }, pollDelay);
  }

  async function noRenderingYet() {
    if (disposed) return false;
    if (hasRendering) return false;
    if (checked) return true;
    const generation = cacheGeneration;
    const found = await latest();
    if (disposed || generation !== cacheGeneration) return false;
    if (!found) return false;
    checked = true;
    hasRendering = Boolean(found.sha);
    return !found.sha;
  }

  async function hold(name, bytes, synctex, current = true, provenance = null) {
    if (disposed) return;
    clearTimer(quietTimer);
    const source = getSourceGeneration();
    const navigation = getNavigationGeneration();
    const value = { name, bytes, synctex, current, provenance, source, navigation };
    held = value;
    if (current && await noRenderingYet()) {
      if (held === value) storeHeld();
      return;
    }
    if (held === value) quietTimer = setTimer(storeHeld, quietDelay);
  }

  function dropHeld() {
    clearTimer(quietTimer);
    quietTimer = null;
    held = null;
  }

  async function store(name, bytes, synctex, current = true, provenance = null) {
    if (disposed) return;
    const source = getSourceGeneration();
    const navigation = getNavigationGeneration();
    const put = (suffix, body) => api.putRendering(name, suffix, body, provenance)
      .then((response) => response.ok)
      .catch(() => false);
    if (!(await put("", bytes)) || disposed) return;
    if (source === getSourceGeneration() && navigation === getNavigationGeneration()) {
      checked = true;
      hasRendering = true;
      onRendering({ sha: name, at: now(), current, provenance });
    }
    if (synctex && !disposed) await put(".synctex", synctex);
  }

  function storeHeld() {
    clearTimer(quietTimer);
    quietTimer = null;
    const value = held;
    held = null;
    if (!value || disposed) return;
    if (value.source !== getSourceGeneration() || value.navigation !== getNavigationGeneration()) return;
    void store(value.name, value.bytes, value.synctex, value.current, value.provenance);
  }

  function invalidate() {
    requestGeneration += 1;
  }

  function reset() {
    if (disposed) return;
    invalidate();
    cacheGeneration += 1;
    clearPoll();
    dropHeld();
    checked = false;
    hasRendering = false;
    onRendering(null);
  }

  function dispose() {
    disposed = true;
    invalidate();
    clearPoll();
    dropHeld();
  }

  return {
    paint,
    hold,
    dropHeld,
    store,
    invalidate,
    reset,
    schedulePoll,
    cancelPoll: clearPoll,
    flushHeld: storeHeld,
    dispose,
    get checked() { return checked; },
  };
}
