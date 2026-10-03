// Owns the iframe's navigation epoch and the replayable payload delivered to
// the document agent. A payload fetched or compiled before readiness remains
// available for the next navigation, while every transfer hands the frame a
// fresh PDF buffer so replay cannot observe a detached one.

/** @typedef {{ kind: "html", html: string, sha?: string, presentation?: "document" } | { kind: "pdf", sha: string | null, bytes: Uint8Array }} FramePreviewContent */
/** @typedef {FramePreviewContent & { generation: number }} FramePreviewPayload */
/** @typedef {{ kind: "html", html: string, sha?: string, presentation?: "document" } | { kind: "pdf", sha?: string | null, bytes: Uint8Array | ArrayBuffer }} FramePreviewInput */
/** @typedef {{ type: "preview", html?: string, pdf?: ArrayBuffer, frameGeneration: number, presentation?: "document" }} FrameMessage */

/** @param {{
 * slug: string, getDocsOrigin: () => string | null, framePath: () => string,
 * setSource: (source: string) => void,
 * send: (message: FrameMessage, transfer?: Transferable[]) => void,
 * onNavigate?: (navigation: { epoch: number, kind: string }) => void,
 * onDelivered?: (payload: FramePreviewPayload) => void,
 * }} options
 */
export function createFramePreview({
  slug,
  getDocsOrigin,
  framePath,
  setSource,
  send,
  onNavigate = () => {},
  onDelivered = () => {},
}) {
  let epoch = 0;
  let readyEpoch = -1;
  let kind = null;
  let generation = 0;
  let source = null;
  let framedSource = null;
  /** @type {FramePreviewPayload | null} */
  let latest = null;
  let contentGeneration = 0;
  let deliveredGeneration = 0;
  let disposed = false;

  function navigate(force = false) {
    const docsOrigin = getDocsOrigin();
    if (disposed || !docsOrigin) return false;
    const nextKind = framePath();
    if (!nextKind || (!force && source && kind === nextKind && true)) return false;

    kind = nextKind;
    epoch += 1;
    readyEpoch = -1;
    deliveredGeneration = 0;
    onNavigate({ epoch, kind });
    const base = `${docsOrigin}/${kind}/${slug}/?v=${++generation}`;
    source = base;
    setSource(source);
    return true;
  }

  function markReady() {
    if (disposed) return false;
    const first = readyEpoch !== epoch;
    readyEpoch = epoch;
    return first;
  }

  /** @param {FramePreviewInput | null | undefined} payload @returns {FramePreviewContent | null} */
  function normalize(payload) {
    if (!payload || (payload.kind !== "pdf" && payload.kind !== "html")) return null;
    if (payload.kind === "html") return {
      kind: "html", html: payload.html,
      ...(payload.sha ? { sha: payload.sha } : {}),
      ...(payload.presentation === "document" ? { presentation: "document" } : {}),
    };
    const bytes = payload.bytes instanceof Uint8Array ? payload.bytes : new Uint8Array(payload.bytes);
    return { kind: "pdf", sha: payload.sha || null, bytes: bytes.slice() };
  }

  /** @param {FramePreviewPayload | null} [payload] */
  function deliver(payload = latest) {
    const payloadKind = payload?.kind === "html" ? "raw" : payload?.kind;
    try {
      if (localStorage.getItem("librepaper-latex-debug")) console.debug("preview: deliver", payloadKind, kind, readyEpoch, epoch, Boolean(payload));
    } catch { /* diagnostics are optional */ }
    if (disposed || !payload || readyEpoch !== epoch || payloadKind !== kind) return false;
    if (payload.kind === "pdf") {
      const buffer = payload.bytes.slice().buffer;
      send({ type: "preview", pdf: buffer, frameGeneration: payload.generation }, [buffer]);
    } else {
      send({ type: "preview", html: payload.html,
        frameGeneration: payload.generation,
        ...(payload.presentation === "document" ? { presentation: "document" } : {}),
      });
    }
    deliveredGeneration = payload.generation;
    onDelivered(payload);
    return true;
  }

  /** @param {FramePreviewInput} payload */
  function publish(payload) {
    if (disposed) return false;
    const normalized = normalize(payload);
    if (!normalized) return false;
    const published = { ...normalized, generation: ++contentGeneration };
    latest = published;
    return deliver(published);
  }

  function clear() {
    latest = null;
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
    readyEpoch = -1;
  }

  function dispose() {
    disposed = true;
    latest = null;
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
    get contentGeneration() { return contentGeneration; },
    get deliveredGeneration() { return deliveredGeneration; },
    get kind() { return kind; },
    get source() { return source; },
    get ready() { return readyEpoch === epoch; },
  };
}
