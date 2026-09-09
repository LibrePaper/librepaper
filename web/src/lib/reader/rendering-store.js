// Fetches server renderings and owns the delayed publication of locally
// compiled bytes. Requests and timers are cancelled by generation/disposal;
// a late response cannot replace a newer checkpoint or source.

export function createRenderingStore({
  api,
  getViewing,
  getSourceGeneration,
  getNavigationGeneration,
  getRenderedSha = () => null,
  deliver,
  onRendering,
  onMissing,
  schedulePreview,
  pollDelay = 30_000,
  quietDelay = 60_000,
  now = () => new Date().toISOString(),
}) {
  let requestGeneration = 0;
  let checked = false;
  let hasRendering = false;
  let held = null;
  let quietTimer = null;
  let pollTimer = null;
  let disposed = false;

  const clearPoll = () => {
    clearTimeout(pollTimer);
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
    pollTimer = setTimeout(() => {
      pollTimer = null;
      schedulePreview();
    }, pollDelay);
  }

  async function noRenderingYet() {
    if (disposed) return false;
    if (hasRendering) return false;
    if (checked) return true;
    const found = await latest();
    if (disposed) return false;
    if (!found) return false;
    checked = true;
    hasRendering = Boolean(found.sha);
    return !found.sha;
  }

  async function hold(name, bytes, synctex, current = true, provenance = null) {
    if (disposed) return;
    clearTimeout(quietTimer);
    const source = getSourceGeneration();
    const navigation = getNavigationGeneration();
    const value = { name, bytes, synctex, current, provenance, source, navigation };
    held = value;
    if (current && await noRenderingYet()) {
      if (held === value) storeHeld();
      return;
    }
    if (held === value) quietTimer = setTimeout(storeHeld, quietDelay);
  }

  function dropHeld() {
    clearTimeout(quietTimer);
    quietTimer = null;
    held = null;
  }

  async function store(name, bytes, synctex, current = true, provenance = null) {
    const source = getSourceGeneration();
    const navigation = getNavigationGeneration();
    const put = (suffix, body) => api.putRendering(name, suffix, body, provenance)
      .then((response) => response.ok)
      .catch(() => false);
    if (!(await put("", bytes)) || disposed) return;
    if (source === getSourceGeneration() && navigation === getNavigationGeneration()) {
      onRendering({ sha: name, at: now(), current, provenance });
    }
    if (synctex) await put(".synctex", synctex);
  }

  function storeHeld() {
    clearTimeout(quietTimer);
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
    invalidate();
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
