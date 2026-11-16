// Owns the iframe's navigation epoch and the replayable payload delivered to
// the document agent. A payload fetched or compiled before readiness remains
// available for the next navigation, while every transfer hands the frame a
// fresh PDF buffer so replay cannot observe a detached one.

/** `sources` maps each `data-sourcepos` source tag to a project-relative path, or null. It is optional and absent when the engine did not return it. */
/** `figures` are the raster images and PDFs the HTML names by reference. The frame gets their bytes once per frame, ahead of the first page that needs them. */
/** @typedef {{ sha: string, type: string, bytes: Uint8Array }} FramePreviewFigure */
/** @typedef {{ kind: "html", html: string, sha?: string, presentation?: "document", sources?: Array<string | null>, figures?: FramePreviewFigure[] } | { kind: "pdf", sha: string | null, bytes: Uint8Array }} FramePreviewContent */
/** @typedef {FramePreviewContent & { generation: number }} FramePreviewPayload */
/** @typedef {{ kind: "html", html: string, sha?: string, presentation?: "document", sources?: Array<string | null>, figures?: FramePreviewFigure[] } | { kind: "pdf", sha?: string | null, bytes: Uint8Array | ArrayBuffer }} FramePreviewInput */
/** @typedef {{ type: "preview", html?: string, pdf?: ArrayBuffer, frameGeneration: number, presentation?: "document", sources?: Array<string | null> } | { type: "figures", items: FramePreviewFigure[] }} FrameMessage */

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
  // The figures this frame already holds, by digest. A frame that loads again
  // has lost them, so the set is emptied wherever the frame is replaced or
  // reloaded: the same places readiness is.
  const sentFigures = new Set();

  function navigate(force = false) {
    const docsOrigin = getDocsOrigin();
    if (disposed || !docsOrigin) return false;
    const nextKind = framePath();
    if (!nextKind || (!force && source && kind === nextKind && true)) return false;

    kind = nextKind;
    epoch += 1;
    readyEpoch = -1;
    deliveredGeneration = 0;
    sentFigures.clear();
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
    if (first) sentFigures.clear();
    return first;
  }

  /** @param {FramePreviewInput | null | undefined} payload @returns {FramePreviewContent | null} */
  function normalize(payload) {
    if (!payload || (payload.kind !== "pdf" && payload.kind !== "html")) return null;
    if (payload.kind === "html") return {
      kind: "html", html: payload.html,
      ...(payload.sha ? { sha: payload.sha } : {}),
      ...(payload.presentation === "document" ? { presentation: "document" } : {}),
      ...(Array.isArray(payload.sources) ? { sources: payload.sources.slice() } : {}),
      ...(Array.isArray(payload.figures) ? { figures: payload.figures.slice() } : {}),
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
      // The figures go ahead of the page that refers to them, and only once:
      // the frame keeps the bytes it was given, so a keystroke sends the HTML
      // alone. The bytes are not transferred, because the reader keeps them
      // for the next frame.
      const unsent = (payload.figures || []).filter((figure) => !sentFigures.has(figure.sha));
      if (unsent.length) {
        send({ type: "figures", items: unsent.map(({ sha, type, bytes }) => ({ sha, type, bytes })) });
        for (const figure of unsent) sentFigures.add(figure.sha);
      }
      send({ type: "preview", html: payload.html,
        frameGeneration: payload.generation,
        ...(payload.presentation === "document" ? { presentation: "document" } : {}),
        ...(payload.sources ? { sources: payload.sources } : {}),
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
    sentFigures.clear();
  }

  function dispose() {
    disposed = true;
    latest = null;
    readyEpoch = -1;
    sentFigures.clear();
  }

  // The frame's document loaded again. A reload can keep the same address and
  // epoch, as the browser's own "Reload frame" does, and the new document has
  // none of the figures the old one was sent.
  function forgetFigures() {
    sentFigures.clear();
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
    forgetFigures,
    preview: () => latest,
    get epoch() { return epoch; },
    get contentGeneration() { return contentGeneration; },
    get deliveredGeneration() { return deliveredGeneration; },
    get kind() { return kind; },
    get source() { return source; },
    get ready() { return readyEpoch === epoch; },
  };
}
