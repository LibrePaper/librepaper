// Owns the iframe's navigation epoch and the replayable payload delivered to
// the document agent. A payload fetched or compiled before readiness remains
// available for the next navigation, while every transfer hands the frame a
// fresh PDF buffer so replay cannot observe a detached one.

export function createFramePreview({
  slug,
  getDocsOrigin,
  framePath,
  api,
  setSource,
  send,
  onNavigate = () => {},
  onDelivered = () => {},
}) {
  let epoch = 0;
  let readyEpoch = -1;
  let kind = null;
  let served = false;
  let requestId = 0;
  let generation = 0;
  let source = null;
  let framedSource = null;
  let latest = null;
  let disposed = false;
  // True while the frame's `src` points at an external page (Quarto's own
  // live preview server) rather than the docs-origin document agent. The
  // next html/pdf publish must force a real navigation back to that agent
  // before anything can be delivered to it by postMessage again.
  let urlMode = false;

  function navigate(force = false) {
    const docsOrigin = getDocsOrigin();
    if (disposed || !docsOrigin) return false;
    const nextKind = framePath();
    const serves = nextKind === "raw";
    if (!nextKind || (!force && source && kind === nextKind && (served || !serves))) return false;

    kind = nextKind;
    epoch += 1;
    readyEpoch = -1;
    served = false;
    onNavigate({ epoch, kind });
    const base = `${docsOrigin}/${kind}/${slug}/?v=${++generation}`;
    const mine = ++requestId;
    if (!serves) {
      source = base;
      setSource(source);
      return true;
    }
    source = base;
    Promise.resolve(api.frame())
      .then((response) => (response.ok ? response.json() : null))
      .catch(() => null)
      .then((pass) => {
        if (disposed || mine !== requestId) return;
        served = Boolean(pass?.token);
        source = served ? `${base}&until=${pass.until}&token=${pass.token}` : base;
        setSource(source);
      });
    return true;
  }

  function markReady() {
    if (disposed) return false;
    const first = readyEpoch !== epoch;
    readyEpoch = epoch;
    return first;
  }

  function normalize(payload) {
    if (!payload || (payload.kind !== "pdf" && payload.kind !== "html")) return null;
    if (payload.kind === "html") return { kind: "html", html: payload.html };
    const bytes = payload.bytes instanceof Uint8Array ? payload.bytes : new Uint8Array(payload.bytes);
    return { kind: "pdf", sha: payload.sha || null, bytes: bytes.slice() };
  }

  function deliver(payload = latest) {
    const payloadKind = payload?.kind === "html" ? "raw" : payload?.kind;
    if (disposed || !payload || readyEpoch !== epoch || payloadKind !== kind) return false;
    if (payload.kind === "pdf") {
      const buffer = payload.bytes.slice().buffer;
      send({ type: "preview", pdf: buffer }, [buffer]);
    } else {
      send({ type: "preview", html: payload.html });
    }
    onDelivered(payload);
    return true;
  }

  function publish(payload) {
    if (disposed) return false;
    if (payload?.kind === "url") {
      if (!payload.url) return false;
      urlMode = true;
      latest = null;
      source = payload.url;
      setSource(source);
      return true;
    }
    const normalized = normalize(payload);
    if (!normalized) return false;
    latest = normalized;
    if (urlMode) {
      // The frame's `src` is an external page; postMessage cannot reach the
      // docs-origin agent that is no longer there. Navigate for real -- the
      // usual "ready" handshake redelivers `latest` once the agent is back.
      urlMode = false;
      navigate(true);
      return false;
    }
    return deliver(normalized);
  }

  function clear() {
    latest = null;
    urlMode = false;
  }

  function refresh(sourceText) {
    if (disposed || framePath() !== "raw") return false;
    if (framedSource === null) {
      framedSource = sourceText;
      return false;
    }
    if (sourceText === framedSource) return false;
    framedSource = sourceText;
    navigate(true);
    return true;
  }

  function invalidate() {
    requestId += 1;
    readyEpoch = -1;
  }

  function dispose() {
    disposed = true;
    requestId += 1;
    latest = null;
    urlMode = false;
    readyEpoch = -1;
  }

  return {
    navigate,
    markReady,
    publish,
    deliver,
    replay: () => deliver(),
    clear,
    refresh,
    invalidate,
    dispose,
    preview: () => latest,
    get epoch() { return epoch; },
    get kind() { return kind; },
    get source() { return source; },
    get ready() { return readyEpoch === epoch; },
  };
}
