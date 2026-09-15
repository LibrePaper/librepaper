<script>
  import { newRequestKey } from "../lib/request-key.js";
  // One document: the source beside it, the page itself, and everything said
  // about it.
  import { anchorAll, anchorAllSources, anchorOne, flatten } from "../lib/anchor.js";
  import * as sync from "../lib/sync.js";
  import * as renderers from "../lib/renderers.js";
  import * as quarto from "../lib/engines/quarto.js";
  import * as figures from "../lib/figures.js";
  import * as history from "../lib/history.js";
  import { createHistorySource } from "../lib/reader/history-source.svelte.js";
  import HistoryWorkspace from "./reader/HistoryWorkspace.svelte";
  import * as passages from "../lib/passages.js";
  import * as suggestions from "../lib/suggestions.js";
  import { diagnosticContext } from "../lib/assistant-review.js";
  import { candidateTree, capturePreviewTree, previewCandidate } from "../lib/assistant-preview.js";
  import { orphanState } from "../lib/orphan.js";
  import * as latex from "../lib/latex.js";
  import { parse as parseSynctex, lineAt as synctexLineAt } from "../lib/synctex.js";
  import * as localQuarto from "../lib/companion/client.js";
  import { companion } from "../lib/companion/status.svelte.js";
  import { basename } from "../lib/file-manager.js";
  import { snapshotDigest } from "../lib/tree-digest.js";
  import { createAnnotations } from "../lib/reader/annotations.svelte.js";
  import { createReaderBoot } from "../lib/reader/boot.js";
  import { createPendingChat } from "../lib/reader/chat.js";
  import { createReaderCollaboration } from "../lib/reader/collaboration.js";
  import { createWorkspace } from "../lib/reader/workspace.svelte.js";
  import { createBuildSettings } from "../lib/reader/build-settings.svelte.js";
  import { createTimeline } from "../lib/reader/timeline.svelte.js";
  import { createPassageTrace } from "../lib/reader/passage-trace.svelte.js";
  import { createPreviewRenderer } from "../lib/reader/preview-render.js";
  import { createPreferences } from "../lib/reader/preferences.svelte.js";
  import { prepareOfflineProject, preparedProject } from "../lib/offline-projects.js";
  import { cacheCurrentShell } from "../lib/offline-shell.js";
  import { createPublication } from "../lib/reader/publication.svelte.js";
  import { buildDisplayBundle } from "../lib/publication-builder.js";
  import { needsSourceRefresh } from "../lib/reader/source-events.js";
  import {
    SHELL_HEADERS,
    authHeaders,
    config as loadConfig,
    keyHeaders,
    signInHref,
  } from "../lib/api.js";
  import {
    LINKED,
    linkFor,
    markViewed,
    read,
    takeKeyFromFragment,
    write,
  } from "../lib/storage.js";
  import { ACTIVITY_WIDTH, DOCUMENT_MIN, GRIP, PANES, RATIOS, clamp, measure, pixels, px, showing } from "../lib/panes.js";
  import { tabsFor } from "../lib/panels.js";

  import { tick, untrack } from "svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import CopyLink from "./CopyLink.svelte";
  import Modal from "./Modal.svelte";
  import Toasts from "./Toasts.svelte";
  import { done as toastDone, problem as toastProblem, said as toastSaid } from "../lib/toast.svelte.js";
  import { archive, availableDownloads, docxFile, entryDownload, projectFiles, renderingFile, saveBlob } from "../lib/reader/downloads.js";
  import Preview from "./Preview.svelte";
  import PreviewControls from "./PreviewControls.svelte";
  import Grip from "./Grip.svelte";
  import { parseRenderOptions } from "../lib/quarto-options.js";
  import SettingsDialog from "./settings/SettingsDialog.svelte";
  import { anonymousIdentity, update as updateBuildPreferences } from "../lib/build-preferences.js";
  import LatexStatus from "./LatexStatus.svelte";
  import PreviewStatus from "./PreviewStatus.svelte";
  import Avatar from "./Avatar.svelte";
  import { extractOutline } from "../lib/outline.js";
  import { createFramePreview } from "../lib/reader/frame-preview.js";
  import { createFrameOverlays } from "../lib/reader/frame-overlays.js";
  import { createRenderDiagnostics } from "../lib/reader/render-diagnostics.js";
  import { createRenderCoordinator } from "../lib/reader/render-coordinator.js";
  import { createGeneration } from "../lib/reader/generation.js";
  import { createRenderStatus } from "../lib/reader/render-status.svelte.js";
  import { createLocalPreview } from "../lib/reader/local-preview.svelte.js";
  import { HIGHLIGHT_COLORS, colorName } from "../lib/annotation-colors.js";
  import { correctedLeft, placeBar as placeSelectionBar } from "../lib/annotation-bar.js";
  import InsertMenu from "./InsertMenu.svelte";
  import ReaderSidebar from "./reader/ReaderSidebar.svelte";
  import PanelRail from "./reader/PanelRail.svelte";
  import Files from "./reader/Files.svelte";
  import Outline from "./reader/Outline.svelte";
  import Agent from "./reader/Agent.svelte";
  import Collaboration from "./reader/Collaboration.svelte";
  import Changes from "./reader/Changes.svelte";
  import Share from "./reader/Share.svelte";
  import Diagnostics from "./reader/Diagnostics.svelte";
  import History from "./reader/History.svelte";
  import PendingAnnotations from "./reader/PendingAnnotations.svelte";

  const SLUG = location.pathname.split("/").pop();

  // The key a reader arrived with, taken out of the fragment before anything
  // asks the server a question. A fragment never leaves the browser, so this
  // is the one part of the URL a link key can safely travel in; from here it
  // is kept under the slug and presented on every request for this document.
  const KEY = takeKeyFromFragment(SLUG);

  /* ------------------------------------------------------------ the document */

  let doc = $state({});
  let readerDisposed = false;
  let docsOrigin = $state(null);
  let frameSrc = $state(null);
  let publishedMode = $state(false);
  let offlinePrepared = $state(false);
  let preparingOffline = $state(false);
  // The published version, whether the source has moved past it, and what
  // publishing a new one involves. An existing publication makes the
  // expected-publication pointer part of a publish request, so the Share
  // panel stays available while that pointer arrives but its publish control
  // waits for it.
  const publication = createPublication({
    slug: SLUG,
    key: KEY,
    liveTree: () => liveTreeNow(),
    committedTree: () => session?.tree?.() || liveTreeNow(),
    heading: (tree) => headingOf(tree),
    gather: (digests) => figures.gather(SLUG, digests, authHeaders(KEY), { strict: true }),
    facts: () => ({ canEdit: mayEdit, hasSession: Boolean(session), source: sourceGeneration }),
    say,
    disposed: () => readerDisposed,
  });
  const publishedPublication = $derived(publication.state.publication);
  const publicationUpdate = $derived(publication.state.update);
  const publicationStatus = $derived(publication.state.stale);
  const publicationMetadataReady = $derived(publication.state.metadataReady);
  const publicationMetadataFailed = $derived(publication.state.metadataFailed);
  let me = $state({});
  // The displayed name, since this is what goes on a comment and what the
  // reader is shown commenting as. A Google account's handle is its email and
  // belongs on neither.
  let identity = $derived(me.name || "");
  let canModerate = $derived(Boolean(doc.can_moderate));
  let connected = $state(true);
  // Whether this tab is the one in front of the reader. Only background work
  // consults it; nothing about the document depends on being looked at.
  let hidden = $state(typeof document !== "undefined" && document.visibilityState === "hidden");
  let liveChat = $state([]);
  let unreadChat = $state(false);
  let pendingChat;
  let mayChat = $derived(["commenter", "editor", "owner"].includes(doc.role));
  // Sharing is the owner's; seeing who else is in the room is anyone's who is
  // named on the document. A reader who arrived by link is offered neither,
  // which is most of the point of a blind review.
  let canSeeSharing = $derived(Boolean(doc.can_see_sharing));
  let canPublish = $derived(["editor", "owner"].includes(doc.role));

  // Whether this browser's work is safe, which is a different question from
  // whether the socket is up. `pending` counts the updates the server has not
  // yet said it has written; `local` says the document is in this browser's
  // own storage, which is what makes a reload safe while the socket is down.
  let persistence = $state({ pending: 0, local: false, joined: false });

  /* --------------------------------------------------------------- anchoring */

  // Legacy suggestion decisions are acknowledged over the room event, just
  // like live revision decisions. Keeping a waiter here lets the Changes pane
  // advance only after the server confirms the apply-on-accept operation.
  const suggestionDecisions = new Map();
  // Everything said about the document is the annotations' to own. Anchoring
  // and repainting stay here: both depend on the frame, which that module
  // cannot see.
  const annotations = createAnnotations({
    slug: SLUG,
    anchor: anchorComments,
    repaint: applyHighlights,
    send: (message) => collaboration?.send(message),
    publicationId: () => publication.id(),
  });
  const comments = $derived(annotations.state.comments);
  const unconfirmed = $derived(annotations.state.unconfirmed);
  const outbox = annotations.outbox;
  const sendAnnotation = annotations.submit;
  const discardAnnotation = annotations.discard;
  let commentsReady = false;
  let frameReady = false;
  let docText = null; // the joined visible text, invariant across repaints
  let docView = null; // flatten(docText), so anchoring does not redo it per call
  let figureAt = $state([]); // text offset of each figure, by its index

  let preview = $state(null);
  // Both Quarto's own live preview and a Typst document previewed through
  // Calepin run the same `createLocalPreview` lifecycle
  // (`../lib/reader/local-preview.svelte.js`) against the same local app.
  // Each controller owns what it is doing -- running, starting, re-rendering,
  // and the last thing that went wrong -- and the aliases further down read
  // it, rather than the page keeping a copy of each and a callback to keep
  // the copy honest.
  let localConnectionError = $state("");
  let quartoLiveSyncTimer = null;
  let calepinSyncTimer = null;
  // The Quarto profile and parameters this browser previews with, set under
  // Settings and remembered per document. The format is never chosen here:
  // it comes from the document's front matter, and older remembered options
  // that still carry one are ignored.
  let quartoOptions = $state({ profile: null, parameters: {} });
  let quartoBindingId = $state("");
  const localAppStatus = $derived(companion.status);

  /// Points the local-app pairing at this document and picks up whatever
  /// binding it already remembers for it. A pairing this browser already
  /// holds is verified now, so the workspace banner shows a connected
  /// preview without a first failed attempt to start one.
  function pairLocalQuarto() {
    localQuarto.configure({ project: SLUG, origin: location.origin, active: mayEdit });
    quartoBindingId = localQuarto.bindingId();
    if (mayEdit) void localQuarto.probe({ pairedOnly: true });
  }
  // Which engine draws each format, and what it produces, belong to the
  // build settings along with the preference they follow from; the aliases
  // are further down, where that module is built. What stays here is the one
  // thing that is not a preference: running a document's code is a gesture
  // this page session asks for and never remembers.
  //
  // Pairing grants the browser permission to ask, but never starts document
  // code. Quarto and Calepin execution begin only with that gesture, and it
  // is never remembered: a shared document arrives with local execution off,
  // whatever build preference this browser happens to have kept for it.
  let localExecution = $state(false);

  async function setLatexOutput(format) {
    const next = format === "html" ? "html" : "pdf";
    if (latexOutput === next) return;
    buildSettings.state.latexOutput = next;
    setBuildPreferences(updateBuildPreferences(buildScope(), "latex", { ...(next === "html" ? { selection: "tool", backend: "browser", tool: "tex", preset: "" } : {}), output: next }));
    navigationGeneration += 1;
    renderers.cancelPreview({ keepWarm: true });
    latex.cancel();
    framePreview?.clear();
    deliveredKind = "";
    everPainted = false;
    everPaintedShown = false;
    renderStatus.resetFailure();
    clearTimeout(previewTimer);
    previewTimer = null;
    await tick();
    if (!readerDisposed) void paintPreview();
  }

  // Switching between the browser's Markdown draft and Quarto's own preview
  // is a choice of tool, not of output. The browser draft is HTML and nothing
  // else, so that direction pins the output; the other keeps whatever output
  // was already asked for, or a PDF preview would quietly become an HTML one
  // at the moment local rendering was enabled to produce it.
  async function setQuartoPreviewMode(mode) {
    setBuildPreferences(updateBuildPreferences(buildScope(), "quarto", { selection: "tool", backend: mode === "markdown" ? "browser" : "local", tool: mode === "markdown" ? "markdown" : "quarto", output: mode === "markdown" ? "html" : (buildPreferences.output || "html") }));
    navigationGeneration += 1;
    buildSettings.state.quartoPreviewMode = mode === "markdown" ? "markdown" : "quarto";
    if (quartoPreviewMode === "markdown") {
      await quartoPreviewController.stop();
      void paintPreview();
    }
  }

  // What running a document's code here actually means. It was a title
  // attribute on a button, which is to say it was invisible on a touch screen
  // and delayed on every other: a consequence of this size is asked about,
  // not hinted at.
  const LOCAL_EXECUTION_WARNING = "Quarto and Calepin execution can run arbitrary code from this document on your computer.";
  let localExecutionConsent = $state(false);

  // The formats whose local tool runs the document rather than only typesets
  // it. The option can be turned on with anything else open -- it is a choice
  // about this session, not about this file -- but there is no companion to
  // reach for until one of these is what is being read.
  const localExecutionRelevant = $derived(["quarto", "typst"].includes(sourceFormat) && mayEdit);

  // Turning it on, once the dialog has been answered: put this format's
  // engine on the local tool and reach the companion. A companion that cannot
  // be reached leaves the choice standing and says why in Diagnostics, rather
  // than silently undoing what was just asked for.
  async function startLocalExecution() {
    localExecutionConsent = false;
    localExecution = true;
    if (!localExecutionRelevant) return;
    if (sourceFormat === "quarto") {
      await setQuartoPreviewMode("quarto");
      await ensureLocalApp();
    } else if (sourceFormat === "typst") {
      await setTypstPreviewMode("calepin");
    }
  }

  // Turning it off puts the browser's own renderer back and stops whatever
  // the companion was running for this document.
  async function stopLocalExecution() {
    localExecution = false;
    if (sourceFormat === "quarto") await setQuartoPreviewMode("markdown");
    else if (sourceFormat === "typst") await setTypstPreviewMode("typst");
  }

  const toggleLocalExecution = () => {
    if (localExecution) void stopLocalExecution();
    else localExecutionConsent = true;
  };

  // The choice is about this session rather than about one file, so pointing
  // the preview at a document in the other executable format puts the
  // companion behind that one too, and checks that it is there, without
  // asking again. Once the mode is set this settles: the guard reads it.
  $effect(() => {
    if (!localExecution || !localExecutionRelevant) return;
    const running = sourceFormat === "quarto" ? quartoPreviewMode === "quarto" : typstPreviewMode === "calepin";
    if (running) return;
    untrack(() => void startLocalExecution());
  });

  async function setTypstPreviewMode(mode) {
    setBuildPreferences(updateBuildPreferences(buildScope(), "typst", { selection: "tool", backend: mode === "calepin" ? "local" : "browser", tool: mode === "calepin" ? "calepin" : "typst" }));
    navigationGeneration += 1;
    buildSettings.state.typstPreviewMode = mode === "calepin" ? "calepin" : "typst";
    if (typstPreviewMode === "calepin") {
      if (await ensureLocalApp() && calepinActive) await calepinPreviewController.start();
    } else {
      await calepinPreviewController.stop();
      void paintPreview();
    }
  }

  async function setTypstOutput(format) {
    const next = format === "html" ? "html" : "pdf";
    if (typstOutput === next) return;
    buildSettings.state.typstOutput = next;
    setBuildPreferences(updateBuildPreferences(buildScope(), "typst", { output: typstOutput }));
    // Invalidate every pending delivery before stopping Calepin. A PDF that
    // finishes after this gesture must never replace the HTML frame.
    navigationGeneration += 1;
    if (typstOutput === "html") await calepinPreviewController.stop();
    void paintPreview();
  }

  function quartoTargetFormat(tree = treeNow()) {
    const main = tree?.main || session?.mainPath?.() || "main.qmd";
    const source = tree?.texts?.[main] || session?.textOf?.(session.mainId?.())?.toString?.() || session?.text?.toString?.() || "";
    const value = quarto.parseQuarto(source, { path: main }).metadata?.format;
    const named = typeof value === "string" ? value : value && typeof value === "object" ? Object.keys(value)[0] : "html";
    const format = String(named || "html").trim().toLowerCase().split(/[+:]/, 1)[0];
    return ["html", "pdf", "docx", "revealjs"].includes(format) ? format : "html";
  }
  function quartoRenderContext(tree = null) {
    return {
      format: quartoTargetFormat(tree || treeNow()),
      profiles: (buildPreferences.profile || quartoOptions.profile) ? [buildPreferences.profile || quartoOptions.profile] : [],
      parameters: { ...(buildPreferences.parameters || quartoOptions.parameters) },
    };
  }

  const tell = (message, transfer) => preview?.tell(message, transfer);
  const frameOverlays = createFrameOverlays({ ready: () => frameReady, send: tell });
  // Initialized after the derived frame kind is available. The controller's
  // callbacks still update the small bits of component state used by the
  // template and annotation code.
  let framePreview;

  // The agent repaints the whole document on every "regions" or "highlight"
  // message, so a call that changes nothing is not free even though it looks
  // idempotent. Each is sent only when its payload actually differs from the
  // last one sent -- reset when the frame republishes its text, since the
  // agent's DOM was rebuilt then and needs the full repaint regardless.
  // The annotation singled out last, from either side: a card clicked in the
  // sidebar or a mark clicked in the document. Its card wears a ring and the
  // frame rings its passage, and both stay until another one is chosen.
  let selectedAnnotation = $state("");
  function applySelection() {
    frameOverlays.selection(selectedAnnotation);
  }
  $effect(() => { void selectedAnnotation; applySelection(); });

  function applyHighlights() {
    frameOverlays.annotations(comments);
  }

  // Whether a comment's passage is lost is answered from two anchors, not
  // one: the rendered quotation, which is what the highlight and the click
  // target are drawn from, and the source quotation, which is the anchor of
  // record. A comment is orphaned only when neither finds its passage; when
  // only the source still has it, the card says so instead and a click on it
  // goes to the source rather than nowhere. A region has no source anchor and
  // is never in either state.
  function applyAnchorFlags(comment) {
    const { orphaned, inSourceOnly } = orphanState({
      renderedFound: comment.start != null,
      sourceFound: comment.sourceStart != null,
      region: Boolean(comment.region),
    });
    comment.orphaned = orphaned;
    comment.inSourceOnly = inSourceOnly;
  }

  // The one place both anchors of a comment are computed, so the orphaning
  // rule above lives in one place too. Used for a whole re-anchoring pass and
  // for the single comment a submission or a broadcast just added -- the
  // rendered pass is skipped for a region annotation, which is placed by the
  // agent rather than by text matching, but the source pass runs over
  // whatever is given it since only a comment that already has a `source`
  // does anything there.
  function anchorComments(list) {
    const renderAnchors = list.filter((comment) => !comment.region);
    anchorAll(docText || "", renderAnchors, docText === null ? null : docView);
    if (publishedMode) {
      for (const comment of list) {
        if (comment.publication_id === publishedPublication?.id) continue;
        const found = !comment.region && anchorOne(docText || "", { ...comment, position: null, requireUnique: true }, docView);
        comment.start = found?.start ?? null;
        comment.end = found?.end ?? null;
        comment.earlierPublication = !found;
        if (comment.region) comment.regionUnplaceable = true;
      }
    } else if (mayEdit) anchorAllSources(treeNow(), list);
    for (const comment of list) applyAnchorFlags(comment);
  }

  // A comment made before the source anchor existed, or whose passage this
  // browser cannot re-derive from the words alone, is missing the anchor of
  // record. An editor's browser backfills it once per page load, quietly: the
  // same heuristic a fresh selection uses, run now against the passage the
  // rendered anchor already found. Tried is remembered so a comment nobody
  // can place is not retried on every repaint, and nothing here is retried
  // automatically -- a comment that stays untried just keeps its rendered
  // anchor as its only one.
  const triedBackfill = new Set();
  // A backfill this browser sent and has not heard back about, so its `error`
  // -- a race with someone else's backfill, or a comment deleted meanwhile --
  // is known to be that and not a failed submission. Nothing else reads or
  // writes this set.
  const pendingBackfill = new Set();
  function backfillSourceAnchors() {
    if (!mayEdit || !session || docText === null) return;
    const tree = treeNow();
    const open = session?.paths?.get(openFile) || "";
    for (const comment of comments) {
      if (comment.source || comment.region || comment.point || comment.pending || comment.temp_id) continue;
      if (comment.start == null || triedBackfill.has(comment.id)) continue;
      triedBackfill.add(comment.id);
      const source = sync.sourceSelectorFor(
        docText,
        { exact: comment.exact, position: comment.start },
        tree,
        { open, formatOf: renderers.formatOf },
      );
      if (source) {
        pendingBackfill.add(comment.id);
        collaboration?.send({ type: "anchor", comment_id: comment.id, source });
      }
    }
  }

  function reanchor() {
    if (!frameReady || !commentsReady || docText === null) return;
    anchorComments(comments);
    annotations.publish();
    applyHighlights();
    tracePassages();
    // The frame's own `ready` is what re-anchors after the source changes --
    // `session.watchSource` schedules a repaint, and every repaint ends here
    // -- so a comment newly findable in the source is caught by the same
    // pass, not by a second path. Batched a tick out so the paint above is
    // never delayed by a socket round trip.
    setTimeout(backfillSourceAnchors, 0);
  }

  /// Marks (or clears) every named comment's region as placeable, reassigning
  /// `comments` afterward -- not because the loop above needs it, but because
  /// that reassignment is what tells Svelte the array changed.
  function markRegionsPlaceable(ids, placeable, reason) {
    for (const comment of comments) {
      if (!comment.region || !ids.has(String(comment.id))) continue;
      if (placeable) {
        delete comment.regionUnplaceable;
        delete comment.regionUnplaceableReason;
      } else {
        comment.regionUnplaceable = true;
        comment.regionUnplaceableReason = String(reason || "figure-unavailable");
      }
    }
    annotations.publish();
  }

  function fromFrame(message) {
    switch (message.type) {
      case "ready":
        docText = typeof message.text === "string" ? message.text : "";
        docView = flatten(docText);
        // Where each figure sits in that text, so a note on a figure can be
        // ordered against the notes on passages.
        figureAt = Array.isArray(message.images) ? message.images.map(Number) : [];
        // The first time the frame says it is there is the first moment
        // anything can be sent to it. A paint made before this went to a
        // window that had not navigated yet and was lost -- which is what a
        // reader saw as a blank document, since a reader makes no edits to
        // trigger a second one. Only the first `ready` paints: the agent
        // sends one after every repaint, and painting on each would be a
        // loop.
        const first = framePreview.markReady();
        frameReady = true;
        if (first && !publishedMode) replayPreview();
        tell({ type: "tool", tool });
        // Whatever was painted before is gone with the rebuilt DOM.
        frameOverlays.reset();
        reanchor();
        applySelection();
        if (first && !publishedMode) {
          void paintPreview();
        }
        break;
      case "viewer-state":
        viewerView = message.drawn
          ? { mode: String(message.mode || "auto"), scale: Number(message.scale) || null }
          : null;
        break;
      case "selection":
        showSelection(message.selector, message.rect);
        break;
      case "region":
        // A rectangle drawn on a figure anchors the same way a quotation
        // does, but it has no words to look up in the source: a region has
        // no source anchor and never will.
        pending = { exact: "", prefix: "", suffix: "", position: null, region: message.region, source: null, publication_id: publishedMode ? publishedPublication?.id || "" : "" };
        placeBar(message.rect);
        break;
      case "regions-unplaceable": {
        const ids = new Set((message.ids || []).map(String));
        if (!ids.size) break;
        markRegionsPlaceable(ids, false, message.reason);
        break;
      }
      case "regions-placeable": {
        const ids = new Set((message.ids || []).map(String));
        if (!ids.size) break;
        markRegionsPlaceable(ids, true);
        break;
      }
      case "caret":
        followDocumentClick(Number(message.offset) || 0, message.pdf);
        break;
      case "pdf-caret":
        followPdfClick(message.pdf);
        break;
      case "focus":
        void focusAnnotation(message.id);
        break;
    }
  }

  /* --------------------------------------------------------------- selection */

  let tool = $state("commenting");
  let highlightColor = $state(HIGHLIGHT_COLORS[0]);
  let pending = $state(null);
  let assistantRequest = $state(null);
  let selectionRevision = Promise.resolve("");
  let bar = $state({ shown: false, left: 0, top: 0 });

  function showSelection(selector, rect) {
    const point = selector?.point === true;
    if (!selector || (point
      ? Boolean(selector.exact) || !Number.isInteger(selector.position) || selector.position < 0
      : !selector.exact)) {
      bar = { ...bar, shown: false };
      pending = null;
      return;
    }
    pending = {
      exact: String(selector.exact || ""),
      prefix: String(selector.prefix || ""),
      suffix: String(selector.suffix || ""),
      // A hint, not a claim: the server keeps it, and anchoring uses it only
      // to choose between passages the context cannot separate.
      position: Number.isInteger(selector.position) && selector.position >= 0 ? selector.position : null,
      ...(point ? { point: true } : {}),
    };
    // The anchor of record, cut from the source at the same moment: a best
    // effort taken here, in the commenter's browser, while the words just
    // selected are still fresh. A page with no text yet, or a phrase the
    // source-matching heuristic cannot place, leaves this null -- which the
    // server reads as "no source anchor yet" rather than as a failure.
    let source = null;
    try {
      if (mayEdit && docText !== null && !point) {
        source = sync.sourceSelectorFor(docText, pending, treeNow(), {
          open: session?.paths?.get(openFile) || "",
          formatOf: renderers.formatOf,
        }) || null;
      }
    } catch {
      source = null;
    }
    pending.source = source;
    const captured = pending;
    pending.publication_id = publishedMode ? publishedPublication?.id || "" : "";
    selectionRevision = mayEdit ? snapshotDigest(capturePreviewTree(treeNow())) : Promise.resolve("");
    void selectionRevision.then((revision) => {
      captured.revision = revision;
    }).catch(() => { captured.revision = ""; });
    placeBar(rect);
  }

  function askDiagnostic(item) {
    assistantRequest = { id: crypto.randomUUID(), diagnostic: { ...item }, revision: item.revision || "",
      task: { kind: "fix", scope: item.file || item.path ? "file" : "document" } };
    showPanel("agent");
    if (width <= 760) showMobileView("sidebar");
  }

  function askCommentAssistant(comment) {
    if (!comment?.id) return;
    const source = comment.source || (comment.exact ? {
      path: comment.sourcePath || session?.paths?.get(openFile) || "",
      exact: comment.exact,
      prefix: comment.prefix || "",
      suffix: comment.suffix || "",
      position: Number.isInteger(comment.position) ? comment.position : null,
    } : null);
    assistantRequest = {
      id: crypto.randomUUID(),
      comment: {
        id: String(comment.id), body: comment.body || "", exact: comment.exact || "",
        proposed: comment.proposed || "", revision: comment.revision || "",
        source, replies: (comment.replies || []).map((reply) => ({ body: reply.body, creator: reply.creator })),
        suggestion: comment.motivation === "editing" ? {
          id: String(comment.id), proposed: comment.proposed || "", revision: comment.revision || "",
          path: source?.path || "", exact: source?.exact || "",
        } : null,
      },
      selection: source,
      revision: comment.revision || "",
    };
    showPanel("agent");
    if (width <= 760) showMobileView("sidebar");
  }

  // Candidate previews are rendered from the runner's immutable file snapshot
  // in this browser. Nothing is written to the shared Yjs tree; only the
  // diagnostics and output kind go back over the private assistant channel.
  async function previewAssistant(request) {
    // Capture before the first await. Asset fetching and compilation may take
    // seconds, and a candidate must be checked against the exact base tree
    // that the runner used when it proposed its revision.
    const tree = capturePreviewTree(request?.candidate ? candidateTree(request.candidate) : treeNow());
    if (Object.keys(tree.digests || {}).length) {
      const held = await figures.gather(SLUG, tree.digests, authHeaders(KEY));
      tree.assets = held.assets;
      tree.urls = held.urls;
    }
    return previewCandidate({ request, tree, render: renderers.render,
      title: headingOf, digest: snapshotDigest });
  }

  async function reviewAssistantResults({ suggestions: ids = [], pass = "" }) {
    const matches = comments.filter((comment) => comment.motivation === "editing" &&
      (ids.includes(comment.id) || (pass && comment.pass === pass)));
    const first = matches.find((comment) => !comment.resolved) || matches[0];
    if (!first) { toastProblem("These suggestions are no longer available."); return; }
    await focusAnnotation(first.id);
  }

  async function focusAnnotation(id) {
    const comment = comments.find((item) => item.id === id);
    if (!comment) return;
    selectedAnnotation = String(comment.id);
    const suggestion = comment.motivation === "editing";
    const plainHighlight = comment.motivation === "highlighting" && !comment.body && !comment.replies?.length;
    if (!suggestion) prefs.collaborationTab = plainHighlight ? "highlights" : "comments";
    void showPanel(suggestion ? "changes" : "collaboration");
    const targetPanel = panel;
    await tick();
    if (panel !== targetPanel) return;
    const prefix = suggestion ? "changes" : plainHighlight ? "collaboration-highlight" : "collaboration-comment";
    const card = document.getElementById(`${prefix}-${id}`);
    card?.focus({ preventScroll: true });
    await tick();
    card?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }

  async function revealAnnotation(comment) {
    selectedAnnotation = String(comment.id);
    if (comment.motivation === "editing") {
      await startEditing();
      showMobileView("source");
      await tick();
      if (comment.sourceStart == null) {
        toastProblem("This proposed change cannot be located in the current source. Its retained text is available in Changes.");
        return;
      }
      const id = session?.idOf(comment.sourcePath);
      if (id) {
        ws.openFile = id;
        editor?.goToIn(id, comment.sourceStart);
      } else editor?.goTo(comment.sourceStart);
      return;
    }
    if (comment.start != null || comment.region) {
      showMobileView("document");
      await tick();
      tell({ type: "reveal", id: comment.id });
    } else if (comment.sourceStart != null && editing) {
      showMobileView("source");
      await tick();
    }
    if (shown.source && comment.sourceStart != null && editing && editor) {
      const id = session.idOf(comment.sourcePath);
      if (id) { ws.openFile = id; editor.goToIn(id, comment.sourceStart); }
      else editor.goTo(comment.sourceStart);
    }
  }

  // Measuring is here, because the selection, the frame and the bar are all
  // things on a screen; where the numbers put the bar is `annotation-bar.js`.
  function placeBar(rect) {
    if (rect && !matchMedia("(max-width:760px)").matches) {
      const frame = document.querySelector(".viewport").getBoundingClientRect();
      bar = {
        shown: true,
        ...placeSelectionBar({
          rect,
          frame,
          width: barElement?.offsetWidth || 250,
          minTop: belowTheBar(),
          windowWidth: innerWidth,
        }),
      };
      return;
    }
    bar = { ...bar, shown: true };
  }

  let barElement = $state(null);

  // How far down the window anything is allowed to start: the height of the
  // bar at the top, taken from the stylesheet that sets it rather than from a
  // number copied out of it, and the gap the placing has always left below it.
  function belowTheBar() {
    const declared = getComputedStyle(document.documentElement).getPropertyValue("--librepaper-bar").trim();
    const height = declared.endsWith("px") ? Number.parseFloat(declared) : (Number.parseFloat(declared) || 3.5) * unit;
    return Math.round(height) + 9;
  }

  // The tool decides how wide the bar is -- highlighting adds five swatches --
  // and the width is only knowable once it is drawn, so the placing above is
  // made good here, after it is.
  $effect(() => {
    if (!bar.shown || !barElement) return;
    void tool;
    const left = correctedLeft({ left: bar.left, width: barElement.offsetWidth, windowWidth: innerWidth });
    if (left !== null) bar = { ...bar, left };
  });

  function chooseTool(which) {
    if (!mayChat) return;
    tool = which;
    if (pending?.point || pending?.region || which === "point" || which === "region") {
      pending = null;
      bar = { ...bar, shown: false };
    }
    tell({ type: "tool", tool: which });
    if (compact) showMobileView("document");
  }

  /* -------------------------------------------------------------- annotating */

  let commenting = $state(false);
  let identifying = $state(false);
  let deleting = $state(false);
  let draft = $state({ body: "", proposed: "" });
  let pendingDelete = $state([]);

  // Opening the dialog. The suggest variant starts its proposal textarea
  // with the source slice when the passage was placed, the rendered words
  // otherwise (`suggestions.prefillFor`, which a check exercises directly).
  // Set here, once, rather than in an effect: an effect that read `draft` to
  // write it would run again on every keystroke and clobber what was typed.
  function openDialog() {
    if (tool === "editing") draft = { ...draft, proposed: suggestions.prefillFor(pending) };
    commenting = true;
  }

  function barClicked() {
    if (!pending || !mayChat) return;
    bar = { ...bar, shown: false };
    if (tool === "highlighting") {
      // No dialog: the passage is the whole annotation.
      submitAnnotation({ motivation: "highlighting", body: "" });
      return;
    }
    if (!identity && me.providers?.length) {
      identifying = true;
      return;
    }
    openDialog();
  }

  function submitAnnotation({ motivation, body, proposed }) {
    if (!pending || !mayChat) return false;
    if (publishedMode && (publicationUpdate || pending.publication_id !== publishedPublication?.id)) {
      say("Refresh the published version and select the passage again before submitting. Your draft is kept.", true);
      return false;
    }
    // The server determines the author when it acknowledges the submission.
    annotations.comment(pending, { motivation, body, proposed, color: motivation === "highlighting" ? highlightColor : undefined }, identity || doc.commenting_as || "Anonymous");
    pending = null;
    return true;
  }

  function submitDialog(event) {
    event.preventDefault();
    const motivation = tool === "region" || tool === "point" ? "commenting" : tool;
    const submitted = submitAnnotation({
      motivation,
      body: draft.body,
      proposed: motivation === "editing" ? draft.proposed : undefined,
    });
    if (!submitted) return;
    draft = { body: "", proposed: "" };
    commenting = false;
  }

  // An editor's decision on a suggestion: optimistically busy, not resolved,
  // until the `accept` or `reject` broadcast settles it (or an `error`
  // clears the busy state back off). `request_id` is what makes a retried
  // decision a no-op on the server rather than a second one; nothing here
  // retries automatically, so a fresh one per click is enough.
  function decideSuggestion(comment, action) {
    suggestions.beginDeciding(comment, action);
    annotations.publish();
    const request_id = newRequestKey();
    const promise = new Promise((resolve, reject) => suggestionDecisions.set(request_id, { commentId: comment.id, resolve, reject }));
    let sent;
    // A suggestion is a proposal with a remark attached (§1.2), so deciding one
    // is deciding its hunk. A suggestion is one hunk by construction, which is
    // why the index is zero, and the tip it was reviewed against travels with
    // the decision so that a suggestion the author has refined since is refused
    // rather than agreed to in a form nobody read.
    try {
      sent = collaboration?.send({
        type: "proposal-decide",
        proposal_id: comment.proposal,
        hunk: 0,
        accepted: action === "accept",
        tip: proposalTip(comment.proposal),
        request_id,
      });
    }
    catch (error) {
      suggestionDecisions.delete(request_id);
      suggestions.clearDeciding(comment);
      annotations.publish();
      return Promise.reject(error);
    }
    if (sent === undefined || sent === false) {
      suggestionDecisions.delete(request_id);
      suggestions.clearDeciding(comment);
      annotations.publish();
      return Promise.reject(new Error("Review transport is unavailable."));
    }
    if (sent?.then) sent.then((result) => {
      if (result?.ok !== false) return;
      const pendingDecision = suggestionDecisions.get(request_id);
      if (!pendingDecision) return;
      suggestionDecisions.delete(request_id);
      suggestions.clearDeciding(comment);
      annotations.publish();
      pendingDecision.reject(result.error instanceof Error ? result.error : new Error(result.error || "Suggestion decision failed."));
    }).catch((error) => {
      const pendingDecision = suggestionDecisions.get(request_id);
      if (!pendingDecision) return;
      suggestionDecisions.delete(request_id);
      suggestions.clearDeciding(comment);
      annotations.publish();
      pendingDecision.reject(error);
    });
    return promise;
  }

  async function rejectConfirmed(comment) {
    const response = await fetch(`/api/documents/${SLUG}/comments`, {
      method: "POST", headers: authHeaders(KEY, "application/json"),
      body: JSON.stringify({ type: "reject", comment_id: comment.id, request_id: newRequestKey() }),
      signal: AbortSignal.timeout(15000),
    });
    const result = await response.json();
    if (!response.ok || result.type === "error") throw new Error(result.message || result.error || "Could not reject this suggestion.");
    receive(result);
  }

  // A suggestion the server refused to apply because the passage it named no
  // longer matches (`{"type":"error","stale":true,...}`): open the merge
  // editor on the proposal applied to the checkpoint it was made against,
  // beside the live text, so an editor can take what still applies by hand.
  async function openStaleSuggestion(comment) {
    if (!comment.source || !comment.revision || !session) {
      toastProblem("the passage has changed since this was suggested");
      return;
    }
    const path = comment.source.path;
    try {
      const point = await history.checkpoint(SLUG, comment.revision, keyHeaders(KEY));
      const baseText = point.texts?.[path] ?? "";
      const oldText = suggestions.applyProposal(baseText, comment.source, comment.proposed ?? "");
      const tree = treeNow();
      const id = session.idOf(path);
      const component = (await import("./MergeEditor.svelte")).default;
      MergeEditor = component;
      mergeTarget = {
        path,
        oldText,
        newText: tree.texts?.[path] ?? "",
        liveText: id ? session.textOf(id) : null,
        ephemeral: session.ephemeral,
        loroDoc: session.doc,
        editable: mayEdit && editing && Boolean(id),
        targetLabel: "Live document",
        note: "this suggestion no longer applies cleanly",
      };
    } catch (error) {
      toastProblem(error.message || "the passage has changed since this was suggested");
    }
  }

  const resolve = annotations.resolve;

  function askDelete(comment) {
    askDeleteMany([comment]);
  }

  function askDeleteMany(items) {
    if (!items.length) return;
    pendingDelete = items;
    deleting = true;
  }

  function confirmDelete() {
    const items = pendingDelete;
    pendingDelete = [];
    deleting = false;
    for (const comment of items) annotations.delete(comment);
  }

  function reply(comment, body, name) {
    if (mayChat) annotations.reply(comment, body, name);
  }

  /* -------------------------------------------------------------------- room */

  let collaboration = $state(null);

  function sendLiveChat(text) {
    if (!collaboration || !connected || !mayChat) return Promise.resolve(false);
    return pendingChat?.send(text) || Promise.resolve(false);
  }

  // What the server says is open, kept so a decision can name the version it
  // was made against. The editor holds the branch somebody is drafting; this
  // is only the list, which the reader needs to draw cards and to decide.
  const openProposals = new Map();
  function proposalTip(id) {
    return openProposals.get(id)?.tip || "";
  }

  function receive(event) {
    if (event.type?.startsWith("proposal-") || (event.type === "error" && event.stale)) {
      if (event.type === "proposal-list") {
        openProposals.clear();
        for (const open of event.proposals || []) openProposals.set(open.id, open);
      } else if (event.type === "proposal-decided" && event.resolved) {
        openProposals.delete(event.proposal_id);
      }
      // Whoever asked for this decision is waiting on it. The reply used to be
      // an `accept` or `reject` frame; it is a decided proposal now, and a
      // refusal still arrives as an error carrying the same request.
      const waiting = event.request_id && suggestionDecisions.get(event.request_id);
      if (waiting) {
        suggestionDecisions.delete(event.request_id);
        if (event.type === "proposal-decided") waiting.resolve(event);
        else waiting.reject(new Error(event.message || "that decision was refused"));
      }
      editor?.receiveProposal?.(event);
      return;
    }

    const pendingSuggestion = event?.request_id && suggestionDecisions.get(event.request_id);
    if (pendingSuggestion && (event.type === "accept" || event.type === "reject")) {
      if (String(event.comment_id) === String(pendingSuggestion.commentId)) {
        suggestionDecisions.delete(event.request_id);
        pendingSuggestion.resolve(event);
      }
    } else if (pendingSuggestion && event.type === "error") {
      suggestionDecisions.delete(event.request_id);
      pendingSuggestion.reject(new Error(event.message || event.error || "Suggestion decision failed."));
    }
    if (annotations.receive(event)) return;
    if (event.type === "chat") {
      if (!liveChat.some((message) => message.id === event.id)) {
        liveChat = [...liveChat, event].slice(-200);
        if (!chatVisible && !pendingChat?.has(event.temp_id)) unreadChat = true;
      }
      if (event.temp_id) pendingChat?.acknowledge(event.temp_id, true);
      return;
    }
    if (event.type === "chat-ack") {
      pendingChat?.acknowledge(event.temp_id, true);
      return;
    }
    if (event.type === "hello") {
      outbox.reconcile(event.comments);
      annotations.replace(event.comments);
      commentsReady = true;
      reanchor();
      if (panel === "history" && !checkpoints.length) void loadHistory();
      return;
    }
    if (event.type === "submission-failed") {
      outbox.failed(event.temp_id, event.message);
      return;
    }
    if (event.type === "error") {
      if (event.temp_id && pendingChat?.has(event.temp_id)) {
        pendingChat.acknowledge(event.temp_id, false);
        toastProblem(event.message || "Chat message was rejected.");
        return;
      }
      // A backfill this browser sent is not a submission and never touched
      // the screen while it waited, so its failure -- somebody else's
      // backfill won the race, or the comment is gone -- is dropped quietly
      // rather than rolled back or reported.
      if (event.comment_id && pendingBackfill.delete(event.comment_id)) return;
      // A decision (accept or reject) that did not go through: the card was
      // marked busy, never resolved, and this clears that back off.
      const deciding = event.comment_id && comments.find((item) => item.id === event.comment_id);
      if (deciding?.deciding) {
        suggestions.clearDeciding(deciding);
        annotations.publish();
        // Stale is not a refusal to show as an error toast: the merge editor
        // it opens says what happened, and the suggestion stays pending
        // rather than being rolled back to nothing.
        if (event.stale) {
          void openStaleSuggestion(deciding);
          return;
        }
      }
      outbox.failed(event.temp_id, event.message);
      // Roll the optimistic row back.
      if (event.temp_id) {
        annotations.removePending(event.temp_id);
      }
      // A refused delete or resolve was applied optimistically before the
      // server had a say; the list is re-fetched so the optimistic change goes
      // back out.
      if (event.comment_id) {
        // The same headers every other call carries. Reading comments does not
        // ask for the marker, but it does ask who is reading: without the link
        // key a reader who arrived by one is a stranger here, and the catch
        // below would swallow the 404 and leave the list uncorrected.
        fetch(`/api/documents/${SLUG}/comments`, {
          headers: authHeaders(KEY),
        })
          .then((response) => response.json())
          .then((data) => receive({ type: "hello", comments: data.comments }))
          .catch(() => {});
      }
      toastProblem(event.message || "The server refused that change.");
      return;
    }

    // The shared document: the state of the session as it stands, one more
    // change to it, what the server has written, who else is in it, or where
    // their carets are. A reader receives all of this too -- that is how they
    // see the current text -- and sends none of it.
    if (event.type === "doc-state") {
      session
        ?.start(event)
        .then(() => {
          // Binding the initial Yjs tree does not produce an observed source
          // edit. Publication metadata must therefore begin only after this
          // state has been applied, rather than waiting for a later edit.
          if (mayEdit && publication.begin()) void refreshPublicationMetadata();
          return paintPreview();
        })
        .catch((error) => say(error.message || "could not open the document", true));
      peers = event.count || 1;
      return;
    }
    if (event.type === "doc-update") {
      session?.apply(event.update);
      return;
    }
    if (event.type === "doc-presence") {
      session?.applyPresence(event.update);
      return;
    }
    if (event.type === "doc-ack") {
      // The server has written this far. Relaying was never durability; this
      // is, and it is what the badge is allowed to speak from.
      session?.acknowledge(event.seq || 0);
      return;
    }
    if (event.type === "doc-peers") {
      peers = event.count || 1;
      return;
    }

    if (event.type === "published") {
      // A publish from outside the session -- the command line, or `sync` --
      // arrives as an ordinary update into the document everyone holds. All
      // that is left to do here is the title.
      if (event.title) {
        doc = { ...doc, title: event.title };
        document.title = `${event.title} · LibrePaper`;
      }
      return;
    }

    if (event.type === "publication-updated") {
      publication.announce({ publication_id: event.publication_id });
      return;
    }

    if (event.type === "anchor") {
      // The server's answer to this browser's own backfill, or somebody
      // else's: either way, a comment that had no anchor of record now does.
      // A comment_id nobody has -- deleted meanwhile -- is answered with
      // nothing to do.
      pendingBackfill.delete(event.comment_id);
      const comment = comments.find((item) => item.id === event.comment_id);
      if (!comment) return;
      comment.source = event.source;
      anchorAllSources(treeNow(), [comment]);
      applyAnchorFlags(comment);
      annotations.publish();
      applyHighlights();
      return;
    }
  }

  /* ----------------------------------------------------- reading and editing */

  // CodeMirror is a third of a megabyte, and most people who open a document
  // are here to read it. The editor component is fetched when one is actually
  // opened, so a reader never pays for it. The session is not the editor: a
  // reader joins it too, because that is where the text comes from.
  let Editor = $state(null);
  let MergeEditor = $state(null);
  let editor = $state(null);
  // Raw on purpose: the session is a bag of Yjs types and functions, and the
  // collaboration module hands the same object back in its callbacks, which
  // are compared to this by identity. A deep proxy would never be equal to it.
  let session = $state.raw(null);
  let editing = $state(false);
  // Which text the editor is bound to. Keying the component on this binds it
  // to the current main file when another file becomes main.
  let sourceEpoch = $state(0);
  let mayEdit = $state(false);
  let sourceFormat = $state("");
  let peers = $state(1);
  let participants = $state([]);
  let linked = $state(read(LINKED, false) === true);

  // Routine saving stays quiet; losing the connection still needs a warning,
  // and the warning has to say what is happening to the typing meanwhile.
  const connectionNote = $derived.by(() => {
    if (connected) return "";
    if (!mayEdit || !session) return "Reconnecting…";
    return persistence.local
      ? "Offline; changes are kept in this browser while reconnecting…"
      : "Offline; reconnecting…";
  });

  // A close is only worth interrupting when the work has reached neither this
  // browser's storage nor the server.
  const atRisk = $derived(Boolean(mayEdit && (persistence.localError || (persistence.pending && !persistence.local))));

  // What the editor has to say about an event -- a render that finished, a
  // download that failed, a lock with nowhere to go -- is said in a toast,
  // where it is set in readable type and goes on its own, rather than as a
  // badge on the bar, where it was the smallest text on the page and clipped
  // to an ellipsis on anything narrower than a desktop. The text is the
  // toast's id, so a line said again while it is still up is refreshed
  // rather than stacked under its twin.
  function say(text, isProblem = false) {
    if (!text) return;
    (isProblem ? toastProblem : toastSaid)(text, { id: `reader:${text}` });
  }

  // What the document is called, which is what the rendered page is titled.
  // The title it was published under wins; a document that never had one is
  // named by its own first heading, the way `publish` names one -- the *main*
  // file's first heading, since a chapter's heading names the chapter.
  async function headingOf(tree) {
    return doc.title || (await renderers.titleOf(tree)) || "Untitled";
  }

  // The whole connection story, on demand and with nothing to type: reach
  // the local app, and when it is there but has not allowed this site yet,
  // ask in a popup. What remains for the person is to have started the app
  // and to click Allow once; the status text says which when it fails.
  async function ensureLocalApp() {
    localConnectionError = "";
    localQuarto.configure({ project: SLUG, origin: location.origin, active: mayEdit });
    let status = await localQuarto.retry();
    if (["unreachable", "unauthorized", "reachable"].includes(status.state)) {
      try { status = status.state === "unreachable" ? await localQuarto.connectViaApp() : await localQuarto.pairViaApp(); }
      catch (error) { localConnectionError = error.message; showPanel("diagnostics"); return false; }
    }
    if (status.state !== "connected") {
      localConnectionError = status.instructions || "Local LibrePaper is unavailable.";
      showPanel("diagnostics");
      return false;
    }
    return true;
  }

  /* --------------------------------------------------------- the timeline */

  // What this document used to say, and when. The manifest is fetched when the
  // panel is opened and not before: a reader who never asks for the history
  // costs no request for it.
  // The comparison state and its async lifetimes live in the controller. The
  // aliases keep the existing panel and redline code readable while making
  // every value a focused controller getter rather than a Reader-owned bag.
  let mergeTarget = $state(null);
  // The checkpoint being shown in the document pane, whole -- its tree and its
  // texts -- or null for the document as it stands.

  // Whether this browser should be running Quarto's own live preview rather
  // than showing its own draft rendering: Quarto preview mode chosen, paired,
  // connected, editable, and not looking at history. `editing` (the source
  // pane) is not required -- an editor who has not opened it yet still gets
  // the live pane the moment they are able to edit.
  const quartoLiveActive = $derived(
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && localExecution && !buildPreferences.preset && (!buildPreferences.output || ["html", "pdf"].includes(buildPreferences.output)) && (buildPreferences.selection === "automatic" || (buildPreferences.backend === "local" && buildPreferences.tool === "quarto")) && mayEdit &&
      localAppStatus.state === "connected",
  );

  // Whether this browser should be showing a Typst document's chunks run by
  // Calepin on this computer rather than this browser's own Typst rendering:
  // the calepin mode chosen, paired, connected, the calepin command itself
  // found, editable, and not looking at history.
  const calepinActive = $derived(
    sourceFormat === "typst" && !typstHtmlPreview && localExecution && typstPreviewMode === "calepin" && !buildPreferences.preset && buildPreferences.backend === "local" && buildPreferences.tool === "calepin" && mayEdit &&
      localAppStatus.state === "connected" && localQuarto.calepinAvailable(),
  );

  async function localPreviewTreeNow() {
    const tree = treeNow();
    if (!Object.keys(tree.digests || {}).length) return tree;
    const { held, missing } = await gatherFigures(tree.digests);
    if (missing.length) throw new Error(`Local preview is missing ${missing.join(", ")}`);
    return { ...tree, assets: held.assets };
  }

  const quartoPreviewController = createLocalPreview({
    local: localQuarto,
    engine: "quarto",
    label: "Quarto",
    publish: (payload) => framePreview.publish(payload),
    treeNow: localPreviewTreeNow,
    entrypointOf: (tree) => tree.main,
    optionsOf: (tree) => {
      const context = quartoRenderContext(tree);
      // Managed Quarto preview serves a single HTML or PDF artifact. DOCX is
      // export-only and therefore never activates this controller.
      return { format: buildPreferences.output || (context.format === "revealjs" ? "revealjs" : "html"), profile: buildPreferences.profile || context.profiles[0] || null, parameters: buildPreferences.parameters || context.parameters };
    },
    // syncWorkspace writes the browser's tree to this binding. A remembered
    // external project binding may be stale and is not synchronized here.
    jobOf: () => ({ binding: localQuarto.HOSTED_BINDING }),
    isDisposed: () => readerDisposed,
    // A managed preview taking the pane invalidates whatever this browser
    // was rendering into it.
    onStartingChange: (value) => { if (value) navigationGeneration += 1; },
    onEnded: () => void paintPreview(),
  });

  const calepinPreviewController = createLocalPreview({
    local: localQuarto,
    engine: "calepin",
    label: "Calepin",
    publish: (payload) => framePreview.publish(payload),
    treeNow: localPreviewTreeNow,
    entrypointOf: (tree) => tree.main,
    optionsOf: () => ({ format: "pdf" }),
    jobOf: () => ({ binding: localQuarto.HOSTED_BINDING }),
    isDisposed: () => readerDisposed,
    onEnded: () => void paintPreview(),
  });

  // What each controller says about itself, read where the page used to read
  // its own copy.
  const quartoPreview = $derived(quartoPreviewController.state.session);
  const quartoPreviewStarting = $derived(quartoPreviewController.state.starting);
  // Whether the bridge says Quarto is currently re-rendering the live preview
  // -- carried on every response of the page route (200, 304, 404 alike),
  // never just the ones with new HTML. Drives the "Rendering…" badge and the
  // faster poll cadence while true.
  const quartoRendering = $derived(quartoPreviewController.state.rendering);
  const quartoPreviewError = $derived(quartoPreviewController.state.error);
  const calepinPreview = $derived(calepinPreviewController.state.session);
  const calepinPreviewStarting = $derived(calepinPreviewController.state.starting);
  const calepinRendering = $derived(calepinPreviewController.state.rendering);
  const calepinPreviewError = $derived(calepinPreviewController.state.error);

  // Drives the whole automatic mode for each engine: starts the managed
  // preview the moment its conditions are met, and tears it down (falling
  // back to the ordinary rendering, no retry) the moment any of them stop
  // holding.
  const localPreviewSwitches = createGeneration();
  $effect(() => {
    const quartoTarget = quartoLiveActive && previewMain;
    const calepinTarget = calepinActive && previewMain;
    const stale = localPreviewSwitches.begin();
    untrack(async () => {
      // Both engines use the same workspace. Release its old watcher before
      // asking the other engine to watch it, including during rapid switches.
      await Promise.all([
        quartoTarget ? Promise.resolve() : quartoPreviewController.reconcile(false),
        calepinTarget ? Promise.resolve() : calepinPreviewController.reconcile(false),
      ]);
      if (readerDisposed || stale()) return;
      if (quartoTarget) await quartoPreviewController.reconcile(quartoTarget);
      if (calepinTarget) await calepinPreviewController.reconcile(calepinTarget);
    });
  });

  // The profile and parameters, applied from Settings: kept for this
  // document, and the live preview -- which reads them only as it starts --
  // restarted so the page shows the new ones rather than the old until the
  // next reconnect. A preview still starting reads the options after its
  // workspace sync, so it picks them up on its own.
  async function applyRenderOptions(next) {
    quartoOptions = parseRenderOptions({ ...next, format: "default" });
    setBuildPreferences(updateBuildPreferences(buildScope(), "quarto", { profile: quartoOptions.profile || null, parameters: quartoOptions.parameters || {} }));
    if (!quartoLiveActive) return;
    await quartoPreviewController.stop();
    if (quartoLiveActive) await quartoPreviewController.start();
  }

  // The manifest, and whether the last read of it succeeded, belong to the
  // timeline; the comparison below owns everything about a selected version.
  const timeline = createTimeline({
    slug: SLUG,
    key: KEY,
    problem: (message) => toastProblem(message),
    onrestored: () => historySource.select(""),
    disposed: () => readerDisposed,
  });
  const checkpoints = $derived(timeline.state.checkpoints);
  const historyDurability = $derived(timeline.state.durability);
  const historyProblem = $derived(timeline.state.problem);
  // Selection is a user-interface fact, not proof that the source could be
  // read: a failed comparison reports itself in the workspace and cannot make
  // a clicked version appear unselected.
  const historySource = createHistorySource({
    checkpoint: sha => history.checkpoint(SLUG, sha, keyHeaders(KEY)),
    checkpoints: () => checkpoints,
    preferredPath: () => session?.paths?.get(openFile) || "",
    currentTree: () => {
      if (session && session.joined !== false) return session.tree();
      // A checkpoint URL can arrive before the initial collaborative state.
      // Wait for that state instead of comparing with a temporary empty tree.
      return (async () => {
        const deadline = Date.now() + 10000;
        while (!readerDisposed && (!session || session.joined === false) && Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 25));
        }
        if (readerDisposed || !session || session.joined === false) throw new Error("The current document is still connecting. Please retry when it has loaded.");
        return session.tree();
      })();
    },
  });
  const historySelectedSha = $derived(historySource.selected);
  let historyLiveVersion = $state(0);
  $effect(() => () => historySource.dispose());
  $effect(() => {
    if (panel !== "history" || historySource.selected) return;
    void historyLiveVersion;
    if (!session) return;
    untrack(() => { void historySource.select(""); });
  });
  let navigationGeneration = 0;
  // Which checkpoint the reader arrived asking for, out of the link somebody
  // sent them. Read once, because after that the panel is where the answer is.
  const ARRIVED_AT = new URLSearchParams(location.search).get("at") || "";
  // And which file, when the landing page's search found the project by one
  // of its files. Honoured once the directory has arrived, and once only.
  const ARRIVED_FILE = new URLSearchParams(location.search).get("file") || "";

  const loadHistory = () => timeline.load();

  // The timeline and the source workspace share one selection. Selecting a
  // version compares it with the current source and nothing else: the
  // document pane always shows the document as it stands.
  function viewPoint(sha) {
    const pending = historySource.select(sha);
    if (compact) showMobileView("source");
    return pending;
  }

  const restoreBusy = $derived(timeline.state.restoreBusy);
  const restoreMoved = $derived(timeline.state.restoreMoved);
  const restoreName = $derived(timeline.describe(timeline.state.restoreSha));
  // What restoring is about to discard, read here and handed over, so that
  // the question and the answer are asked of the same project.
  const projectSignature = () => {
    const tree = session?.tree?.();
    return JSON.stringify({ main: tree?.main || "", texts: tree?.texts || {}, files: tree?.files || {} });
  };
  const restoreCheckpoint = (sha) => timeline.ask(sha, projectSignature());
  const confirmRestore = () => timeline.confirm(projectSignature());
  const nameCheckpoint = (sha, given) => timeline.name(sha, given);

  // The link to a moment: the document's own link with the checkpoint on it.
  // A query rather than a fragment, because the fragment is where a link key
  // travels and the two must not have to share.
  function checkpointLink(sha) {
    const link = new URL(linkFor(SLUG));
    link.searchParams.set("at", sha);
    return link.href;
  }

  /* -------------------------------------------- the passage, then and now */

  // Where each orphaned comment's passage went, by comment id: the manifest
  // entry at which it stopped being found. This is what replaces "Needs
  // re-anchoring" on the card, which told the person who wrote the comment
  // nothing they did not already know.
  const passageTrace = createPassageTrace({
    slug: SLUG,
    key: KEY,
    now: () => ({ source: sourceGeneration, visible: docText }),
    loadCheckpoints: async () => {
      await loadHistory();
      return timeline.state.checkpoints;
    },
  });
  const went = $derived(passageTrace.state.went);
  const replacements = $derived(passageTrace.state.replacements);

  async function tracePassages() {
    if (docText === null || !session) return;
    return passageTrace.trace({ comments, tree: session.tree(), checkpoints });
  }

  // The document as a renderer takes it: every text in it, the figures by
  // digest, and which file is the document. A compiler given only the main
  // file produces the error a reader would otherwise be shown.
  //
  // Before a session arrives there is no source tree to render.
  // The file manager always operates on the live directory. In particular,
  // its list remains live while the document pane is showing a checkpoint, so
  // downloads must use the same source as that list.
  function liveTreeNow() {
    if (!session) return { main: "", texts: {}, digests: {} };
    const tree = session.tree();
    if (!tree.main) return tree;
    // CodeMirror owns the active Yjs binding and exposes the text it is
    // displaying. During a local transaction its view can be one tick ahead
    // of the directory observer, so use that current source for snapshots
    // taken by the preview scheduler.
    const activeText = typeof editing !== "undefined" && editing && typeof editor !== "undefined" && editor?.text?.(openFile) != null && (!openFile || session.paths?.get?.(openFile) === tree.main)
      ? String(editor.text(openFile))
      : null;
    return activeText == null ? tree : { ...tree, texts: { ...tree.texts, [tree.main]: activeText } };
  }

  function treeNow() {
    const tree = liveTreeNow();
    if (!previewMain || !(previewMain in tree.texts)) return tree;
    const activeText = editing && editor?.text?.(openFile) != null && session?.paths?.get(openFile) === previewMain
      ? String(editor.text(openFile)) : tree.texts[previewMain];
    return { ...tree, main: previewMain, texts: { ...tree.texts, [previewMain]: activeText } };
  }

  // Painting the preview is sending it to the frame: the draft is a document,
  // and a document belongs on the documents origin, not in this page. The
  // agent republishes its text from there, which re-anchors every comment
  // against what was just typed.
  let sourceGeneration = 0;
  const renderCoordinator = createRenderCoordinator({
    navigation: () => navigationGeneration,
    source: () => sourceGeneration,
    main: () => treeNow().main,
    // Called rather than passed: the renderer is built further down the file,
    // and this closure is what lets the coordinator exist before it.
    render: (ticket) => renderPreview(ticket),
  });

  /// Fetches the figures a tree's digests point to and reports which of the
  /// requested digests the server did not have, so a caller can decide
  /// whether a hole in the result is tolerable or should fail the whole call.
  async function gatherFigures(digests) {
    const held = await figures.gather(SLUG, digests, authHeaders(KEY));
    const missing = Object.keys(digests).filter(
      (path) => !Object.prototype.hasOwnProperty.call(held.assets, path),
    );
    return { held, missing };
  }

  // Outline reads the same live text as the editor. The revision dependency
  // is explicit because Yjs changes happen outside Svelte's normal tracking;
  // both local and remote source observers increment it.
  const outlineSource = $derived.by(() => {
    void outlineRevision;
    if (!session || !openFile) return "";
    const live = editing ? editor?.text?.(openFile) : null;
    if (live != null) return String(live);
    return String(session.textOf?.(openFile)?.toString?.() || "");
  });
  const outlineFormat = $derived.by(() => {
    const path = toolbarPath;
    return renderers.formatOf(path) || (path && path === session?.mainPath?.() ? sourceFormat : "");
  });
  const outlineHeadings = $derived.by(() => panel === "outline" && mayEdit
    ? extractOutline(outlineSource, outlineFormat) : []);
  // Keep one render in flight and coalesce requests into the latest tree.
  // This bounds the worker queue while typing, and keeps each LaTeX PDF
  // tied to the snapshot digest of the tree that produced it.
  let previewTimer = null;

  // What the last compile said. When it is painted is `diagnostics.js`'s
  // rule, and the wait it counts is from the keystroke rather than from the
  // render that noticed the error, so a slow render does not add its own
  // length to it. From clean to red at reading speed, from red to clean at
  // typing speed.
  let diagnostics = $state([]);
  const diagnosticsController = createRenderDiagnostics({
    active: () => editing,
    local: () => localAppDiagnostics,
    update: (list) => (diagnostics = list),
    deliver: (list) => editor?.setDiagnostics?.(list),
  });
  const diagnosticPainter = diagnosticsController.painter;
  // Whether the frame has ever shown a page. Until it has, a document that
  // does not compile has nothing to keep on the screen.
  let everPainted = false;
  // The same fact, in a form the markup may read. `everPainted` is a plain
  // variable on purpose -- it is written from inside a render and read
  // nowhere near one -- and a reader's "not yet rendered" is the one place
  // the question is asked from the template.
  let everPaintedShown = $state(false);

  const errorCount = $derived(diagnostics.filter((d) => d.severity !== "warning").length);
  const warningCount = $derived(diagnostics.length - errorCount);
  const counted = (n, thing) => `${n} ${thing}${n === 1 ? "" : "s"}`;
  const diagnosticBadge = $derived(
    errorCount && warningCount
      ? `${counted(errorCount, "error")}, ${counted(warningCount, "warning")}`
      : errorCount
        ? counted(errorCount, "error")
        : warningCount
          ? counted(warningCount, "warning")
          : "",
  );

  // What the rail says about a panel beyond its name: a fuller label for a
  // screen reader, counts to print under the icon, a dot to mark it. Written
  // here because this is where a diagnostic and a tracked change mean
  // something; the rail draws whatever it is handed.
  const badges = $derived({
    diagnostics: {
      says: diagnosticBadge ? `Diagnostics: ${diagnosticBadge}` : "",
      counts: diagnostics.length
        ? [
            errorCount ? { tone: "errors", of: errorCount } : null,
            warningCount ? { tone: "warnings", of: warningCount } : null,
          ].filter(Boolean)
        : [],
    },
    changes: {},
  });

  // A reader is told nothing about live editor diagnostics. They cannot fix
  // them, and a red badge for a typo in somebody else's editing session would
  // only interrupt reading. The source and the current transient page remain
  // available while the editor works.
  function bibliographyAnalyzed(result, request) {
    diagnosticsController.bibliography(result, request);
  }

  function diagnosticFile(item) {
    if (!(item.line > 0)) return null;
    const id = item.file ? session?.idOf(item.file) : session?.mainId();
    return files.find((file) => file.id === id && file.kind === "text") || null;
  }

  async function openDiagnostic(item) {
    const file = diagnosticFile(item);
    if (!file) return;
    openTheFile(file);
    await tick();
    editor?.openAt?.(file.id, item.line, item.column || 1);
  }

  // A document whose format is `html` is served into the frame as the page it
  // is, so that the scripts a notebook or a Quarto page carries actually run.
  // Painting over it from here would replace a live page with an inert copy of
  // itself -- `innerHTML` runs no scripts -- so a reader never paints one. It
  // still sees changes: the frame is served from the live document, so
  // reloading it is showing the current text, and `refreshFramedPage` below
  // does that when the text has actually moved and the typing has stopped.
  //
  // An editor previewing an HTML document still gets the inert preview, which
  // is what the source pane has always shown and what keystroke-speed feedback
  // requires; the two are different jobs.
  //
  // The documents origin shares no cookie with this one and holds no link key,
  // so on its own it has no way to tell who is asking. It serves a document's
  // bytes only to a frame whose URL carries a short-lived token, which this
  // page fetches over the channel that does carry an identity -- see
  // `navigateFrame`. That is what lets every HTML document be served as
  // itself, scripts and all, now that reading always takes a credential.
  //
  // And a checkpoint is always painted, whatever the format: the frame is
  // served from the live document, so there is nothing on the documents origin
  // that is the document as it was on Tuesday.
  const displayedFormat = $derived(
    sourceFormat,
  );
  // This browser-only choice affects the pane and is never published.
  const previewFormat = $derived(
    editing && ((displayedFormat === "typst" && typstOutput === "html") ||
      (displayedFormat === "latex" && latexOutput === "html")) ? "html" : displayedFormat,
  );
  const previewOutputKind = $derived(
    editing && ["markdown", "quarto"].includes(displayedFormat) && buildPreferences.output === "pdf"
      ? "pdf"
      : renderers.outputKind(previewFormat),
  );
  const typstHtmlPreview = $derived(displayedFormat === "typst" && editing && typstOutput === "html");
  const latexHtmlPreview = $derived(displayedFormat === "latex" && editing && latexOutput === "html");
  const paintsTheFrame = $derived(!publishedMode);

  /* -------------------------------------------------------------- LaTeX */

  // Paged source formats have no HTML to paint, so their frame is the PDF
  // viewer on the documents origin rather than the empty shell. Everything
  // else about the frame is the same: same origin, same CSP, same channel.
  const pdfOutput = $derived(previewOutputKind === "pdf");
  const framePath = $derived(publishedMode ? "" : (previewOutputKind === "pdf" ? "pdf" : "raw"));

  // How long the last compile took, and whether one is running now. Paged
  // formats expose the same short-lived loading state; the elapsed time is
  // especially useful for LaTeX, whose compiler can take seconds.
  const renderStatus = createRenderStatus();
  const renderState = renderStatus.state;

  // Which tool builds this document is the build settings' to own; what the
  // page keeps is the consequence -- which output each format is producing,
  // and which preview mode it is in.
  const buildUserId = $derived(me.provider && me.handle ? `${me.provider}:${me.handle}` : anonymousIdentity());
  const buildSettings = createBuildSettings({
    slug: SLUG,
    origin: location.origin,
    interrupt: (next) => {
      navigationGeneration += 1;
      renderers.cancelPreview({ keepWarm: true });
      latex.cancel();
      // A local preview survives only a choice that keeps using it. `next` is
      // null when the reader changed rather than the preference, and then
      // neither does.
      const local = next?.selection === "tool" && next?.backend === "local";
      if (sourceFormat === "quarto" && !(local && next.tool === "quarto")) void quartoPreviewController?.stop?.();
      if (sourceFormat === "typst" && !(local && next.tool === "calepin")) void calepinPreviewController?.stop?.();
    },
    paint: () => void paintPreview(),
  });
  const buildPreferences = $derived(buildSettings.state.preferences);
  const latexSettingsState = $derived(buildSettings.state.latex);
  const latexOutput = $derived(buildSettings.state.latexOutput);
  const typstOutput = $derived(buildSettings.state.typstOutput);
  const quartoPreviewMode = $derived(buildSettings.state.quartoPreviewMode);
  const typstPreviewMode = $derived(buildSettings.state.typstPreviewMode);
  const buildScope = () => buildSettings.scopeFor(buildUserId);
  $effect(() => {
    const format = sourceFormat;
    const user = buildUserId;
    if (format) untrack(() => buildSettings.follow({ format, user }));
  });
  const setBuildPreferences = (next) => buildSettings.choose(next, sourceFormat);
  const stopLatex = () => buildSettings.stopLatex();

  const configureLatex = (format) => buildSettings.configureLatex(format, session, buildUserId);

  // The most recent LaTeX compile result -- success or failure -- kept whole
  // for Diagnostics' "Compiled with" block and "Earlier attempts" list
  // (preserve both attempts' logs when a browser failure
  // led to a local attempt). Null for every other format.
  // Set by the "Compile now" button and read once, at the next
  // `renderers.render` call: the plainest way to ask for
  // `latex.compile(tree, {manual: true})` semantics without `renderers.js`
  // growing a LaTeX-specific parameter of its own. Cleared as soon as it is
  // read, so it applies to exactly the one compile it was meant for.
  let manualCompile = false;
  function compileNow() {
    manualCompile = true;
    paintPreview();
  }

  // Whether this browser is the one producing the pages. There is no chooser
  // and no "not ready yet" gate any more: an editor's browser initializes
  // the engine automatically the first time it is asked to compile (`paintPreview`
  // below), and the loading itself is what the Preview header reports.
  // Readers use the same source renderer as editors. A missing compiler is
  // reported as a tool requirement instead of opening a retained result.
  const compilesHere = $derived(renderers.compilerAvailable(sourceFormat));
  const unrendered = $derived(pdfOutput && !everPaintedShown && !renderState.failure);
  const failedBeforeRender = $derived(renderState.failure && !everPaintedShown);

  // A paged compile that is running says so, and says how long the last one
  // took once there has been one. Before the first, there is no honest number
  // to give. LaTeX has its own, richer status detail -- `LatexStatus.svelte`,
  // fed straight from `latex.subscribe` -- so this badge is Typst's alone.
  const compileBadge = $derived(
    !renderState.compiling ? "" : renderState.lastCompile ? `compiling… (last took ${renderState.lastCompile.toFixed(1)}s)` : "compiling…",
  );

  // Whether `LatexStatus` has anything to draw. It draws nothing while the
  // engine is idle, and the Preview header uses the phase for its compact
  // status control.
  let latexState = $state.raw(latex.status());
  const latexPhase = $derived(latexState.phase);
  $effect(() => latex.subscribe((next) => (latexState = next)));

  // Quarto preview mode chosen, but not yet paired with the local app on
  // this computer: the pane shows the draft, and Diagnostics explains why.
  const quartoNeedsLocalApp = $derived(
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && localExecution && mayEdit &&
      localAppStatus.state !== "connected",
  );
  // A read-only visitor cannot authorize the companion to receive a
  // workspace, but the browser's Markdown draft still leaves executable
  // Quarto cells unrun. Say why the page is a draft instead of implying that
  // the document has no complete preview available.
  const quartoReaderNeedsLocalTool = $derived(
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && !mayEdit,
  );

  // A PDF from Markdown or Quarto source is only ever produced by a tool on
  // this computer: this browser's own renderer makes HTML and nothing else.
  // Until that tool is running the PDF frame has nothing to show, and the
  // "not yet rendered" card would otherwise claim a render is under way that
  // can never finish -- which is exactly the state an author lands in by
  // choosing PDF output and then closing, or never installing, the local app.
  const pdfNeedsLocalTool = $derived(
    pdfOutput && ["markdown", "quarto"].includes(sourceFormat) && mayEdit &&
      (localAppStatus.state !== "connected" || (sourceFormat === "quarto" && !localExecution)),
  );
  // The gesture that is missing, told apart so the card offers one action
  // rather than a menu: Quarto has to be allowed to run this document's code
  // before anything else is worth suggesting.
  const pdfNeedsLocalExecution = $derived(pdfNeedsLocalTool && sourceFormat === "quarto" && !localExecution);

  // The way out for an author who wants to keep reading rather than install
  // anything: the browser's own HTML preview, which every one of these
  // formats has.
  function previewAsHtml() {
    setBuildPreferences(updateBuildPreferences(buildScope(), sourceFormat, { output: "html" }));
    void paintPreview();
  }

  // The same two banner cases, for a Typst document with Calepin preview
  // chosen: not yet connected to the local app, or connected but without the
  // calepin command itself.
  const typstNeedsLocalApp = $derived(
    sourceFormat === "typst" && !typstHtmlPreview && localExecution && typstPreviewMode === "calepin" && mayEdit &&
      localAppStatus.state !== "connected",
  );
  const typstNeedsCalepinCommand = $derived(
    sourceFormat === "typst" && !typstHtmlPreview && localExecution && typstPreviewMode === "calepin" && mayEdit &&
      localAppStatus.state === "connected" && !localQuarto.calepinAvailable(),
  );

  const localAppDiagnostics = $derived([
    localAppStatus.state !== "connected" ? localConnectionError : "",
    quartoNeedsLocalApp && !localConnectionError ? "Quarto preview needs the local LibrePaper app. Connect to execute code chunks." : "",
    typstNeedsLocalApp && !localConnectionError ? "Calepin preview needs the local LibrePaper app." : "",
    typstNeedsCalepinCommand ? "Calepin preview needs the calepin command on this computer." : "",
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && localExecution ? quartoPreviewError : "",
    sourceFormat === "typst" && typstPreviewMode === "calepin" && localExecution ? calepinPreviewError : "",
  ].filter(Boolean).map((message) => ({ severity: "error", message, source: "local-app" })));
  $effect(() => {
    void localAppDiagnostics;
    void editing;
    untrack(() => diagnosticsController.refresh());
  });

  // The kind of the payload the frame was last handed -- "pdf", "html", or
  // "" since the last navigation. `framePreview.preview()` knows the same,
  // but not reactively, and the File menu's download items follow this.
  let deliveredKind = $state("");
  // What the PDF frame is showing, as the frame itself reports it: whether a
  // page is drawn at all, the zoom mode in force, and the scale it came out
  // at. The header's controls are drawn from this rather than from what was
  // last asked for, so the percentage beside them is the one on screen -- a
  // frame too narrow for "150%" draws it smaller, and says so.
  //
  // A format with no paged frame never sends it, which is exactly how the
  // controls stay out of the header for flowing HTML.
  let viewerView = $state(null);
  let viewerTool = $state("select");
  let activeSynctex = null;
  framePreview = createFramePreview({
    slug: SLUG,
    getDocsOrigin: () => docsOrigin,
    framePath: () => framePath,
    setSource: (source) => (frameSrc = source),
    send: tell,
    onNavigate: () => {
      frameReady = false;
      // A new frame has nothing drawn in it and no tool set; it says so once
      // it loads. Until then the controls go away rather than describing the
      // document that just left.
      viewerView = null;
      viewerTool = "select";
      renderCoordinator.invalidate();
      deliveredKind = "";
      renderStatus.clearDocx();
      activeSynctex = null;
      frameOverlays.resetAnnotations();
    },
    onDelivered: (payload) => {
      deliveredKind = payload.kind;
      everPainted = true;
      everPaintedShown = true;
    },
  });

  const navigateFrame = (force = false) => framePreview.navigate(force);
  const replayPreview = () => paintsTheFrame && framePreview.replay();
  const frameLoaded = () => {
    const first = framePreview.markReady();
    frameReady = true;
    if (first && !publishedMode) replayPreview();
  };

  // The kind of frame follows the tree being displayed, including a
  // historical tree. This effect is also what navigates when the live main
  // file changes from Markdown/Typst/HTML to LaTeX or back.
  $effect(() => {
    void docsOrigin;
    void framePath;
    untrack(() => navigateFrame());
  });

  function refreshFramedPage() {
    if (paintsTheFrame || !session || !docsOrigin) return;
    framePreview.refresh(session.text.toString());
  }

  const previewBusy = $derived(Boolean(
    (sourceFormat === "latex" && !latexHtmlPreview && ["loading", "compiling", "browser-biber"].includes(latexPhase))
      || compileBadge || quartoPreviewStarting || quartoRendering || calepinRendering,
  ));
  const previewProblem = $derived(Boolean(
    (sourceFormat === "latex" && (latexHtmlPreview ? renderState.failure : latexPhase === "failed")) || quartoPreviewError || (!typstHtmlPreview && calepinPreviewError)
      || quartoNeedsLocalApp || typstNeedsLocalApp || typstNeedsCalepinCommand,
  ));
  const previewStatusLabel = $derived(
    previewProblem ? "Preview needs attention"
      : previewBusy ? (sourceFormat === "quarto" ? "Rendering Quarto" : sourceFormat === "typst" ? "Rendering Typst" : "Compiling")
      : !everPaintedShown && !renderState.failure ? "Rendering from source" : "",
  );

  let previousConnected = null;
  $effect(() => {
    if (previousConnected === false && connected) toastDone("Connection restored", { id: "reader:connection-restored" });
    previousConnected = connected;
  });

  async function paintPreview() {
    return renderCoordinator.request();
  }

  // The render itself is `reader/preview-render.js`. What stays here is the
  // page's answer to "what is true now", which that render asks for again
  // after every await, and the few effects it cannot perform itself.
  const renderPreview = createPreviewRenderer({
    slug: SLUG,
    coordinator: renderCoordinator,
    status: renderStatus,
    framePreview: { publish: (payload) => framePreview.publish(payload) },
    diagnostics: diagnosticPainter,
    renderers,
    snapshotDigest,
    diagnosticContext,
    parseSynctex,
    facts: () => ({
      disposed: readerDisposed,
      navigation: navigationGeneration,
      source: sourceGeneration,
      everPainted,
      editing,
      format: sourceFormat,
      hasSession: Boolean(session),
      paintsTheFrame,
      latexOutput,
      typstOutput,
      buildPreferences,
      localExecution,
      // A local tool's own preview owns the pane while it is running or
      // starting. Answered here because only this page knows which of the two
      // local previews a given format would be using.
      livePreviewOwnsPane:
        (sourceFormat === "quarto" && quartoLiveActive && Boolean(quartoPreview || quartoPreviewStarting))
        || (sourceFormat === "typst" && calepinActive && Boolean(calepinPreview || calepinPreviewStarting)),
    }),
    tree: () => treeNow(),
    gather: (digests) => gatherFigures(digests),
    heading: (tree) => headingOf(tree),
    takeManual: () => {
      const manual = manualCompile;
      manualCompile = false;
      return manual;
    },
    onsynctex: (parsed) => { activeSynctex = parsed; },
    send: (message) => tell(message),
    clearDebounce: () => {
      clearTimeout(previewTimer);
      previewTimer = null;
    },
    refreshFrame: () => refreshFramedPage(),
    debug: (...parts) => {
      try {
        if (localStorage.getItem("librepaper-latex-debug")) console.debug(...parts);
      } catch { /* diagnostics are optional */ }
    },
  });

  // A source that is not actively being edited in this browser waits for a
  // pause, so remote changes and preview-only views do not render every word.
  const PASSIVE_PREVIEW_DEBOUNCE = 1000;

  // A publication records both the complete source tree and the HTML renderer
  // identity. `document.sha` is only the main source file's digest, so it
  // cannot answer whether a multi-file project is still the one published.
  const refreshPublicationStatus = (against) => publication.refreshStatus(against);
  const refreshPublicationMetadata = () => publication.refreshMetadata();

  function sourceChanged() {
    if (readerDisposed) return;
    sourceGeneration += 1;
    historyLiveVersion += 1;
    if (mayEdit && publishedPublication?.source_sha256) {
      void refreshPublicationStatus();
    }
    outlineRevision += 1;
    if (typeof quartoLiveActive !== "undefined" && quartoLiveActive && typeof quartoPreview !== "undefined" && (quartoPreview || quartoPreviewStarting)) {
      clearTimeout(quartoLiveSyncTimer);
      quartoLiveSyncTimer = setTimeout(() => void quartoPreviewController.sync(), 500);
    }
    if (typeof calepinActive !== "undefined" && calepinActive && typeof calepinPreview !== "undefined" && calepinPreview) {
      clearTimeout(calepinSyncTimer);
      calepinSyncTimer = setTimeout(() => void calepinPreviewController.sync(), 500);
    }
    if (sourceFormat === "quarto" && session) {
      const main = previewMain || session.mainPath() || "main.qmd";
      const id = session.idOf?.(main) || session.mainId();
      const parsed = quarto.parseQuarto(session.textOf(id)?.toString?.() || session.text.toString(), { path: main });
      const nextDiagnostics = parsed.diagnostics.map((item) => diagnosticContext(item, { main, texts: { [main]: parsed.source } }, ""));
      const diagnosticsGeneration = sourceGeneration;
      // Yjs can notify this observer from inside CodeMirror's update
      // listener. Editor.setDiagnostics dispatches another update, which
      // CodeMirror rejects while the first one is still in progress. Source
      // invalidation stays synchronous, but the editor repaint waits until
      // the current transaction has returned and is skipped if newer text
      // arrived before then.
      queueMicrotask(() => {
        if (readerDisposed || diagnosticsGeneration !== sourceGeneration) return;
        renderDiagnostics = nextDiagnostics;
        paintCombinedDiagnostics();
      });
    }
    // The keystroke, which is what the diagnostic wait is measured from.
    diagnosticPainter.typed();
    // Fast formats use a short bounded preview cadence. Only LaTeX owns its
    // longer compiler debounce; postponing this timer for every Typst
    // keystroke would starve the preview indefinitely.
    if (editing && sourceFormat !== "latex" && previewTimer !== null) return;
    clearTimeout(previewTimer);
    // A LaTeX compile takes seconds, so it waits for the source to be quiet
    // for longer -- `latex.DEBOUNCE`, which is that module's number and not
    // one written twice. A reader watching somebody else type waits longer
    // still, and the longer of the two wins.
    const wait =
      sourceFormat === "latex"
        ? Math.max(latex.DEBOUNCE, editing ? 0 : PASSIVE_PREVIEW_DEBOUNCE)
        : editing
          ? 50
          : PASSIVE_PREVIEW_DEBOUNCE;
    previewTimer = setTimeout(paintPreview, wait);
  }

  /* ------------------------------------------------------- keeping in step */

  let stepTimer = null;
  function outlineTextChanged() {
    // CodeMirror has applied the local or remote transaction by this point.
    // The Yjs source observer can run before its caret has been mapped.
    outlineActiveFrom = editor?.caret?.() ?? null;
  }
  function outlineCaretChanged() {
    outlineTextChanged();
    followCaret();
  }

  function followCaret() {
    if (!linked || !editing || docText === null) return;
    clearTimeout(stepTimer);
    stepTimer = setTimeout(() => {
      // The format of the file being edited, which is not always the
      // document's: a .bib beside a .tex has comments of its own kind, and
      // stripping .tex comments out of it would blank the wrong runs.
      const format = renderers.formatOf(session?.paths?.get(openFile) || "") || sourceFormat;
      if (format === "latex" && typeof activeSynctex !== "undefined" && activeSynctex) {
        const path = session?.paths?.get(openFile) || "";
        const precise = activeSynctex.forward(path, synctexLineAt(editor.text(), editor.caret()));
        if (precise) {
          tell({ type: "synctex-locate", page: precise.page, x: precise.x, y: precise.y });
          return;
        }
      }
      const place = sync.documentPlaceFor(editor.text(), editor.caret(), docText, format);
      if (place) {
        tell({ type: "locate", start: place.at, length: place.length });
        return;
      }
    }, 120);
  }

  function followDocumentClick(offset, pdfPoint = null) {
    if (!linked || !editing || docText === null || !editor) return;
    // The words clicked in the document may belong to any file: a reader
    // clicking a paragraph of chapter three is asking for chapter three, not
    // for the file that happens to be on screen. So the whole directory is
    // searched, and the file the words are in is opened.
    const tree = treeNow();
    if (followPdfClick(pdfPoint)) return;
    const found = sync.sourcePlaceInTree(docText, offset, tree, {
      open: session?.paths?.get(openFile) || "",
      formatOf: renderers.formatOf,
    });
    if (!found) return;
    const id = session.idOf(found.path);
    if (id) {
      ws.openFile = id;
      editor.goToIn(id, found.at);
    } else {
      editor.goTo(found.at);
    }
  }

  function followPdfClick(pdfPoint) {
    if (!linked || !editing || !editor || sourceFormat !== "latex" ||
        typeof activeSynctex === "undefined" || !activeSynctex || !pdfPoint) return false;
    const precise = activeSynctex.inverse(Number(pdfPoint.page), Number(pdfPoint.x), Number(pdfPoint.y));
    const id = precise && session.idOf(precise.path);
    if (!id) return false;
    ws.openFile = id;
    editor.openAt(id, precise.line, 1);
    return true;
  }

  function setLinked(on) {
    linked = on;
    write(LINKED, on);
  }

  /* ------------------------------------------------------------------ panes */

  // How the window is divided, which side the source is on, which keys the
  // editor answers to, which panel is open and how wide the panes are: one
  // reader's habits rather than anything about a document. The preferences
  // own them, and own writing them down -- see `reader/preferences.svelte.js`
  // for why those are one act and not two.
  const preferences = createPreferences();
  const prefs = preferences.state;
  const layout = $derived(prefs.layout);
  const sourceSide = $derived(prefs.sourceSide);
  const keys = $derived(prefs.keys);
  const sizes = $derived(prefs.sizes);

  // Settings are not a column: they open as a dialog from the navbar menu, so
  // the sidebar stays what its icons offer. Every entry point opens the same
  // dialog on the category it is about. A browser that last left the column
  // on the old settings tab falls through to the files below.
  let settingsOpen = $state(false);
  let settingsCategory = $state("editor");
  function openSettings(category = "editor") {
    settingsCategory = category;
    settingsOpen = true;
  }

  // The column at the left, and what is in it: the files, the comments or the
  // history, or "" for closed. One value rather than a switch per panel,
  // because the column shows one thing at a time. An editor's first visit
  // opens on the files -- the shape of the project is what a project space
  // starts with -- and every visit after that opens where they left it.
  //
  // Somebody who came by a read or a comment link is shown the document and
  // its comments first. History is available to compare review rounds; files
  // and editor settings remain in the editor workspace.
  const panel = $derived(prefs.panel);
  const chatVisible = $derived(panel === "collaboration" && prefs.collaborationTab === "chat" && shown.comments);
  $effect(() => { if (chatVisible) unreadChat = false; });
  // Mount panels on their first visit and retain them across view changes.
  // This preserves scroll positions, expanded folders and unsent chat drafts.
  let visitedPanels = $state([]);
  $effect(() => {
    if (settled && shown.comments && panel && !visitedPanels.includes(panel)) visitedPanels = [...visitedPanels, panel];
  });
  // Annotations waiting on the server are offered back from the collaboration
  // panel, so that panel has to exist as soon as there are any -- including
  // for a reader who has not opened it yet, whose unsent comment would
  // otherwise have nowhere to be recovered from.
  const mountedPanels = $derived(
    unconfirmed.length && !visitedPanels.includes("collaboration")
      ? [...visitedPanels, "collaboration"]
      : visitedPanels,
  );
  const mobileView = $derived(prefs.mobileView);
  const preferredPane = $derived(prefs.preferredPane);
  function showMobileView(view) {
    if (view === "source" && !editing && panel !== "history") view = "document";
    preferences.setMobileView(view);
    if (view !== "sidebar") prefs.preferredPane = view;
    if (!compact && view !== "sidebar" && layout !== "split") preferences.setLayout(view);
    if (view === "sidebar" && !panel) showPanel(home);
  }
  function selectPanel(name) {
    const next = panel === name && (!compact || activeMobileView === "sidebar") ? "" : name;
    showPanel(next);
    if (width <= 760) showMobileView(next ? "sidebar" : "document");
  }
  // The tabs this browser is offered. Which reader may see which panel is
  // written with the panels, in lib/panels.js; this only says who is asking.
  const tabs = $derived(tabsFor({ mayEdit, editing, canSeeSharing, canPublish }));
  // Where the column goes back to when what it showed is taken away: the
  // files for an editor, the comments for everybody else.
  const home = $derived(mayEdit ? "files" : "collaboration");
  // Whether the document has said who this browser is. Until it has, the
  // column is drawn empty rather than as one audience's and then the other's.
  let settled = $state(false);

  // Kept current for the background work that consults it.
  $effect(() => {
    const track = () => (hidden = document.visibilityState === "hidden");
    document.addEventListener("visibilitychange", track);
    return () => document.removeEventListener("visibilitychange", track);
  });

  // The history panel refreshes itself while it is open. It polls rather than
  // listening because a checkpoint is written by the server and there is no
  // room event that says so.
  //
  // Only while the panel is open, only while this tab is the one being looked
  // at, and only while the room socket is up: a backgrounded tab polling a
  // list nobody can see is a request every fifteen seconds for as long as the
  // tab exists, and a disconnected one is a request that is going to fail
  // anyway. Coming back to the tab refreshes once, immediately, so what is on
  // screen is current rather than up to fifteen seconds stale.
  $effect(() => {
    if (panel !== "history" || !connected || hidden) return;
    void loadHistory();
    const timer = setInterval(() => void loadHistory(), 15000);
    return () => clearInterval(timer);
  });

  // Showing a panel; "" closes the column. Opening the timeline is what
  // fetches the manifest; leaving it drops the comparison, which is all the
  // panel ever held -- the document pane shows the current document
  // throughout.
  function showPanel(name, remembered = true) {
    if (panel === "history" && name !== "history") historySource.close();
    // Reopening starts at the current source.
    const enteringHistory = name === "history" && panel !== "history";
    preferences.setPanel(name, remembered);
    if (enteringHistory) void historySource.select("");
    // Changes is a source-review queue. On desktop, opening it establishes a
    // source-only workspace; on compact screens the sidebar remains visible
    // until the reviewer chooses a row, which then moves to source.
    if (name === "changes" && mayEdit && !compact) {
      void startEditing().then(() => {
        if (panel === "changes") showMobileView("source");
      });
    }
    // Not remembered: a panel opening the sidebar on a narrow screen is what
    // this visit is doing, not what the reader asked to come back to.
    if (compact) prefs.mobileView = name ? "sidebar" : "document";
    return name === "history" ? loadHistory() : Promise.resolve();
  }

  // The source and the document are kept as a share of what they have between
  // them; the comment column is kept in pixels. Two units because they are two
  // different kinds of pane: half a window stays half when the window changes,
  // and a comment card wants the same readable width whatever the screen is.
  let guide = $state({ shown: false, left: 0, held: false });
  let grabbing = $state(false);
  // The width the panes are divided out of. Bound rather than read when
  // something happens to ask: it is what every size below is measured against,
  // and a measurement taken once is a layout that is right until the window
  // moves.
  let width = $state(innerWidth);
  // And what a rem is worth, since every limit in panes.js is one. It moves
  // when the reader changes their text size or zooms, which is a resize; it is
  // held here rather than read where it is used so that what depends on it is
  // worked out again when it does.
  let unit = $state(measure());
  const compact = $derived(width <= 760);
  const activeMobileView = $derived(panel === "history" && mobileView === "document" ? "source"
    : mobileView === "source" && !editing && panel !== "history" ? "document"
    : mobileView === "sidebar" && !panel ? "document" : mobileView);
  const splitTight = $derived(width < px(ACTIVITY_WIDTH, unit) + px(PANES.editor.min, unit)
    + px(DOCUMENT_MIN, unit) + px(GRIP, unit)
    + (panel ? px(PANES.sidebar.min, unit) + px(GRIP, unit) : 0));
  const effectiveLayout = $derived(!compact && panel === "history"
    ? "source"
    : compact
    ? (activeMobileView === "source" ? "source" : "document")
    : layout === "split" && splitTight ? preferredPane : layout);

  // What every measurement below is made against.
  // The column holds one of three things -- the files, the comments or the
  // timeline -- so what the layout needs to know is whether it is there, not
  // which of them is in it.
  const panes = $derived({
    layout: effectiveLayout,
    comments: Boolean(panel),
    editing: editing || panel === "history",
    sourceSide,
    sizes,
    width,
    // Passed so that a change of text size reaches everything measured in rem,
    // which is every limit there is: `px` holds the measurement itself, and
    // what is never seen to read it is never worked out again.
    unit,
  });
  const shown = $derived(compact ? {
    source: (editing || panel === "history") && activeMobileView === "source",
    document: activeMobileView === "document",
    comments: activeMobileView === "sidebar",
  } : showing(panes));

  // What the reader asked for is kept; what fits is worked out again every
  // time it is needed. Writing the fitted size back would make a narrow window
  // permanent -- drag the window in and the split is squeezed, drag it out and
  // it stays squeezed, because what was asked for is gone.
  function setSize(pane, size) {
    preferences.setSize(pane, clamp(pane, panes, size));
  }

  // The three arrangements, in the order the button walks through them. The
  // icon is the one it is in rather than the one it is going to: the button is
  // as much a statement of where you are as a way of leaving.
  const ARRANGEMENTS = {
    split: { icon: "columns-2", says: "Split", next: "document" },
    source: { icon: "panel-left", says: "Source", next: "split" },
    document: { icon: "file-text", says: "Preview", next: "source" },
  };

  let editAvailability = $state({});
  const editGroups = [
    [["undo", "Undo"], ["redo", "Redo"]],
    [["cut", "Cut"], ["copy", "Copy"], ["paste", "Paste"]],
    [["select-all", "Select All"]],
    [["find", "Find…"], ["replace", "Replace…"]],
  ];
  function chooseEditCommand(command) {
    showMobileView("source");
    const target = editor;
    // Restore source focus after the menu finishes closing.
    setTimeout(async () => {
      if (editor !== target || !mayEdit) return;
      try { await target?.editCommand(command); }
      catch (error) { toastProblem(error.message || "Clipboard access failed. Try the keyboard shortcut."); }
    }, 0);
  }

  function cycleLayout() {
    preferences.setLayout(ARRANGEMENTS[layout].next);
    if (compact) showMobileView(layout === "source" ? "source" : "document");
  }

  const putSourceOn = (side) => preferences.setSourceSide(side);

  const setKeys = (next) => preferences.setKeys(next);

  // What ":q" in Vim mode asks for: the document alone, set directly rather
  // than reached by cycling, and remembered like any other choice of layout.
  const showDocumentAlone = () => preferences.setLayout("document");

  // Everything the layout menu offers, named by what was chosen. The menu
  // reports the value of the line rather than each line calling back, so this
  // is the one place those names are read.
  function chose(what) {
    if (what.startsWith("layout-")) {
      preferences.setLayout(what.slice(7));
      if (compact) showMobileView(layout === "source" ? "source" : "document");
      return;
    }
    if (what === "side-left" || what === "side-right") return putSourceOn(what.slice(5));
    if (what.startsWith("ratio-")) return setSize(PANES.editor, Number(what.slice(6)));
    if (what === "linked") return setLinked(!linked);
  }

  function chooseToolCommand(value) {
    if (value === "settings") return openSettings("editor");
    if (value === "compile") return compileNow();
  }

  function chooseViewCommand(value) {
    if (value === "format-pdf") {
      if (displayedFormat === "latex") return void setLatexOutput("pdf");
      if (displayedFormat === "typst") return void setTypstOutput("pdf");
    }
    if (value === "format-html") {
      if (displayedFormat === "latex") return void setLatexOutput("html");
      if (displayedFormat === "typst") return void setTypstOutput("html");
    }
    if (value.startsWith("engine-latex-")) {
      const engine = value.slice("engine-latex-".length);
      if (["auto", "pdflatex", "xelatex", "lualatex"].includes(engine)) setBuildPreferences(updateBuildPreferences(buildScope(), "latex", { selection: "tool", backend: "browser", tool: "tex", engine }));
      return;
    }
    if (value === "preview-latex-pdf") return void setLatexOutput("pdf");
    if (value === "preview-latex-html") return void setLatexOutput("html");
    if (value === "preview-file") return previewThisFile();
    if (value === "local-execution") return toggleLocalExecution();
    if (value === "preview-typst-pdf") return void setTypstOutput("pdf");
    if (value === "preview-typst-html") return void setTypstOutput("html");
    return chose(value);
  }

  // The File menu. Its first three items are what the Files panel's toolbar
  // does, reached without first switching layouts and opening the panel; the
  // rest open the panels a person looks for under File. The Files panel is
  // mounted on its first visit and draws the name field it focuses, so the
  // panel is opened and the DOM given a turn before the panel is asked.
  const FILE_COMMANDS = ["new-file", "new-folder", "upload", "offline", "download-pdf", "download-html", "download-docx", "download", "share", "history"];
  async function chooseFileCommand(value) {
    if (value === "offline") return makeAvailableOffline();
    if (value === "download") return downloadTree();
    if (value === "download-docx") return downloadDocx();
    if (value === "download-pdf" || value === "download-html") return downloadRendering(value.slice(9));
    if (value === "share" || value === "history") return openPanel(value);
    if (!["new-file", "new-folder", "upload"].includes(value)) return;
    await openPanel("files");
    await tick();
    if (value === "upload") fileList?.choose();
    else fileList?.start(value === "new-file" ? "file" : "folder");
  }

  // Which of the rendering downloads the File menu can honour: the one for
  // this document's output kind, once there is something to hand over.
  const downloads = $derived(availableDownloads({
    outputKind: renderers.outputKind(displayedFormat),
    deliveredKind,
    displayedFormat,
  }));

  const docxDownload = $derived(renderState.docxArtifact && renderState.docxArtifact.source === sourceGeneration && renderState.docxArtifact.navigation === navigationGeneration ? renderState.docxArtifact : null);

  function downloadDocx() {
    if (!docxDownload) return;
    const file = docxFile(docxDownload.bytes, SLUG);
    saveBlob(file.blob, file.name);
  }

  // Exports read the transient result currently held by this tab. LibrePaper
  // never fetches or uploads a generated result for an export.
  async function downloadRendering(kind) {
    try {
      const authored = displayedFormat === "html";
      const tree = authored ? treeNow() : null;
      const file = await renderingFile({
        kind,
        slug: SLUG,
        preview: framePreview.preview(),
        authored,
        authoredHtml: authored ? tree.texts?.[tree.main] ?? null : null,
      });
      saveBlob(file.blob, file.name);
    } catch (error) {
      say(error.message || "Could not download the rendering.", true);
    }
  }

  const publishCurrent = () => publication.publish();
  // Open a panel from the menu: unlike the activity bar, choosing an item
  // that is already open leaves it open rather than closing the column.
  function openPanel(name) {
    if (!visitedPanels.includes(name)) visitedPanels = [...visitedPanels, name];
    const opening = showPanel(name);
    if (width <= 760) showMobileView("sidebar");
    return opening;
  }

  // The one menu a narrow screen has stands in for all of them.
  function chooseCompactCommand(value) {
    if (FILE_COMMANDS.includes(value)) return void chooseFileCommand(value);
    if (value.startsWith("preview-") || value.startsWith("layout-") || value.startsWith("side-") || value.startsWith("ratio-") || value === "linked" || value === "local-execution") return chooseViewCommand(value);
    return chooseToolCommand(value);
  }

  /* ------------------------------------------------------------------- boot */

  /* ------------------------------------------------------------- the files */

  // The directory as the list shows it, and where everyone's caret is. The
  // workspace owns all of it; what is read here is read through it, and the
  // aliases below exist so that reading it looks like reading a local.
  const workspace = createWorkspace({
    slug: SLUG,
    key: KEY,
    arrivedFile: ARRIVED_FILE,
    say,
    paint: () => paintPreview(),
    retarget: () => updatePreviewTarget(),
    onarrived: (file) => openTheFile(file),
  });
  const ws = workspace.state;
  const files = $derived(ws.files);
  const folders = $derived(ws.folders);
  const openFile = $derived(ws.openFile);
  const peersByFile = $derived(ws.peersByFile);
  const rules = $derived(ws.rules);
  const shownFigure = $derived(ws.figure);
  const figureUrl = $derived(ws.figureUrl);
  let outlineActiveFrom = $state(null);
  let outlineRevision = $state(0);
  let previewMain = $state("");
  // Pin an explicitly previewed file by identity so renames keep it selected.
  // Empty means follow the shared main file; opening an include never pins it.
  let previewFile = $state("");
  const toolbarPath = $derived(files.find((file) => file.id === openFile)?.path || "");
  const editorFormat = $derived(renderers.formatOf(toolbarPath) || sourceFormat);
  const canPreviewFile = $derived(files.some((file) =>
    file.id === openFile && file.kind === "text" && Boolean(renderers.formatOf(file.path))));
  loadConfig()
    .then((answer) => {
      ws.rules = answer || {};
      if (typeof answer?.latexMirror === "string") latex.at(answer.latexMirror);
    })
    .catch(() => {
      /* the server checks every path again; this only explains it sooner */
    });

  // Reading the directory into the list is the workspace's; what is left
  // here is the preview target, which is a rendering question rather than a
  // directory one.
  const refreshFiles = () => workspace.refresh();

  function updatePreviewTarget() {
    if (!session) return;
    const selected = files.find((file) => file.id === previewFile);
    if (!selected || selected.kind !== "text" || !renderers.formatOf(selected.path)) previewFile = "";
    const path = selected?.kind === "text" && renderers.formatOf(selected.path)
      ? selected.path : session.mainPath();
    const format = renderers.formatOf(path);
    if (!format || (previewMain === path && sourceFormat === format)) return;
    const previous = previewMain || sourceFormat;
    previewMain = path;
    sourceFormat = format;
    configureLatex(format);
    renderers.warm(format);
    if (["quarto", "typst", "markdown"].includes(format)) pairLocalQuarto();
    if (!previous) return;
    // Invalidate both running compiles and replayed pages, even when the
    // two selected files use the same renderer or both produce PDFs.
    navigationGeneration += 1;
    const mine = navigationGeneration;
    renderCoordinator.invalidate();
    framePreview.clear();
    everPainted = false;
    everPaintedShown = false;
    renderStatus.resetFailure();
    if (docsOrigin) navigateFrame(true);
    void tick().then(() => {
      if (!readerDisposed && mine === navigationGeneration) void paintPreview();
    });
  }

  function previewThisFile() {
    if (!canPreviewFile) return;
    previewFile = openFile;
    updatePreviewTarget();
    if (!compact && layout === "source") preferences.setLayout("split");
    if (compact) showMobileView("document");
  }

  // A file added, renamed, removed, or made the main one: the list is redrawn
  // and the document is rendered again, because every one of those changes
  // what a compiler would produce. Main-text edits also arrive through the
  // source watcher; skip them here so one Yjs transaction does not schedule
  // the same diagnostics and preview twice. Included-file edits still need a
  // source change of their own because they can alter a Quarto render without
  // changing the main Y.Text.
  function filesChanged(events, active = session) {
    if (!active || active !== session) return;
    // Nested text edits change the preview, but not the file list. An event
    // names the container it happened to by id, so that is what the directory
    // is compared against -- not the handle this component is holding.
    const directory = active.files?.id;
    if (!Array.isArray(events) || events.some((event) => event.target === directory)) refreshFiles();
    if (needsSourceRefresh(events, active.text)) sourceChanged();
  }

  function refreshPeers() {
    workspace.refreshPeers();
    participants = ws.participants;
  }

  function openTheFile(file) {
    if (openFile !== file.id) outlineActiveFrom = null;
    workspace.show(file);
    // Choosing a file is asking to see it, so an arrangement with no source
    // pane makes room for one. Remembered like any other choice of layout.
    if (mayEdit && !compact && layout === "document") preferences.setLayout("split");
    if (mayEdit) showMobileView("source");
  }

  // An outline entry is a source navigation command. It may be the first
  // source command in a document-only or compact view, so mount the lazy
  // editor before asking it to move the caret.
  async function openOutlineHeading(heading) {
    if (!mayEdit || !session || !heading) return;
    const id = openFile;
    const file = files.find((entry) => entry.id === id && entry.kind === "text");
    if (!file) return;
    outlineActiveFrom = heading.from;
    const revision = outlineRevision;
    openTheFile(file);
    if (!editing) await startEditing();
    if (readerDisposed || id !== openFile) return;
    showMobileView("source");
    await tick();
    if (readerDisposed || id !== openFile || revision !== outlineRevision || !editor
        || !outlineHeadings.some((item) => item.from === heading.from)) return;
    if (editor.goToIn) editor.goToIn(id, heading.from);
    else editor.openAt?.(id, heading.line, 1);
  }

  // The bytes of the figure being looked at are fetched by the workspace;
  // this is the effect that asks it to, whenever the figure changes.
  $effect(() => {
    void ws.figure;
    workspace.watchFigure();
  });

  const addFile = (path) => workspace.addText(path);
  const relocateFiles = (entries, destination, rename) => workspace.relocate(entries, destination, rename);
  const deleteFiles = (entries) => workspace.remove(entries);
  const makeMain = (file) => workspace.setMain(file);
  const addFigure = (file, path) => workspace.addFigure(file, path);

  function insertContext() {
    if (!mayEdit) return null;
    const context = editor?.getInsertContext?.();
    if (!context) return null;
    return { ...context, files: [...context.files, ...workspace.assets()] };
  }
  function applyInsertion(result, context) {
    if (!mayEdit || !editor?.applyInsertResult?.(result, context)) {
      throw new Error("The document or selected text changed. Close this dialog and choose the insertion point again.");
    }
    return true;
  }
  async function uploadInsertAsset(file) {
    return await addFigure(file);
  }
  const previewInsertAsset = (path) => workspace.assetUrl(path);

  /// The whole directory, as a zip. Built here rather than by a route,
  /// because everything it needs is already in this browser: the texts are in
  /// the shared document and the figures were fetched to render them, so
  /// asking the server to assemble what is already here would be a round trip
  /// to be told what we know.
  async function downloadTree() {
    if (!mayEdit) {
      say("Editor access is required to download the project.", true);
      return;
    }
    try {
      // The tree and the directory are read here, before anything is awaited,
      // and handed over as values: `projectFiles` cannot then be cut from a
      // tree newer than the one whose assets it fetched.
      const files = await projectFiles({ tree: liveTreeNow(), folders: [...folders], gather: gatherFigures });
      // Named for the document rather than for its main file: what is being
      // downloaded is the directory, and the slug is what a person knows it by.
      saveBlob(await archive(files), `${SLUG}.zip`);
    } catch (error) {
      say(error.message || "could not download the project", true);
    }
  }

  const addDroppedText = (file, path) => workspace.addDroppedText(file, path);

  async function downloadEntry(entry) {
    try {
      const chosen = await entryDownload({ entry, tree: liveTreeNow(), folders: [...folders], gather: gatherFigures });
      saveBlob(chosen.files ? await archive(chosen.files) : new Blob([chosen.bytes]), chosen.name);
    } catch (error) { say(error.message || "Could not download this item.", true); }
  }

  // Dropping a file on the source pane does what the controls in the list do:
  // a text becomes a file, a figure is uploaded, and a name that is neither is
  // refused by the same rules.
  let fileList = $state(null);
  async function dropped(event) {
    if (!mayEdit) return;
    event.preventDefault();
    const chosen = [...(event.dataTransfer?.files || [])];
    if (!chosen.length) return;
    // The files panel is where a refusal is shown, so a drop on the comments
    // brings it forward first.
    if (panel !== "files") {
      showPanel("files");
      await tick();
    }
    fileList?.offer(chosen);
  }

  // The source pane. Nothing is fetched here and nothing is seeded: the text
  // is already in the session, and this only shows it.
  async function startEditing() {
    if (readerDisposed || editing || !mayEdit) return;
    const component = (await import("./Editor.svelte")).default;
    if (readerDisposed || editing || !mayEdit) return;
    Editor = component;
    editing = true;
    paintPreview();
  }

  // The CRDT is loaded here and nowhere else. A reader with a link to a
  // published document never joins the source room, so it never asks for the
  // three megabytes of loro wasm behind it; an editor pays for them once, on
  // the way in, rather than before the page has drawn.
  async function startCollaboration(document_, { sourceSync = true } = {}) {
    collaboration?.close();
    const collab = sourceSync ? await import("../lib/collab.js") : null;
    // Awaiting the module gave the reader time to be torn down or replaced.
    if (readerDisposed) return;
    collaboration = createReaderCollaboration({
      slug: SLUG,
      key: KEY,
      collab,
      getIdentity: () => identity,
      getCanEdit: () => mayEdit,
      sourceSync,
      onMessage: receive,
      onConnected: (up) => {
        connected = up;
        if (!up) {
          pendingChat?.disconnect();
          outbox.disconnected();
        }
      },
      onPeers: (count) => (peers = Math.max(peers, count)),
      onState: (state_) => {
        const newlyFailed = state_.localError && state_.localError !== persistence.localError;
        persistence = state_;
        if (newlyFailed) toastProblem(`${state_.localError}. Download the project to keep a recovery copy.`);
      },
      onSession: (active) => {
        session = active;
        workspace.attach(active);
        refreshFiles();
        refreshPeers();
        // A restored Yjs document can already contain its complete tree when
        // the session is handed over, so no later source event is guaranteed.
        // Render once from that hydrated tree after Svelte has applied the
        // format and file-list state.
        void tick().then(() => {
          if (!readerDisposed && active === session) void paintPreview();
        });
      },
      onSource: (active) => {
        // The first source of a session is what says the document exists to
        // be compared against a publication; later ones are edits.
        if (mayEdit && publication.begin()) void refreshPublicationMetadata();
        sourceChanged();
      },
      onSwap: () => (sourceEpoch += 1),
      onFiles: filesChanged,
      onAwareness: refreshPeers,
      onDocumentChanged: () => {
        passages.clearPassageCache();
        location.reload();
      },
    });
    session = collaboration.start(document_);
  }

  // What this browser may do with the document, and whether it can render it
  // at all.
  async function prepare(document_) {
    // The role endpoint answer is authoritative for every affordance.
    const ROLES = ["reader", "commenter", "editor", "owner"];
    const allowed = ROLES.indexOf(document_.role) >= ROLES.indexOf("editor");
    // The local companion must see the resolved permission on its first
    // configuration. In particular, account examples arrive as editable
    // Quarto documents; configuring before this assignment leaves the
    // companion inactive and the pane stuck on its Markdown fallback.
    mayEdit = Boolean(allowed);
    // The workspace guards its own writes, so it is told once, here, where
    // the permission is settled.
    ws.canEdit = mayEdit;
    timeline.state.canEdit = mayEdit;
    offlinePrepared = Boolean(document_.offline_prepared);
    if (mayEdit && !offlinePrepared) {
      void preparedProject({ server: location.origin, slug: SLUG })
        .then((record) => { offlinePrepared = Boolean(record); })
        .catch(() => {});
    }
    if (mayEdit) publication.watchAsEditor();
    // Reader and commenter links receive the current immutable display bundle.
    // They never join the source room, ask for a snapshot, or start a local
    // renderer. The publication URL is an authenticated server response and
    // remains useful even when there is no publication yet.
    if (!mayEdit) {
      publishedMode = true;
      await startCollaboration(document_, { sourceSync: false });
      const visitor = publication.watchAsVisitor({
        // Where the published bundle is served from. Checked against the
        // origin this document names: a URL from anywhere else is refused
        // rather than loaded into the frame.
        onPublication: (value) => {
          if (!value?.html_url) return;
          const source = value.html_url;
          if (typeof source !== "string" || !/^https?:\/\//.test(source)) return;
          const resolved = new URL(source);
          if (document_.docs_origin && resolved.origin !== document_.docs_origin) {
            say("The published document URL was refused.", true);
            return;
          }
          frameSrc = resolved.href;
          docsOrigin = resolved.origin;
          everPainted = true;
          everPaintedShown = true;
        },
      });
      await visitor.refresh();
      // No publication is an explicit state, never a reason to fall back to
      // source rendering. Keep the document pane available for the notice.
      sourceFormat = "html";
      settled = true;
      return;
    }
    // Rendering follows the durable source format. Generated-result identity
    // is deliberately absent: a reader compiles this source on demand.
    const format = document_.source_format ||
      (document_.execution_engine === "quarto" ? "quarto" : "html");
    sourceFormat = format;
    if (["quarto", "typst", "markdown"].includes(format)) pairLocalQuarto();
    // Every reader compiles from source when the browser has the engine. A
    // missing engine produces an actionable local-tool message below.
    const list = Array.isArray(document_.renderers) ? document_.renderers : ["markdown"];
    renderers.offerLatex(list.includes("latex"));
    if (!renderers.outputKind(format)) {
      say(`${format} documents are read where their renderer is built`, true);
      settled = true;
      return;
    }
    // A panel remembered from an editor's visit is not one a link-holder is
    // offered. Coerced without being remembered: the preference is this
    // browser's, and an editor coming back to their own document keeps it.
    if (panel && !tabs.some((tab) => tab.id === panel)) showPanel(home, false);
    settled = true;
    // Load the browser renderer for readers as well as editors; this is all
    // transient and does not create a server-side result.
    renderers.warm(format);
    await startCollaboration(document_);
    // `onSource` starts publication metadata only after the initial Yjs state
    // has populated the source tree. A session object alone is not a snapshot.
    // No chooser and no saved distribution: `latex.configure` tells the
    // controller which project this is and what it is allowed to do, and the
    // first `paintPreview` (from `startEditing` below, or an edit) is what
    // actually starts loading the engine. `session.latexSettings()` needs the
    // session that `startCollaboration` just built, which is why this comes after
    // it rather than beside the old restore-a-distribution code above.
    configureLatex(sourceFormat);
    // A document its author may edit opens ready to be worked on: that is what
    // they came for.
    if (mayEdit) startEditing();
    // Somebody sent a link to a moment rather than to the document. Opening
    // it opens the panel on that version's comparison with the document as it
    // stands, which is what a link to a moment is for.
    if (ARRIVED_AT) {
      showPanel("history", false).then(() => {
        // The history request may outlive the panel. Do not select a version
        // after the reader has explicitly left history.
        if (panel === "history") void viewPoint(ARRIVED_AT);
      });
    } else if (panel === "history") {
      // The column reopened where it was left, and this panel has to fetch
      // what it shows.
      loadHistory();
    }
  }

  $effect(() => {
    markViewed(SLUG);
    const stopQuartoStatus = companion.watch();
    pendingChat = createPendingChat({
      send: (message) => collaboration?.sendLive(message) || { ok: false },
    });
    const boot = createReaderBoot({
      slug: SLUG,
      key: KEY,
      onIdentity: (who) => {
        me = who;
        if (who.name) session?.rename(who.name);
      },
      onDocument: (found) => {
        doc = found;
        document.title = `${found.title} · LibrePaper`;
        docsOrigin = found.docs_origin || location.origin;
        // The frame is an empty page with the agent in it, on the documents
        // origin. What goes into it is what this browser renders -- or, for a
        // LaTeX document, a PDF an editor's browser compiled, which needs a
        // frame that can draw one. Set after `prepare`, which is what settles
        // the format and so which frame this document wants.
        void prepare(found);
      },
      // A document answers a stranger exactly as a missing one does, which
      // tells a stranger nothing -- and tells an owner who has not signed in
      // nothing either. That is what this line is for: the page was opened
      // at a real URL, so the honest thing to say is both.
      onError: () => {
        doc = { title: "Document not found" };
        say(
          me.providers?.length && !identity
            ? "not found — sign in, if this was shared with you"
          : "not found",
          true,
        );
      },
    });
    boot.start();

    return () => {
      // The session on the server ends when the last person in it
      // disconnects, which the socket closing does on its own; this is only
      // this browser letting go of its half.
      readerDisposed = true;
      stopQuartoStatus();
      renderCoordinator.invalidate();
      navigationGeneration += 1;
      renderers.cancelPreview();
      publication.dispose();
      clearTimeout(previewTimer);
      previewTimer = null;
      boot.dispose();
      passages.clearPassageCache();
      void quartoPreviewController.stop();
      void calepinPreviewController.stop();
      framePreview.dispose();
      stopLatex();
      pendingChat?.dispose();
      for (const pendingDecision of suggestionDecisions.values()) pendingDecision.reject(new Error("The review context was closed."));
      suggestionDecisions.clear();
      collaboration?.close();
    };
  });

  // Ctrl-S is what a hand does after typing a paragraph, and there is nothing
  // for it to do: the document is already durable. What it must not do is
  // claim that pending writes are saved, so it says what is actually true.
  function reportPersistence() {
    // Never claim pending or disconnected writes have reached the server.
    if (persistence.localError || persistence.localPending) return;
    if (!connected) {
      if (persistence.local) say("saved on this device");
      return;
    }
    if (persistence.pending) return;
    say("saved on the server");
  }

  async function makeAvailableOffline() {
    if (!mayEdit || !session || preparingOffline) return;
    preparingOffline = true;
    try {
      await session.persist();
      await cacheCurrentShell();
      await prepareOfflineProject({ ...doc, slug: SLUG, server: location.origin });
      offlinePrepared = true;
      toastDone("Available offline on this device");
    } catch (error) {
      toastProblem(error?.message || "Could not make this project available offline");
    } finally {
      preparingOffline = false;
    }
  }

  // There is no save, so a close is almost never worth interrupting: the
  // document is durable, and what is not yet on the server is in this
  // browser. The one case left is work that has reached neither.
  function beforeUnload(event) {
    if (atRisk) event.preventDefault();
  }

  // The arrangement is changed often enough to be worth a key. Ctrl-\ is what
  // an editor usually puts a split on, and nothing here or in CodeMirror wants
  // it.
  function shortcut(event) {
    if (editing && (event.ctrlKey || event.metaKey) && event.altKey && event.key.toLowerCase() === "l") {
      event.preventDefault();
      setLinked(!linked);
      return;
    }
    if (!editing || !(event.ctrlKey || event.metaKey) || event.key !== "\\") return;
    event.preventDefault();
    cycleLayout();
  }
</script>

<svelte:window bind:innerWidth={width} onresize={() => (unit = measure())} onkeydown={shortcut} onbeforeunload={beforeUnload} onpagehide={() => session?.leave()} />

{#snippet fileItems()}
  {#if mayEdit}
    <Menu.Item value="new-file" class="menuitem">New file…</Menu.Item>
    <Menu.Item value="new-folder" class="menuitem">New folder…</Menu.Item>
    <Menu.Item value="upload" class="menuitem">Upload files…</Menu.Item>
    <hr class="hr my-1" />
  {/if}
  <!-- One of the two, never both: a document renders to a PDF or to a page.
       Greyed out, not hidden, while there is nothing to download yet, so the
       menu says what the document will offer. -->
  {#if renderers.outputKind(displayedFormat) === "pdf"}
    <Menu.Item value="download-pdf" class="menuitem" disabled={!downloads.pdf}>Download PDF</Menu.Item>
  {:else}
    <Menu.Item value="download-html" class="menuitem" disabled={!downloads.html}>Download HTML</Menu.Item>
  {/if}
  <Menu.Item value="download-docx" class="menuitem" disabled={!docxDownload}>Download DOCX</Menu.Item>
  {#if mayEdit}
    <Menu.Item value="offline" class="menuitem" disabled={offlinePrepared || preparingOffline || Boolean(persistence.localError)}>
      {offlinePrepared ? "Available offline" : preparingOffline ? "Preparing offline copy…" : "Make available offline"}
    </Menu.Item>
    <Menu.Item value="download" class="menuitem">Download project</Menu.Item>
  {/if}
  <hr class="hr my-1" />
  {#if canSeeSharing || canPublish}<Menu.Item value="share" class="menuitem">Share…</Menu.Item>{/if}
  <Menu.Item value="history" class="menuitem">History</Menu.Item>
{/snippet}

{#snippet layoutItems()}
  {#each [["source", "Source"], ["document", "Preview"], ["split", "Split"]] as [value, label]}
    <Menu.Item value="layout-{value}" class="menuitem" disabled={compact && value === "split"}>
      <span class="w-4">{(compact ? activeMobileView === value : layout === value) ? "✓" : ""}</span>{label}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  {#each [["left", "Source on left"], ["right", "Source on right"]] as [side, label]}
    <Menu.Item value="side-{side}" class="menuitem" disabled={compact}>
      <span class="w-4">{sourceSide === side ? "✓" : ""}</span>{label}
    </Menu.Item>
  {/each}
  {#each RATIOS as ratio}
    <Menu.Item value="ratio-{ratio.share}" class="menuitem" disabled={compact}>
      <span class="w-4">{sizes[PANES.editor.key] === ratio.share ? "✓" : ""}</span>{ratio.says}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  <Menu.Item value="linked" class="menuitem">
    <span class="w-4">{linked ? "✓" : ""}</span>Keep in step
  </Menu.Item>
{/snippet}

{#snippet previewItems()}
  {@const selectedFormat = displayedFormat === "latex" ? latexOutput : displayedFormat === "typst" ? typstOutput : displayedFormat === "quarto" ? (quartoTargetFormat() === "pdf" ? "pdf" : "html") : "html"}
  {@const selectableFormat = ["latex", "typst"].includes(displayedFormat)}
  <div class="menu-section-label">Format</div>
  <Menu.Item value="format-html" class="menuitem" disabled={!selectableFormat}>
    <span class="w-4">{selectedFormat === "html" ? "✓" : ""}</span>HTML
  </Menu.Item>
  <Menu.Item value="format-pdf" class="menuitem" disabled={!selectableFormat}>
    <span class="w-4">{selectedFormat === "pdf" ? "✓" : ""}</span>PDF
  </Menu.Item>
  <hr class="hr my-1" />
  <div class="menu-section-label">Engine</div>
  {#if sourceFormat === "quarto"}
    <!-- Which engine is drawing follows from Local execution below, rather
         than being chosen twice: with it off this is the browser's own
         Markdown draft, which never runs any code; with it on, Quarto runs
         the document on this computer through the local app and its own page
         is what appears here. Nothing rendered is ever uploaded. -->
    <Menu.Item value="engine-quarto" class="menuitem" disabled>
      <span class="w-4">✓</span>{localExecution && quartoPreviewMode === "quarto" ? "Quarto" : "Markdown"}
    </Menu.Item>
  {:else if displayedFormat === "typst"}
    <!-- The same, for a Typst document: this browser's own Typst rendering,
         or the PDF Calepin delivers after running the document's chunks on
         this computer. HTML is always the browser Typst renderer, even while
         local execution is on. -->
    <Menu.Item value="engine-typst" class="menuitem" disabled>
      <span class="w-4">✓</span>{localExecution && typstPreviewMode === "calepin" && typstOutput !== "html" ? "Calepin" : "Typst"}
    </Menu.Item>
  {:else if displayedFormat === "latex"}
    {#each [["auto", "Automatic"], ["pdflatex", "pdfLaTeX"], ["xelatex", "XeLaTeX"], ["lualatex", "LuaLaTeX"]] as [engine, label]}
      <Menu.Item value="engine-latex-{engine}" class="menuitem">
        <span class="w-4">{(latexSettingsState.engine || "auto") === engine ? "✓" : ""}</span>{label}
      </Menu.Item>
    {/each}
  {:else if displayedFormat === "markdown"}
    <Menu.Item value="engine-markdown" class="menuitem" disabled><span class="w-4">✓</span>Markdown</Menu.Item>
  {:else if displayedFormat === "html"}
    <Menu.Item value="engine-html" class="menuitem" disabled><span class="w-4">✓</span>HTML</Menu.Item>
  {/if}
  <hr class="hr my-1" />
{/snippet}

{#snippet viewItems()}
  <Menu.Item value="preview-file" class="menuitem" disabled={!canPreviewFile}>Preview this file</Menu.Item>
  <hr class="hr my-1" />
  {@render previewItems()}
  {#if mayEdit}
    <!-- Off for every document every time it is opened, including one
         somebody else shared: what a document may run on this computer is
         answered by the person sitting at it, in this session, and is never
         remembered or carried by the document. -->
    <div class="menu-section-label">Local execution</div>
    <Menu.Item value="local-execution" class="menuitem">
      <span class="w-4">{localExecution ? "✓" : ""}</span>Run this document's code here
    </Menu.Item>
    <hr class="hr my-1" />
  {/if}
  {@render layoutItems()}
{/snippet}

{#snippet toolItems()}
  {#if mayEdit && sourceFormat === "latex" && compilesHere}
    <Menu.Item value="compile" class="menuitem">Compile now</Menu.Item>
  {/if}
  <Menu.Item value="settings" class="menuitem">Settings…</Menu.Item>
{/snippet}

<Nav {me} documentation={false}>
  {#snippet tools()}
    {#if publishedMode && publicationUpdate}
      <button type="button" class="btn btn-sm preset-tonal-warning" onclick={() => void publication.acceptUpdate()}>
        New published version available · Refresh
      </button>
    {/if}
    {#if mayEdit && publicationStatus}
      <button type="button" class="btn btn-sm preset-tonal-warning" onclick={() => openPanel("share")}>Unpublished changes</button>
    {/if}
    <div class="presence" role="group" aria-label={connected ? `${peers} people connected` : connectionNote} title={connected ? `${peers} people connected` : connectionNote}>
      <span class="connection-dot" class:offline={!connected} aria-hidden="true"></span>
      {#if !connected}<span class="connection-label">Offline</span>{/if}
      {#each participants.slice(0, 3) as person (person.key)}
        <Avatar name={person.name} key={person.key} size={6} />
      {/each}
      {#if participants.length > 3}<span class="presence-more">+{participants.length - 3}</span>{/if}
    </div>
  {/snippet}
  {#snippet menus()}
    {#if shown.document}
      <div class="desktop-workspace-menu">
        <Menu onSelect={(chosen) => void chooseFileCommand(chosen.value)}>
          <Menu.Trigger class="menubar-item">File</Menu.Trigger>
          <ExplorerMenu>{@render fileItems()}</ExplorerMenu>
        </Menu>
      </div>
    {/if}
    {#if editing && mayEdit}
      <Menu onOpenChange={(event) => { if (event.open) editAvailability = editor?.editAvailability() || {}; }} onSelect={(chosen) => chooseEditCommand(chosen.value)}>
        <Menu.Trigger class="menubar-item" disabled={!editor || !!mergeTarget || !!shownFigure}>Edit</Menu.Trigger>
        <ExplorerMenu>
          {#each editGroups as group, index}
            {#if index}<hr class="hr my-1" />{/if}
            {#each group as [command, label]}
              <Menu.Item value={command} class="menuitem" disabled={!editAvailability[command]}>{label}</Menu.Item>
            {/each}
          {/each}
        </ExplorerMenu>
      </Menu>
      <InsertMenu getContext={insertContext} oninsert={applyInsertion} onupload={uploadInsertAsset} onpreview={previewInsertAsset} oncancel={(context) => editor?.releaseInsertContext?.(context)} onfocus={() => editor?.focus?.()} disabled={!mayEdit || !editor} />
    {/if}
    {#if editing}
      <div class="desktop-workspace-menu">
        <Menu onSelect={(chosen) => chooseViewCommand(chosen.value)}>
          <Menu.Trigger class="menubar-item">View</Menu.Trigger>
          <ExplorerMenu>{@render viewItems()}</ExplorerMenu>
        </Menu>
      </div>
    {/if}
    {#if mayEdit}
      <div class="desktop-workspace-menu">
        <Menu onSelect={(chosen) => chooseToolCommand(chosen.value)}>
          <Menu.Trigger class="menubar-item">Tools</Menu.Trigger>
          <ExplorerMenu>{@render toolItems()}</ExplorerMenu>
        </Menu>
      </div>
    {/if}
    {#if shown.document}
      <div class="compact-workspace-menu">
        <Menu onSelect={(chosen) => chooseCompactCommand(chosen.value)}>
          <Menu.Trigger class="menubar-item" aria-label="File, view and tools">Menu</Menu.Trigger>
          <ExplorerMenu>
            {@render fileItems()}<hr class="hr my-1" />
            {#if editing}{@render viewItems()}<hr class="hr my-1" />{/if}
            {#if mayEdit}{@render toolItems()}{/if}
          </ExplorerMenu>
        </Menu>
      </div>
    {/if}
  {/snippet}
  {#snippet children()}
    <span id="docTitle" class="nav-document truncate" title={toolbarPath || doc.title || ""}>
      {toolbarPath ? basename(toolbarPath) : doc.title || "LibrePaper"}
    </span>
    {#if editing}
      <IconButton icon="eye" label="Preview this file" disabled={!canPreviewFile}
                  pressed={toolbarPath === previewMain} onclick={previewThisFile} />
    {/if}
  {/snippet}
</Nav>

<main id="main" tabindex="-1" class="reader" class:editing={shown.source} class:no-preview={!shown.document}
      class:no-comments={!shown.comments} class:source-right={sourceSide === "right"}
      class:mobile-document={activeMobileView === "document"} class:mobile-source={activeMobileView === "source"}
      class:mobile-sidebar={activeMobileView === "sidebar"} class:adapted={compact || (splitTight && layout === "split")}
      style="height: calc(100dvh - var(--librepaper-bar)); --librepaper-activity: {px(ACTIVITY_WIDTH, unit)}px; --librepaper-editor: {pixels(PANES.editor, panes)}px; --librepaper-sidebar: {pixels(PANES.sidebar, panes)}px">
  <!-- What this page is. The title is in the bar, where it is a span beside
       the menus rather than a heading, so the one heading a screen reader
       looks for first is here instead: said once, seen by nobody. -->
  <h1 class="sr-only">{doc.title || "Untitled document"}</h1>
  <!-- Each panel, wired where its state lives. The column renders whichever
       of these its rail has selected; none of it travels through the column
       as props, so a panel can grow a field without the column hearing of it. -->
  {#snippet filesPanel()}
    <Files bind:this={fileList} {files} {folders} open={openFile} peers={peersByFile}
      {mayEdit} {rules} onopen={openTheFile} onadd={addFile}
      onmkdir={(path) => workspace.addFolder(path)} onrelocate={relocateFiles}
      ondelete={deleteFiles} onduplicate={(entry, path) => workspace.duplicate(entry, path)}
      onmain={makeMain} onfigure={addFigure} ontext={addDroppedText}
      ondownload={downloadTree} ondownloaditem={downloadEntry} />
  {/snippet}

  {#snippet outlinePanel()}
    <Outline headings={outlineHeadings} activeFrom={outlineActiveFrom} onselect={openOutlineHeading} />
  {/snippet}

  {#snippet agentPanel()}
    <Agent slug={SLUG} link={linkFor(SLUG)} canShare={doc.role === "owner"}
      path={session?.paths?.get(openFile) || ""} selection={pending}
      revision={pending?.revision || ""} request={assistantRequest}
      {comments} {diagnostics} oncommenttask={askCommentAssistant}
      ondiagnostictask={askDiagnostic} onreview={reviewAssistantResults} onpreview={previewAssistant} />
  {/snippet}

  {#snippet collaborationPanel()}
    <Collaboration messages={liveChat} {connected} canPost={mayChat} onsend={sendLiveChat}
      {unreadChat} bind:tab={prefs.collaborationTab} {comments} {figureAt} {identity}
      commentingAs={doc.commenting_as || "Anonymous"} {canModerate} {tool} {went} {replacements}
      canComment={mayChat} hasFigures={figureAt.length > 0} ontool={chooseTool}
      onreveal={revealAnnotation} selected={selectedAnnotation} onresolve={resolve}
      ondelete={askDelete} ondeletemany={askDeleteMany} onreply={reply} />
    {@render pendingRecovery("collaboration")}
  {/snippet}

  {#snippet changesPanel()}
    <Changes {comments} {files} {figureAt} {identity} commentingAs={doc.commenting_as || "Anonymous"}
      {canModerate} canReview={mayEdit} {tool}
      {went} {replacements} canComment={mayChat} ontool={chooseTool} onreveal={revealAnnotation}
      selected={selectedAnnotation} onresolve={resolve} ondelete={askDelete}
      ondeletemany={askDeleteMany} onreply={reply}
      onaccept={(comment) => decideSuggestion(comment, "accept")}
      onreject={(comment) => decideSuggestion(comment, "reject")}
      onrejectconfirmed={rejectConfirmed}
      onhistory={() => { void showPanel("history"); }} />
    {@render pendingRecovery("changes")}
  {/snippet}

  {#snippet sharePanel()}
    <Share open={panel === "share" && shown.comments} inline slug={SLUG}
      canShare={doc.role === "owner"} {canSeeSharing} mayPublish={canPublish}
      publishedVersion={publishedPublication} unpublishedChanges={publicationStatus}
      publicationReady={publicationMetadataReady} publicationFailed={publicationMetadataFailed}
      onpublish={publishCurrent} onrefreshpublication={refreshPublicationMetadata}
      onclose={() => showPanel("")} />
  {/snippet}

  {#snippet diagnosticsPanel()}
    <Diagnostics {diagnostics} localAppProblem={localAppDiagnostics.length > 0}
      onretrylocal={() => localExecutionRelevant ? startLocalExecution() : ensureLocalApp()}
      main={previewMain || session?.mainPath() || ""}
      canOpen={(item) => Boolean(diagnosticFile(item))} onopen={openDiagnostic}
      provenance={renderState.lastLatexResult?.provenance || null}
      attempts={renderState.lastLatexResult?.attempts || []} />
  {/snippet}

  {#snippet historyPanel()}
    <History {checkpoints} viewing={historySelectedSha} canEdit={mayEdit}
      durability={historyDurability} problem={historyProblem} onview={viewPoint}
      onname={nameCheckpoint} oncopy={checkpointLink} />
  {/snippet}

  <!-- Annotations the server has not acknowledged, offered back wherever
       annotations are shown, so work is never only in a failed request. -->
  {#snippet pendingRecovery(id)}
    {#if panel === id && unconfirmed.length}
      <div class="pending-recovery">
        <PendingAnnotations items={unconfirmed}
          onretry={(annotationId) => outbox.retry(annotationId, (message) => collaboration?.send(message))}
          ondiscard={discardAnnotation} />
      </div>
    {/if}
  {/snippet}

  <ReaderSidebar
    {shown} {tabs} {panel} {settled} {compact} {editing} mounted={mountedPanels}
    layout={layout} arrangements={ARRANGEMENTS} {badges}
    panels={{ files: filesPanel, outline: outlinePanel, agent: agentPanel,
              collaboration: collaborationPanel, changes: changesPanel,
              share: sharePanel, diagnostics: diagnosticsPanel, history: historyPanel }}
    panes={panes} sidebarPane={PANES.sidebar}
    onselectpanel={selectPanel} oncyclelayout={cycleLayout} ondrop={dropped}
    onsize={(size) => setSize(PANES.sidebar, size)} onguide={(where) => (guide = where)}
    ongrab={(on) => { grabbing = on; guide = { ...guide, shown: on }; }}
  />

  {#if editing || panel === "history"}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section id="reader-source" class="editorpane" class:away={!shown.source}
             ondragover={(event) => event.preventDefault()} ondrop={dropped}>
      <!-- A figure has no editor. Choosing one shows it: an image as itself,
           a PDF through the browser's own viewer, which shows the first page
           without this application carrying a PDF renderer of its own. -->
      {#if panel === "history"}
        <HistoryWorkspace source={historySource} canEdit={mayEdit} onrestore={restoreCheckpoint} />
      {:else if mergeTarget && MergeEditor}
        <MergeEditor path={mergeTarget.path} oldText={mergeTarget.oldText} newText={mergeTarget.newText}
                     liveText={mergeTarget.liveText} ephemeral={mergeTarget.ephemeral} loroDoc={mergeTarget.loroDoc}
                     diff
                     editable={mergeTarget.editable !== false && mayEdit && editing}
                     targetLabel={mergeTarget.targetLabel}
                     note={mergeTarget.note || ""}
                     onclose={() => (mergeTarget = null)} />
      {:else if shownFigure}
        <div class="figureview">
          {#if !figureUrl}
            <p>Fetching {shownFigure.path}…</p>
          {:else if shownFigure.path.toLowerCase().endsWith(".pdf")}
            <object data={figureUrl} type="application/pdf" title={shownFigure.path}>
              <p>{shownFigure.path}</p>
            </object>
          {:else}
            <img src={figureUrl} alt={shownFigure.path} />
          {/if}
        </div>
      {:else if Editor && session?.text}
        {#key sourceEpoch}
          <Editor bind:this={editor} {session} format={editorFormat} file={openFile} {keys} editable={mayEdit}
                  send={collaboration?.sendLive}
                  onbibliography={bibliographyAnalyzed} onchange={outlineTextChanged} oncaret={outlineCaretChanged} onsave={reportPersistence} onquit={showDocumentAlone}
                  onfilechange={(id) => { ws.openFile = id; outlineActiveFrom = null; ws.figure = null; }} />
        {/key}
      {/if}
    </section>
  {/if}

  <!-- A separator only where there are two things to separate. -->
  {#if shown.source && shown.document}
    <Grip pane={PANES.editor} label="Split between the source and the document" panes={panes}
          controls="reader-source"
          onsize={(size) => setSize(PANES.editor, size)}
          onguide={(where) => (guide = where)}
          ongrab={(on) => { grabbing = on; guide = { ...guide, shown: on }; }}>
      {#snippet aside()}
        {#if !linked}<Icon name="unlock" />{/if}
      {/snippet}
    </Grip>
  {/if}

  <!-- What stands where the document would be, before there is one to show.
       There is no compiler card any more: an editor's browser initializes
       the engine on its own, automatically, and the Preview header carries
       loading and failure states. Every paged format
       renders from source on demand; generated output remains transient. -->
  {#if shown.document && failedBeforeRender}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">Could not render</h2>
        {#if renderState.failureReason}
          <p class="text-surface-700-300 text-sm">
            The compiler produced no {pdfOutput ? (latexHtmlPreview ? "HTML preview" : "PDF") : "HTML preview"}, and Diagnostics has nothing to show for it. What it said:
          </p>
          <pre class="text-surface-700-300 text-xs">{renderState.failureReason}</pre>
        {:else}
          <p class="text-surface-700-300 text-sm">
            {#if !compilesHere}
              This browser cannot render this {sourceFormat === "latex" ? "LaTeX" : sourceFormat === "typst" ? "Typst" : sourceFormat === "quarto" ? "Quarto/Markdown" : "Markdown"} source. {sourceFormat === "latex" ? "Configure the browser LaTeX renderer to view it." : "Install or configure the local companion or browser renderer to view it."}
            {:else}
              Fix the errors in Diagnostics to produce a {latexHtmlPreview ? "HTML preview" : "PDF preview"}.
            {/if}
          </p>
        {/if}
      </div>
    </section>
  {:else if shown.document && unrendered}
    <section class="latexpane">
      <div class="notyet">
        {#if pdfNeedsLocalTool}
          <h2 class="h4">PDF needs {sourceFormat === "quarto" ? "Quarto" : "a local build"} on this computer</h2>
          <p class="text-surface-700-300 text-sm">
            This browser renders {sourceFormat === "quarto" ? "Quarto" : "Markdown"} to HTML only. The PDF is produced by
            {sourceFormat === "quarto" ? "Quarto itself" : "Pandoc or Quarto"}, run through the local LibrePaper app,
            so the preview stays empty until that is connected{sourceFormat === "quarto" ? " and allowed to run this document" : ""}.
          </p>
          <div class="notyet-actions">
            {#if pdfNeedsLocalExecution}
              <button class="btn btn-sm preset-filled-primary-500" onclick={() => (localExecutionConsent = true)}>Turn on local execution</button>
            {:else}
              <button class="btn btn-sm preset-filled-primary-500" onclick={() => void ensureLocalApp()}>Connect the local app</button>
            {/if}
            <button class="btn btn-sm preset-outlined-surface-300-700" onclick={() => openSettings("local")}>Install or configure companion</button>
            <button class="btn btn-sm preset-outlined-surface-300-700" onclick={previewAsHtml}>Preview as HTML instead</button>
          </div>
        {:else}
          <h2 class="h4">Not yet rendered</h2>
          <p class="text-surface-700-300 text-sm">
            This {sourceFormat === "typst" ? "Typst" : "paged"} document is being rendered from source in this browser.
          </p>
          {#if !compilesHere && session?.text}
            <details open>
              <summary>View source · {session.mainPath?.() || "document"}</summary>
              <pre>{session.text.toString()}</pre>
            </details>
          {/if}
        {/if}
      </div>
    </section>
  {/if}

  <!-- Kept mounted whatever the arrangement: taking the frame out of the tree
       would reload the document and lose the reader's place in it. -->
  {#snippet previewStatusDetails()}
    <div class="preview-status-details">
      {#if renderState.provenance}
        <p>Built with {renderState.provenance.builder} · {renderState.provenance.backend}{renderState.provenance.engine ? ` · ${renderState.provenance.engine}` : ""}{renderState.provenance.version ? ` · ${renderState.provenance.version}` : ""}{renderState.provenance.preset ? ` · preset ${renderState.provenance.preset}` : ""}</p>
      {/if}
      {#if latexHtmlPreview}
        {#if compileBadge}<p>{compileBadge}</p>{/if}
        {#if renderState.failureReason}<p>{renderState.failureReason}</p>{/if}
      {:else if sourceFormat === "latex" && editing}
        <LatexStatus />
      {:else if sourceFormat === "typst" && compileBadge}
        <p>{compileBadge}</p>
      {/if}
      {#if sourceFormat === "quarto" && (quartoNeedsLocalApp || quartoPreviewError)}
        <p>Showing Markdown preview. {quartoPreviewError || localConnectionError || "Use Quarto on this computer to generate the full preview."}</p>
        <button class="btn btn-sm preset-outlined-surface-300-700" onclick={() => void startLocalExecution()}>
          {quartoNeedsLocalApp ? "Enable local rendering" : "Retry Quarto preview"}
        </button>
        {#if quartoNeedsLocalApp}<button class="btn btn-sm preset-outlined-surface-300-700" onclick={() => openSettings("local")}>Install or configure companion</button>{/if}
      {/if}
      {#if quartoReaderNeedsLocalTool}
        <p>Showing the browser's Markdown draft. Full Quarto preview requires the local LibrePaper app with Quarto and its execution tools.</p>
      {/if}
      {#if sourceFormat === "typst" && !typstHtmlPreview && (typstNeedsLocalApp || typstNeedsCalepinCommand || calepinPreviewError)}
        <p>{calepinPreviewError || localConnectionError || (typstNeedsCalepinCommand ? "Install the calepin command to use this preview." : "Connect the local LibrePaper app to use Calepin preview.")}</p>
      {/if}
      {#if previewProblem}
        <button class="btn btn-sm preset-outlined-surface-300-700" onclick={() => showPanel("diagnostics")}>Open Diagnostics</button>
      {/if}
    </div>
  {/snippet}
  {#snippet previewControls()}
    {#if viewerView}
      <PreviewControls mode={viewerView.mode} scale={viewerView.scale} tool={viewerTool}
        onmode={(mode) => {
          // Optimistic, so the select does not snap back while the page is
          // redrawn; the frame corrects it with the scale it really used.
          viewerView = { ...viewerView, mode };
          tell({ type: "viewer-scale", mode });
        }}
        ontool={(next) => {
          viewerTool = next;
          tell({ type: "viewer-tool", tool: next });
        }} />
    {/if}
  {/snippet}
  {#snippet previewStatusControl()}
    {#if sourceFormat === "quarto" && mayEdit && !localExecution}
      <button class="btn btn-sm preset-filled-primary-500" onclick={() => (localExecutionConsent = true)}>Turn on local execution</button>
    {:else}
      <PreviewStatus label={previewStatusLabel} busy={previewBusy}
        tone={previewProblem ? "error" : "neutral"}>
        {#snippet details()}{@render previewStatusDetails()}{/snippet}
      </PreviewStatus>
    {/if}
  {/snippet}
  {#if publishedMode && !publishedPublication?.id}
    <section class="latexpane"><div class="notyet" role="status">
      <h2 class="h4">Not published yet</h2>
      <p class="text-surface-700-300 text-sm">The publisher has not published a reader version of this document.</p>
    </div></section>
  {/if}
  <Preview bind:this={preview} src={frameSrc} {docsOrigin} onmessage={fromFrame} onload={frameLoaded} {grabbing}
           path={previewMain} status={previewStatusControl} controls={previewControls}
           away={!shown.document || unrendered || failedBeforeRender} />

  <nav class="mobile-pane-nav" aria-label="Workspace view">
    <IconButton icon="book" label="Document" pressed={shown.document}
                onclick={() => showMobileView("document")} />
    {#if editing}
      <IconButton icon="file-text" label="Source" pressed={shown.source}
                  onclick={() => showMobileView("source")} />
    {/if}
    <PanelRail tabs={compact ? tabs : []} {panel} open={shown.comments} onselect={selectPanel} />
  </nav>

  <!-- Shown only while a separator is dragged: a line that follows the pointer
       so the split can be seen moving without the iframe reflowing on every
       pointermove. -->
  {#if guide.shown}<div class="grip-guide" class:held={guide.held} style="left: {guide.left}px"></div>{/if}
</main>

{#if bar.shown && mayChat}
  <div
    bind:this={barElement}
    id="selectionbar"
    class="flex gap-1"
    style="display: flex; left: {bar.left}px; top: {bar.top}px"
  >
    {#if tool === "highlighting"}
      <div class="highlight-colors" role="group" aria-label="Highlight color">
        {#each HIGHLIGHT_COLORS as color}
          <button type="button" class:selected={highlightColor === color} class="color-swatch" style="background:{color}"
            aria-label="Use {colorName(color)} highlight" aria-pressed={highlightColor === color}
            onclick={() => (highlightColor = color)}></button>
        {/each}
        <label class="custom-color" title="Choose highlight color">
          <span class="sr-only">Custom highlight color</span>
          <input type="color" bind:value={highlightColor} />
        </label>
      </div>
    {/if}
    {#if mayChat}<button class="btn btn-sm preset-filled-primary-500 shadow-lg" onclick={barClicked}>
      {tool === "highlighting" ? "Highlight" : tool === "region" ? "Box" : tool === "editing" ? "Suggest" : "Comment"}
    </button>{/if}
  </div>
{/if}

<!-- The preferences, opened from the navbar menu rather than the column:
     the sidebar is for what its icons offer, and a page of settings reads
     better at the width of the window than in a column beside the text. -->
<SettingsDialog bind:open={settingsOpen} bind:category={settingsCategory}
                {sourceFormat} {mayEdit}
                {keys} onkeys={setKeys}
                buildPreferences={buildPreferences} documentId={SLUG} userId={buildUserId} onbuildpreferences={setBuildPreferences}
                main={previewMain} bindingId={quartoBindingId} onbindingid={(id) => { quartoBindingId = id; localQuarto.setBindingId(id); }}
                options={quartoOptions}
                onapplyoptions={applyRenderOptions}
                account={me} />

<Modal bind:open={commenting} title={tool === "editing" ? "Suggest a change" : "Add comment"}>
  {#snippet children()}
    <form id="commentForm" class="flex flex-col gap-3" onsubmit={submitDialog}>
      <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3 text-sm">
        {pending?.output_anchor ? `Current output: ${pending.exact}${pending.region ? " (selected region)" : ""}` : pending?.point ? "Comment at this point" : pending?.region ? `Figure ${pending.region.image_index + 1}` : `“${pending?.exact ?? ""}”`}
      </blockquote>
      {#if identity}
        <p class="text-surface-600-400 text-sm">
          {tool === "editing" ? "suggesting" : "commenting"} as {me.provider === "github" ? `@${identity}` : identity}
        </p>
      {:else if me.comments_need_login}
        <p class="text-sm">
          <a class="anchor" href={signInHref()}>Sign in</a> to {tool === "editing" ? "suggest a change to" : "comment on"} this document.
        </p>
      {:else}
        <!-- No name to type: the server hands out a per-document pseudonym for
             an anonymous commenter, so this is only ever a statement. -->
        <p class="text-surface-600-400 text-sm">
          {tool === "editing" ? "suggesting" : "commenting"} as {doc.commenting_as || "Anonymous"}
        </p>
      {/if}
      {#if tool === "editing"}
        {#if !pending?.source}
          <!-- No anchor of record: the server stores and shows the
               suggestion anyway, but an editor has to apply it by hand
               rather than clicking Accept. -->
          <p class="text-warning-600-400 text-sm">
            LibrePaper could not place this passage in the source. An editor will have to apply the suggestion by hand.
          </p>
        {/if}
        <label class="label">
          <span class="label-text">Suggested replacement</span>
          <!-- svelte-ignore a11y_autofocus -->
          <textarea
            class="textarea"
            rows="5"
            maxlength="5000"
            autofocus
            bind:value={draft.proposed}
            placeholder="Leave empty to suggest deleting the passage"
          ></textarea>
        </label>
        <label class="label">
          <span class="label-text">Note (optional)</span>
          <textarea class="textarea" rows="2" maxlength="5000" bind:value={draft.body}></textarea>
        </label>
      {:else}
        <label class="label">
          <span class="label-text">Comment</span>
          <!-- svelte-ignore a11y_autofocus -->
          <textarea class="textarea" rows="5" maxlength="5000" required autofocus bind:value={draft.body}></textarea>
        </label>
      {/if}
    </form>
  {/snippet}
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (commenting = false)}>
      Cancel
    </button>
    <button
      type="submit"
      form="commentForm"
      class="btn preset-filled-primary-500"
      disabled={!identity && me.comments_need_login}
    >
      Save
    </button>
  {/snippet}
</Modal>

<Modal bind:open={localExecutionConsent} title="Run this document's code on this computer?"
  description="{LOCAL_EXECUTION_WARNING} Only turn this on for a document whose authors you trust.">
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (localExecutionConsent = false)}>Cancel</button>
    <button type="button" class="btn preset-filled-primary-500" onclick={() => void startLocalExecution()}>OK</button>
  {/snippet}
</Modal>

<Modal bind:open={timeline.state.restoring} title="Restore this version?"
  description={restoreMoved
    ? `The project changed while this was open. Restore ${restoreName} over the project as it now stands? The current version will be preserved in history.`
    : `Restore ${restoreName}. Every file in the project is replaced, and the current version is preserved in history.`}>
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" disabled={restoreBusy} onclick={() => (timeline.state.restoring = false)}>Cancel</button>
    <button type="button" class="btn preset-filled-primary-500" disabled={restoreBusy || !mayEdit} onclick={confirmRestore}>
      {restoreBusy ? "Restoring…" : restoreMoved ? "Restore anyway" : "Restore version"}
    </button>
  {/snippet}
</Modal>

<!-- Deleting a thread cannot be undone, so it is confirmed. -->
<Modal
  bind:open={deleting}
  title={pendingDelete.length > 1 ? `Delete ${pendingDelete.length} comments?` : "Delete comment?"}
  description={pendingDelete.length > 1
    ? "This removes these comments and their replies for everyone. It cannot be undone."
    : "This removes the comment and its replies for everyone. It cannot be undone."}
>
  {#snippet footer()}
    <button type="button" class="btn preset-outlined-surface-300-700" onclick={() => (deleting = false)}>
      Cancel
    </button>
    <button type="button" class="btn preset-filled-error-500" onclick={confirmDelete}>Delete</button>
  {/snippet}
</Modal>

<!-- Signed out, a commenter chooses how to be named before they write. -->
<Modal
  bind:open={identifying}
  title="Who are you?"
  description="Choose how to identify yourself in this comment."
>
  {#snippet footer()}
    <button
      type="button"
      class="btn preset-outlined-surface-300-700"
      onclick={() => { identifying = false; location.href = signInHref(); }}
    >
      Sign in
    </button>
    <button
      type="button"
      class="btn preset-filled-primary-500"
      onclick={() => { identifying = false; openDialog(); }}
    >
      Continue as {doc.commenting_as || "Anonymous"}
    </button>
  {/snippet}
</Modal>

<Toasts />

<style>
  .nav-document {
    display: block;
    max-width: min(38vw, 20rem);
    color: var(--color-surface-700-300);
    font-size: var(--text-sm);
  }
  .compact-workspace-menu { display: none; }
  .presence { display: inline-flex; align-items: center; gap: calc(var(--spacing) * .5); color: var(--color-surface-600-400); font-size: var(--text-xs); }
  .connection-dot { width: .5rem; height: .5rem; margin-inline: var(--spacing); border-radius: 50%; background: var(--color-success-500); }
  .connection-dot.offline { background: var(--color-warning-500); }
  .connection-label { color: var(--color-warning-600-400); font-weight: 600; }
  .presence :global(.avatar + .avatar) { margin-left: calc(var(--spacing) * -1.5); box-shadow: 0 0 0 2px var(--color-shell); }
  .presence-more { display: inline-grid; place-items: center; min-width: 1.5rem; height: 1.5rem; margin-left: calc(var(--spacing) * -1.5); border-radius: 50%; background: var(--color-surface-200-800); color: var(--color-surface-700-300); font-size: .65rem; }
  .preview-status-details { display: grid; gap: calc(var(--spacing) * 2); }
  .menu-section-label { padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 2); color: var(--color-surface-600-400); font-size: var(--text-xs); font-weight: 600; }
  .preview-status-details :global(.latex-status) { display: flex; }
  @media (max-width: 600px) {
    .desktop-workspace-menu { display: none; }
    .compact-workspace-menu { display: block; }
  }
  .highlight-colors { display:flex; align-items:center; gap:3px; padding:2px; border-radius:4px; background:var(--color-surface-100-900); }
  .color-swatch { width:1.5rem; height:1.5rem; border:2px solid transparent; border-radius:50%; }
  .color-swatch.selected { border-color:var(--color-surface-900-100); box-shadow:0 0 0 1px var(--color-primary-500); }
  .custom-color { display:grid; place-items:center; width:1.5rem; height:1.5rem; }
  .custom-color input { width:1.5rem; height:1.5rem; padding:0; border:0; background:transparent; }
  .pending-recovery { flex-shrink: 0; max-height: 35%; overflow-y: auto; border-top: 1px solid var(--color-surface-300-700); background: var(--color-surface-100-900); }
</style>
