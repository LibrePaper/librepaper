<script>
  import { listChanges, accept, reject, rejectMany } from "../lib/track-changes.js";
  // One document: the source beside it, the page itself, and everything said
  // about it.
  import { aboutWholeDocument, anchorAll, flatten, placeSources, shownSelector } from "../lib/anchor.js";
  import { barHiddenAfter, revealOnNavFocus, watchScrolls } from "../lib/bar-scroll.js";
  import * as renderers from "../lib/renderers.js";
  import * as sync from "../lib/sync.js";
  import * as quarto from "../lib/engines/quarto.js";
  import * as figures from "../lib/figures.js";
  import * as history from "../lib/history.js";
  import { createHistorySource } from "../lib/reader/history-source.svelte.js";
  import HistoryWorkspace from "./reader/HistoryWorkspace.svelte";
  import * as passages from "../lib/passages.js";
  import { diagnosticContext } from "../lib/assistant-review.js";
  import { candidateTree, capturePreviewTree, previewCandidate } from "../lib/assistant-preview.js";
  import { orphanState } from "../lib/orphan.js";
  import * as latex from "../lib/latex.js";
  import { parse as parseSynctex, lineAt as synctexLineAt } from "../lib/synctex.js";
  import * as localQuarto from "../lib/desktop/client.js";
  import { backups as backupStatusStore } from "../lib/desktop/backups.svelte.js";
  import { basename } from "../lib/file-manager.js";
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
  import { forgetProject, localProject, rememberProject } from "../lib/offline-projects.js";
  import { cacheCurrentShell } from "../lib/offline-shell.js";
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
  import { slotId, tabsFor } from "../lib/panels.js";
  import { APPLE, availability, commandFor, menuKeys } from "../lib/commands.js";

  import { tick, untrack } from "svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import MenubarMenu from "./MenubarMenu.svelte";
  import { menubar } from "../lib/menubar.svelte.js";
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import Modal from "./Modal.svelte";
  import CommandPalette from "./CommandPalette.svelte";
  import Toasts from "./Toasts.svelte";
  import { say } from "../lib/toast.svelte.js";
  import { archive, availableDownloads, docxFile, entryDownload, projectFiles, renderingFile, saveBlob } from "../lib/reader/downloads.js";
  import Preview from "./Preview.svelte";
  import PreviewControls from "./PreviewControls.svelte";
  import Grip from "./Grip.svelte";
  import { parseRenderOptions } from "../lib/quarto-options.js";
  import SettingsDialog from "./settings/SettingsDialog.svelte";
  import ProjectFolderSetting from "./settings/ProjectFolderSetting.svelte";
  import DesktopOnly from "./DesktopOnly.svelte";
  import { anonymousIdentity, read as readBuildPreferences, update as updateBuildPreferences, writeQuartoOptions } from "../lib/build-preferences.js";
  import Avatar from "./Avatar.svelte";
  import { extractOutline } from "../lib/outline.js";
  import { createFramePreview } from "../lib/reader/frame-preview.js";
  import { rememberPreview, lastPreview, flushPreview } from "../lib/reader/preview-cache.js";
  import { createFrameOverlays } from "../lib/reader/frame-overlays.js";
  import { createRenderDiagnostics } from "../lib/reader/render-diagnostics.js";
  import { createRenderCoordinator } from "../lib/reader/render-coordinator.js";
  import { createGeneration } from "../lib/reader/generation.js";
  import { createRenderStatus } from "../lib/reader/render-status.svelte.js";
  import { provide } from "../lib/support/providers.js";
  import { createSteadyBusy } from "../lib/reader/steady-busy.js";
  import { createLocalPreview } from "../lib/reader/local-preview.svelte.js";
  import { previewNotes, unrenderableMain } from "../lib/reader/preview-notes.js";
  import { HIGHLIGHT_COLORS, colorName } from "../lib/annotation-colors.js";
  import { recenteredLeft, placeBar as placeSelectionBar } from "../lib/annotation-bar.js";
  import { VERBS, motivationFor } from "../lib/annotating.js";
  import InsertMenu from "./InsertMenu.svelte";
  import SymbolPalette from "./SymbolPalette.svelte";
  import MathPalette from "./MathPalette.svelte";
  import Sidebar from "./layout/Sidebar.svelte";
  import CompactPanelMenu from "./layout/CompactPanelMenu.svelte";
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
  // The sign-in return path carries only the path and query: the key stays in
  // this browser's storage and never travels in a URL the server sees.
  const KEY = takeKeyFromFragment(SLUG);

  let { initialSettings = "" } = $props();

  /* ------------------------------------------------------------ the document */

  /** @typedef {{title?: string; docs_origin?: string; document_id?: string; created_at?: string; role?: string; can_edit?: boolean; can_moderate?: boolean; can_see_sharing?: boolean; commenting_as?: string}} ReaderDocument */
  /** @typedef {{id?: string; name?: string; handle?: string; provider?: string; providers?: {id: string, kind: string, label: string}[]; comments_need_login?: boolean}} ReaderUser */
  /** @type {ReaderDocument} */
  let doc = $state({});
  let documentNeedsSignIn = $state(false);
  /** @type {boolean} */
  let readerDisposed = false;
  let docsOrigin = $state(null);
  let frameSrc = $state(null);
  // Non-editors render this allowlisted current-state projection. It has the
  // same shape as the editor adapter but contains no CRDT or history payload.
  let currentProject = $state(null);
  const readerProjectOwner = {};
  let projectEtag = "";
  let projectFetch = null;
  let projectRefreshPending = false;
  let projectRefreshTimer = null;
  let renderedProjectDigest = $state("");
  // Set only when this browser has never successfully shown a projection for
  // this document: an empty pane has nothing else to say for itself, so this
  // is what the document pane below renders instead of staying blank while
  // `settled` silently ends the loading state (§2.2).
  let projectUnreadable = $state("");
  const previewCacheKey = () => `${SLUG}:${doc?.created_at || "unknown"}`;
  // Set when the page was opened from this browser's copy because the server
  // could not be reached, so the page says it may be missing newer changes.
  let openedLocally = $state(false);
  /** @type {ReaderUser} */
  let me = $state({ id: "" });
  const backupStatus = $derived(backupStatusStore.status);
  // The displayed name, since this is what goes on a comment and what the
  // reader is shown commenting as. A Google account's handle is its email and
  // belongs on neither.
  let identity = $derived(me.name || "");
  let canModerate = $derived(Boolean(doc.can_moderate));
  let connected = $state(false);
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
  let persistence = $state({ pending: 0, local: false, localPending: 0, localError: "", joined: false });

  /* --------------------------------------------------------------- anchoring */

  // Everything said about the document is the annotations' to own. Anchoring
  // and repainting stay here: both depend on the frame, which that module
  // cannot see.
  const annotations = createAnnotations({
    slug: SLUG,
    key: KEY,
    anchor: anchorComments,
    repaint: applyHighlights,
    send: (message) => collaboration?.send(message),
    renderDigest: () => renderedProjectDigest,
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

  // Taking a newer published version without losing the reader's place. The
  // bookmark is the words they were looking at, kept across the swap and
  // looked up again in the page that replaces this one; the ask is the frame's
  // one-shot answer about where those words are, and the guard keeps a second
  // update from starting while the first is still in flight.

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
  let quartoBindingId = $state("");
  // The desktop app is there or it is not; this page never changes its mind.
  const hasDesktop = Boolean(localQuarto.desktop());
  // Whether the desktop app found the calepin command, read once the Typst
  // preview asks for it.
  let calepinFound = $state(false);

  /// Which document this browser tab is about, as far as the desktop app is
  /// concerned. It picks up whatever binding the client already remembers for
  /// it. Nothing is asked of the desktop app here.
  ///
  /// Deliberately not behind a format check. It used to live inside the
  /// Quarto binding lookup below, which meant an HTML or LaTeX document never
  /// scoped the client at all, and anything else that wanted the local app
  /// for that document -- the agent panel -- asked with a null project and
  /// was refused. Rendering locally and using the desktop app are different
  /// questions about the same document.
  function scopeLocalApp() {
    localQuarto.configure({ project: SLUG, origin: location.origin });
  }

  /// The Quarto project binding this document already has, if any. This one
  /// really is format-specific: only a locally renderable project has one.
  function readQuartoBinding() {
    quartoBindingId = localQuarto.bindingId();
  }
  // Which engine draws each format, and what it produces, belong to the
  // build settings along with the preference they follow from; the aliases
  // are further down, where that module is built. Whether this document's
  // code runs is not a preference and is never stored: it is off each time a
  // document opens.
  let localExecution = $state(false);

  async function setLatexOutput(format) {
    const next = format === "html" ? "html" : "pdf";
    if (latexOutput === next) return;
    buildSettings.state.latexOutput = next;
    setBuildPreferences(updateBuildPreferences(buildScope(), "latex", { ...(next === "html" ? { selection: "tool", backend: "browser", tool: "tex" } : {}), output: next }));
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

  // The formats whose local tool runs the document rather than only typesets
  // it. Running is off whenever a document opens and is never remembered: the
  // toggle is the consent, and it is given again each time.
  const localExecutionRelevant = $derived.by(() => ["quarto", "typst"].includes(sourceFormat) && mayEdit);

  // Turning it on puts this format's engine on the local tool and uses the
  // desktop app. A browser without the desktop app says so and stays off.
  async function startLocalExecution() {
    if (!hasDesktop) { say(localQuarto.DESKTOP_ONLY_MESSAGE, { kind: "problem", id: "reader:desktop-only" }); return; }
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
  // the desktop app was running for this document.
  async function stopLocalExecution() {
    localExecution = false;
    if (sourceFormat === "quarto") await setQuartoPreviewMode("markdown");
    else if (sourceFormat === "typst") await setTypstPreviewMode("typst");
  }

  const toggleLocalExecution = () => {
    if (localExecution) void stopLocalExecution();
    else void startLocalExecution();
  };

  // A document always opens with its code off. A build preference saved
  // while it was running must not turn it back on, so the preview starts on
  // the browser's own renderer.
  $effect(() => {
    SLUG;
    untrack(() => {
      localExecution = false;
      if (quartoPreviewMode === "quarto") void setQuartoPreviewMode("markdown");
      if (typstPreviewMode === "calepin") void setTypstPreviewMode("typst");
    });
  });

  // Pointing the preview at a file in the other executable format, while Run
  // code is on, puts the desktop app behind that one too.
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

  // Markdown and Quarto share one output choice, made for the visit like the
  // others. Pages need a local tool; the preview says so when none is chosen.
  async function setMarkdownOutput(format) {
    const next = format === "html" ? "html" : "pdf";
    if (markdownOutput === next) return;
    setBuildPreferences(updateBuildPreferences(buildScope(), sourceFormat, { output: next }));
    navigationGeneration += 1;
    renderers.cancelPreview({ keepWarm: true });
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
      profiles: buildPreferences.profile ? [buildPreferences.profile] : [],
      parameters: { ...buildPreferences.parameters },
    };
  }

  const tell = (message, transfer) => preview?.tell(message, transfer);
  const frameOverlays = createFrameOverlays({ ready: () => frameReady, send: tell });
  // Initialized after the derived frame kind is available. The controller's
  // callbacks still update the small bits of component state used by the
  // template and annotation code.
  let framePreview;

  // The agent repaints the whole document on every "highlight"
  // message, so a call that changes nothing is not free even though it looks
  // idempotent. Each is sent only when its payload actually differs from the
  // last one sent -- reset when the frame republishes its text, since the
  // agent's DOM was rebuilt then and needs the full repaint regardless.
  // The annotation singled out last, from either side: a card clicked in the
  // sidebar or a mark clicked in the document. Its card wears a ring and the
  // frame rings its passage. It clears when the comment panel closes or the
  // reader clicks somewhere other than a comment card or a mark.
  let selectedAnnotation = $state("");
  function applySelection() {
    frameOverlays.selection(selectedAnnotation);
  }
  $effect(() => { void selectedAnnotation; applySelection(); });

  function applyHighlights() {
    frameOverlays.annotations(comments);
  }

  // Source resolution determines whether the passage survived. Rendered
  // matching only locates its display mark; for readers, whose source state
  // is private, the published page is the only available display answer.
  function applyAnchorFlags(comment) {
    const { orphaned, inSourceOnly } = orphanState({
      renderedFound: comment.start != null,
      sourceFound: comment.sourceStart != null,
      sourceKnown: comment.original_anchor?.kind === "source_text",
      wholeDocument: aboutWholeDocument(comment),
    });
    comment.orphaned = orphaned;
    comment.inSourceOnly = inSourceOnly;
  }

  // Where to draw each comment, and where its passage is in the source.
  //
  // The two are different questions with different answers. Where to draw it
  // is a question about the page in front of this browser, and the rendered
  // quotation is what answers it -- display evidence, matched against display
  // text, and nothing follows from it being wrong beyond a mark in the wrong
  // place. Where the passage is in the source is not this browser's question
  // at all: the server resolved it through the document's own history and
  // sent the answer, and `placeSources` only translates that answer's file id
  // into a path.
  //
  // When the rendered quotation no longer matches because the passage was
  // edited, the passage's current words at the server's source range are
  // looked for on the page instead, so an edited passage keeps its mark. Only
  // a comment that the quotation could not place is given this fallback.
  function anchorComments(list) {
    anchorAll(docText || "", list, docText === null ? null : docView);
    if (mayEdit) placeSources(session, list);
    if (mayEdit && docText !== null) {
      let tree = null;
      for (const comment of list) {
        if (comment.start != null || comment.sourceStart == null || !comment.sourcePath) continue;
        tree ??= treeNow();
        const text = tree.texts[comment.sourcePath];
        if (typeof text !== "string") continue;
        const format = renderers.formatOf(comment.sourcePath) || "";
        const range = sync.documentRangeFor(text, comment.sourceStart, comment.sourceEnd, docText, format);
        if (range) {
          comment.start = range.start;
          comment.end = range.end;
        }
      }
    }
    for (const comment of list) applyAnchorFlags(comment);
  }

  function reanchor() {
    if (!frameReady || !commentsReady || docText === null) return;
    anchorComments(comments);
    annotations.publish();
    applyHighlights();
    tracePassages();
  }

  // Scroll tracking for mobile bar hide/show: called by scroll listeners and
  // frame scroll messages. Hide the bar when scrolling down past threshold,
  // show it when scrolling up or at top.
  function noteScroll(delta, top, bottom) {
    barHidden = compact ? barHiddenAfter(barHidden, delta, top, bottom) : false;
  }

  function fromFrame(message) {
    switch (message.type) {
      case "ready":
        docText = typeof message.text === "string" ? message.text : "";
        docView = flatten(docText);
        // The first time the frame says it is there is the first moment
        // anything can be sent to it. A paint made before this went to a
        // window that had not navigated yet and was lost -- which is what a
        // reader saw as a blank document, since a reader makes no edits to
        // trigger a second one. Only the first `ready` paints: the agent
        // sends one after every repaint, and painting on each would be a
        // loop.
        const first = framePreview.markReady();
        frameReady = true;
        if (first) replayPreview();
        // Whatever was painted before is gone with the rebuilt DOM.
        frameOverlays.reset();
        reanchor();
        applySelection();
        if (first) {
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
      case "annotate":
        if (pending) annotate("comment");
        break;
      case "caret":
        followDocumentClick(Number(message.offset) || 0, message.pdf);
        break;
      case "pdf-caret":
        followPdfClick(message.pdf);
        break;
      case "focus":
        void focusAnnotation(message.id);
        break;
      case "unfocus":
        selectedAnnotation = "";
        break;
      case "scroll":
        noteScroll(
          Number(message.delta) || 0,
          Number(message.top) || 0,
          Number.isFinite(Number(message.bottom)) ? Number(message.bottom) : Infinity,
        );
        break;
    }
  }

  /* --------------------------------------------------------------- selection */

  // Making an annotation is one gesture: select the words, then choose what
  // to do to them -- comment on them, or highlight them. See
  // `lib/annotating.js`.
  //
  // What the bar offers over what is selected. Nothing is offered when
  // nothing is.
  const renderedProjectCurrent = $derived.by(() =>
    mayEdit || Boolean(renderedProjectDigest && renderedProjectDigest === currentProject?.project_digest),
  );
  const shownVerbs = $derived.by(() => pending && renderedProjectCurrent ? VERBS : []);
  let highlightColor = $state(HIGHLIGHT_COLORS[0]);
  let palette = $state(false);
  let pending = $state(null);
  let assistantRequest = $state(null);
  let selectionRevision = Promise.resolve("");
  let bar = $state({ shown: false, left: 0, top: 0, center: 0 });

  // Mobile top bar slide behaviour: on phones (compact ≤760px), hide the bar
  // when scrolling down, show it again when scrolling up. The bar slides
  // away via transform with CSS transitions.
  let barHidden = $state(false);

  // What was selected, as the page had it. That is all this browser sends and
  // all it can honestly send: which passage of which source file those words
  // came from is the server's answer, worked out against the checkpoint it
  // holds -- a reader of a published document has no source here to answer it
  // from, and an editor's answer would still have to be checked.
  function showSelection(selector, rect) {
    if (!selector?.exact) {
      bar = { ...bar, shown: false };
      pending = null;
      return;
    }
    pending = {
      exact: String(selector.exact),
      prefix: String(selector.prefix || ""),
      suffix: String(selector.suffix || ""),
      position: Number.isInteger(selector.position) && selector.position >= 0 ? selector.position : null,
    };
    pending.render_digest = mayEdit ? "" : renderedProjectDigest;
    placeBar(rect);
  }

  // The agent runs only in the desktop app. A browser says so, with a way to get it.
  function agentNeedsDesktop() {
    if (hasDesktop) return false;
    say("The agent needs the LibrePaper desktop app.", {
      kind: "note",
      id: "reader:agent-desktop-only",
      action: { label: "Get the desktop app", onClick: () => window.open(localQuarto.INSTALL_URL, "_blank", "noopener") },
    });
    return true;
  }

  function askDiagnostic(item) {
    if (agentNeedsDesktop()) return;
    assistantRequest = { id: crypto.randomUUID(), diagnostic: { ...item }, revision: item.revision || "",
      task: { kind: "fix", scope: item.file || item.path ? "file" : "document" } };
    showPanel("agent");
    if (width <= 760) showMobileView("sidebar");
  }

  function askCommentAssistant(comment) {
    if (agentNeedsDesktop()) return;
    if (!comment?.id) return;
    const target = comment.original_anchor?.kind === "source_text"
      ? comment.original_anchor.target : null;
    const source = target ? {
      path: comment.sourcePath || session?.paths?.get(target.file_id) || "",
      exact: target.exact,
      prefix: target.prefix || "",
      suffix: target.suffix || "",
      position: comment.sourceStart ?? target.start_utf16,
    } : null;
    // Keep the server-owned comment token separate from source and render
    // identities. In particular, source_sequence is not a tree digest.
    const revision = "";
    const renderDigest = comment.render_digest || "";
    assistantRequest = {
      id: crypto.randomUUID(),
      comment: {
        id: String(comment.id), body: comment.body || "", exact: shownSelector(comment).exact,
        revision,
        render_digest: renderDigest,
        source, replies: (comment.replies || []).map((reply) => ({ body: reply.body, creator: reply.creator })),
      },
      selection: source,
      revision,
      comment_id: String(comment.id),
      render_digest: renderDigest,
    };
    showPanel("agent");
    if (width <= 760) showMobileView("sidebar");
  }

  // Candidate previews are rendered from the runner's immutable file snapshot
  // in this browser. Nothing is written to the shared document; only the
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
      title: headingOf, digest: renderers.snapshotDigest });
  }

  // A task's tracked changes are reviewed in the Changes panel, narrowed to
  // the ids the task returned. Separate runs of one task all stay on show.
  async function reviewAssistantResults({ changes: ids = [] }) {
    const made = changeRows.find((row) => ids.includes(row.id));
    changesIds = ids.filter((id) => changeRows.some((row) => row.id === id));
    await showPanel("changes");
    if (made) await revealChange(made);
  }

  async function focusAnnotation(id) {
    const comment = comments.find((item) => item.id === id);
    if (!comment) return;
    selectedAnnotation = String(comment.id);
    const plainHighlight = comment.motivation === "highlighting" && !comment.body && !comment.replies?.length;
    collaborationTab = plainHighlight ? "highlights" : "comments";
    void showPanel("collaboration");
    const targetPanel = panel;
    await tick();
    if (panel !== targetPanel) return;
    const prefix = plainHighlight ? "collaboration-highlight" : "collaboration-comment";
    const card = document.getElementById(`${prefix}-${id}`);
    card?.focus({ preventScroll: true });
    await tick();
    card?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }

  async function revealAnnotation(comment) {
    selectedAnnotation = String(comment.id);
    if (comment.start != null) {
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
  // The bar goes next to the selection at every width. On a phone the visible
  // area is what the keyboard and the browser's own bars leave, which is the
  // visual viewport rather than the window, so that is what keeps the bar from
  // sinking out of sight.
  function placeBar(rect) {
    if (rect) {
      const frame = document.querySelector(".viewport").getBoundingClientRect();
      const visibleHeight = window.visualViewport?.height ?? innerHeight;
      bar = {
        shown: true,
        ...placeSelectionBar({
          rect,
          frame,
          width: barElement?.offsetWidth || 250,
          minTop: belowTheBar(),
          maxTop: visibleHeight - (barElement?.offsetHeight || 40) - 8,
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

  // How wide the bar is is only knowable once it is drawn, so the placing
  // above is made good here, after it is: re-centred on the point it was
  // centred on, now that the width it was centred with is the real one.
  $effect(() => {
    if (!bar.shown || !barElement) return;
    const left = recenteredLeft({
      center: bar.center,
      left: bar.left,
      width: barElement.offsetWidth,
      windowWidth: innerWidth,
    });
    if (left !== null) bar = { ...bar, left };
  });

  /* -------------------------------------------------------------- annotating */

  // The draft being written, or null. It holds the anchor it was started
  // from rather than reading `pending`, so a stray selection elsewhere in
  // the document cannot move a note that is half written onto other words.
  let composing = $state(null);
  let identifying = $state(false);
  let deleting = $state(false);
  let pendingDelete = $state([]);

  const needsLogin = $derived(!identity && Boolean(me.comments_need_login));

  // Opening the draft in the column, and showing the reader the column it is
  // in: a composer in a panel nobody is looking at is a dialog that failed to
  // open.
  function openDraft() {
    if (!pending) return;
    composing = {
      // A fresh id per draft: the composer keeps the words being typed in its
      // own state, so starting a second draft has to be a second component
      // rather than the first one handed new props.
      id: crypto.randomUUID(),
      pending,
    };
    collaborationTab = "comments";
    if (panel !== "collaboration") void showPanel("collaboration");
    if (compact) showMobileView("sidebar");
  }

  // A verb chosen on the bar over the selection. Highlighting is the one that
  // never opens a draft: the passage is the whole annotation, so the click
  // that names the verb is also the one that makes it.
  function annotate(which) {
    if (!pending || !mayChat) return;
    if (!renderedProjectCurrent) {
      say("The source changed and the current page is still rendering. Your selection is kept; retry after the page updates.", { kind: "problem", id: "reader:source-rendering" });
      return;
    }
    bar = { ...bar, shown: false };
    palette = false;
    if (which === "highlight") {
      submitAnnotation({ motivation: "highlighting", body: "" });
      return;
    }
    if (!identity && me.providers?.length) {
      identifying = true;
      return;
    }
    openDraft();
  }

  // Picking a colour highlights with it, rather than arming a colour for a
  // second click: the swatches only ever open from Highlight, so the verb is
  // already chosen by the time one is pressed.
  function highlightWith(color) {
    highlightColor = color;
    annotate("highlight");
  }

  function submitAnnotation({ motivation, body }) {
    if (!pending || !mayChat) return false;
    if (!mayEdit && pending.render_digest !== currentProject?.project_digest) {
      say("The source changed since this passage was selected. Wait for the current page to finish rendering, then select the passage again; the draft is kept.", { kind: "problem", id: "reader:source-moved" });
      return false;
    }
    // The server determines the author when it acknowledges the submission.
    annotations.comment(pending, { motivation, body, color: motivation === "highlighting" ? highlightColor : undefined }, identity || doc.commenting_as || "Anonymous");
    pending = null;
    return true;
  }

  // What the composer sends. The passage comes from the draft rather than
  // from `pending`, so a stray selection made while someone is typing cannot
  // retarget what they are writing about, and a refusal leaves the card open
  // with the words still in it.
  //
  // One exception, and it is the recovery this reader asks for by name: when
  // the document has been published to since the draft was started, it says
  // "refresh, select it again, and submit". Selecting it again has to mean
  // something, so a fresh selection made against the projection now on
  // screen replaces the one the draft was holding -- which is stale by then
  // and would only be refused.
  function sendDraft({ body }) {
    if (!composing) return false;
    const held = composing.pending;
    const moved = !mayEdit && held.render_digest !== currentProject?.project_digest;
    const reselected = moved && pending && pending.render_digest === renderedProjectDigest;
    pending = reselected ? { ...pending } : held;
    if (!submitAnnotation({ motivation: motivationFor("comment"), body })) return false;
    composing = null;
    return true;
  }

  function cancelDraft() {
    composing = null;
    pending = null;
  }

  const resolve = annotations.resolve;
  const edit = annotations.edit;

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

  // The pending changes in the document, as one row each. They are read from
  // the shared document itself, so an edit, an accept or a reject by anyone
  // arrives here the same way.
  let changeRows = $state([]);
  let selectedChange = $state(null);
  // The change ids the Changes panel opens filtered to, when a task sent the
  // reader there.
  let changesIds = $state([]);

  function recomputeChanges() {
    if (!session) {
      // A reader or commenter has no live document; their rows come with the
      // current project (`readerChangeRows`), so there is nothing to clear.
      if (mayEdit) changeRows = [];
      return;
    }
    changeRows = listChanges(session.doc).map((change) => ({
      id: change.id,
      file_id: change.file,
      path: session.paths?.get(change.file) || "",
      author: change.author,
      who: change.who,
      run: change.run || "",
      at: change.at,
      position: change.from,
      before: change.deleted,
      after: change.inserted,
    }));
    if (selectedChange && !changeRows.some((row) => row.id === selectedChange)) selectedChange = null;
  }

  // The pending changes a reader or commenter is sent with the current
  // project, in the row shape the live document gives an editor.
  function readerChangeRows(list) {
    return (list || []).map((change) => ({
      id: change.id,
      file_id: change.file,
      path: change.path,
      author: change.author,
      who: change.who,
      run: change.run || "",
      at: change.at,
      position: change.from,
      before: change.deleted,
      after: change.inserted,
    }));
  }

  // Recomputing reads every change, and the thing that asks for it most often
  // is a keystroke. So typing coalesces: the rows a reviewer is reading settle
  // once the typing stops.
  const REVIEW_COALESCE_MS = 200;
  let reviewTimer = null;
  let changesUnsubscribe = null;
  function refreshReview() {
    clearTimeout(reviewTimer);
    reviewTimer = setTimeout(recomputeChanges, REVIEW_COALESCE_MS);
  }

  // Whether what this browser types is recorded as a pending change. The
  // switch is in the Changes panel; the recording is in the editor, which is
  // where the text is.
  const trackingMe = $derived({
    who: me.id || identity || doc.commenting_as || "Anonymous",
    author: identity || doc.commenting_as || "Anonymous",
  });
  let tracking = $state(false);
  function setTracking(on) {
    if (!editor) return;
    if (on) editor.startTracking(); else editor.stopTracking();
    // What the editor actually did, not what was asked of it.
    tracking = editor.trackingOn?.() ?? Boolean(on);
    if (on && !tracking) {
      say("Track changes could not be turned on: this document is not connected for editing.", { kind: "problem", id: "reader:tracking" });
    }
  }

  /// Look at a change: open the file it belongs to, put the caret where it
  /// starts, and mark it as the one being reviewed so the editor outlines it.
  async function revealChange(row) {
    if (!row) return;
    selectedChange = row.id;
    if (!editing) return;
    if (!row.file_id || session?.textOf && !session.textOf(row.file_id)) {
      say("The file of this change cannot be opened in the current paper.",
        { kind: "problem", id: "reader:change-file" });
      return;
    }
    showMobileView("source");
    await tick();
    if (shown.source && editor) {
      ws.openFile = row.file_id;
      editor.goToIn(row.file_id, row.position);
    }
  }

  /// Accept and reject are plain edits to the shared document, so they need no
  /// answer from the server. They return a resolved promise so the panel can
  /// advance the same way it would after a round trip.
  function acceptChange(row) {
    if (mayEdit && row?.id) accept(session.doc, row.id);
    return Promise.resolve();
  }

  function rejectChange(row) {
    if (mayEdit && row?.id) reject(session.doc, row.id);
    return Promise.resolve();
  }

  /// Rejects several changes in one commit, owners before the changes they
  /// swallowed. Resolves to the number rejected.
  function rejectChanges(rows) {
    const count = mayEdit ? rejectMany(session.doc, rows.map((row) => row.id)) : 0;
    return Promise.resolve(count);
  }

  function receive(event) {
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
      // The first page of the comments, and what the server says is behind
      // it. Never the collection: "Load more" walks the rest, and the
      // counts in the panel come from `state` rather than from this list.
      annotations.seed(event.comments, event.state);
      commentsReady = true;
      reanchor();
      if (panel === "history" && !labels.length) void loadHistory();
      return;
    }
    if (event.type === "submission-failed") {
      outbox.failed(event.temp_id, event.message);
      return;
    }
    if (event.type === "error") {
      // An update the server had no room to hold. It is
      // still here, still unsaved, and the session resends it on a timer
      // from the head the refusal named -- so recovery does not wait on
      // another keystroke. Told once, quietly: the toolbar already shows
      // unsaved work, and a refusal a second is not news.
      if (event.retryable && event.vector && session) {
        session.pressure(event.vector);
        say("This server is busy saving; your work is kept here and will be saved shortly.",
          { kind: "problem", id: "reader:save-delayed" });
        return;
      }
      if (event.temp_id && pendingChat?.has(event.temp_id)) {
        pendingChat.acknowledge(event.temp_id, false);
        say(event.message || "The server rejected that chat message.", { kind: "problem", id: "reader:chat-rejected" });
        return;
      }
      // A comment on rendered text was refused because the passage's
      // projection moved under it before the comment landed
      // (`CommandError::StaleSelection`, room.md "Annotation writes"). The
      // server names the digest a retry should read against;
      // a reader holds no source to reconcile a stale selection by hand, so
      // the useful response is to refresh to that projection -- which also
      // brings `renderedProjectDigest` current for the next attempt -- and
      // ask the reader to reselect, rather than a generic conflict toast.
      if (event.stale_source && event.digest) {
        outbox.failed(event.temp_id, "That passage changed before the comment could be attached.");
        if (event.temp_id) annotations.removePending(event.temp_id);
        renderedProjectDigest = event.digest;
        if (!mayEdit) void refreshCurrentProject();
        say(
          "The page changed while that comment was being placed. It has been refreshed to the current text; reselect the passage and try again.",
          { kind: "problem", id: "reader:stale-selection" },
        );
        return;
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
        // Re-read the pages this browser is holding, from the first
        // forward. The controller owns the request -- it carries the link
        // key, drops a response an even newer refresh has overtaken, and
        // leaves the comments on screen alone if it fails.
        void annotations.refresh();
      }
      say(event.message || "The server refused that change. Nothing on screen has changed.", { kind: "problem", id: "reader:change-refused" });
      return;
    }

    // The shared document: the state of the session as it stands, one more
    // change to it, what the server has written, who else is in it, or where
    // their carets are. A reader receives all of this too -- that is how they
    // see the current text -- and sends none of it.
    if (event.type === "doc-state") {
      const active = session;
      active
        ?.start(event)
        .then(() => {
          if (active !== session) return;
          paintPreview();
        })
        .catch((error) => {
          say(error.message || "This document could not be opened.", { kind: "problem", id: "reader:open-failed" });
        });
      peers = event.count || 1;
      return;
    }
    if (event.type === "doc-rows") {
      // The rows this browser's vector did not cover, plus whatever is still
      // in the server's buffer (§6.2). It is a join like `doc-state`, and it
      // is awaited the same way: `doc-rows` can arrive as the whole join
      // reply on its own, so `session.rows` awaits hydration itself, and it
      // is sequenced behind any `start` that is still mid-fetch for a
      // referenced base (project-session.js `joinGate`), so it can no longer
      // be assumed synchronous.
      const active = session;
      active
        ?.rows(event)
        .then(() => {
          if (active !== session) return;
          paintPreview();
        })
        .catch((error) => {
          say(error.message || "This document could not be opened.", { kind: "problem", id: "reader:open-failed" });
        });
      return;
    }
    if (event.type === "doc-gap") {
      // The server refused a batch because it did not cover what that batch
      // started from, and dropped it rather than queueing it (§5 step 4).
      // Nothing is lost: exporting from the vector the server named resends
      // the refused operations along with anything else it is missing.
      session?.gap(event.vector);
      return;
    }
    if (event.type === "source-changed") {
      // Readers and commenters never join CRDT sync (§2.1), so this frame is
      // the only way this browser learns the head moved: refetch the
      // projection, carrying the digest this fetch already remembers as the
      // etag (§11). An editor's own `session` hears every edit directly and
      // has nothing to do here.
      if (!mayEdit) void refreshCurrentProject();
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
      // Retire transmission bookkeeping. An empty catch-up can be acknowledged
      // while earlier work is still buffered, so this does not prove saving.
      session?.acknowledge(event.upTo || 0);
      return;
    }
    if (event.type === "doc-durable") {
      session?.durable(event.vector);
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

    if (event.type === "project-changed") {
      scheduleCurrentProjectRefresh();
      return;
    }
  }

  /* ----------------------------------------------------- reading and editing */

  // CodeMirror is a third of a megabyte, and most people who open a document
  // are here to read it. The editor component is fetched when one is actually
  // opened, so a reader never pays for it. The session is not the editor: a
  // reader joins it too, because that is where the text comes from.
  let Editor = $state(null);
  let editor = $state(null);
  // The palettes are overlays over the editor. Their components are kept so a
  // command can ask the Math overlay to open a template, or the Symbols overlay
  // to take the focus.
  let symbolPalette = $state(null);
  let mathPalette = $state(null);
  let symbolsOpen = $state(false);
  let mathOpen = $state(false);
  // Raw on purpose: the session is a bag of Loro types and functions, and the
  // collaboration module hands the same object back in its callbacks, which
  // are compared to this by identity. A deep proxy would never be equal to it.
  let session = $state.raw(null);
  let editing = $state(false);
  // Which text the editor is bound to. Keying the component on this binds it
  // to the current main file when another file becomes main.
  let sourceEpoch = $state(0);
  // Whether there is a main text to edit yet. `session.text` is a getter over
  // a handle the session rebinds when the document hydrates, which happens
  // outside Svelte's knowledge: reading it straight in the markup is read once
  // and never again, so a document that arrives after the editor module has
  // loaded leaves the source pane blank until something unrelated -- opening a
  // panel, say -- happens to re-run that branch. `sourceEpoch` is the signal
  // that a rebind occurred, so the read is tied to it.
  const sourceBound = $derived.by(() => {
    void sourceEpoch;
    return Boolean(session?.text);
  });
  let mayEdit = $state(false);
  let sourceFormat = $state("");

  // Where the toggle is offered: every editable Quarto document, and an
  // editable Typst document when the desktop app found Calepin.
  const runCodeOffered = $derived(mayEdit && (sourceFormat === "quarto" || (sourceFormat === "typst" && hasDesktop && calepinFound)));
  let peers = $state(1);
  let participants = $state([]);
  let linked = $state(read(LINKED, false) === true);

  // Routine saving stays quiet; losing the connection still needs a warning,
  // and the warning has to say what is happening to the typing meanwhile.
  const connectionNote = $derived.by(() => {
    if (connected) return "";
    if (openedLocally) return "Offline. This is the copy saved in this browser, and it may be missing changes made elsewhere.";
    if (!mayEdit || !session) return "Reconnecting…";
    return persistence.local
      ? "Offline; changes are kept in this browser while reconnecting…"
      : "Offline; reconnecting…";
  });

  // A close is only worth interrupting when the work has reached neither this
  // browser's storage nor the server.
  const atRisk = $derived(Boolean(mayEdit && (persistence.localError || (persistence.pending && !persistence.local))));

  // What the document is called, which is what the rendered page is titled.
  // The title it was published under wins; a document that never had one is
  // named by its own first heading, the way `publish` names one -- the *main*
  // file's first heading, since a chapter's heading names the chapter.
  async function headingOf(tree) {
    return doc.title || (await renderers.titleOf(tree)) || "Untitled";
  }

  // Scopes the desktop client to this document and says so in Diagnostics
  // when this is a browser.
  function ensureLocalApp() {
    localConnectionError = "";
    if (!hasDesktop) {
      localConnectionError = localQuarto.DESKTOP_ONLY_MESSAGE;
      showPanel("diagnostics");
      return false;
    }
    localQuarto.configure({ project: SLUG, origin: location.origin });
    return true;
  }

  /* --------------------------------------------------------- the timeline */

  // What this document used to say, and when. The manifest is fetched when the
  // panel is opened and not before: a reader who never asks for the history
  // costs no request for it.
  // The comparison state and its async lifetimes live in the controller. The
  // aliases keep the existing panel and redline code readable while making
  // every value a focused controller getter rather than a Reader-owned bag.
  // The label being shown in the document pane, whole -- its tree and its
  // texts -- or null for the document as it stands.

  // Whether this browser should be running Quarto's own live preview rather
  // than showing its own draft rendering: Quarto preview mode chosen, ready,
  // connected, editable, and not looking at history. `editing` (the source
  // pane) is not required -- an editor who has not opened it yet still gets
  // the live pane the moment they are able to edit.
  const quartoLiveActive = $derived.by(() =>
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && localExecution && (!buildPreferences.output || ["html", "pdf"].includes(buildPreferences.output)) && (buildPreferences.selection === "automatic" || (buildPreferences.backend === "local" && buildPreferences.tool === "quarto")) && mayEdit && hasDesktop,
  );

  // Whether this browser should be showing a Typst document's chunks run by
  // Calepin on this computer rather than this browser's own Typst rendering:
  // the calepin mode chosen, ready, the calepin command itself
  // found, editable, and not looking at history.
  const calepinActive = $derived.by(() =>
    sourceFormat === "typst" && !typstHtmlPreview && localExecution && typstPreviewMode === "calepin" && buildPreferences.backend === "local" && buildPreferences.tool === "calepin" && mayEdit && hasDesktop && calepinFound,
  );

  async function localPreviewTreeNow() {
    const tree = treeNow();
    if (!Object.keys(tree.digests || {}).length) return tree;
    const { held, missing } = await gatherFigures(tree.digests);
    if (missing.length) throw new Error(`Local preview is missing ${missing.join(", ")}`);
    return { ...tree, assets: held.assets };
  }

  // A sync can forget a folder the desktop app no longer grants. Mirroring that
  // here restarts the preview in the hosted workspace.
  const previewClient = {
    ...localQuarto,
    syncWorkspace: async (input) => {
      const result = await localQuarto.syncWorkspace(input);
      const id = localQuarto.bindingId();
      if (id !== quartoBindingId) quartoBindingId = id;
      return result;
    },
  };

  const quartoPreviewController = createLocalPreview({
    local: previewClient,
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
    // syncWorkspace writes the browser's tree to the project's binding, the
    // chosen project folder when there is one, and the preview runs in that same place.
    jobOf: () => ({ binding: localQuarto.bindingId() }),
    isDisposed: () => readerDisposed,
    // A managed preview taking the pane invalidates whatever this browser
    // was rendering into it.
    onStartingChange: (value) => { if (value) navigationGeneration += 1; },
    onEnded: () => void paintPreview(),
  });

  const calepinPreviewController = createLocalPreview({
    local: previewClient,
    engine: "calepin",
    label: "Calepin",
    publish: (payload) => framePreview.publish(payload),
    treeNow: localPreviewTreeNow,
    entrypointOf: (tree) => tree.main,
    optionsOf: () => ({ format: "pdf" }),
    jobOf: () => ({ binding: localQuarto.bindingId() }),
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
  const quartoPreviewKept = $derived(quartoPreviewController.state.kept);
  const calepinPreview = $derived(calepinPreviewController.state.session);
  const calepinPreviewStarting = $derived(calepinPreviewController.state.starting);
  const calepinRendering = $derived(calepinPreviewController.state.rendering);
  const calepinPreviewError = $derived(calepinPreviewController.state.error);
  const calepinPreviewKept = $derived(calepinPreviewController.state.kept);

  // Drives the whole automatic mode for each engine: starts the managed
  // preview the moment its conditions are met, and tears it down (falling
  // back to the ordinary rendering, no retry) the moment any of them stop
  // holding.
  const localPreviewSwitches = createGeneration();
  $effect(() => {
    // The binding is part of the target so choosing a folder restarts the preview there.
    // The output format is too, because Quarto reads it only when it starts, so
    // switching View between HTML and PDF restarts the preview.
    const quartoTarget = quartoLiveActive && previewMain && `${previewMain}|${quartoBindingId}|${buildPreferences.output || ""}`;
    const calepinTarget = calepinActive && previewMain && `${previewMain}|${quartoBindingId}`;
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

  // Calepin is found by the desktop app, not by this page, so it is asked
  // when a Typst document opens there. Whether it is found decides whether
  // the Typst document offers the toggle.
  $effect(() => {
    if (!hasDesktop || sourceFormat !== "typst" || !mayEdit) return;
    let stale = false;
    localQuarto.calepinAvailable().then((found) => { if (!stale) calepinFound = found; }, () => { if (!stale) calepinFound = false; });
    return () => { stale = true; };
  });

  // The profile and parameters, applied from Settings: kept for every Quarto
  // build in this browser, and the live preview -- which reads them only as
  // it starts -- restarted so the page shows the new ones rather than the
  // old until the next reconnect. A preview still starting reads the options after its
  // workspace sync, so it picks them up on its own.
  async function applyRenderOptions(next) {
    const options = parseRenderOptions({ ...next, format: "default" });
    writeQuartoOptions(buildScope(), { profile: options.profile || null, parameters: options.parameters || {} });
    if (sourceFormat === "markdown" || sourceFormat === "quarto") {
      setBuildPreferences(readBuildPreferences(buildScope(), sourceFormat));
    }
    if (!quartoLiveActive) return;
    await quartoPreviewController.stop();
    if (quartoLiveActive) await quartoPreviewController.start();
  }

  // The manifest, and whether the last read of it succeeded, belong to the
  // timeline; the comparison below owns everything about a selected version.
  const timeline = createTimeline({
    slug: SLUG,
    key: KEY,
    // Open on what the restore replaced: the version just before the
    // restore's own row. Compared against now, that is the change the
    // restore made, and its Restore button is the undo.
    onrestored: () => {
      const newest = [...timeline.state.labels].sort((a, b) => b.seq - a.seq);
      return historySource.select(newest[0]?.why === "restore" ? newest[1]?.sha || "" : "");
    },
    disposed: () => readerDisposed,
  });
  const labels = $derived(timeline.state.labels);
  const historyProblem = $derived(timeline.state.problem);
  // Selection is a user-interface fact, not proof that the source could be
  // read: a failed comparison reports itself in the workspace and cannot make
  // a clicked version appear unselected.
  const historySource = createHistorySource({
    read: sha => history.read(SLUG, sha, keyHeaders(KEY)),
    labels: () => labels,
    preferredPath: () => session?.paths?.get(openFile) || "",
    currentTree: () => {
      if (session && session.joined !== false) return session.tree();
      // A label URL can arrive before the initial collaborative state.
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
  // Which file the reader arrived asking for, when the landing page's search
  // found the project by one of its files. Honoured once the directory has
  // arrived, and once only.
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
  // The base64 Loro frontier of the session as it stands right now: what a
  // restore's `expected_frontier` names (§7.1), so the server can tell a
  // concurrent edit nobody here has seen from the moment this browser is
  // actually asking to replace. `loro-crdt` is imported dynamically because
  // restoring is an editor-only action and a reader, who never opens a
  // source session, must not pay for the wasm to reach this line.
  async function projectFrontier() {
    if (!session?.doc) return "";
    const { encodeFrontiers } = await import("loro-crdt");
    const bytes = encodeFrontiers(session.doc.frontiers());
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary);
  }
  const restoreLabel = async (sha) => timeline.ask(sha, projectSignature(), await projectFrontier());
  const confirmRestore = async () => timeline.confirm(projectSignature(), await projectFrontier());
  const nameLabel = (sha, given) => timeline.name(sha, given);

  /// A version's plain-source archive is produced on request (room.md
  /// "Labels", §8.5): the first ask starts it, and it is a 404 until it
  /// finishes. Without this the only affordance would be a click that either
  /// does nothing visible for a while or silently 404s a moment later; this
  /// polls `archive_status` and keeps the panel saying so until it is
  /// `"ready"` (then downloads) or `"failed"` (then says that instead of
  /// spinning forever).
  let downloadingSha = $state("");
  let downloadProblem = $state("");
  async function downloadArchive(sha) {
    if (downloadingSha === sha) return;
    downloadingSha = sha;
    downloadProblem = "";
    try {
      let retry = true;
      for (;;) {
        const label = await history.requestArchive(SLUG, sha, keyHeaders(KEY), retry);
        retry = false;
        if (downloadingSha !== sha) return; // superseded by a later request
        if (label.archive_status === "ready") {
          const anchor = document.createElement("a");
          anchor.href = history.archiveUrl(SLUG, sha);
          anchor.download = "";
          anchor.click();
          return;
        }
        if (label.archive_status === "failed") {
          downloadProblem = "That version's archive could not be prepared.";
          return;
        }
        await new Promise((resolve) => setTimeout(resolve, 1500));
      }
    } catch (error) {
      if (downloadingSha === sha) downloadProblem = error.message || "That version's archive could not be prepared.";
    } finally {
      if (downloadingSha === sha) downloadingSha = "";
    }
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
    paths: () => session?.paths,
    loadLabels: async () => {
      await loadHistory();
      return timeline.state.labels;
    },
  });
  const went = $derived(passageTrace.state.went);
  const replacements = $derived(passageTrace.state.replacements);

  async function tracePassages() {
    if (docText === null || !session) return;
    return passageTrace.trace({ comments, tree: session.tree(), labels });
  }

  // The document as a renderer takes it: every text in it, the figures by
  // digest, and which file is the document. A compiler given only the main
  // file produces the error a reader would otherwise be shown.
  //
  // Before a session arrives there is no source tree to render.
  // The file manager always operates on the live directory. In particular,
  // its list remains live while the document pane is showing a label, so
  // downloads must use the same source as that list.
  function liveTreeNow() {
    if (!session) {
      if (!currentProject) return { main: "", texts: {}, digests: {} };
      const digests = {};
      for (const [path, file] of Object.entries(currentProject.tree?.files || {})) {
        if (file?.kind === "asset" && file.digest) digests[path] = file.digest;
      }
      return {
        main: currentProject.main || currentProject.tree?.main || "",
        texts: { ...(currentProject.texts || {}) },
        digests,
        settings: currentProject.tree?.settings || null,
      };
    }
    const tree = session.tree();
    if (!tree.main) return tree;
    // CodeMirror owns the active Loro binding and exposes the text it is
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
  // is explicit because document changes happen outside Svelte's normal tracking;
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
    local: () => [...localAppDiagnostics, ...backupDiagnostics, ...previewNoteItems],
    failure: () => buildFailure,
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

  const errorCount = $derived(diagnostics.filter((d) => d.severity !== "warning" && d.severity !== "info").length);
  const warningCount = $derived(diagnostics.filter((d) => d.severity === "warning").length);
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
    collaboration: unreadChat ? { says: "Collaboration: unread messages", dot: true } : {},
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
  // The documents origin serves only an empty shell. This reader fetches the
  // source projection with its credentials, renders it here, then sends the
  // resulting HTML to the frame over an origin-checked message. The frame URL
  // carries only `navigateFrame`'s refresh generation, never a credential.
  //
  // And a label is always painted, whatever the format: the frame is
  // served from the live document, so there is nothing on the documents origin
  // that is the document as it was on Tuesday.
  const displayedFormat = $derived(
    sourceFormat,
  );
  // This browser-only choice affects the pane and is never published. It is
  // not conditioned on the source pane being open: HTML is the default output
  // of every format, so somebody who only reads the document gets the same
  // page its author is looking at rather than a paged fallback.
  const previewFormat = $derived.by(() =>
    ((displayedFormat === "typst" && typstOutput === "html") ||
      (displayedFormat === "latex" && latexOutput === "html")) ? "html" : displayedFormat,
  );
  const previewOutputKind = $derived.by(() =>
    ["markdown", "quarto"].includes(displayedFormat) && buildPreferences.output === "pdf"
      ? "pdf"
      : renderers.outputKind(previewFormat),
  );
  const typstHtmlPreview = $derived.by(() => displayedFormat === "typst" && typstOutput === "html");
  const latexHtmlPreview = $derived.by(() => displayedFormat === "latex" && latexOutput === "html");
  const paintsTheFrame = $derived(true);

  /* -------------------------------------------------------------- LaTeX */

  // Paged source formats have no HTML to paint, so their frame is the PDF
  // viewer on the documents origin rather than the empty shell. Everything
  // else about the frame is the same: same origin, same CSP, same channel.
  const pdfOutput = $derived(previewOutputKind === "pdf");
  const framePath = $derived(previewOutputKind === "pdf" ? "pdf" : "raw");

  // How long the last compile took, and whether one is running now. Paged
  // formats expose the same short-lived loading state; the elapsed time is
  // especially useful for LaTeX, whose compiler can take seconds.
  const renderStatus = createRenderStatus();

  // These feed the diagnostics report in settings. They carry no document text
  // except the build log, which the user opts into.
  $effect(() => {
    const undo = [
      provide("document", () => ({ id: doc.document_id, format: sourceFormat, role: doc.role, mayEdit })),
      provide("account", () => ({ id: me.id, signedIn: Boolean(me.provider) })),
      provide("sync", () => ({
        connected,
        joined: persistence.joined,
        pending: persistence.pending,
        localPending: persistence.localPending,
        ...collaboration?.syncStatus(),
      })),
      provide("engines", () => {
        const built = latex.status();
        const result = built.lastResult;
        return {
          format: sourceFormat,
          compiling: renderStatus.state.compiling,
          failure: Boolean(renderStatus.state.failure),
          latex: {
            phase: built.phase,
            backend: built.backend,
            engine: built.engine,
            release: built.release,
            ok: result?.ok,
            seconds: result?.seconds,
            diagnostics: result?.diagnostics?.length,
            errors: result?.diagnostics?.filter((item) => item.severity === "error").length,
          },
        };
      }),
      provide("buildLog", () => renderStatus.state.lastLog || renderStatus.state.lastLatexResult?.log || ""),
    ];
    return () => undo.forEach((unregister) => unregister());
  });
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
  const latexOutput = $derived(buildSettings.state.latexOutput);
  const typstOutput = $derived(buildSettings.state.typstOutput);
  const markdownOutput = $derived(buildPreferences.output === "pdf" ? "pdf" : "html");
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

  const configureLatex = (format, owner = session) => buildSettings.configureLatex(format, owner, buildUserId);

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

  // The LaTeX compile status: idle, running, or failed, with progress and
  // messages. Used by Diagnostics and the preview's progress line.
  let latexState = $state.raw(latex.status());
  const latexPhase = $derived(latexState.phase);
  $effect(() => latex.subscribe((next) => (latexState = next)));

  // A build that failed and said nothing a diagnostic could carry must
  // still be reported somewhere.
  const buildFailure = $derived(
    sourceFormat === "latex" && !latexHtmlPreview
      ? (latexPhase === "failed" ? latexState.message || "The LaTeX build failed without reporting why." : "")
      : renderState.failure ? renderState.failureReason || "The build failed without reporting why." : "",
  );

  // Quarto preview mode chosen, but this is a browser: the pane
  // shows the draft, and Diagnostics explains why.
  const quartoNeedsLocalApp = $derived(
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && localExecution && mayEdit &&
      !hasDesktop,
  );
  // A PDF from Markdown or Quarto source is only ever produced by a tool on
  // this computer: this browser's own renderer makes HTML and nothing else.
  // Until that tool is running the PDF frame has nothing to show, and the
  // "not yet rendered" card would otherwise claim a render is under way that
  // can never finish -- which is exactly the state an author lands in by
  // choosing PDF output and then closing, or never installing, the local app.
  const pdfNeedsLocalTool = $derived(
    pdfOutput && ["markdown", "quarto"].includes(sourceFormat) && mayEdit &&
      (!hasDesktop || (sourceFormat === "quarto" && !localExecution)),
  );
  // The gesture that is missing, told apart so the card offers one action
  // rather than a menu: Run code has to be on before anything else is worth
  // suggesting.
  const pdfNeedsLocalExecution = $derived(pdfNeedsLocalTool && sourceFormat === "quarto" && !localExecution);

  // The way out for an author who wants to keep reading rather than install
  // anything: the browser's own HTML preview, which every one of these
  // formats has.
  function previewAsHtml() {
    setBuildPreferences(updateBuildPreferences(buildScope(), sourceFormat, { output: "html" }));
    void paintPreview();
  }

  // The same two banner cases, for a Typst document with Calepin preview
  // chosen: this is a browser, or the desktop app has no calepin command.
  const typstNeedsLocalApp = $derived(
    sourceFormat === "typst" && !typstHtmlPreview && localExecution && typstPreviewMode === "calepin" && mayEdit &&
      !hasDesktop,
  );
  const typstNeedsCalepinCommand = $derived(
    sourceFormat === "typst" && !typstHtmlPreview && localExecution && typstPreviewMode === "calepin" && mayEdit &&
      hasDesktop && !calepinFound,
  );

  const localAppDiagnostics = $derived([
    !hasDesktop ? { severity: "error", message: localConnectionError } : null,
    quartoNeedsLocalApp && !localConnectionError ? { severity: "warning", message: "The Markdown preview is shown. Running code needs the LibrePaper desktop app." } : null,
    typstNeedsCalepinCommand ? { severity: "warning", message: "Calepin preview needs the calepin command on this computer." } : null,
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && localExecution ? { severity: "error", message: quartoPreviewError } : null,
    ...(sourceFormat === "quarto" && quartoPreviewMode === "quarto" && localExecution ? quartoPreviewKept : []).map((note) => ({ severity: "warning", message: note.charAt(0).toUpperCase() + note.slice(1) + "." })),
    sourceFormat === "typst" && typstPreviewMode === "calepin" && localExecution ? { severity: "error", message: calepinPreviewError } : null,
    ...(sourceFormat === "typst" && typstPreviewMode === "calepin" && localExecution ? calepinPreviewKept : []).map((note) => ({ severity: "warning", message: note.charAt(0).toUpperCase() + note.slice(1) + "." })),
  // An item with no message is dropped. An empty error would show as a blank
  // entry and would hide the failed render's own message.
  ].filter((item) => item?.message).map((item) => ({ ...item, source: "local-app" })));
  // Backups that are off say nothing. Only a failing backup is reported.
  const backupDiagnostics = $derived.by(() => {
    const data = backupStatus.active ? backupStatus.data : null;
    if (!data?.enabled) return [];
    const message = data.needs_login ? "Backups are on but the desktop app lost access to this account. Open Settings, then Backups, to reconnect."
      : data.error ? `Backups are on but failing: ${data.error}. Open Settings, then Backups.`
      : !data.destination_set ? "Backups are on but have no destination. Open Settings, then Backups."
      : "";
    return message ? [{ severity: "warning", message, source: "backups" }] : [];
  });
  // What the Diagnostics panel names when a preview here needs the desktop
  // app and this is a browser. Empty when nothing needs it.
  const desktopNeeds = $derived(
    hasDesktop ? ""
      : quartoNeedsLocalApp ? "Quarto preview"
      : typstNeedsLocalApp ? "Calepin preview"
      : pdfNeedsLocalTool ? "The PDF preview"
      : localConnectionError ? "This preview"
      : "",
  );
  $effect(() => {
    void localAppDiagnostics;
    void backupDiagnostics;
    void previewNoteItems;
    void buildFailure;
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


  /// Puts the last page this browser drew for this document into the frame,
  /// if the frame has nothing newer in it yet.
  async function restoreLastPreview() {
    const remembered = await lastPreview(previewCacheKey());
    // A compile can beat this; IndexedDB is fast but it is not free. The
    // cached page is only ever a stand-in for an empty pane.
    if (!remembered || readerDisposed || everPainted || !paintsTheFrame) return;
    framePreview.publish(remembered.kind === "pdf"
      ? { kind: "pdf", sha: remembered.identity, bytes: new Uint8Array(remembered.bytes).slice() }
      : { kind: "html", html: remembered.html });
  }

  const navigateFrame = (force = false) => framePreview.navigate(force);
  const replayPreview = () => paintsTheFrame && framePreview.replay();
  const frameLoaded = () => {
    const first = framePreview.markReady();
    frameReady = true;
    if (first) replayPreview();
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

  const compiling = $derived(Boolean(
    (sourceFormat === "latex" && !latexHtmlPreview && ["loading", "compiling", "browser-biber"].includes(latexPhase))
      || renderState.compiling || quartoPreviewStarting || quartoRendering || calepinRendering,
  ));
  // The preview pane's progress line shows this flag. A Typst or Markdown
  // render finishes in tens of milliseconds, and a control that mounts and
  // unmounts with each one strobes the line while somebody types.
  // `steady-busy.js` holds it: nothing appears until a render has lasted
  // long enough to be worth saying, and what appears stays long enough to
  // be read.
  let previewBusy = $state(false);
  const steadyBusy = createSteadyBusy({ onchange: (shown) => (previewBusy = shown) });
  // One subscription to `compiling`, feeding the one smoother. The smoother
  // is made outside the effect: rebuilding it per change would reset the hold
  // it exists to keep.
  $effect(() => {
    steadyBusy.set(compiling);
  });
  $effect(() => () => steadyBusy.stop());

  let previousConnected = null;
  $effect(() => {
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
    framePreview: {
      publish: (payload) => framePreview.publish(payload),
      clear: () => framePreview.clear(),
    },
    diagnostics: diagnosticPainter,
    renderers,
    snapshotDigest: renderers.snapshotDigest,
    diagnosticContext,
    parseSynctex,
    rememberPreview: (page, identity) => {
      rememberPreview(previewCacheKey(), page, identity);
    },
    onRendered: (_identity, projectDigest) => {
      if (!mayEdit) renderedProjectDigest = projectDigest || "";
    },
    onEmpty: () => {
      deliveredKind = "";
      everPainted = false;
      everPaintedShown = false;
      activeSynctex = null;
    },
    facts: () => ({
      disposed: readerDisposed,
      navigation: navigationGeneration,
      source: sourceGeneration,
      everPainted,
      format: sourceFormat,
      hasSession: Boolean(session),
      paintsTheFrame,
      latexOutput,
      typstOutput,
      buildPreferences,
      localExecution,
      projectDigest: mayEdit ? "" : currentProject?.project_digest || "",
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

  function sourceChanged() {
    if (readerDisposed) return;
    sourceGeneration += 1;
    historyLiveVersion += 1;
    refreshReview();
    outlineRevision += 1;
    if (typeof quartoLiveActive !== "undefined" && quartoLiveActive && typeof quartoPreview !== "undefined" && (quartoPreview || quartoPreviewStarting)) {
      clearTimeout(quartoLiveSyncTimer);
      quartoLiveSyncTimer = setTimeout(() => void quartoPreviewController.sync(), 500);
    }
    if (typeof calepinActive !== "undefined" && calepinActive && typeof calepinPreview !== "undefined" && calepinPreview) {
      clearTimeout(calepinSyncTimer);
      calepinSyncTimer = setTimeout(() => void calepinPreviewController.sync(), 500);
    }
    if (sourceFormat === "quarto" && session?.text) {
      const main = previewMain || session.mainPath() || "main.qmd";
      const id = session.idOf?.(main) || session.mainId();
      const parsed = quarto.parseQuarto(session.textOf(id)?.toString?.() || session.text.toString(), { path: main });
      const nextDiagnostics = parsed.diagnostics.map((item) => diagnosticContext(item, { main, texts: { [main]: parsed.source } }, ""));
      const diagnosticsGeneration = sourceGeneration;
      // Loro can notify this observer from inside CodeMirror's update
      // listener. Editor.setDiagnostics dispatches another update, which
      // CodeMirror rejects while the first one is still in progress. Source
      // invalidation stays synchronous, but the editor repaint waits until
      // the current transaction has returned and is skipped if newer text
      // arrived before then.
      queueMicrotask(() => {
        if (readerDisposed || diagnosticsGeneration !== sourceGeneration) return;
        diagnosticsController.render(nextDiagnostics);
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
    // The source observer can run before its caret has been mapped.
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
      const openPath = session?.paths?.get(openFile) || "";
      const format = openPath ? renderers.formatOf(openPath) : sourceFormat;
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

  // Which side the source is on, which keys the editor answers to and how
  // wide the panes are: one reader's habits rather than anything about a
  // document. The preferences own them, and own writing them down -- see
  // `reader/preferences.svelte.js` for why those are one act and not two.
  const preferences = createPreferences();
  const prefs = preferences.state;
  // Every project opens the same way: the files, the main file and its
  // preview, or the files alone on a phone. Where the last visit ended is
  // not kept.
  let layout = $state("split");
  let panel = $state("files");
  let mobileView = $state("sidebar");
  let preferredPane = $state("document");
  let collaborationTab = $state("comments");
  // The selected comment lasts only while the comment panel is open.
  $effect(() => { if (panel !== "collaboration") selectedAnnotation = ""; });
  // A click outside a comment card or a mark clears the selection. Clicks in
  // the frame do not reach this listener; the frame reports them as "unfocus".
  $effect(() => {
    const clear = (event) => {
      if (selectedAnnotation && !(event.target instanceof Element && event.target.closest(".comment-card"))) {
        selectedAnnotation = "";
      }
    };
    document.addEventListener("pointerdown", clear);
    return () => document.removeEventListener("pointerdown", clear);
  });
  const sourceSide = $derived(prefs.sourceSide);
  const keys = $derived(prefs.keys);
  const sizes = $derived(prefs.sizes);

  // Settings open as a dialog from the navbar menu or the sidebar. Every entry
  // point opens the same dialog on the category it is about.
  let settingsOpen = $state(untrack(() => initialSettings === "tools"));
  let settingsCategory = $state(untrack(() => initialSettings || "editor"));
  let projectFolderOpen = $state(false);
  let savingTemplate = $state(false);
  let templateModalOpen = $state(false);
  let templateName = $state("");
  let templateError = $state("");
  let templateSaveKey = "";
  let templateSaveRequestId = "";

  function openSettings(category = "editor") {
    // The editor category carries the shortcut table, and the table greys what
    // the source cannot currently do, so the editor is asked before the page
    // that shows the answer is drawn.
    refreshEditAvailability();
    settingsCategory = category;
    settingsOpen = true;
  }
  // The desktop app asks for Settings from its menu and from Ctrl or Cmd+,.
  $effect(() => {
    const open = () => openSettings();
    window.addEventListener("librepaper:open-settings", open);
    return () => window.removeEventListener("librepaper:open-settings", open);
  });

  // The column at the left, and what is in it: the files, the comments or the
  // history, or "" for closed. One value rather than a switch per panel,
  // because the column shows one thing at a time. An editor opens on the
  // files: the shape of the project is what a project space starts with.
  //
  // Somebody who came by a read or a comment link is shown the document and
  // its comments first. History is available to compare review rounds; files
  // and editor settings remain in the editor workspace.
  const chatVisible = $derived.by(() => panel === "collaboration" && collaborationTab === "chat" && shown.comments);
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
  function showMobileView(view) {
    if (view === "source" && !editing && panel !== "history") view = "document";
    mobileView = view;
    if (view !== "sidebar") preferredPane = view;
    if (!compact && view !== "sidebar" && layout !== "split") layout = view;
    if (view === "sidebar" && !panel) showPanel(home);
  }
  function selectPanel(name) {
    const next = panel === name && (!compact || activeMobileView === "sidebar") ? "" : name;
    showPanel(next);
    if (width <= 760) showMobileView(next ? "sidebar" : "document");
  }
  // The tabs this browser is offered. Which reader may see which panel is
  // written with the panels, in lib/panels.js; this only says who is asking.
  const tabs = $derived(tabsFor({ role: doc.role, mayEdit, editing, canSeeSharing, canPublish }));
  // The robot buttons hand work to the agent. The agent runs only in the
  // desktop app, so in a browser they say so rather than disappear.
  const agentOffered = $derived(tabs.some((tab) => tab.id === "agent"));
  // Where the column goes back to when what it showed is taken away: the
  // files for an editor, the comments for a commenter, and for a reader no
  // column at all -- there is nothing they are offered to go back to.
  const home = $derived(
    mayEdit ? "files"
    : tabs.some((tab) => tab.id === "collaboration") ? "collaboration"
    : "",
  );
  // Whether the document has said who this browser is. Until it has, the
  // column is drawn empty rather than as one audience's and then the other's.
  let settled = $state(false);

  // Kept current for the background work that consults it.
  $effect(() => {
    const track = () => {
      hidden = document.visibilityState === "hidden";
      if (!hidden && projectRefreshPending && !mayEdit) scheduleCurrentProjectRefresh();
    };
    document.addEventListener("visibilitychange", track);
    return () => document.removeEventListener("visibilitychange", track);
  });

  // The history panel refreshes itself while it is open. It polls rather than
  // listening because a label is written by the server and there is no
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
  function showPanel(name) {
    if (panel === "history" && name !== "history") historySource.close();
    // Reopening starts at the current source.
    const enteringHistory = name === "history" && panel !== "history";
    panel = name;
    if (enteringHistory) void historySource.select("");
    // Changes is a source-review queue. On desktop, opening it establishes a
    // source-only workspace; on compact screens the sidebar remains visible
    // until the reviewer chooses a row, which then moves to source.
    if (name === "changes" && mayEdit && !compact) {
      void startEditing().then(() => {
        if (panel === "changes") showMobileView("source");
      });
    }
    if (compact) mobileView = name ? "sidebar" : "document";
    if (name !== "history") return Promise.resolve();
    return loadHistory();
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
  // A window that cannot hold the split it was asked for: the phone, and the
  // tight split above it. Both show one pane at a time, so both need the
  // switch between the two faces in the bar; only the phone loses the rail
  // beside the column and needs the row along the bottom.
  const adapted = $derived(compact || (splitTight && layout === "split"));
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
    [["bold", "Bold"], ["italic", "Italic"]],
  ];

  // What the keyboard already does, named beside the item the way a desktop
  // menu names it: the menu is where people go to learn the shortcut, not
  // only to avoid it. Read from lib/commands.js rather than written here, so a
  // menu cannot name a key the window has stopped listening for.
  //
  // In Vim or Emacs mode the editing keys belong to the mode, and a menu
  // naming them would be lying about what the next keystroke does; the
  // workspace's own keys are not the mode's and stay named.
  const MENU_KEYS = $derived(menuKeys({ modalEditor: keys === "vim" || keys === "emacs", apple: APPLE }));

  function chooseEditCommand(command) {
    showMobileView("source");
    const target = editor;
    // Restore source focus after the menu finishes closing.
    setTimeout(async () => {
      if (editor !== target || !mayEdit) return;
      try { await target?.editCommand(command); }
      catch (error) { say(error.message || "The browser refused clipboard access. Use the keyboard shortcut instead.", { kind: "problem", id: "reader:clipboard" }); }
    }, 0);
  }

  function cycleLayout() {
    layout = ARRANGEMENTS[layout].next;
    if (compact) showMobileView(layout === "source" ? "source" : "document");
  }

  const putSourceOn = (side) => preferences.setSourceSide(side);

  const setKeys = (next) => preferences.setKeys(next);

  // What ":q" in Vim mode asks for: the document alone, set directly rather
  // than reached by cycling.
  const showDocumentAlone = () => { layout = "document"; };

  // Everything the layout menu offers, named by what was chosen. The menu
  // reports the value of the line rather than each line calling back, so this
  // is the one place those names are read.
  function chose(what) {
    if (what.startsWith("layout-")) {
      layout = what.slice(7);
      if (compact) showMobileView(layout === "source" ? "source" : "document");
      return;
    }
    if (what === "side-left" || what === "side-right") return putSourceOn(what.slice(5));
    if (what.startsWith("ratio-")) return setSize(PANES.editor, Number(what.slice(6)));
    if (what === "linked") return setLinked(!linked);
  }

  function chooseViewCommand(value) {
    if (value === "format-pdf") {
      if (displayedFormat === "latex") return void setLatexOutput("pdf");
      if (displayedFormat === "typst") return void setTypstOutput("pdf");
      if (["markdown", "quarto"].includes(displayedFormat)) return void setMarkdownOutput("pdf");
    }
    if (value === "format-html") {
      if (displayedFormat === "latex") return void setLatexOutput("html");
      if (displayedFormat === "typst") return void setTypstOutput("html");
      if (["markdown", "quarto"].includes(displayedFormat)) return void setMarkdownOutput("html");
    }
    if (value === "preview-latex-pdf") return void setLatexOutput("pdf");
    if (value === "preview-latex-html") return void setLatexOutput("html");
    if (value === "preview-file") return previewTheFile(openFile);
    if (value === "preview-typst-pdf") return void setTypstOutput("pdf");
    if (value === "preview-typst-html") return void setTypstOutput("html");
    return chose(value);
  }

  // The File menu. Its first three items are what the Files panel's toolbar
  // does, reached without first switching layouts and opening the panel; the
  // rest open the panels a person looks for under File, and its last lines
  // are the two commands that are about the workspace rather than a document.
  // In the desktop app only, its last item quits the app.
  // The Files panel is mounted on its first visit and draws the name field it
  // focuses, so the panel is opened and the DOM given a turn before the panel
  // is asked.
  async function chooseFileCommand(value) {
    if (value === "settings") return openSettings("editor");
    if (value === "quit") return void localQuarto.shellAction("quit").catch((error) => say(error.message || "The desktop app could not quit.", { kind: "problem", id: "reader:quit" }));
    if (value === "project-folder") return void (projectFolderOpen = true);
    if (value === "compile") return compileNow();
    if (value === "download") return downloadTree();
    if (value === "download-docx") return downloadDocx();
    if (value === "download-pdf" || value === "download-html") return downloadRendering(value.slice(9));
    if (value === "share" || value === "history") return openPanel(value);
    if (value === "save-template") return openSaveTemplateModal();
    if (!["new-file", "new-folder", "upload"].includes(value)) return;
    await openPanel("files");
    await tick();
    if (value === "upload") fileList?.choose();
    else fileList?.start(value === "new-file" ? "file" : "folder");
  }

  // Which of the rendering downloads the File menu can honour: the one for
  // what the preview is producing, once there is something to hand over. What
  // it is producing rather than what the source format ultimately produces,
  // because a LaTeX document previewing as HTML has a page to hand over and
  // no PDF.
  const downloads = $derived(availableDownloads({
    outputKind: previewOutputKind,
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
      say(error.message || "The rendered document could not be downloaded.", { kind: "problem", id: "reader:download-render" });
    }
  }

  function openSaveTemplateModal() {
    templateName = doc.title || "";
    templateError = "";
    templateSaveKey = "";
    templateSaveRequestId = "";
    templateModalOpen = true;
  }

  async function saveTemplate() {
    const title = templateName.trim();
    if (!title) {
      templateError = "Give the template a name.";
      return;
    }
    const key = `librepaper:template-save:${SLUG}:${title}`;
    if (templateSaveKey !== key || !templateSaveRequestId) {
      templateSaveKey = key;
      try {
        templateSaveRequestId = sessionStorage.getItem(key) || "";
      } catch {
        templateSaveRequestId = "";
      }
      if (!templateSaveRequestId) {
        templateSaveRequestId = crypto.randomUUID();
        try { sessionStorage.setItem(key, templateSaveRequestId); } catch { /* in-memory retries still reuse it */ }
      }
    }
    savingTemplate = true;
    try {
      const response = await fetch(`/api/documents/${SLUG}/template`, {
        method: "POST",
        headers: { ...SHELL_HEADERS, "Content-Type": "application/json" },
        body: JSON.stringify({ title, request_id: templateSaveRequestId }),
      });
      if (!response.ok) {
        templateError = (await response.json().catch(() => ({}))).error || "The template could not be saved.";
        return;
      }
      try { sessionStorage.removeItem(key); } catch { /* no persistent key was available */ }
      templateSaveKey = "";
      templateSaveRequestId = "";
      templateModalOpen = false;
      say(`Saved "${title}" as a template.`, { id: "reader:saved-template" });
    } catch (error) {
      templateError = error?.message || "The template could not be saved.";
    } finally {
      savingTemplate = false;
    }
  }

  // Open a panel from the menu: unlike the activity bar, choosing an item
  // that is already open leaves it open rather than closing the column.
  function openPanel(name) {
    if (!visitedPanels.includes(name)) visitedPanels = [...visitedPanels, name];
    const opening = showPanel(name);
    if (width <= 760) showMobileView("sidebar");
    return opening;
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
    say: (message) => say(message, { id: "reader:files-moved" }),
    paint: () => paintPreview(),
    retarget: () => updatePreviewTarget(),
    onarrived: (file) => openTheFile(file),
  });
  const ws = workspace.state;
  const files = $derived(ws.files);
  const folders = $derived(ws.folders);
  const openFile = $derived(ws.openFile);
  const rules = $derived(ws.rules);
  const shownFigure = $derived(ws.figure);
  const figureUrl = $derived(ws.figureUrl);
  // Test whether a file path can be displayed as an image in the figure view.
  const isDisplayableImage = (path) => /\.(?:png|jpg|jpeg|gif|svg|webp)$/i.test(path);
  // At compact widths, the File/Edit/Insert/View menus are accessed through the
  // hamburger menu instead of being visible in the top bar. This array mirrors
  // exactly when each MenubarMenu is rendered in the menus() snippet.
  const compactMenus = $derived([
    { id: "file", label: "File" },
    ...(editing && mayEdit ? [
      { id: "edit", label: "Edit", disabled: !editor || !!shownFigure },
      { id: "insert", label: "Insert", disabled: !mayEdit || !editor },
    ] : []),
    ...(editing ? [{ id: "view", label: "View" }] : []),
  ]);
  let outlineActiveFrom = $state(null);
  let outlineRevision = $state(0);
  let previewMain = $state("");
  // Pin an explicitly previewed file by identity so renames keep it selected.
  // Empty means follow the shared main file; opening an include never pins it.
  let previewFile = $state("");
  const toolbarPath = $derived(files.find((file) => file.id === openFile)?.path || "");
  const previewNoteItems = $derived(previewNotes({
    openPath: toolbarPath,
    openIsText: files.find((file) => file.id === openFile)?.kind === "text",
    mainPath: previewMain || session?.mainPath?.() || "",
    formatOf: renderers.formatOf,
  }));
  // session.mainPath() is not reactive on its own, so read files to recompute
  // when the directory changes.
  const mainUnrenderable = $derived.by(() => {
    void files;
    return unrenderableMain({ mainPath: previewMain || session?.mainPath?.() || "", formatOf: renderers.formatOf });
  });
  const editorFormat = $derived(toolbarPath ? renderers.formatOf(toolbarPath) : sourceFormat);
  const canPreviewFile = $derived(files.some((file) =>
    file.id === openFile && file.kind === "text" && Boolean(renderers.formatOf(file.path))));
  loadConfig()
    .then((answer) => {
      workspace.setRules(answer || {});
      if (typeof answer?.latexMirror === "string") latex.at(answer.latexMirror);
    })
    .catch(() => {
      /* the server checks every path again; this only explains it sooner */
    });

  // Reading the directory into the list is the workspace's; what is left
  // here is the preview target, which is a rendering question rather than a
  // directory one.
  const refreshFiles = () => workspace.refresh();

  // Set when the preview was emptied because there was nothing to show, so
  // that the next file to become renderable is painted: with no previous
  // target, the first target is otherwise left to the initial paint.
  let previewCleared = false;

  /// Empties the preview: no file is being followed, no format is selected and
  /// nothing is drawn. No render is attempted and no engine is warmed.
  function clearPreview() {
    previewCleared = true;
    previewMain = "";
    previewFile = "";
    sourceFormat = "";
    configureLatex("");
    navigationGeneration += 1;
    renderCoordinator.invalidate();
    clearTimeout(previewTimer);
    previewTimer = null;
    framePreview.clear();
    deliveredKind = "";
    everPainted = false;
    everPaintedShown = false;
    activeSynctex = null;
    renderStatus.resetFailure();
    renderStatus.clearDocx();
    diagnosticPainter.rendered({ page: false, diagnostics: [] });
  }

  // A project with no file at all, as opposed to one whose directory has not
  // arrived yet. The directory is read from the session itself so that a
  // stale file list cannot report a project as empty.
  const projectEmpty = $derived(mayEdit
    ? Boolean(session && persistence.joined && files.length === 0 && session.list().length === 0)
    : Boolean(currentProject && !Object.keys(currentProject.tree?.files || currentProject.texts || {}).length));
  $effect(() => { if (projectEmpty) untrack(clearPreview); });

  function updatePreviewTarget() {
    if (!session) return;
    const selected = files.find((file) => file.id === previewFile);
    if (!selected || selected.kind !== "text" || !renderers.formatOf(selected.path)) previewFile = "";
    const path = selected?.kind === "text" && renderers.formatOf(selected.path)
      ? selected.path : session.mainPath();
    const format = renderers.formatOf(path);
    // Nothing the preview could follow: the file it showed is gone and the
    // main file now is not a format that renders, or there is no file at all.
    if (!format) {
      if (previewMain) clearPreview();
      return;
    }
    if (previewMain === path && sourceFormat === format) return;
    const previous = previewMain || sourceFormat || (previewCleared ? "cleared" : "");
    previewCleared = false;
    previewMain = path;
    sourceFormat = format;
    configureLatex(format);
    renderers.warm(format);
    if (["quarto", "typst", "markdown"].includes(format)) readQuartoBinding();
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

  function previewTheFile(id) {
    const file = files.find((entry) => entry.id === id);
    if (!file || file.kind !== "text" || !renderers.formatOf(file.path)) return;
    // Choosing the main file goes back to following it, so a later change of main still shows.
    previewFile = file.path === session?.mainPath?.() ? "" : id;
    updatePreviewTarget();
    if (!compact && layout === "source") layout = "split";
    if (compact) showMobileView("document");
  }

  // The eye in the file list opens the file as well as previewing it, so the
  // source and the preview show the same file.
  function openAndPreview(file) {
    if (openFile !== file.id) outlineActiveFrom = null;
    workspace.show(file);
    previewTheFile(file.id);
  }

  // A file added, renamed, removed, or made the main one: the list is redrawn
  // and the document is rendered again, because every one of those changes
  // what a compiler would produce. Main-text edits also arrive through the
  // source watcher; skip them here so one commit does not schedule
  // the same diagnostics and preview twice. Included-file edits still need a
  // source change of their own because they can alter a Quarto render without
  // changing the main Y.Text.
  function filesChanged(events, active = session) {
    if (!active || active !== session) return;
    // Nested text edits change the preview, but not the file list. Which
    // events are the directory's is the session's to say: the list moves with
    // four maps, not one.
    if (active.directoryChanged(events)) refreshFiles();
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
    // pane makes room for one.
    if (mayEdit && !compact && layout === "document") layout = "split";
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
      say("Downloading the whole project is an editor's. A reader can download the rendered document instead.", { kind: "problem", id: "reader:download-project" });
      return;
    }
    if (projectEmpty) {
      say("This project has no files to download.", { kind: "problem", id: "reader:download-project" });
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
      say(error.message || "The project could not be downloaded.", { kind: "problem", id: "reader:download-project" });
    }
  }

  const addDroppedText = (file, path) => workspace.addDroppedText(file, path);

  async function downloadEntry(entry) {
    try {
      const chosen = await entryDownload({ entry, tree: liveTreeNow(), folders: [...folders], gather: gatherFigures });
      saveBlob(chosen.files ? await archive(chosen.files) : new Blob([chosen.bytes]), chosen.name);
    } catch (error) { say(error.message || "That file could not be downloaded.", { kind: "problem", id: "reader:download-entry" }); }
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
        // A publish that happened while this browser was not listening was
        // announced to nobody: the offer is pushed over this very socket. So
        // the way back up is where a reader asks whether they missed one.
        if (up && !mayEdit) void refreshCurrentProject();
      },
      onPeers: (count) => { if (!readerDisposed) peers = Math.max(peers, count); },
      onState: (state_) => {
        // Teardown sets this before it closes the collaboration, so a local
        // write landing in that window has a UI to miss.
        if (readerDisposed) return;
        const newlyFailed = state_.localError && state_.localError !== persistence.localError;
        persistence = state_;
        if (newlyFailed) say(`${state_.localError}. This browser can no longer keep its own copy of the project, so a reload while the connection is down would lose what is not yet on the server. Download the project to have a copy that does not depend on either.`, { kind: "problem", id: "reader:local-storage-failed" });
      },
      onSession: (active) => {
        session = active;
        changesUnsubscribe?.();
        // The changes live beside the text in the document, so any commit
        // can move them, an accept or reject included, and that is not a text
        // event. Imports from other peers arrive here as well.
        changesUnsubscribe = active?.doc.subscribe(refreshReview) || null;
        refreshReview();
        workspace.attach(active);
        refreshFiles();
        refreshPeers();
        // A restored document can already contain its complete tree when
        // the session is handed over, so no later source event is guaranteed.
        // Render once from that hydrated tree after Svelte has applied the
        // format and file-list state.
        void tick().then(() => {
          if (!readerDisposed && active === session) void paintPreview();
        });
      },
      onSource: () => sourceChanged(),
      onSwap: () => { if (!readerDisposed) sourceEpoch += 1; },
      onFiles: filesChanged,
      onAwareness: () => { if (!readerDisposed) refreshPeers(); },
      onDocumentChanged: () => {
        passages.clearPassageCache();
        location.reload();
      },
    });
    session = collaboration.start(document_);
  }

  async function refreshCurrentProject() {
    if (readerDisposed || mayEdit) return;
    if (projectFetch) {
      projectRefreshPending = true;
      return projectFetch;
    }
    const headers = authHeaders(KEY);
    if (projectEtag) headers["If-None-Match"] = projectEtag;
    projectFetch = (async () => {
      const response = await fetch(`/api/documents/${encodeURIComponent(SLUG)}/project`, { headers });
      if (response.status === 304) return;
      if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        // A document whose projection exceeds a resource bound answers 503
        // and stays unreadable on the server until recovered (§2.2); its
        // editors can still type and export locally, but this browser is a
        // reader with no source of its own to fall back to. Carry the status
        // so the catch below can say that plainly rather than the generic
        // "could not be loaded" a transient network failure gets.
        const failure = Object.assign(new Error(body.error || "The current project could not be loaded."), { status: response.status });
        throw failure;
      }
      const next = await response.json();
      if (readerDisposed || mayEdit) return;
      projectEtag = response.headers.get("etag") || `"${next.project_digest}"`;
      currentProject = next;
      changeRows = readerChangeRows(next.changes);
      if (selectedChange && !changeRows.some((row) => row.id === selectedChange)) selectedChange = null;
      projectUnreadable = "";
      sourceFormat = next.format || sourceFormat;
      configureLatex(sourceFormat, readerProjectOwner);
      sourceGeneration += 1;
      outlineRevision += 1;
      settled = true;
      if (sourceFormat) renderers.warm(sourceFormat);
      void paintPreview();
    })().catch((error) => {
      if (readerDisposed) return;
      const message = error?.status === 503
        ? (error.message || "This document is too large to render right now. It stays unreadable here until its editors bring it back under the limit; they can still type and export locally in the meantime.")
        : (error?.message || "The current project could not be loaded.");
      // Never leave the pane spinning on a project this browser has not yet
      // shown anything for: a reader with no source of its own has nothing
      // else to paint while this stays broken, so the loading state has to
      // end here rather than waiting on a render that is never coming.
      if (!currentProject) {
        projectUnreadable = message;
        settled = true;
      }
      say(message, { kind: "problem", id: "reader:project-load" });
    }).finally(() => {
      projectFetch = null;
      if (projectRefreshPending) {
        scheduleCurrentProjectRefresh();
      }
    });
    return projectFetch;
  }

  function scheduleCurrentProjectRefresh() {
    projectRefreshPending = true;
    if (hidden || projectRefreshTimer !== null) return;
    projectRefreshTimer = setTimeout(() => {
      projectRefreshTimer = null;
      projectRefreshPending = false;
      void refreshCurrentProject();
    }, 1000);
  }

  // What this browser may do with the document, and whether it can render it
  // at all.
  async function prepare(document_) {
    // The role endpoint answer is authoritative for every affordance.
    const ROLES = ["reader", "commenter", "editor", "owner"];
    const allowed = ROLES.indexOf(document_.role) >= ROLES.indexOf("editor");
    // The desktop app must see the resolved permission on its first
    // configuration. In particular, account examples arrive as editable
    // Quarto documents; configuring before this assignment leaves the
    // desktop app inactive and the pane stuck on its Markdown fallback.
    mayEdit = Boolean(allowed);
    const offeredRenderers = Array.isArray(document_.renderers) ? document_.renderers : ["markdown"];
    renderers.offerLatex(offeredRenderers.includes("latex"));
    // The workspace guards its own writes, so it is told once, here, where
    // the permission is settled.
    ws.canEdit = mayEdit;
    timeline.state.canEdit = mayEdit;
    openedLocally = Boolean(document_.local_copy);
    // Access was reduced, so this browser should stop offering an editable copy
    // of a project it may no longer edit. A copy opened from this browser has no
    // server answer behind its role, so it is kept as it is.
    if (!mayEdit && !openedLocally) {
      forgetProject({ server: location.origin, documentId: document_.document_id, slug: SLUG, createdAt: document_.created_at })
        .catch(() => {});
    }
    // Reader and commenter links receive an allowlisted projection of the
    // current project. They share the renderer but never join CRDT sync.
    if (!mayEdit) {
      await startCollaboration(document_, { sourceSync: false });
      await refreshCurrentProject();
      return;
    }
    // Rendering follows the durable source format. Generated-result identity
    // is deliberately absent: a reader compiles this source on demand.
    // An empty project has no format; the first file added gives it one.
    const format = document_.source_format ||
      (document_.execution_engine === "quarto" ? "quarto" : "");
    sourceFormat = format;
    scopeLocalApp();
    if (["quarto", "typst", "markdown"].includes(format)) readQuartoBinding();
    // Every reader compiles from source when the browser has the engine. A
    // missing engine produces an actionable local-tool message below.
    if (format && !renderers.outputKind(format)) {
      say(`A ${format} document is read where its renderer is built, and this browser has none for it.`, { kind: "problem", id: "reader:no-renderer" });
      settled = true;
      return;
    }
    // The column opens on the files, which a link-holder is not offered; and
    // a panel this visit has already moved to may not be theirs either.
    if (panel && !tabs.some((tab) => tab.id === panel)) showPanel(home);
    settled = true;
    // Load the browser renderer for readers as well as editors; this is all
    // transient and does not create a server-side result.
    if (format) {
      renderers.warm(format);
      // And put the last page this browser drew for this document up while that
      // happens. Loading an engine and compiling a paper takes seconds, and the
      // pane would otherwise spend all of them blank -- showing nothing, while
      // holding the page that is almost always about to be drawn again.
      void restoreLastPreview();
    }
    await startCollaboration(document_);
    // `onSource` starts projection metadata only after the initial document state
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
  }

  $effect(() => {
    markViewed(SLUG);
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
      // at a real URL, so the honest thing to say is both. A stopped server or
      // a failing one is not a missing document.
      onError: (error) => {
        if (error?.status === 401) {
          documentNeedsSignIn = true;
          doc = { title: "Sign in required" };
        } else if (error?.status === 404) {
          doc = { title: "Document not found" };
          say(
            me.providers?.length && !identity
              ? "No document at this address. If it was shared with you, sign in: a document shared with an account opens only for that account."
            : "No document at this address. The link may be wrong, or the document may have been deleted.",
            { kind: "problem", id: "reader:not-found" },
          );
        } else if (error?.status) {
          doc = { title: "Document unavailable" };
          say(
            `The server could not open this document (error ${error.status}). Reload the page to try again.`,
            { kind: "problem", id: "reader:not-found" },
          );
        } else {
          doc = { title: "Document unavailable" };
          say(
            "Could not reach the server. Check your connection, then reload the page.",
            { kind: "problem", id: "reader:not-found" },
          );
        }
      },
    });
    boot.start();

    return () => {
      // The session on the server ends when the last person in it
      // disconnects, which the socket closing does on its own; this is only
      // this browser letting go of its half.
      readerDisposed = true;
      // A page compiled in the last few seconds is still waiting out its
      // settling pause. Leaving is the moment it is worth most: it is the
      // page this document will open with next time.
      void flushPreview(previewCacheKey());
      renderCoordinator.invalidate();
      navigationGeneration += 1;
      renderers.cancelPreview();
      clearTimeout(reviewTimer);
      clearTimeout(previewTimer);
      clearTimeout(projectRefreshTimer);
      projectRefreshTimer = null;
      previewTimer = null;
      boot.dispose();
      passages.clearPassageCache();
      void quartoPreviewController.stop();
      void calepinPreviewController.stop();
      framePreview.dispose();
      stopLatex();
      pendingChat?.dispose();
      changesUnsubscribe?.();
      changesUnsubscribe = null;
      collaboration?.close();
    };
  });

  // Account scope and the desktop app are the only triggers for reading
  // backup status. A signed-out page or a browser never makes a request.
  $effect(() => backupStatusStore.setScope(me?.id, hasDesktop));
  $effect(() => () => backupStatusStore.reset());

  // Ctrl-S reports the durability the server has confirmed for local work.
  // Typing is saved automatically, but a buffered edit is not durable yet.
  //
  // "Saved" is the word to be careful with. This browser's own storage is not
  // a save: it is evictable, it is one device, and it holds nothing the
  // server has not also been sent only while the socket is down. So an
  // offline Ctrl-S says where the work currently is and what will happen to
  // it, and leaves the word alone. Only the server copy is called saved.
  function reportPersistence() {
    // A local write that failed or is still in flight is not worth
    // reassuring anybody about; the failure says so on its own.
    if (persistence.localError || persistence.localPending) return;
    if (!connected) {
      if (!persistence.local) return;
      say(
        persistence.pending
          ? "Offline. Your latest changes are in this browser only, and will be sent as soon as the connection returns."
          : "Offline. Everything typed so far has already reached the server.",
        { id: "reader:persistence" },
      );
      return;
    }
    if (!persistence.joined || persistence.pending) return;
    say("Saved on the server.", { id: "reader:persistence" });
  }

  // Every project an editor opens is kept in this browser, so it can be opened
  // again without a connection. A new project reports local before anything is
  // stored, so a failed attempt is retried on the next report instead of being
  // given up for the rest of the page load.
  let localCopyRemembered = false;
  $effect(() => {
    if (localCopyRemembered || !mayEdit || openedLocally || !persistence.local) return;
    localCopyRemembered = true;
    const server = location.origin;
    localProject({ server, slug: SLUG })
      .then(async (existing) => {
        await rememberProject({ ...doc, slug: SLUG, server });
        // The page shell is cached with the first copy, so the landing page can
        // list local projects when the server cannot be reached.
        if (!existing) await cacheCurrentShell().catch(() => {});
      })
      .catch(() => { localCopyRemembered = false; });
  });

  // There is no save, so a close is almost never worth interrupting: the
  // document is durable, and what is not yet on the server is in this
  // browser. The one case left is work that has reached neither.
  function beforeUnload(event) {
    if (atRisk) event.preventDefault();
  }

  // The arrangement is changed often enough to be worth a key. Ctrl-\ is what
  // an editor usually puts a split on, and nothing here or in CodeMirror wants
  // it.
  /* ------------------------------------------------------------- the keyboard */

  let commandPalette = $state(false);

  // What the workspace can currently offer, as facts rather than rules. The
  // rules are in lib/commands.js, which decides from these whether a command
  // can be asked for; this is the only place that knows both.
  const commandContext = $derived({
    mayEdit, editing, compact,
    panels: tabs.map((tab) => tab.id),
    preview: shown.document,
    compilable: mayEdit && sourceFormat === "latex" && compilesHere,
    canPreviewFile,
    downloads: {
      pdf: previewOutputKind === "pdf" && Boolean(downloads.pdf),
      html: previewOutputKind !== "pdf" && Boolean(downloads.html),
      docx: Boolean(docxDownload),
    },
    edit: editAvailability,
  });

  // What each command can do about it, asked once. The menus grey a line by
  // this and the keyboard refuses a chord by this, so a greyed item and a dead
  // key are the same answer rather than two rules that can disagree in public.
  const can = $derived(availability(commandContext));

  // Whether a selection can be cut, whether there is anything to undo: the
  // editor is asked, because only it knows, and it is asked when something is
  // about to show the answer rather than on every keystroke. The menu does
  // this on opening; the palette and the settings are the same question
  // asked from two more places.
  function refreshEditAvailability() {
    editAvailability = editor?.editAvailability() || {};
  }

  // One dispatcher, and it dispatches: every command here is a call to the
  // thing the menu already calls, so a command cannot drift from the menu item
  // that names it. Nothing new is implemented in this function.
  async function runCommand(id) {
    if (EDIT_COMMANDS.includes(id)) return chooseEditCommand(id);
    if (id === "palette") { refreshEditAvailability(); commandPalette = true; return; }
    if (id === "shortcuts") return openSettings("editor");
    if (id === "focus-files") return focusPanel("files");
    if (id === "focus-outline") return focusPanel("outline");
    if (id === "symbols" || id === "math") return openPalette(id);
    // A template command opens the Math overlay on its template.
    if (id.startsWith("math-")) return openPalette("math", id.slice("math-".length));
    if (id === "focus-comments") return focusPanel("collaboration");
    if (id === "focus-preview") return focusPreview();
    if (id === "layout-cycle") return cycleLayout();
    if (LAYOUT_COMMANDS.includes(id)) return chose(id);
    if (id === "preview-file") return chooseViewCommand(id);
    return chooseFileCommand(id);
  }

  const EDIT_COMMANDS = ["undo", "redo", "cut", "copy", "paste", "select-all", "find", "replace", "bold", "italic"];
  const LAYOUT_COMMANDS = ["linked", "layout-source", "layout-document", "layout-split", "side-left", "side-right"];

  // The palettes open over the editor. Each one inserts at the caret the
  // editor had when it opened, so there is nothing to open without a source.
  async function openPalette(name, template = "") {
    if (!can[name]) return;
    symbolsOpen = name === "symbols";
    mathOpen = name === "math";
    await tick();
    if (template) mathPalette?.openTemplate?.(template);
  }

  // However an overlay closes, the editor gets the keyboard back, once the
  // dialog has finished returning the focus to whatever opened it. The opener
  // can be a menu that no longer exists.
  const returnToEditor = () => requestAnimationFrame(() => editor?.focus?.());

  function closePalette() {
    symbolsOpen = false;
    mathOpen = false;
    returnToEditor();
  }

  // An Escape the palette keeps, to clear its search or leave a form, is
  // cancelled so the overlay stays open.
  function paletteEscape(palette, event) {
    if (palette?.claimsEscape?.(event)) event.preventDefault();
  }

  // Open the panel, then put the keyboard in it. The first of those was all a
  // panel command used to do, and a panel that appears while the focus stays
  // where it was is a panel somebody reached for and then has to Tab to.
  //
  // What is focused is whatever the panel offers first; the region itself,
  // which carries the panel's name, catches a panel that offers nothing.
  async function focusPanel(name) {
    await openPanel(name);
    await tick();
    const slot = document.getElementById(slotId(name));
    if (!slot) return;
    const first = [...slot.querySelectorAll(FOCUSABLE)].find((node) => node instanceof HTMLElement && node.offsetParent !== null);
    if (first instanceof HTMLElement) return first.focus();
    slot.tabIndex = -1;
    slot.focus();
  }

  const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

  // The preview, which on a narrow screen has to be the face that is showing
  // before it can be the thing that has the focus.
  function focusPreview() {
    if (compact) showMobileView("document");
    preview?.focus();
  }

  function shortcut(event) {
    // One Escape, one thing: the swatches, then the bar. Nothing here
    // touches a dialog -- those close themselves -- and nothing happens when
    // neither is up, so Escape stays the browser's.
    if (event.key === "Escape") {
      if (palette) { palette = false; return; }
      if (bar.shown) { bar = { ...bar, shown: false }; return; }
      return;
    }
    // Everything else is the registry's. `commandFor` answers with a command
    // only when the chord is bound, the focus is somewhere a workspace command
    // may act, and the command has a target right now; the key is cancelled
    // only once it has said yes, so an unclaimed chord is still the browser's.
    const command = commandFor(event, commandContext);
    if (!command) return;
    event.preventDefault();
    void runCommand(command.id);
  }

  // Track scrolls on elements inside the main reader pane. Ignore scrolls on
  // menus, popovers, and the nav itself. Store the last scrollTop per element
  // in a WeakMap to detect scroll direction and magnitude.
  $effect(() => watchScrolls("#main", noteScroll));

  // Reveal the bar when transitioning out of compact mode, or when switching
  // panes (each pane has its own scroll position).
  $effect(() => {
    compact; // track compact changes
    activeMobileView;
    panel;
    barHidden = false;
  });

  // Reveal the bar when keyboard focus enters the nav element, so off-screen
  // controls are never focused invisibly.
  $effect(() => revealOnNavFocus(() => {
    barHidden = false;
  }));
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
  {#if previewOutputKind === "pdf"}
    <Menu.Item value="download-pdf" class="menuitem" disabled={!can["download-pdf"]}>Download PDF</Menu.Item>
  {:else}
    <Menu.Item value="download-html" class="menuitem" disabled={!can["download-html"]}>Download HTML</Menu.Item>
  {/if}
  <Menu.Item value="download-docx" class="menuitem" disabled={!can["download-docx"]}>Download DOCX</Menu.Item>
  {#if mayEdit}
    <Menu.Item value="download" class="menuitem">Download project</Menu.Item>
  {/if}
  {#if me?.id}<Menu.Item value="save-template" class="menuitem">Save as template…</Menu.Item>{/if}
  <hr class="hr my-1" />
  {#if canSeeSharing || canPublish}<Menu.Item value="share" class="menuitem">Share…</Menu.Item>{/if}
  <!-- Only where the panel behind it is one this reader is offered: a menu
       that opens a column the rail does not have is the second list of who
       may see what that lib/panels.js exists to prevent. -->
  {#if tabs.some((tab) => tab.id === "history")}<Menu.Item value="history" class="menuitem">History</Menu.Item>{/if}
  <hr class="hr my-1" />
  {#if mayEdit && sourceFormat === "latex" && compilesHere}
    <Menu.Item value="compile" class="menuitem"><span class="menuitem-label">Compile now</span>{#if MENU_KEYS.compile}<span class="menuitem-keys">{MENU_KEYS.compile}</span>{/if}</Menu.Item>
  {/if}
  {#if mayEdit && ["typst", "markdown", "quarto"].includes(sourceFormat)}
    <Menu.Item value="project-folder" class="menuitem">Project folder…</Menu.Item>
  {/if}
  <Menu.Item value="settings" class="menuitem"><span class="menuitem-label">Settings…</span>{#if MENU_KEYS.settings}<span class="menuitem-keys">{MENU_KEYS.settings}</span>{/if}</Menu.Item>
  {#if hasDesktop}
    <hr class="hr my-1" />
    <Menu.Item value="quit" class="menuitem">Quit</Menu.Item>
  {/if}
{/snippet}

{#snippet layoutItems()}
  {#each [["source", "Source"], ["document", "Preview"], ["split", "Split"]] as [value, label]}
    <Menu.Item value="layout-{value}" class="menuitem" disabled={compact && value === "split"}>
      <span class="menuitem-check">{(compact ? activeMobileView === value : layout === value) ? "✓" : ""}</span>{label}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  {#each [["left", "Source on left"], ["right", "Source on right"]] as [side, label]}
    <Menu.Item value="side-{side}" class="menuitem" disabled={compact}>
      <span class="menuitem-check">{sourceSide === side ? "✓" : ""}</span>{label}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  {#each RATIOS as ratio}
    <Menu.Item value="ratio-{ratio.share}" class="menuitem" disabled={compact}>
      <span class="menuitem-check">{sizes[PANES.editor.key] === ratio.share ? "✓" : ""}</span>{ratio.says}
    </Menu.Item>
  {/each}
  <hr class="hr my-1" />
  <Menu.Item value="linked" class="menuitem">
    <span class="menuitem-check">{linked ? "✓" : ""}</span><span class="menuitem-label">Keep in step</span>{#if MENU_KEYS.linked}<span class="menuitem-keys">{MENU_KEYS.linked}</span>{/if}
  </Menu.Item>
{/snippet}

{#snippet previewItems()}
  {@const selectedFormat = displayedFormat === "latex" ? latexOutput : displayedFormat === "typst" ? typstOutput : ["markdown", "quarto"].includes(displayedFormat) ? markdownOutput : "html"}
  {@const selectableFormat = ["latex", "typst", "markdown", "quarto"].includes(displayedFormat)}
  <Menu.Item value="format-html" class="menuitem" disabled={!selectableFormat}>
    <span class="menuitem-check">{selectedFormat === "html" ? "✓" : ""}</span>HTML
  </Menu.Item>
  <Menu.Item value="format-pdf" class="menuitem" disabled={!selectableFormat}>
    <span class="menuitem-check">{selectedFormat === "pdf" ? "✓" : ""}</span>PDF
  </Menu.Item>
{/snippet}

{#snippet viewItems()}
  <Menu.Item value="preview-file" class="menuitem" disabled={!can["preview-file"]}>Preview this file</Menu.Item>
  <hr class="hr my-1" />
  {@render previewItems()}
  <hr class="hr my-1" />
  {@render layoutItems()}
{/snippet}

{#snippet faceSwitch()}
  <div class="face-switch" role="group" aria-label="Workspace view">
    <IconButton icon="eye" label="Preview" visibleLabel={compact ? "Preview" : null} pressed={shown.document}
                onclick={() => showMobileView("document")} />
    {#if editing || panel === "history"}
      <IconButton icon="writing-hand" label="Source" visibleLabel={compact ? "Source" : null} pressed={shown.source}
                  onclick={() => showMobileView("source")} />
    {/if}
  </div>
{/snippet}
{#snippet localExecutionControl()}
  {#if runCodeOffered}
    <label class="run-code-toggle" title="Run this document's code on this computer. Off whenever a document opens.">
      <button type="button" role="switch" class="switch" aria-checked={localExecution} aria-label="Run code" data-state={localExecution ? "checked" : "unchecked"} onclick={toggleLocalExecution}>
        <span class="switch-thumb" data-state={localExecution ? "checked" : "unchecked"}></span>
      </button>
      <span class="run-code-label">Run code</span>
    </label>
  {/if}
{/snippet}

<Nav {me} phoneMenu hidden={barHidden}>
  {#snippet tools()}
    {#if compact}
      <!-- At compact widths (≤760px), the top bar holds the Menu trigger (the
           panels, File/Edit/Insert/View and the account) and the Preview/Source
           switch. Each carries a caption. The menubar stays mounted but is
           invisible; a menu chosen from Menu opens in the menu's own box. -->
      <CompactPanelMenu tabs={tabs} {panel} {me} {badges} open={shown.comments} onselect={openPanel} onsettings={() => openSettings()}
                        menus={compactMenus} onmenu={(id, anchor) => menubar.show(id, anchor)} />
      {@render faceSwitch()}
      {#if !connected}<span class="presence-offline" title={connectionNote} aria-label={connectionNote}><span class="presence-offline-dot" aria-hidden="true"></span>Offline</span>{/if}
    {:else}
      <!-- Between 761px and the split's narrow point, the face choice stays in
           the top bar. Compact widths render it beside the hamburger above. -->
      {#if adapted && (editing || panel === "history")}
        {@render faceSwitch()}
      {/if}
      <!-- The build's state is told by the preview pane's progress line and
           by Diagnostics, so the bar only carries the local execution switch. -->
      {@render localExecutionControl()}
      <div class="presence" role="group" aria-label={connected ? `${peers} people connected` : connectionNote} title={connected ? `${peers} people connected` : connectionNote}>
        {#each participants.slice(0, 3) as person (person.key)}
          <Avatar name={person.name} key={person.key} colour={person.colour} size={6} />
        {/each}
        {#if participants.length > 3}<span class="presence-more">+{participants.length - 3}</span>{/if}
        {#if !connected}<span class="presence-offline"><span class="presence-offline-dot" aria-hidden="true"></span>Offline</span>{/if}
      </div>
    {/if}
  {/snippet}
  {#snippet menus()}
    <!-- The File menu is the project's, not the preview's: new files, uploads,
         downloads, sharing, history and settings all belong to the project
         whatever pane is on screen, so it stays put when the document pane is
         hidden. -->
    <MenubarMenu id="file" label="File" onselect={(command) => void chooseFileCommand(command)}>
      {@render fileItems()}
    </MenubarMenu>
    {#if editing && mayEdit}
      <MenubarMenu id="edit" label="Edit" disabled={!editor || !!shownFigure}
                   onopen={refreshEditAvailability}
                   onselect={chooseEditCommand}>
        {#each editGroups as group, index}
          {#if index}<hr class="hr my-1" />{/if}
          {#each group as [command, label]}
            <Menu.Item value={command} class="menuitem" disabled={!can[command]}>
              <span class="menuitem-label">{label}</span>{#if MENU_KEYS[command]}<span class="menuitem-keys">{MENU_KEYS[command]}</span>{/if}
            </Menu.Item>
          {/each}
        {/each}
      </MenubarMenu>
      <InsertMenu getContext={insertContext} oninsert={applyInsertion} onupload={uploadInsertAsset} onpreview={previewInsertAsset} onpalette={(id) => void openPalette(id)} keys={{ symbols: MENU_KEYS.symbols, math: MENU_KEYS.math }} oncancel={(context) => editor?.releaseInsertContext?.(context)} onfocus={() => editor?.focus?.()} disabled={!mayEdit || !editor} />
    {/if}
    {#if editing}
      <MenubarMenu id="view" label="View" onselect={chooseViewCommand}>
        {@render viewItems()}
      </MenubarMenu>
    {/if}
  {/snippet}
  {#snippet children()}
    <span id="docTitle" class="nav-document truncate" title={toolbarPath || doc.title || ""}>
      {toolbarPath ? basename(toolbarPath) : doc.title || "LibrePaper"}
    </span>
  {/snippet}
</Nav>

<main id="main" tabindex="-1" class="reader" class:editing={shown.source} class:no-preview={!shown.document}
      class:no-comments={!shown.comments} class:source-right={sourceSide === "right"}
      class:mobile-document={activeMobileView === "document"} class:mobile-source={activeMobileView === "source"}
      class:mobile-sidebar={activeMobileView === "sidebar"} class:adapted={compact} class:bar-hidden={barHidden}
      style="height: calc(100dvh - var(--librepaper-bar-visible, var(--librepaper-bar))); --librepaper-activity: {px(ACTIVITY_WIDTH, unit)}px; --librepaper-editor: {pixels(PANES.editor, panes)}px; --librepaper-sidebar: {pixels(PANES.sidebar, panes)}px">
  <!-- What this page is. The title is in the bar, where it is a span beside
       the menus rather than a heading, so the one heading a screen reader
       looks for first is here instead: said once, seen by nobody. -->
  <h1 class="sr-only">{doc.title || "Untitled document"}</h1>
  <!-- Each panel, wired where its state lives. The column renders whichever
       of these its rail has selected; none of it travels through the column
       as props, so a panel can grow a field without the column hearing of it. -->
  {#snippet filesPanel()}
    <Files bind:this={fileList} {files} {folders} open={openFile} preview={previewMain || session?.mainPath() || ""}
      {mayEdit} {rules} onopen={openTheFile} onadd={addFile}
      onmkdir={(path) => workspace.addFolder(path)} onrelocate={relocateFiles}
      ondelete={deleteFiles} onduplicate={(entry, path) => workspace.duplicate(entry, path)}
      onmain={makeMain} onfigure={addFigure} ontext={addDroppedText}
      ondownload={downloadTree} ondownloaditem={downloadEntry}
      previewable={(path) => Boolean(renderers.formatOf(path))} onpreview={openAndPreview} />
  {/snippet}

  {#snippet outlinePanel()}
    <Outline headings={outlineHeadings} activeFrom={outlineActiveFrom} onselect={openOutlineHeading} />
  {/snippet}

  {#snippet agentPanel()}
    <Agent slug={SLUG} link={linkFor(SLUG)} canShare={doc.role === "owner"}
      path={session?.paths?.get(openFile) || ""} selection={pending}
      revision={pending?.revision || ""} request={assistantRequest}
      {comments} changes={changeRows} {diagnostics} onreview={reviewAssistantResults} onpreview={previewAssistant}
      onsettings={() => openSettings("agents")} userName={identity || doc.commenting_as || "You"} />
  {/snippet}

  {#snippet collaborationPanel()}
    <Collaboration messages={liveChat} {connected} canPost={mayChat} onsend={sendLiveChat}
      {unreadChat} bind:tab={collaborationTab} {comments} {identity}
      commentingAs={doc.commenting_as || "Anonymous"} {canModerate} {went} {replacements}
      canComment={mayChat}
      onreveal={revealAnnotation} selected={selectedAnnotation} onresolve={resolve}
      ondelete={askDelete} ondeletemany={askDeleteMany} onreply={reply} onedit={edit}
      onagent={agentOffered ? askCommentAssistant : undefined}
      {composing} {needsLogin} signInHref={signInHref()}
      page={annotations.state.page}
      onloadmore={() => void annotations.loadMore()}
      onloadreplies={(comment) => void annotations.loadReplies(comment)}
      oncommentsend={sendDraft} oncommentcancel={cancelDraft} />
    {@render pendingRecovery("collaboration")}
  {/snippet}

  {#snippet changesPanel()}
    <Changes changes={changeRows} {files} {identity} {tracking} canTrack={mayEdit && editing}
      canReview={mayEdit} selected={selectedChange} ids={changesIds}
      ontracking={setTracking} onaccept={acceptChange} onreject={rejectChange} onrejectmany={rejectChanges} onreveal={revealChange} />
    {@render pendingRecovery("changes")}
  {/snippet}

  {#snippet sharePanel()}
    <Share open={panel === "share" && shown.comments} inline slug={SLUG}
      canShare={doc.role === "owner"}
      onclose={() => showPanel("")} />
  {/snippet}

  {#snippet diagnosticsPanel()}
    <Diagnostics {diagnostics} localAppProblem={localAppDiagnostics.length > 0} {desktopNeeds}
      onretrylocal={() => localExecutionRelevant ? startLocalExecution() : ensureLocalApp()}
      main={previewMain || session?.mainPath() || ""}
      canOpen={(item) => Boolean(diagnosticFile(item))} onopen={openDiagnostic}
      provenance={renderState.lastLatexResult?.provenance || null}
      attempts={renderState.lastLatexResult?.attempts || []}
      onagent={agentOffered ? askDiagnostic : undefined}
      log={renderState.lastLatexResult?.log || (quartoPreviewError && quartoPreviewController.state.log) || (calepinPreviewError && calepinPreviewController.state.log) || renderState.lastLog || ""}
      logOpen={renderState.lastLatexResult ? undefined : false} />
  {/snippet}

  {#snippet historyPanel()}
    <History {labels} viewing={historySelectedSha} canEdit={mayEdit}
      problem={historyProblem} onview={viewPoint}
      onname={nameLabel} />
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

  <Sidebar
    {shown} {tabs} {panel} {settled} {compact} mounted={mountedPanels} {badges}
    label="Document"
    panels={{ files: filesPanel, outline: outlinePanel, agent: agentPanel,
              collaboration: collaborationPanel, changes: changesPanel,
              share: sharePanel, diagnostics: diagnosticsPanel, history: historyPanel }}
    panes={panes} sidebarPane={PANES.sidebar}
    onselectpanel={selectPanel} ondrop={dropped}
    onsize={(size) => setSize(PANES.sidebar, size)} onguide={(where) => (guide = where)}
    ongrab={(on) => { grabbing = on; guide = { ...guide, shown: on }; }}
  >
    {#snippet controls()}
      <!-- Back out to the projects. The rail it returns to is this rail, in
           the same place, which is why this is a plain link and not a
           gesture: the page changes, the furniture does not. -->
      <IconButton icon="home" label="All projects" href="/" />
      {#if editing}
        <IconButton icon={ARRANGEMENTS[layout].icon}
          label={`Layout: ${ARRANGEMENTS[layout].says}. Switch to ${ARRANGEMENTS[ARRANGEMENTS[layout].next].says}`}
          onclick={cycleLayout} />
      {/if}
      <IconButton icon="settings" label="Settings" tone="plain"
                  onclick={() => openSettings()} />
    {/snippet}
  </Sidebar>

  {#if editing || panel === "history"}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section id="reader-source" class="editorpane" class:away={!shown.source}
             ondragover={(event) => event.preventDefault()} ondrop={dropped}>
      <!-- A figure has no editor. Choosing one shows it: an image as itself,
           a PDF through the browser's own viewer, which shows the first page
           without this application carrying a PDF renderer of its own. -->
      {#if panel === "history"}
        <HistoryWorkspace source={historySource} canEdit={mayEdit} onrestore={restoreLabel}
          ondownload={downloadArchive}
          downloading={downloadingSha === historySource.selected ? downloadingSha : ""}
          downloadProblem={downloadingSha === historySource.selected ? downloadProblem : ""} />
      {:else if shownFigure}
        <div class="figureview">
          {#if !isDisplayableImage(shownFigure.path) && !shownFigure.path.toLowerCase().endsWith(".pdf")}
            <div class="notyet">
              <p class="lp-text-secondary text-sm">{basename(shownFigure.path)} can't be displayed here.</p>
            </div>
          {:else if !figureUrl}
            <p>Fetching {shownFigure.path}…</p>
          {:else if shownFigure.path.toLowerCase().endsWith(".pdf")}
            <object data={figureUrl} type="application/pdf" title={shownFigure.path}>
              <p>{shownFigure.path}</p>
            </object>
          {:else}
            <img src={figureUrl} alt={shownFigure.path} />
          {/if}
        </div>
      {:else if Editor && sourceBound}
        {#key sourceEpoch}
          <Editor bind:this={editor} {session} format={editorFormat} file={openFile} {keys} {rules} {tracking} me={trackingMe} {selectedChange} editable={mayEdit}
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
  {#if documentNeedsSignIn}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">Sign in to open this document</h2>
        <p class="lp-text-secondary text-sm">
          Readers, commenters, and editors need to sign in before they can open a shared document.
        </p>
        <div class="notyet-actions">
          <a class="btn btn-sm lp-control-brand" href={signInHref()}>Sign in</a>
        </div>
      </div>
    </section>
  {:else if shown.document && projectUnreadable}
    <!-- No projection ever arrived, so there is no tree to compile and no
         renderer card below has anything to say. This is the one state that
         is not "not yet" -- the server itself has nothing to hand this
         browser -- so it gets its own plain message instead of a blank pane
         or a spinner that `settled` silently ended (§2.2). -->
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">This document cannot be shown right now</h2>
        <p class="lp-text-secondary text-sm">{projectUnreadable}</p>
        <div class="notyet-actions">
          <button class="btn btn-sm lp-control-tonal-brand" onclick={() => void refreshCurrentProject()}>Try again</button>
        </div>
      </div>
    </section>
  {:else if shown.document && mainUnrenderable}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">Nothing to preview</h2>
        <p class="lp-text-secondary text-sm">{mainUnrenderable}</p>
      </div>
    </section>
  {:else if shown.document && failedBeforeRender}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">Could not render</h2>
        {#if latexHtmlPreview}
          <p class="lp-text-secondary text-sm">
            The HTML converter could not convert this document. It supports fewer LaTeX packages than the PDF compiler, so the PDF view (View &gt; PDF) may still work.
          </p>
          <p class="lp-text-secondary text-sm">
            <Icon name="triangle-alert" /> Diagnostics in the sidebar lists what the converter reported.
          </p>
          <div class="notyet-actions">
            <button class="btn btn-sm lp-control-brand" onclick={() => void setLatexOutput("pdf")}>Show the PDF</button>
            <button class="btn btn-sm lp-control-tonal-brand" onclick={() => showPanel("diagnostics")}>Open Diagnostics</button>
          </div>
        {:else if renderState.failureReason}
          <p class="lp-text-secondary text-sm">
            The compiler produced no {pdfOutput ? "PDF" : "HTML preview"}, and Diagnostics has nothing to show for it. What it said:
          </p>
          <pre class="lp-text-secondary text-xs">{renderState.failureReason}</pre>
        {:else}
          <p class="lp-text-secondary text-sm">
            {#if !compilesHere}
              This browser cannot render this {sourceFormat === "latex" ? "LaTeX" : sourceFormat === "typst" ? "Typst" : sourceFormat === "quarto" ? "Quarto/Markdown" : "Markdown"} source. {sourceFormat === "latex" ? "Configure the browser LaTeX renderer to view it." : "The LibrePaper desktop app can render it on this computer."}
            {:else}
              Fix the errors in Diagnostics to produce a PDF preview.
            {/if}
          </p>
          {#if !compilesHere && sourceFormat !== "latex"}<DesktopOnly feature="This preview" />{/if}
        {/if}
      </div>
    </section>
  {:else if shown.document && unrendered}
    <section class="latexpane">
      <div class="notyet">
        {#if pdfNeedsLocalTool}
          <h2 class="h4">PDF needs {sourceFormat === "quarto" ? "Quarto" : "a local build"} on this computer</h2>
          <p class="lp-text-secondary text-sm">
            This browser renders {sourceFormat === "quarto" ? "Quarto" : "Markdown"} to HTML only. The PDF is produced by
            {sourceFormat === "quarto" ? "Quarto itself" : "Pandoc or Quarto"}, run through the LibrePaper desktop app,
            so the preview stays empty until you open this document there{sourceFormat === "quarto" ? " and turn on Run code" : ""}.
          </p>
          <div class="notyet-actions">
            {#if pdfNeedsLocalExecution}
              <button class="btn btn-sm lp-control-brand" onclick={() => void startLocalExecution()}>Run code</button>
              <button class="btn btn-sm lp-control-tonal-brand" onclick={previewAsHtml}>Preview as HTML instead</button>
            {:else}
              <DesktopOnly feature="The PDF preview" />
              <button class="btn btn-sm lp-control-tonal-brand" onclick={previewAsHtml}>Preview as HTML instead</button>
            {/if}
          </div>
        {:else}
          <h2 class="h4">{#if previewBusy}<span class="spinner"></span>{/if}Not yet rendered</h2>
          <p class="lp-text-secondary text-sm">
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
  {#snippet previewControls()}
    {#if viewerView}
      <PreviewControls mode={viewerView.mode} scale={viewerView.scale} tool={viewerTool} busy={previewBusy}
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
  <Preview bind:this={preview} src={frameSrc} {docsOrigin} onmessage={fromFrame} onload={frameLoaded} {grabbing}
           controls={previewControls}
           busy={previewBusy}
           progress={sourceFormat === "latex" && !latexHtmlPreview && latexState.progress?.total ? latexState.progress : null}
           away={documentNeedsSignIn || !shown.document || unrendered || failedBeforeRender || Boolean(projectUnreadable) || Boolean(mainUnrenderable)} />

  <!-- Shown only while a separator is dragged: a line that follows the pointer
       so the split can be seen moving without the iframe reflowing on every
       pointermove. -->
  {#if guide.shown}<div class="grip-guide" class:held={guide.held} style="left: {guide.left}px"></div>{/if}
</main>

<!-- What can be done to the passage that is selected, over the passage that
     is selected. The verbs are here rather than in the column because the
     selection is the subject: by the time this is on the screen the reader
     has already said what they are annotating, and all that is left is which
     annotation it is to be.

     Two icons, because two verbs with a word each read as a sentence nobody
     finishes: the bar sits over the reader's own words and the smaller it is
     the less of them it covers. The colour belongs to Highlight rather than
     standing beside it -- it is not a third thing that can be done to a
     passage, it is how the second one comes out. -->
{#if bar.shown && mayChat}
  <div
    bind:this={barElement}
    id="selectionbar"
    style="display: flex; left: {bar.left}px; top: {bar.top}px"
  >
    <div class="verbs">
      {#each shownVerbs as item (item.id)}
        {#if item.id === "highlight"}
          <!-- The swatches hang under Highlight rather than beside it: one
               click highlights in the colour the strip is showing, and the
               strip itself is for the times that is the wrong colour. It is
               a sibling rather than a child because a button inside a button
               is not a thing a browser will draw. -->
          <div class="verb highlighter">
            <button type="button" class="verb-face" title={item.title} aria-label={item.label}
                    onclick={() => annotate(item.id)}>
              <Icon name={item.icon} size={18} />
            </button>
            <button type="button" class="ink" aria-expanded={palette} aria-haspopup="true"
                    aria-label="Choose highlight colour" title="Choose highlight colour"
                    style="--ink: {highlightColor}"
                    onclick={() => (palette = !palette)}></button>
            {#if palette}
              <div class="highlight-colors" role="group" aria-label="Highlight colour">
                {#each HIGHLIGHT_COLORS as color}
                  <button type="button" class:selected={highlightColor === color} class="color-swatch" style="background:{color}"
                    aria-label="Highlight in {colorName(color)}" onclick={() => highlightWith(color)}></button>
                {/each}
                <label class="custom-color" title="Choose highlight colour">
                  <span class="sr-only">Custom highlight colour</span>
                  <input type="color" bind:value={highlightColor} onchange={() => highlightWith(highlightColor)} />
                </label>
              </div>
            {/if}
          </div>
        {:else}
          <button type="button" class="verb verb-face" title={item.title} aria-label={item.label}
                  onclick={() => annotate(item.id)}>
            <Icon name={item.icon} size={18} />
          </button>
        {/if}
      {/each}
    </div>
  </div>
{/if}

<!-- The keyboard as a list to search. It is drawn from lib/commands.js and runs
     what the menus run, so it cannot drift from what the keys actually do. The
     table to read lives in the editor settings. -->
<CommandPalette bind:open={commandPalette} context={commandContext} apple={APPLE}
                onrun={(id) => void runCommand(id)} />

<!-- The palettes open over the editor rather than in the sidebar. Each is
     mounted only while open, so a form left half filled does not come back. -->
<Modal bind:open={symbolsOpen} title="Symbols" wider onclose={returnToEditor}
       initialFocus={() => symbolPalette?.focusTarget?.()} onEscapeKeyDown={(event) => paletteEscape(symbolPalette, event)}>
  {#if symbolsOpen}
    <div class="palette-overlay">
      <SymbolPalette bind:this={symbolPalette} format={editorFormat} getContext={insertContext} oninsert={applyInsertion} onfocus={closePalette} />
    </div>
  {/if}
</Modal>
<Modal bind:open={mathOpen} title="Math" wider onclose={returnToEditor}
       initialFocus={() => mathPalette?.focusTarget?.()} onEscapeKeyDown={(event) => paletteEscape(mathPalette, event)}>
  {#if mathOpen}
    <MathPalette bind:this={mathPalette} format={editorFormat} getContext={insertContext} oninsert={applyInsertion} onfocus={closePalette} oncancel={(context) => editor?.releaseInsertContext?.(context)} />
  {/if}
</Modal>

<!-- The preferences dialog is shared by the navbar menu and sidebar button. -->
<SettingsDialog bind:open={settingsOpen} bind:category={settingsCategory}
                remoteConnected={connected} remoteNote={connectionNote}
                {keys} onkeys={setKeys} commands={commandContext}
                userId={buildUserId}
                onbuildpreferences={(format, next) => { if (format === sourceFormat) setBuildPreferences(next); }}
                onquartooptions={applyRenderOptions}
                account={me} />

<Modal bind:open={projectFolderOpen} title="Project folder">
  <ProjectFolderSetting {sourceFormat} main={previewMain} {mayEdit} onbindingid={(id) => { quartoBindingId = id; localQuarto.setBindingId(id); }} />
</Modal>

<Modal bind:open={templateModalOpen} title="Save as template"
  onclose={() => { templateError = ""; templateName = ""; }}
  confirm={{ label: savingTemplate ? "Saving..." : "Save", disabled: savingTemplate, onclick: saveTemplate }}>
  {#snippet children()}
    <label class="label">
      <span class="label-text">Template name</span>
      <!-- svelte-ignore a11y_autofocus -- the dialog exists to ask this one thing -->
      <input class="input" autofocus bind:value={templateName} />
    </label>
    {#if templateError}<p class="lp-text-error text-sm">{templateError}</p>{/if}
  {/snippet}
</Modal>

<Modal bind:open={timeline.state.restoring} title="Restore this version?"
  description={restoreMoved
    ? `The project changed while this was open. Restore ${restoreName} over the project as it now stands? The current version will be preserved in history.`
    : `Restore ${restoreName}. Every file in the project is replaced, and the current version is preserved in history.`}
  confirm={{
    label: restoreBusy ? "Restoring…" : restoreMoved ? "Restore anyway" : "Restore version",
    disabled: restoreBusy || !mayEdit,
    onclick: confirmRestore,
  }}>
  {#if historyProblem}
    <p class="lp-text-error text-sm" role="alert">{historyProblem}</p>
  {/if}
</Modal>

<!-- Deleting a thread cannot be undone, so it is confirmed. -->
<Modal
  bind:open={deleting}
  title={pendingDelete.length > 1 ? `Delete ${pendingDelete.length} comments?` : "Delete comment?"}
  description={pendingDelete.length > 1
    ? "This removes these comments and their replies for everyone. It cannot be undone."
    : "This removes the comment and its replies for everyone. It cannot be undone."}
  confirm={{ label: "Delete", tone: "error", onclick: confirmDelete }}
></Modal>

<!-- Signed out, a commenter chooses how to be named before they write. -->
<Modal
  bind:open={identifying}
  title="Who are you?"
  description="Choose how to identify yourself in this comment."
  cancelLabel="Sign in"
  confirm={{
    label: `Continue as ${doc.commenting_as || "Anonymous"}`,
    oncancel: () => { identifying = false; location.href = signInHref(); },
    onclick: () => { identifying = false; openDraft(); },
  }}
></Modal>

<Toasts />

<style>
  .run-code-toggle { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); cursor: pointer; font-size: var(--text-sm); white-space: nowrap; }
  .nav-document {
    display: block;
    max-width: min(38vw, 20rem);
    color: var(--color-text-secondary);
    font-size: var(--text-sm);
  }
  /* Adapted widths keep the two faces grouped. Compact widths put them in the
     top bar beside the Menu trigger, with the same column sizing and captions. */
  .face-switch { display: inline-flex; align-items: center; gap: 2px; padding: 2px; border-radius: var(--radius-container); background: var(--color-subtle); }
  @media (max-width: 760px) {
    .face-switch { gap: calc(var(--spacing) * 2); padding: 0; border-radius: 0; background: none; }
    .face-switch :global(.icon-control) { flex-direction: column; gap: 2px; width: auto; min-width: 3.5rem; height: 2.75rem; padding: 0; }
    .face-switch :global(.compact-label) { font-size: 11px; }
  }
  @media (max-width: 600px) {
    /* Nobody else here yet: an empty group would still take a gap. */
    .presence:empty { display: none; }
  }
  .presence { display: inline-flex; flex: none; align-items: center; gap: calc(var(--spacing) * .5); color: var(--color-text-secondary); font-size: var(--text-xs); }
  .presence-offline { display: inline-flex; align-items: center; gap: calc(var(--spacing) * .75); color: var(--color-text-secondary); font-size: var(--text-xs); }
  .presence-offline-dot { width: .5rem; height: .5rem; flex: 0 0 .5rem; border-radius: 50%; background: var(--color-error-solid); }
  .presence :global(.avatar + .avatar) { margin-left: calc(var(--spacing) * -1.5); box-shadow: 0 0 0 2px var(--color-shell); }
  .presence-more { display: inline-grid; place-items: center; min-width: 1.5rem; height: 1.5rem; margin-left: calc(var(--spacing) * -1.5); border-radius: 50%; background: var(--color-divider); color: var(--color-text-secondary); font-size: .65rem; }
  /* A heading over a run of items. Aligned with the names rather than with
     the panel edge, so the column the items make starts once. */
  @media (max-width: 760px) {
    :global(main.reader.adapted .sidebar-activity) { display: none; }
  }
  /* The bar over a selection: one row of icons, with the swatches unfolding
     beneath Highlight when its marked colour strip is pressed. */
  #selectionbar { align-items:flex-start; }
  .verbs { display:flex; align-items:stretch; gap:2px; padding:2px; border:1px solid var(--color-border); border-radius:var(--radius-base); background:var(--color-raised); box-shadow:var(--shadow-lg); }
  .verb { position:relative; display:flex; }
  .verb-face { display:flex; align-items:center; justify-content:center; width:2rem; height:2rem; border-radius:calc(var(--radius-base) - 2px); color:var(--color-text-secondary); }
  .verb-face:hover { background:var(--color-row-hover); color:var(--color-link); }
  /* Highlight's icon sits above a strip of the ink it is about to lay down.
     The small corner marker makes the strip's palette action visible. */
  .highlighter .verb-face { padding-bottom:6px; }
  .ink { position:absolute; right:4px; bottom:0; left:4px; height:14px; border-radius:2px; background:linear-gradient(transparent 0 8px, var(--ink) 8px); }
  .ink::before { content:""; position:absolute; right:0; bottom:0; left:0; height:6px; border-radius:2px; box-shadow:inset 0 0 0 1px var(--color-text-muted); pointer-events:none; }
  .ink::after { content:""; position:absolute; right:0; bottom:0; width:0; height:0; border-right:7px solid var(--color-text-secondary); border-top:7px solid transparent; pointer-events:none; }
  .ink:hover::before, .ink[aria-expanded="true"]::before { box-shadow:inset 0 0 0 1px var(--color-focus); }
  .highlight-colors { position:absolute; z-index:1; top:calc(100% + 6px); left:50%; transform:translateX(-50%); display:flex; align-items:center; gap:3px; padding:2px; border:1px solid var(--color-border); border-radius:var(--radius-base); background:var(--color-raised); box-shadow:var(--shadow-lg); }
  /* The Symbols overlay has a fixed height, so the grid scrolls inside it and the dialog does not jump as the tabs change. The Math overlay fits its content. */
  .palette-overlay { height: min(70vh, 36rem); min-height: 0; }
  .color-swatch { width:1.5rem; height:1.5rem; border:2px solid transparent; border-radius:50%; }
  .color-swatch.selected { border-color:var(--color-text); box-shadow:0 0 0 1px var(--color-brand); }
  .custom-color { display:grid; place-items:center; width:1.5rem; height:1.5rem; }
  .custom-color input { width:1.5rem; height:1.5rem; padding:0; border:0; background:transparent; }
  .pending-recovery { flex-shrink: 0; max-height: 35%; overflow-y: auto; border-top: 1px solid var(--color-border); background: var(--color-subtle); }
</style>
