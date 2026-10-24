// One render of the document, from the source in this tab.
//
// This is the long one, and it stays long on purpose: what it does is a
// single sequence -- gather the figures, take an identity for the tree,
// compile it, and hand whatever came back to the frame -- and every step of
// it has to be able to abandon the whole thing. Cutting it into pieces would
// mean threading the abandon through each of them.
//
// What a render must not do is finish over a newer one. There are three ways
// it can be overtaken: a newer ticket, a navigation (the document being
// looked at changed), and a source change. A render therefore captures all
// three when it starts and compares them again after every await, which is
// what `facts()` is for: it answers what is true *now*, not what was true
// when the render began, and the difference between those two is the whole
// subject of this file.
//
// It was the last thing left inside Reader.svelte that could not be
// imported. Its collaborators used to be free variables in the component,
// which is why it carried `typeof x !== "undefined"` guards: a test could
// only reach it by slicing its text out of the component and running it in a
// context that declared some of those names and not others. They are
// arguments now, and the guards are gone with them.

/** @typedef {ReturnType<ReturnType<typeof import("../project-session.js").createProjectSession>["tree"]> & { main?: string, assets?: Record<string, Uint8Array>, urls?: Record<string, string> }} PreviewTree */
/** @typedef {{ disposed: boolean, navigation: number, source: number, everPainted: boolean, format: string, hasSession: boolean, paintsTheFrame: boolean, latexOutput: string, typstOutput: string, buildPreferences: { selection?: string, backend?: string, tool?: string, output?: string, [key: string]: unknown }, localExecution: boolean, projectDigest?: string, livePreviewOwnsPane: boolean }} PreviewFacts */
/** @typedef {Awaited<ReturnType<typeof import("../renderers.js").render>>} PreviewResult */
/** @typedef {ReturnType<typeof import("./render-status.svelte.js").createRenderStatus>} PreviewStatus */
/** @typedef {ReturnType<typeof import("./render-coordinator.js").createRenderCoordinator>} PreviewCoordinator */
/** @typedef {Pick<ReturnType<typeof import("./frame-preview.js").createFramePreview>, "publish" | "clear">} PreviewFrame */
/** @typedef {typeof import("../renderers.js")} PreviewRendererRegistry */
/** @typedef {ReturnType<typeof import("../diagnostics.js").painter>} PreviewDiagnostics */

/** @param {{
 * slug: string, coordinator: PreviewCoordinator, status: PreviewStatus, framePreview: PreviewFrame,
 * diagnostics: PreviewDiagnostics, renderers: PreviewRendererRegistry,
 * snapshotDigest: (tree: PreviewTree) => Promise<string>,
 * diagnosticContext: (item: NonNullable<PreviewResult["diagnostics"]>[number] | {severity: string, message: string}, tree: PreviewTree, identity: string) => NonNullable<PreviewResult["diagnostics"]>[number],
 * parseSynctex?: (data: Uint8Array | string, files: string[]) => Promise<unknown>,
 * rememberPreview?: (page: { kind: "html", html: string } | { kind: "pdf", sha: string, bytes: Uint8Array }, identity: string) => void,
 * onRendered?: (identity: string, projectDigest: string) => void, onEmpty?: () => void,
 * facts: () => PreviewFacts, tree: () => PreviewTree,
 * gather: (digests: Record<string, string>) => Promise<{ held: { assets: Record<string, Uint8Array>, urls: Record<string, string> }, missing: string[] }>,
 * heading: (tree: PreviewTree) => Promise<string>, takeManual?: () => boolean, manualPending?: () => boolean,
 * onsynctex?: (map: unknown | null) => void, send?: (message: { type: string, html?: string }) => void,
 * clearDebounce?: () => void, refreshFrame?: () => void, debug?: (message: string, ...details: unknown[]) => void,
 * }} options
 */
export function createPreviewRenderer({
  slug,
  coordinator,
  status,
  framePreview,
  diagnostics,
  renderers,
  snapshotDigest,
  diagnosticContext,
  parseSynctex,
  /// Keeps the page that was just drawn, so the next visit to this document
  /// has something to show while its engine loads and its source compiles.
  rememberPreview = (_page, _identity) => {},
  onRendered = (_identity) => {},
  onEmpty = () => {},
  /// Everything about the page that a render has to re-read after an await.
  /// One call rather than one getter per fact, because they are read
  /// together and a render compares a whole moment against another whole
  /// moment.
  facts,
  /// The document as a renderer takes it.
  tree,
  /// The figures the tree names: `{ held, missing }`.
  gather,
  /// The document's title, for a renderer that wants one.
  heading,
  /// Whether this render was asked for by hand, and clearing that if so. Read
  /// at the one call site that reaches the compiler and cleared immediately,
  /// so it cannot linger onto an edit's ordinary debounced compile.
  takeManual = () => false,
  /// Whether a manual compile is waiting, without taking it: a render that is
  /// superseded before it compiles must leave the request for the next one.
  manualPending = () => false,
  /// The parsed SyncTeX map for the PDF just published, or null to drop the
  /// one being held.
  onsynctex = () => {},
  /// A page to put in the frame directly, used only for the failure page.
  send = () => {},
  /// The debounced render that was pending, if one was: this is it.
  clearDebounce = () => {},
  /// The frame is showing something this render does not own.
  refreshFrame = () => {},
  debug = () => {},
}) {
  /// The renders that finished during this visit, by entry path, newest last.
  /// A file whose source has not changed since comes back from here without a
  /// compile. Nothing is persisted: the stand-in page in IndexedDB covers the
  /// next visit, and this covers switching back and forth within one.
  const keep = 8;
  /** @type {Map<string, { key: string, page: any, diagnostics: any[], latex: any, log: string, synctex: unknown }>} */
  const finished = new Map();
  /** @param {string} path @param {{ key: string, page: any, diagnostics: any[], latex: any, log: string, synctex: unknown }} entry */
  const remember = (path, entry) => {
    // Delete first so the map's order is recency, then drop the oldest.
    finished.delete(path);
    finished.set(path, entry);
    while (finished.size > keep) finished.delete(finished.keys().next().value);
  };
  /// A page that can be handed to the frame again. Publishing transfers the
  /// buffer of a PDF, so every delivery gets its own copy.
  const fresh = (page) => (page.kind === "pdf"
    ? { ...page, bytes: new Uint8Array(page.bytes).slice() }
    : page);

  /// Render once. `ticket` is the coordinator's; every guard below is against
  /// it and against the navigation and source captured at the start.
  /** @param {number} ticket */
  async function render(ticket) {
    const start = facts();
    debug("preview: paint requested", start.format, start.hasSession);
    if (start.disposed) return;
    // A render is happening, so a render that was merely scheduled is not.
    clearDebounce();
    // A live preview owns the pane while it is running or starting: the frame
    // shows the local tool's own page, kept current by its poller rather than
    // by anything painted here. Keyed to a preview actually running, not to
    // whether this browser is paired -- one whose preview failed to start
    // falls through to the draft below rather than leaving the pane at
    // whatever it last showed.
    if (start.livePreviewOwnsPane) return;
    // Every preview is compiled from the source in this tab. Results remain
    // in the frame only for the active view or an explicit user export.
    if (!start.paintsTheFrame) {
      refreshFrame();
      return;
    }
    const source = tree();
    const capturedNavigation = start.navigation;
    const capturedSource = start.source;
    const capturedProjectDigest = start.projectDigest || "";
    let identity = "";
    const format = renderers.previewFormatOf(source.main);
    debug("preview: tree", source.main, format, Object.keys(source.texts || {}).length);
    // The collaboration session exists before its first document state arrives.
    // Rendering that placeholder used to call the renderer registry with an
    // empty format and permanently consume the initial paint.
    if (!source.main || !format) return;
    // A known main path distinguishes an intentionally empty document from
    // the pre-hydration placeholder above. Never leave the previous document
    // or its downloadable artifact on screen after select-all/delete.
    if (!String(source.texts?.[source.main] ?? "").trim()) {
      coordinator.commit(ticket);
      status.clearDocx();
      framePreview.clear?.();
      onRendered("", capturedProjectDigest);
      onEmpty();
      send({ type: "preview", html: "<!doctype html><meta charset=utf-8><title>Empty document</title><main style='font:16px system-ui;padding:2rem'>This document is empty.</main>" });
      diagnostics.rendered({ page: false, diagnostics: [] });
      return;
    }
    // The paged formats produce a flow page unless this browser has asked for
    // pages. Whether the source pane is open does not come into it: HTML is
    // the default output, and a reader is shown what an author is shown.
    const htmlPreview = (format === "typst" && start.typstOutput === "html")
      || (format === "latex" && start.latexOutput === "html");
    const markdownPdf = ["markdown", "quarto"].includes(format) && start.buildPreferences.output === "pdf";
    const paged = (renderers.producesPdf(format) && !htmlPreview) || markdownPdf;
    const slow = format === "latex";
    // Shared by both guard points so asset fetching and compilation reject
    // against the same captured navigation, source policy, and main path.
    const capturedMain = source.main;
    const guard = {
      ticket,
      capturedNavigation,
      capturedSource,
      strictSource: slow || format === "quarto",
      capturedMain: source.main,
    };
    try {
      // A manual render is the reader forcing a recompile, so it never takes
      // the shortcut below. It is only looked at here; the compile takes it.
      const manualWaiting = manualPending();
      /** @param {string} id */
      const keyOf = (id) => JSON.stringify([id, format, htmlPreview, markdownPdf, start.buildPreferences, start.localExecution]);
      const held = finished.get(source.main);
      if (held && !manualWaiting) {
        // The identity depends on the texts, their digests and the main path,
        // not on the bytes of any figure, so taking it before the figures are
        // gathered gives the value the compile would have taken.
        const early = await snapshotDigest(source);
        if (coordinator.superseded(guard) || facts().disposed) return;
        if (keyOf(early) === held.key) {
          // Unchanged since it was drawn: no spinner, no compile, no fetch.
          coordinator.commit(ticket);
          if (held.latex) status.recordLatex(held.latex);
          else status.recordLog(held.log);
          status.succeeded();
          status.clearDocx();
          if (held.page.kind === "pdf") {
            onsynctex(held.synctex);
            framePreview.publish(fresh(held.page));
            onRendered(early, capturedProjectDigest);
            if (capturedSource === facts().source) {
              diagnostics.rendered({ page: false, diagnostics: held.diagnostics });
            }
          } else {
            framePreview.publish(held.page);
            onRendered(early, capturedProjectDigest);
            if (capturedSource === facts().source) {
              diagnostics.rendered({ page: Boolean(held.page.html), diagnostics: held.diagnostics });
            }
          }
          return;
        }
        // The source changed since. The old page is still better than an empty
        // pane while the new one compiles.
        if (!facts().everPainted) framePreview.publish(fresh(held.page));
      }
      // A LaTeX compile takes seconds rather than milliseconds, so the pane
      // says one is running. The last page that compiled stays up under it:
      // an author who is typing has something to look at, which is the whole
      // difference between this and a pane that blanks for four seconds.
      // The spinner starts before the figures are fetched, because fetching
      // them is part of the wait.
      if (paged || htmlPreview) status.begin({ clearFailure: !facts().everPainted });
      // The figures, if this document has any. A figure not yet here is
      // awaited before the first compile that needs it, and the page that is
      // already up stays up meanwhile: rendering without them would produce a
      // document with holes in it and replace it a moment later, which reads
      // as a flicker rather than as progress.
      if (Object.keys(source.digests || {}).length) {
        const { held, missing } = await gather(source.digests);
        debug("preview: assets", Object.keys(held.assets || {}).length, missing);
        if (coordinator.superseded(guard)) return;
        // A flow page remains useful when an image blob is unavailable:
        // render its text and let the image appear missing. Paged formats
        // still require a complete tree.
        if (paged || (format === "latex" && htmlPreview)) {
          if (missing.length) {
            throw new Error(`could not fetch figure${missing.length === 1 ? "" : "s"}: ${missing.join(", ")}`);
          }
        }
        source.assets = held.assets;
        // Authored figure URLs and cached Quarto output assets are both part
        // of this render. Keep both inventories when the figure collector
        // returns its authenticated blob URLs.
        source.urls = { ...(source.urls || {}), ...held.urls };
      }
      // Keep a transient source identity for diagnostics and race checks. It
      // is never sent to the server as a rendering name or persisted result.
      identity = await snapshotDigest(source);
      debug("preview: rendering", format, identity);
      if (facts().disposed) return;
      const manual = takeManual();
      let rendered;
      const title = await heading(source);
      if (facts().disposed) return;
      /** @type {{ manual?: boolean, format?: string }} */
      const options = { ...(manual ? { manual: true } : {}) };
      if (htmlPreview) options.format = "html";
      const current = facts();
      // Run code is a switch that is off whenever a document opens and is
      // never stored. While it is off, a stored preference for a tool that
      // runs the document -- Quarto, Calepin -- is ignored. A Quarto document
      // is then drawn as the Markdown it is, and a Typst one by this
      // browser's own renderer, rather than by running code without consent.
      const executes = format === "quarto" || ["quarto", "calepin"].includes(current.buildPreferences.tool);
      const build = executes && !current.localExecution
        ? (format === "typst"
            ? { selection: "tool", backend: "browser", tool: "typst", output: current.buildPreferences.output }
            : { selection: "tool", backend: "browser", tool: "markdown", output: current.buildPreferences.output === "pdf" ? "pdf" : "html" })
        : current.buildPreferences;
      rendered = await renderers.render(source, title, { ...options, buildPreferences: build, project: slug });
      debug("preview: result", Boolean(rendered?.pdf), rendered?.ok, rendered?.failure?.message || "");
      if (facts().disposed) return;
      if (format === "latex" && !htmlPreview) status.recordLatex(rendered);
      // Every other format keeps its tool's output too, so a failed Quarto or
      // Calepin run can show its whole log. Each render replaces the last one,
      // including with "" when this one produced none.
      else status.recordLog(rendered?.log || "");
      const { html, pdf, artifact, artifactKind, synctex, diagnostics: said, log } = rendered;
      const contextual = (said || []).map((item) => diagnosticContext(item, source, identity));
      // An in-flight preview may finish after another keystroke: HTML and
      // Typst may show that intermediate progress while the queued render
      // catches up. Navigation and main-file changes still invalidate it;
      // LaTeX keeps its strict source guard.
      if (coordinator.superseded(guard)) {
        // Collaboration can emit a bookkeeping-only commit while a
        // slow compile is running. Accept the result when the canonical tree
        // is still byte-for-byte the one that produced it.
        const unchanged = format === "latex" && coordinator.isNewer(ticket)
          && capturedNavigation === facts().navigation
          && identity === await snapshotDigest(tree());
        if (!unchanged) return;
      }
      coordinator.commit(ticket);
      if (artifact && artifactKind === "docx" && rendered.ok !== false) {
        const buffer = artifact.buffer
          ? artifact.buffer.slice(artifact.byteOffset, artifact.byteOffset + artifact.byteLength)
          : artifact;
        status.recordDocx({
          bytes: new Uint8Array(buffer.slice(0)),
          snapshot: identity,
          source: capturedSource,
          navigation: capturedNavigation,
        });
      } else if (artifactKind !== "docx") status.clearDocx();
      // A render carries `html` or `pdf`, and the reader posts whichever it
      // has. The bytes are transferred rather than copied: a PDF is megabytes
      // and this page has no further use for it once the frame has it.
      if (pdf) {
        status.succeeded();
        const buffer = pdf.buffer ? pdf.buffer.slice(pdf.byteOffset, pdf.byteOffset + pdf.byteLength) : pdf;
        onsynctex(null);
        const page = { kind: /** @type {const} */ ("pdf"), sha: identity, bytes: new Uint8Array(buffer.slice(0)) };
        framePreview.publish(page);
        rememberPreview(page, identity);
        onRendered(identity, capturedProjectDigest);
        if (capturedSource === facts().source) {
          diagnostics.rendered({ page: false, diagnostics: contextual });
        }
        const key = keyOf(identity);
        const entry = {
          key,
          page: { kind: /** @type {const} */ ("pdf"), sha: identity, bytes: new Uint8Array(buffer.slice(0)) },
          diagnostics: contextual,
          latex: format === "latex" && !htmlPreview ? rendered : null,
          log: rendered?.log || "",
          synctex: null,
        };
        if (rendered.ok !== false) remember(source.main, entry);
        // SyncTeX is navigation metadata, not part of the rendered page.
        // Large maps can take long enough to decompress and index that
        // awaiting them here left a finished PDF behind the placeholder.
        if (format === "latex" && synctex && parseSynctex) {
          void parseSynctex(synctex, Object.keys(source.texts || {})).then((parsed) => {
            // The same map serves the next visit, if this render is still the
            // one kept for the file.
            if (finished.get(capturedMain) === entry) entry.synctex = parsed;
            const at = facts();
            if (!at.disposed && coordinator.isCommitted(ticket)
                && capturedNavigation === at.navigation && capturedSource === at.source) {
              onsynctex(parsed);
            }
          }).catch(() => {
            // The PDF remains useful when optional source mapping is invalid.
          });
        }
        return;
      }
      if (typeof html === "string") {
        status.succeeded();
        // The page is what the document says now, so every error said about an
        // earlier state of it is cleared at once. The warnings that came with
        // this page are painted on the same slow schedule the errors are, so
        // that a font name half typed does not flash a badge on every
        // keystroke.
        framePreview.publish({ kind: "html", html });
        rememberPreview({ kind: "html", html }, identity);
        onRendered(identity, capturedProjectDigest);
        if (capturedSource === facts().source) {
          diagnostics.rendered({ page: Boolean(html), diagnostics: contextual });
        }
        if (rendered.ok !== false) {
          remember(source.main, {
            key: keyOf(identity),
            page: { kind: "html", html },
            diagnostics: contextual,
            latex: format === "latex" && !htmlPreview ? rendered : null,
            log: rendered?.log || "",
            synctex: null,
          });
        }
        return;
      }
      if (artifactKind === "docx" && artifact && rendered.ok !== false) {
        status.succeeded();
        diagnostics.rendered({ page: false, diagnostics: contextual });
        return;
      }
      // No new page: an already painted page stays up transiently while the
      // diagnostics for the current source settle.
      if (capturedSource !== facts().source) return;
      // A log the parser found nothing in is still the only account there
      // is of what happened, and its last lines are where an engine says
      // why it stopped.
      const hasError = contextual.some((item) => item.severity !== "warning");
      const reason = hasError
        ? ""
        : rendered.failure?.message || (log || "").trim().split("\n").slice(-12).join("\n")
          || "the compiler produced no preview and no log";
      const failed = hasError
        ? contextual
        : [...contextual, diagnosticContext({ severity: "error", message: reason }, source, identity)];
      status.failed(reason);
      diagnostics.rendered({ page: false, diagnostics: failed });
      if (reason) console.error(`${format}: could not render:`, reason);
      // Unless nothing was ever painted, the frame gets a transient failure
      // page for flow formats; paged formats use the source and diagnostics
      // pane below.
      if (!facts().everPainted) {
        const page = await renderers.failurePage(await heading(source), format).catch(() => null);
        const at = facts();
        if (!at.disposed && page && coordinator.atLeastCommitted(ticket) && capturedNavigation === at.navigation) {
          send({ type: "preview", html: page });
        }
      }
    } catch (error) {
      // Not a document that did not compile: a renderer that could not be
      // fetched, which is this page's problem rather than the author's.
      const at = facts();
      const stillCurrent = capturedNavigation === at.navigation
        && (!(paged || slow) || capturedSource === at.source);
      if (coordinator.isNewer(ticket) && stillCurrent && error.name !== "Superseded") {
        // The same string the status is set to, said once: what the reader is
        // told and what the diagnostics record must not be able to differ.
        const reason = String(error.message || "could not render");
        status.failed(reason);
        console.error(`${format}: could not render:`, error);
        diagnostics.rendered({
          page: false,
          diagnostics: [diagnosticContext({ severity: "error", message: reason }, source, identity)],
        });
      }
    } finally {
      // Only the newest render owns the spinner. An older one finishing
      // afterwards must not turn it off under a newer one. This render may
      // have committed its own ticket by now, so the test is "nothing newer
      // committed", not "not yet committed".
      if ((paged || htmlPreview) && coordinator.atLeastCommitted(ticket)) status.finish();
    }
  }
  /// Whether a finished render of this entry path is held, so that it will be
  /// painted from memory within a tick and needs no stand-in from elsewhere.
  render.holds = (/** @type {string} */ path) => finished.has(path);
  return render;
}
