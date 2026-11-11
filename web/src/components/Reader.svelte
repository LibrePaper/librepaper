<script>
  import { listChanges, accept, rejectRestoring } from "../lib/track-changes.js";
  // One document: the source beside it, the page itself, and everything said
  // about it.
  import { aboutWholeDocument, anchorAll, flatten, placeSources, shownSelector } from "../lib/anchor.js";
  import { ANCHORS_ORIGIN, attachByHand, composeEdits, deriveAttachment, restoreByFragments } from "../lib/anchors.js";
  import { barHiddenAfter, revealOnNavFocus, watchScrolls } from "../lib/bar-scroll.js";
  import * as renderers from "../lib/renderers.js";
  import * as sync from "../lib/sync.js";
  import * as quarto from "../lib/engines/quarto.js";
  import * as figures from "../lib/figures.js";
  import * as history from "../lib/history.js";
  import { createHistorySource } from "../lib/reader/history-source.svelte.js";
  import { CURRENT, NOTHING } from "../lib/history-endpoints.js";
  import ComparisonBar from "./reader/ComparisonBar.svelte";
  import { createComparison } from "../lib/reader/comparison.svelte.js";
  import { createWebGit } from "../lib/reader/web-git.svelte.js";
  import { createWebConnection } from "../lib/reader/web-connection.svelte.js";
  import { diagnosticContext } from "../lib/assistant-review.js";
  import { passageContext, sourcePassage } from "../lib/assistant.js";
  import { candidateTree, capturePreviewTree, previewCandidate } from "../lib/assistant-preview.js";
  import { orphanState } from "../lib/orphan.js";
  import * as latex from "../lib/latex.js";
  import { parse as parseSynctex, lineAt as synctexLineAt, offsetOfLine as synctexOffsetOfLine } from "../lib/synctex.js";
  import * as localQuarto from "../lib/desktop/client.js";
  import { backups as backupStatusStore } from "../lib/desktop/backups.svelte.js";
  import { basename } from "../lib/file-manager.js";
  import { createAnnotations } from "../lib/reader/annotations.svelte.js";
  import { createReaderBoot } from "../lib/reader/boot.js";
  import { createPendingChat } from "../lib/reader/chat.js";
  import { createLiveChat } from "../lib/reader/live-chat.svelte.js";
  import { createOpening } from "../lib/reader/opening.svelte.js";
  import { createWorkspace } from "../lib/reader/workspace.svelte.js";
  import { createProjectionSource } from "../lib/reader/projection-source.svelte.js";
  import { createPainted } from "../lib/reader/painted.svelte.js";
  import { createPreviewTarget } from "../lib/reader/preview-target.svelte.js";
  import { createArrangement } from "../lib/reader/arrangement.svelte.js";
  import { createPhone } from "../lib/reader/phone.svelte.js";
  import { createJumps, isBackChord } from "../lib/reader/jumps.svelte.js";
  import { createBuildSettings } from "../lib/reader/build-settings.svelte.js";
  import { createTimeline } from "../lib/reader/timeline.svelte.js";
  import { createPreviewRenderer } from "../lib/reader/preview-render.js";
  import { createPreferences } from "../lib/reader/preferences.svelte.js";
  import { shareIntroKind } from "../lib/reader/share-intro.js";
  import LegalNotice from "./LegalNotice.svelte";
  import { forgetProject, markShellReady, rememberProject } from "../lib/offline-projects.js";
  import { cacheCurrentShell } from "../lib/offline-shell.js";
  import { needsSourceRefresh } from "../lib/reader/source-events.js";
  import {
    authHeaders,
    configOrRemembered,
    keyHeaders,
    me as whoami,
    post,
    signInHref,
    signOut,
  } from "../lib/api.js";
  import {
    LINKED,
    PDF_INVERT,
    linkFor,
    markShareIntroSeen,
    markViewed,
    read,
    shareIntroSeen,
    takeLinkKey,
    write,
  } from "../lib/storage.js";
  import { localDataForgotten } from "../lib/signed-out.js";
  import { ACTIVITY_WIDTH, PANES, clamp, measure, pixels, px } from "../lib/panes.js";
  import { slotId, tabsFor } from "../lib/panels.js";
  import { APPLE, availability, captured, commandFor, labelOf, menuKeys, writing } from "../lib/commands.js";
  // Aliased because this file has its own setSize for the pane sizes.
  import { size as textSize, setSize as setTextSize, DEFAULT_SIZE } from "../lib/editor-type.js";

  import { tick, untrack } from "svelte";
  import { lazyComponent } from "../lib/lazy-component.svelte.js";
  import { onMode } from "../lib/theme.js";
  import { menubar } from "../lib/menubar.svelte.js";
  import Nav from "./Nav.svelte";
  import FormatToolbar from "./FormatToolbar.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import { DOCUMENTATION_URL } from "../lib/site-links.js";
  import { VISUAL_FORMATS } from "../lib/visual-markup/formats.js";
  import Modal from "./Modal.svelte";
  import Toasts from "./Toasts.svelte";
  import { say, toaster } from "../lib/toast.svelte.js";
  import { archive, docxFile, entryDownload, projectFiles, saveBlob } from "../lib/reader/downloads.js";
  import { commentsMarkdown, loadAllComments } from "../lib/reader/comments-export.js";
  import { exportAvailability, exportFile, unrunCells } from "../lib/reader/exports.js";
  import { pandoc } from "../lib/pandoc/client.js";
  import Preview from "./Preview.svelte";
  import PreviewControls from "./PreviewControls.svelte";
  import { onPageZoom } from "../lib/desktop/zoom.js";
  import { agentWaitingBadge } from "../lib/agent-permission.js";
  import Grip from "./Grip.svelte";
  import DesktopOnly from "./DesktopOnly.svelte";
  import DesktopMismatch from "./DesktopMismatch.svelte";
  import { buildPreferenceUser, readLivePatch, update as updateBuildPreferences } from "../lib/build-preferences.js";
  import { createLivePreview } from "../lib/live-preview/controller.js";
  import Avatar from "./Avatar.svelte";
  import { presenceJumpTarget } from "../lib/presence-jump.js";
  import { extractOutline } from "../lib/outline.js";
  import { onOrgSyntax, onTypstSyntax } from "../lib/source-trees.js";
  import { countWordsIn, documentWords, sectionWords } from "../lib/word-count.js";
  import { createFramePreview } from "../lib/reader/frame-preview.js";
  import { rememberPreview, lastPreview, flushAll } from "../lib/reader/preview-cache.js";
  import { previewTarget } from "../lib/opening.js";
  import { createFrameOverlays } from "../lib/reader/frame-overlays.js";
  import { createRenderDiagnostics } from "../lib/reader/render-diagnostics.js";
  import { createRenderCoordinator } from "../lib/reader/render-coordinator.js";
  import { createSourceSchedule } from "../lib/reader/source-schedule.js";
  import { createRenderStatus } from "../lib/reader/render-status.svelte.js";
  import { provide } from "../lib/support/providers.js";
  import { createSteadyBusy } from "../lib/reader/steady-busy.js";
  import { waitStageOf } from "../lib/reader/wait-stage.js";
  import PreviewWaiting from "./reader/PreviewWaiting.svelte";
  import { previewNotes } from "../lib/reader/preview-notes.js";
  import { HIGHLIGHT_COLORS, colorName } from "../lib/annotation-colors.js";
  import { recenteredLeft, placeBar as placeSelectionBar } from "../lib/annotation-bar.js";
  import { VERBS, motivationFor } from "../lib/annotating.js";
  import InsertMenu from "./InsertMenu.svelte";
  import Menus from "./reader/Menus.svelte";
  import Sidebar from "./layout/Sidebar.svelte";
  import TabBar from "./mobile/TabBar.svelte";
  import ModeSwitch from "./mobile/ModeSwitch.svelte";
  import MoreSheet from "./mobile/MoreSheet.svelte";
  import CommandView from "./mobile/CommandView.svelte";
  import KeyboardBar from "./mobile/KeyboardBar.svelte";
  import Presence from "./mobile/Presence.svelte";
  import { createOverlayHistory } from "../lib/mobile/overlay-history.js";
  import { watchKeyboard } from "../lib/mobile/keyboard.js";
  import Files from "./reader/Files.svelte";
  import Outline from "./reader/Outline.svelte";
  import WordCount from "./reader/WordCount.svelte";
  import Collaboration from "./reader/Collaboration.svelte";
  import Changes from "./reader/Changes.svelte";
  import Share from "./reader/Share.svelte";
  import Diagnostics from "./reader/Diagnostics.svelte";
  import PendingAnnotations from "./reader/PendingAnnotations.svelte";
  import SaveTemplateDialog from "./reader/SaveTemplateDialog.svelte";
  import DeleteCommentsDialog from "./reader/DeleteCommentsDialog.svelte";
  import ClearChatDialog from "./reader/ClearChatDialog.svelte";
  import IdentityDialog from "./reader/IdentityDialog.svelte";
  import { m } from "../lib/paraglide/messages.js";

  // The demo sandbox at /try runs wholly in this browser. Its slug and
  // document URL are the ones in lib/try-project.js, which is imported only
  // when the demo starts so that no other reader loads the template sources.
  const DEMO = location.pathname === "/try";
  const SLUG = DEMO ? "try-librepaper" : location.pathname.split("/")[2];
  // Panels that need the server. The demo draws them greyed out.
  const DEMO_OFF = ["collaboration", "agent", "history", "git", "share"];
  const DEMO_HINT = m.reader_demo_hint();

  // The key a reader arrived with, taken out of the address before anything
  // asks the server a question. From here it is kept under the slug and
  // presented on every request for this document.
  // The sign-in return path carries only the path and query: the key stays in
  // this browser's storage and never travels in a URL the server sees.
  const KEY = DEMO ? "" : takeLinkKey(SLUG);

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
  const NO_DOCS_ORIGIN = m.reader_no_docs_origin();
  let frameSrc = $state(null);
  // Non-editors render this allowlisted current-state projection. It has the
  // same shape as the editor adapter but contains no CRDT or history payload.
  // Its fetch, etag and refresh schedule are owned by the projection source;
  // what stays here is what the Reader does when one lands or fails.
  const projection = createProjectionSource({
    slug: SLUG,
    key: KEY,
    active: () => !mayEdit,
    hidden: () => hidden,
    onProject: (next) => {
      workspace.fromProject(next);
      changeRows = readerChangeRows(next.changes);
      if (selectedChange && !changeRows.some((row) => row.id === selectedChange)) selectedChange = null;
      // A reader opens on the file the project opens on, whichever that is
      // now. The target puts its format in place and paints the page this
      // browser last drew for it.
      target.update();
      schedule.bump();
      outlineRevision += 1;
      lifecycle.settle();
      void paintPreview();
    },
    onProblem: (message) => {
      // Never leave the pane spinning on a project this browser has not yet
      // shown anything for: a reader with no source of its own has nothing
      // else to paint while this stays broken, so the loading state ends here.
      if (!projection.state.project) lifecycle.settle();
      say(message, { kind: "problem", id: "reader:project-load" });
    },
  });
  const readerProjectOwner = {};
  let renderedProjectDigest = $state("");
  // One page is kept per file, so the key names the file as well as the project.
  const previewCacheKey = (path) => `${SLUG}:${doc?.created_at || "unknown"}|${path}`;
  const lastCachedPage = async (path) => lastPreview(previewCacheKey(path));
  const cachePage = (path, page, identity) => rememberPreview(previewCacheKey(path), page, identity);
  // The opening sequence, and what it knows once it has run: the role, the
  // socket, whether the work is safe, who else is here, the session and the
  // room. Every effect is a callback, and every callback is lazy, so the
  // things they name may be declared further down. The state is read here
  // under the names the page has always used; every write goes through
  // `lifecycle`.
  const lifecycle = createOpening({
    slug: SLUG,
    key: KEY,
    standalone: DEMO,
    getIdentity: () => identity,
    // Only a signed-in person has an account; the id is empty for anybody else.
    getAccountId: () => (me.provider ? me.id || "" : ""),
    disposed: () => readerDisposed,
    onMessage: receive,
    seed: DEMO
      ? async (active) => {
        const { seedTryProject } = await import("../lib/try-project.js");
        if (readerDisposed) return;
        await seedTryProject(active);
      }
      : null,
    roleKnown: (document_) => {
      const offeredRenderers = Array.isArray(document_.renderers) ? document_.renderers : ["markdown"];
      renderers.offerLatex(offeredRenderers.includes("latex"));
      // The workspace guards its own writes, so it is told once, here, where
      // the permission is settled.
      ws.canEdit = mayEdit;
      timeline.state.canEdit = mayEdit;
      // Access was reduced, so this browser should stop offering an editable copy
      // of a project it may no longer edit. A copy opened from this browser has no
      // server answer behind its role, so it is kept as it is.
      if (!mayEdit && !openedLocally) {
        forgetProject({ server: location.origin, documentId: document_.document_id, slug: SLUG, createdAt: document_.created_at })
          .catch(() => {});
      }
      // A panel this visit has already moved to may not be one this role is
      // offered. A phone keeps the view it opened on.
      if (panel && !tabs.some((tab) => tab.id === panel)) {
        const view = mobileView;
        arrangement.showPanel(home);
        arrangement.state.mobileView = view;
      }
    },
    formatKnown: (format) => {
      target.state.format = format;
      scopeLocalApp();
    },
    canRender: (format) => Boolean(renderers.outputKind(format)),
    noRenderer: (format) => {
      say(m.readerc_no_renderer({ format }), { kind: "problem", id: "reader:no-renderer" });
    },
    refreshProjection: () => projection.refresh(),
    editorReady: (format) => {
      // Load the browser renderer for readers as well as editors; this is all
      // transient and does not create a server-side result. Started after the
      // collaboration module is in, because the editor cannot open without its
      // CRDT and the engine (ten megabytes for Typst) would otherwise share the
      // connection with it: the engine finishes no later this way, and the
      // editor opens about a second sooner on a slow link.
      if (format) {
        warmEngine(format);
      }
      // The directory arrives after the session does. A session object alone is
      // not a snapshot.
      // No chooser and no saved distribution: `latex.configure` tells the
      // controller which project this is and what it is allowed to do, and the
      // first `paintPreview` (from `startEditing` below, or an edit) is what
      // actually starts loading the engine. `session.latexSettings()` needs the
      // session that `start` just built, which is why this comes after
      // it rather than beside the old restore-a-distribution code above.
      configureLatex(sourceFormat);
      // A document its author may edit opens ready to be worked on: that is what
      // they came for.
      if (mayEdit) startEditing();
    },
    onConnected: (up) => {
      if (!up) {
        pendingChat?.disconnect();
        outbox.disconnected();
      }
      // A publish that happened while this browser was not listening was
      // announced to nobody: the offer is pushed over this very socket. So
      // the way back up is where a reader asks whether they missed one.
      if (up && !mayEdit) void projection.refresh();
    },
    onSession: (active) => {
      changesUnsubscribe?.();
      // The changes live beside the text in the document, so any commit
      // can move them, an accept or reject included, and that is not a text
      // event. Imports from other peers arrive here as well.
      changesUnsubscribe = active?.doc.subscribe(refreshReview) || null;
      // The attachments of an editor are derived from the document, so they
      // are followed on every commit.
      followAnnotations?.();
      followAnnotations = active ? annotations.follow(active.doc) : null;
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
    onFiles: (events, active) => filesChanged(events, active),
    onAwareness: () => { if (!readerDisposed) refreshPeers(); },
    onDocumentChanged: () => {
      location.reload();
    },
    localStorageFailed: (error) => {
      say(m.readerc_local_storage_failed({ error }), { kind: "problem", id: "reader:local-storage-failed" });
    },
    paint: () => { paintPreview(); },
    openFailed: (error) => {
      say(error.message || m.reader_open_failed(), { kind: "problem", id: "reader:open-failed" });
    },
  });
  const mayEdit = $derived(lifecycle.state.mayEdit);
  const connected = $derived(lifecycle.state.connected);
  const persistence = $derived(lifecycle.state.persistence);
  const peers = $derived(lifecycle.state.peers);
  const settled = $derived(lifecycle.state.settled);
  const openedLocally = $derived(lifecycle.state.openedLocally);
  const session = $derived(lifecycle.session);
  const collaboration = $derived(lifecycle.collaboration);
  const atRisk = $derived(lifecycle.atRisk);
  /** @type {ReaderUser} */
  let me = $state({ id: "" });
  const backupStatus = $derived(backupStatusStore.status);
  // The displayed name, since this is what goes on a comment and what the
  // reader is shown commenting as. A Google account's handle is its email and
  // belongs on neither.
  let identity = $derived(me.name || "");
  let canModerate = $derived(Boolean(doc.can_moderate));
  // Whether this tab is the one in front of the reader. Only background work
  // consults it; nothing about the document depends on being looked at.
  let hidden = $state(typeof document !== "undefined" && document.visibilityState === "hidden");
  const liveChat = createLiveChat({ slug: SLUG, key: KEY });
  const chatState = liveChat.state;
  let unreadChat = $state(false);
  // Whether an agent session is waiting for the person's permission. Reported
  // by the Agent panel; the badge on its tab says so while the panel is hidden.
  let agentWaiting = $state(false);
  // Whether the Agent panel is in view; set below, once `panel` and `shown` exist.
  let agentInView = $state(false);
  let pendingChat;
  let mayChat = $derived(["commenter", "editor", "owner"].includes(doc.role));
  // Sharing is the owner's; seeing who else is in the room is anyone's who is
  // named on the document. A reader who arrived by link is offered neither,
  // which is most of the point of a blind review.
  let canSeeSharing = $derived(Boolean(doc.can_see_sharing));
  let canPublish = $derived(["editor", "owner"].includes(doc.role));

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
    // An editor's attachment is read from the document's anchor records, which
    // move with every edit. A reader's comes from the page and is not derived.
    attachmentOf: (comment, bodies) => (mayEdit && session ? deriveAttachment(session.doc, session.anchors, comment, bodies) : undefined),
  });
  // The document subscription that re-derives the attachments; held per session.
  let followAnnotations = null;
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
  // The desktop app is there or it is not; this page never changes its mind.
  const hasDesktop = Boolean(localQuarto.desktop());

  /// Which document this browser tab is about, as far as the desktop app is
  /// concerned. It picks up whatever binding the client already remembers for
  /// it. Nothing is asked of the desktop app here.
  ///
  /// Deliberately not behind a format check: anything that wants the local
  /// app for this document, such as the agent panel, would otherwise ask with
  /// a null project and be refused. Rendering locally and using the desktop
  /// app are different questions about the same document.
  function scopeLocalApp() {
    localQuarto.configure({ project: SLUG, origin: location.origin });
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
    live.reset();
    setBuildPreferences(updateBuildPreferences(buildScope(), "latex", { output: next }));
    target.bump();
    renderers.cancelPreview({ keepWarm: true });
    latex.cancel();
    framePreview?.clear();
    painted.reset({ keepSynctex: true });
    renderStatus.resetFailure();
    schedule.cancel();
    await tick();
    if (!readerDisposed) void paintPreview();
  }

  // Run code is the consent to run a document's code on this computer. It is
  // off whenever a document opens and is never remembered. While it is on, a
  // format set to this computer renders there; the pane is drawn again
  // because the renderer that draws it changes hands.
  function toggleLocalExecution() {
    localExecution = !localExecution;
    stopRendering();
    void paintPreview();
  }

  // Everything in flight stops: a render that finishes after the renderer, or
  // the reader, has changed must not reach the pane.
  function stopRendering() {
    target.bump();
    renderers.cancelPreview({ keepWarm: true });
    latex.cancel();
  }

  async function setTypstOutput(format) {
    const next = format === "html" ? "html" : "pdf";
    if (typstOutput === next) return;
    buildSettings.state.typstOutput = next;
    setBuildPreferences(updateBuildPreferences(buildScope(), "typst", { output: typstOutput }));
    // Invalidate every pending delivery. A PDF that finishes after this
    // gesture must never replace the HTML frame.
    target.bump();
    void paintPreview();
  }

  // Markdown and Quarto share one output choice, made for the visit like the
  // others. Pages are made in this browser, through Pandoc and Typst.
  async function setMarkdownOutput(format) {
    const next = format === "html" ? "html" : "pdf";
    if (markdownOutput === next) return;
    setBuildPreferences(updateBuildPreferences(buildScope(), sourceFormat, { output: next }));
    target.bump();
    renderers.cancelPreview({ keepWarm: true });
    framePreview?.clear();
    painted.reset({ keepSynctex: true });
    renderStatus.resetFailure();
    schedule.cancel();
    await tick();
    if (!readerDisposed) void paintPreview();
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

  // The comment marks in the source, from the same placement the preview uses:
  // `sourceStart` and `sourceEnd` are set by `anchorComments`, so this is sent
  // again after every re-anchor as well as when the comments or the selection
  // change. Resolved comments are drawn too, as the preview draws them.
  function pushCommentMarks() {
    if (!editor || !session) return;
    const list = [];
    const ids = new Map();
    for (const comment of comments) {
      if (comment.sourceStart == null || comment.sourceEnd == null || !comment.sourcePath) continue;
      if (!ids.has(comment.sourcePath)) ids.set(comment.sourcePath, session.idOf(comment.sourcePath));
      const file = ids.get(comment.sourcePath);
      if (!file) continue;
      list.push({
        id: String(comment.id),
        file,
        from: comment.sourceStart,
        to: comment.sourceEnd,
        highlight: comment.motivation === "highlighting",
        color: comment.color || null,
        selected: String(comment.id) === String(selectedAnnotation),
      });
    }
    untrack(() => editor.setCommentMarks(list));
  }
  $effect(() => {
    void comments;
    void selectedAnnotation;
    void editor;
    void session;
    pushCommentMarks();
  });

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
  // quotation is what answers it. That is display evidence, matched against
  // display text, and nothing follows from it being wrong beyond a mark in the
  // wrong place. Where the passage is in the source is not this browser's
  // question at all. An editor's answer is read from the document's anchor
  // records (`deriveAttachment`), and `placeSources` only translates that
  // answer's file id into a path.
  //
  // When the rendered quotation no longer matches because the passage was
  // edited, the passage's current words at the server's source range are
  // looked for on the page instead, so an edited passage keeps its mark. A
  // comment that the quotation could not place is given this fallback as well.
  //
  // A comment attached by hand is painted from its source range and from
  // nothing else, since its rendered quotation names the passage it used to be
  // about. Its page match is dropped before the fallback runs.
  function anchorComments(list) {
    anchorAll(docText || "", list, docText === null ? null : docView);
    if (mayEdit) {
      placeSources(session, list);
      for (const comment of list) {
        if (!comment.attachment?.attached_by_hand) continue;
        comment.start = comment.end = null;
        comment.orphaned = true;
      }
    }
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
          comment.orphaned = false;
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
    pushCommentMarks();
  }

  // Scroll tracking for mobile bar hide/show: called by scroll listeners and
  // frame scroll messages. Hide the bars when scrolling down past threshold,
  // show them when scrolling up or at top. The mode switch and the tab bar
  // step aside together.
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
      case "viewer-drawn":
        // The pages of the newest delivery are on screen, so the waiting
        // display gives way to them. A paint of an older delivery does not.
        frameHasPage = true;
        if (message.frameGeneration >= framePreview.deliveredGeneration) painted.drawn();
        break;
      case "viewer-state":
        viewerView = message.drawn
          ? {
              mode: String(message.mode || "auto"), scale: Number(message.scale) || null,
              page: message.page, pages: message.pages, sidebar: message.sidebar, spread: message.spread,
              invert: message.invert, presenting: message.presenting, find: message.find,
            }
          : null;
        // The remembered inversion is sent once per document, the first time
        // the frame says it has drawn: a frame that was just opened starts
        // with the pages as they are, whatever was chosen last time. After
        // that the frame is the authority, and a toggle sends its own.
        if (message.drawn && !invertSent) {
          invertSent = true;
          if (invertPages) tell({ type: "viewer-invert", invert: true });
        }
        if (message.drawn && !densitySent) {
          densitySent = true;
          if (pageZoom !== 1 || pageZoomChanged) tell({ type: "viewer-density", zoom: pageZoom });
        }
        break;
      case "viewer-find-open":
        findAsk += 1;
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
      case "click":
        followPreviewClick(message.offset, message.pdf);
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
    mayEdit || Boolean(renderedProjectDigest && renderedProjectDigest === projection.state.project?.project_digest),
  );
  const shownVerbs = $derived.by(() => pending && renderedProjectCurrent ? VERBS : []);
  let highlightColor = $state(HIGHLIGHT_COLORS[0]);
  let palette = $state(false);
  let pending = $state(null);
  let assistantRequest = $state(null);
  let selectionRevision = Promise.resolve("");
  let bar = $state({ shown: false, left: 0, top: 0, center: 0 });

  // Mobile bar slide behaviour: on phones (compact, under 768px), the Source
  // / Document switch at the top collapses when scrolling down and comes back
  // when scrolling up. The bottom tab bar stays put.
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
    say(m.reader_agent_needs_desktop(), {
      kind: "note",
      id: "reader:agent-desktop-only",
      action: { label: m.common_get_desktop_app(), onClick: () => window.open(localQuarto.INSTALL_URL, "_blank", "noopener") },
    });
    return true;
  }

  function askDiagnostic(item) {
    if (agentNeedsDesktop()) return;
    assistantRequest = { id: crypto.randomUUID(), diagnostic: { ...item }, revision: item.revision || "",
      task: { kind: "fix", scope: item.file || item.path ? "file" : "document" } };
    arrangement.showPanel("agent");
    if (width <= 760) arrangement.showMobileView("sidebar");
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
    arrangement.showPanel("agent");
    if (width <= 760) arrangement.showMobileView("sidebar");
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
    await arrangement.showPanel("changes");
    if (made) await revealChange(made);
  }

  // A click on a mark in the source shows the card without taking the
  // keyboard: the editor clicked into the passage to type in it.
  async function focusAnnotation(id, { focus = true } = {}) {
    const comment = comments.find((item) => item.id === id);
    if (!comment) return;
    selectedAnnotation = String(comment.id);
    const plainHighlight = comment.motivation === "highlighting" && !comment.body && !comment.replies?.length;
    collaborationTab = plainHighlight ? "highlights" : "comments";
    void arrangement.showPanel("collaboration");
    const targetPanel = panel;
    await tick();
    if (panel !== targetPanel) return;
    const prefix = plainHighlight ? "collaboration-highlight" : "collaboration-comment";
    const card = document.getElementById(`${prefix}-${id}`);
    if (focus) card?.focus({ preventScroll: true });
    await tick();
    card?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }

  // A click on a tracked change in the source does what a click on a comment
  // mark does: the row is selected and its panel opened, without taking the
  // keyboard. The Changes panel brings the row into view.
  function selectChangeFromSource(id) {
    const row = changeRows.find((item) => String(item.id) === String(id));
    if (!row) return;
    selectedChange = row.id;
    void arrangement.showPanel("changes");
  }

  // Remembers the editor's place before a jump to `file` at `offset`. Nothing
  // is remembered when the editor would not move, or when the jump lands where
  // the editor already is. An undefined offset is a line the editor finds
  // itself, so that jump always counts as a new place.
  function rememberPlace(file, offset) {
    // The editor stays mounted while a narrow window shows the sidebar, so a
    // jump from there is remembered too.
    if (!editing || !editor || !ws.openFile) return;
    const here = { file: ws.openFile, offset: editor.caret() };
    if (here.file === file && here.offset === offset) return;
    jumps.remember(here);
  }

  // A click on a collaborator's avatar goes to where they are: their file and
  // their caret in it, brought into view and flashed in their colour. This
  // reader's own caret and selection are left alone. With no caret known, the
  // file is opened and nothing more. It needs the source pane, so it does
  // nothing while the source is closed.
  async function jumpToPerson(person) {
    if (!editing || !editor || !session) return;
    const target = presenceJumpTarget({ file: person.file, caret: session.caretOf(person.peer) });
    if (!target || !fileExists(target.file)) return;
    rememberPlace(target.file, target.from);
    if (compact) {
      arrangement.showMobileView("source");
      await tick();
    }
    if (!editing || !editor) return;
    ws.openFile = target.file;
    if (target.from != null) editor.revealPeer(target.file, target.from, target.to, person.colour);
  }

  // Back is not a jump, so the place it leaves is not remembered. It does
  // nothing when the source is not shown, and keeps the place for later.
  function goBack() {
    if (!editing || !shown.source || !editor) return;
    const place = jumps.back(fileExists);
    if (!place) return;
    ws.openFile = place.file;
    editor.goToIn(place.file, place.offset);
  }

  async function revealAnnotation(comment) {
    selectedAnnotation = String(comment.id);
    if (comment.start != null) {
      arrangement.showMobileView("document");
      await tick();
      tell({ type: "reveal", id: comment.id });
    } else if (comment.sourceStart != null && editing) {
      arrangement.showMobileView("source");
      await tick();
    }
    if (shown.source && comment.sourceStart != null && editing && editor) {
      const id = session.idOf(comment.sourcePath);
      rememberPlace(id, comment.sourceStart);
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
      // The phone's tab bar covers the bottom of the visible area, so the
      // selection bar must not land under it.
      const visibleHeight = (window.visualViewport?.height ?? innerHeight) - (tabBarShown ? 3.5 * unit : 0);
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
    // The formatting row, when it is showing, sits under the top bar too.
    const format = /** @type {HTMLElement | null} */ (document.querySelector(".format-bar"))?.offsetHeight ?? 0;
    return Math.round(height) + format + 9;
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
  let identityDialog = $state(null);
  let deleteCommentsDialog = $state(null);

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
    if (panel !== "collaboration") void arrangement.showPanel("collaboration");
    if (compact) arrangement.showMobileView("sidebar");
  }

  // A verb chosen on the bar over the selection. Highlighting is the one that
  // never opens a draft: the passage is the whole annotation, so the click
  // that names the verb is also the one that makes it.
  function annotate(which) {
    if (!pending || !mayChat) return;
    if (!renderedProjectCurrent) {
      say(m.reader_source_rendering(), { kind: "problem", id: "reader:source-rendering" });
      return;
    }
    bar = { ...bar, shown: false };
    palette = false;
    if (which === "highlight") {
      submitAnnotation({ motivation: "highlighting", body: "" });
      return;
    }
    if (!identity && me.providers?.length) {
      identityDialog?.ask();
      return;
    }
    openDraft();
  }

  // A verb chosen on the bar over a selection in the code pane. The passage is
  // the source range the editor holds, so the annotation is anchored by file
  // and offset rather than by words found in the preview. `annotate` hides the
  // preview's bar and does the rest, as it does for a preview selection.
  function annotateSource(which) {
    if (!mayEdit || !mayChat || !editor || !session) return;
    const sel = editor.selection();
    if (!sel) return;
    const text = editor.text(sel.file) ?? editor.text();
    const { prefix, suffix } = passageContext(text, sel.from, sel.to);
    pending = {
      exact: text.slice(sel.from, sel.to),
      prefix,
      suffix,
      position: null,
      render_digest: "",
      source: { file_id: sel.file, start: sel.from, end: sel.to },
    };
    annotate(which);
  }

  // A remark about the whole document, from the Comments header or the palette.
  // It names no passage, so there is no selection to make: the draft is opened
  // as a comment on the document, with nothing to quote.
  function commentOnDocument() {
    if (!mayChat) return;
    if (!renderedProjectCurrent) {
      say(m.reader_source_rendering(), { kind: "problem", id: "reader:source-rendering" });
      return;
    }
    pending = {
      exact: "",
      prefix: "",
      suffix: "",
      position: null,
      whole: true,
      render_digest: mayEdit ? "" : renderedProjectDigest,
    };
    annotate("comment");
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
    if (!mayEdit && pending.render_digest !== projection.state.project?.project_digest) {
      say(m.reader_source_moved(), { kind: "problem", id: "reader:source-moved" });
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
    const moved = !mayEdit && held.render_digest !== projection.state.project?.project_digest;
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
    deleteCommentsDialog?.ask([comment]);
  }

  function askDeleteMany(items) {
    deleteCommentsDialog?.ask(items);
  }

  function reply(comment, body, name) {
    if (mayChat) annotations.reply(comment, body, name);
  }

  // "Attach to selection": the passage the source has selected becomes the one
  // this comment is about. The record moves with the commit, and the card is
  // derived now rather than on the tick after it.
  function attachComment(comment) {
    const selection = editor?.selection?.();
    if (!mayEdit || !session || !selection) {
      say(m.reader_select_passage_first(), { kind: "note", id: "reader:attach-selection" });
      return;
    }
    if (
      !attachByHand(
        session.doc,
        session.anchors,
        session.anchorMemory,
        comment.id,
        selection.file,
        selection.from,
        selection.to,
      )
    ) {
      say(m.reader_attach_failed(), { kind: "problem", id: "reader:attach-failed" });
      return;
    }
    annotations.deriveNow();
  }

  /* -------------------------------------------------------------------- room */

  let clearChatDialog = $state(null);
  function askClearChat() {
    if (chatState.canClear) clearChatDialog?.ask();
  }
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

  // The 1-based line a character offset falls on. Counting newlines with
  // indexOf avoids building an array of lines for every change.
  function lineAt(text, offset) {
    let line = 1;
    let at = text.indexOf("\n");
    while (at !== -1 && at < offset) {
      line += 1;
      at = text.indexOf("\n", at + 1);
    }
    return line;
  }

  function recomputeChanges() {
    if (!session) {
      // A reader or commenter has no live document; their rows come with the
      // current project (`readerChangeRows`), so there is nothing to clear.
      if (mayEdit) changeRows = [];
      return;
    }
    // Several changes can sit in one file, so each file's text is read once.
    const texts = new Map();
    changeRows = listChanges(session.doc).map((change) => {
      if (!texts.has(change.file)) texts.set(change.file, session.textOf(change.file)?.toString() ?? null);
      const text = texts.get(change.file);
      const line = text !== null && Number.isFinite(change.from) ? lineAt(text, change.from) : undefined;
      return {
        id: change.id,
        file_id: change.file,
        path: session.paths?.get(change.file) || "",
        author: change.author,
        who: change.who,
        run: change.run || "",
        at: change.at,
        position: change.from,
        line,
        before: change.deleted,
        after: change.inserted,
      };
    });
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
      line: change.line,
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
      say(m.reader_tracking_failed(), { kind: "problem", id: "reader:tracking" });
    }
  }

  /// Look at a change: open the file it belongs to, put the caret where it
  /// starts, and mark it as the one being reviewed so the editor outlines it.
  async function revealChange(row) {
    if (!row) return;
    selectedChange = row.id;
    if (!editing) return;
    if (!row.file_id || session?.textOf && !session.textOf(row.file_id)) {
      say(m.reader_change_file(),
        { kind: "problem", id: "reader:change-file" });
      return;
    }
    // A narrow window shows one pane at a time and the panel is covering it,
    // so the source comes forward. A wide window keeps the layout the reader
    // chose: with the preview alone, the preview goes to the change.
    if (compact) arrangement.showMobileView("source");
    await tick();
    if (!editor) return;
    rememberPlace(row.file_id, row.position);
    ws.openFile = row.file_id;
    editor.goToIn(row.file_id, row.position);
    if (!shown.source && shown.document) {
      clearTimeout(stepTimer);
      showCaretInDocument();
    }
  }

  /// Accept and reject are plain edits to the shared document, so they need no
  /// answer from the server. They return a resolved promise so the panel can
  /// advance the same way it would after a round trip.
  function acceptChange(row) {
    if (mayEdit && row?.id && accept(session.doc, row.id)) settleAnchors([row.file_id]);
    return Promise.resolve();
  }

  // A rejection is planned like an edit. The text it put back goes through the
  // session's planner with its move memory, so a piece that a tracked deletion
  // cut from a passage rejoins that passage. Comments that another session
  // detached are then put back by fragment. Each file the decision went through
  // is read again into the session's anchor cache.
  function settleAnchors(files, restored = [], edits = []) {
    const touched = [...new Set([...files, ...restored.map((r) => r.file)].filter(Boolean))];
    // A file the index never loaded has no before state to plan from. Its
    // detached records are still put back by fragment below.
    if (edits.length > 0 && session.anchors.has(files[0])) {
      session.anchors.record([{ fileId: files[0], edits: composeEdits(edits.map((edit) => [edit])), fragments: [] }], session.anchorMemory);
    }
    for (const file of touched) restoreByFragments(session.doc, file, restored.filter((r) => r.file === file));
    if (edits.length > 0 || restored.length > 0) session.doc.commit({ origin: ANCHORS_ORIGIN });
    for (const file of touched) session.anchors.load(file);
  }

  // One rejection, with the comments on the text it put back settled before
  // anything else moves that text. Returns whether it was rejected.
  function rejectSettled(row) {
    const { ok, restored, edits } = rejectRestoring(session.doc, row.id);
    settleAnchors([row.file_id], restored, edits);
    return ok;
  }

  function rejectChange(row) {
    if (mayEdit && row?.id) rejectSettled(row);
    return Promise.resolve();
  }

  /// Rejects several changes in the order given, owners before the changes
  /// they swallowed. Each is settled before the next, since each moves the text
  /// the next one looks at. Resolves to the number rejected.
  function rejectChanges(rows) {
    let count = 0;
    if (mayEdit) for (const row of rows) if (row?.id && rejectSettled(row)) count += 1;
    return Promise.resolve(count);
  }

  function receive(event) {
    if (annotations.receive(event)) return;
    if (event.type === "chat") {
      const fresh = !liveChat.has(event.id);
      liveChat.receive(event);
      if (fresh && !chatVisible && !pendingChat?.has(event.temp_id)) unreadChat = true;
      if (event.temp_id) pendingChat?.acknowledge(event.temp_id, true);
      return;
    }
    if (liveChat.receive(event)) return;
    if (event.type === "chat-ack") {
      pendingChat?.acknowledge(event.temp_id, true);
      return;
    }
    if (event.type === "hello") {
      liveChat.seed(event);
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
        say(m.reader_save_delayed(),
          { kind: "problem", id: "reader:save-delayed" });
        return;
      }
      if (event.temp_id && pendingChat?.has(event.temp_id)) {
        pendingChat.acknowledge(event.temp_id, false);
        say(event.message || m.reader_chat_refused(), { kind: "problem", id: "reader:chat-rejected" });
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
        outbox.failed(event.temp_id, m.reader_passage_changed());
        if (event.temp_id) annotations.removePending(event.temp_id);
        renderedProjectDigest = event.digest;
        if (!mayEdit) void projection.refresh();
        say(
          m.reader_stale_selection(),
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
      say(event.message || m.reader_change_refused(), { kind: "problem", id: "reader:change-refused" });
      return;
    }

    if (lifecycle.receive(event)) return;
    if (event.type === "source-changed") {
      // Readers and commenters never join CRDT sync (§2.1), so this frame is
      // the only way this browser learns the head moved: refetch the
      // projection, carrying the digest this fetch already remembers as the
      // etag (§11). An editor's own `session` hears every edit directly and
      // has nothing to do here.
      if (!mayEdit) void projection.refresh();
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
      projection.schedule();
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
  // Panels and dialogs that are not on screen at open load on first use.
  const lazy = {
    agent: lazyComponent(() => import("./reader/Agent.svelte")),
    git: lazyComponent(() => import("./reader/Git.svelte")),
    history: lazyComponent(() => import("./reader/History.svelte")),
    historyWorkspace: lazyComponent(() => import("./reader/HistoryWorkspace.svelte")),
    settings: lazyComponent(() => import("./settings/SettingsDialog.svelte")),
    commandPalette: lazyComponent(() => import("./CommandPalette.svelte")),
    symbols: lazyComponent(() => import("./SymbolPalette.svelte")),
    math: lazyComponent(() => import("./MathPalette.svelte")),
  };
  // Loads every lazy panel and the authorship module, so their code is fetched
  // and cached. Loading does not show anything: each panel is still gated on
  // its own open or mounted state. Resolves true only when every chunk loaded,
  // so the offline copy is not called ready with a panel missing.
  async function prefetchLazyPanels() {
    const results = await Promise.allSettled([
      ...Object.values(lazy).map((entry) => entry.load()),
      import("../lib/authorship.js"),
    ]);
    return results.every((result) => result.status === "fulfilled");
  }
  // A panel whose code cannot be fetched (offline, or a release that has moved
  // on) says so, rather than doing nothing.
  function lazyFailed() {
    say(m.reader_panel_unavailable(), { kind: "problem", id: "reader:panel-unavailable" });
  }
  let insertMenu = $state(null);
  // The palettes are overlays over the editor. Their components are kept so a
  // command can ask the Math overlay to open a template, or the Symbols overlay
  // to take the focus.
  let symbolPalette = $state(null);
  let mathPalette = $state(null);
  let symbolsOpen = $state(false);
  let mathOpen = $state(false);
  let editing = $state(false);
  // What the preview draws: the file, its format, why it is that one, the
  // choices when the open file has no preview of its own, and the lock. The
  // rule is `lib/opening.js`'s; the state and its changes are kept in
  // preview-target.svelte.js. Every callback is lazy, so the things it names
  // may be declared further down.
  const target = createPreviewTarget({
    texts: () => liveTreeNow().texts,
    formatOf: (path) => renderers.previewFormatOf(path),
    open: () => ({ textPath: openTextPath, hasFile: Boolean(openFile) }),
    lockedPath: () => lockedPath,
    // A link holder reading a file in place of the preview keeps the preview
    // as it was underneath, so going back to it shows the same page at once.
    skip: () => (!session && !projection.state.project) || (!mayEdit && viewing),
    painted: () => painted,
    configure: (format) => configureLatex(format, format ? (session || readerProjectOwner) : undefined),
    warm: (format, text) => warmEngine(format, text),
    invalidate: () => renderCoordinator.invalidate(),
    clearFrame: () => framePreview.clear(),
    navigateFrame: () => { if (docsOrigin) navigateFrame(); },
    cancelDebounce: () => schedule.cancel(),
    resetStatus: () => renderStatus.resetFailure(),
    cleared: () => {
      renderStatus.clearDocx();
      diagnosticPainter.rendered({ page: false, diagnostics: [] });
    },
    paint: () => { void paintPreview(); },
    settle: () => tick(),
    disposed: () => readerDisposed,
  });
  // Reads of the preview's state under the names the page has always used.
  // They are reads only: every write goes through `target`.
  const previewPath = $derived(target.state.path);
  const sourceFormat = $derived(target.state.format);
  const previewReason = $derived(target.state.reason);
  const previewChoices = $derived(target.state.choices);
  const lockedFile = $derived(target.state.lockedFile);
  // The paths that render on their own, which is what the eye in Files offers
  // to lock. Worked out with the target, from the same texts.
  const aloneSet = $derived(target.state.alone);

  let participants = $state([]);
  let linked = $state(read(LINKED, false) === true);
  // The formatting at the caret, from the Editor while visual mode is on.
  let formatActive = $state(null);

  // Routine saving stays quiet; losing the connection still needs a warning,
  // and the warning has to say what is happening to the typing meanwhile.
  const connectionNote = $derived.by(() => {
    if (connected) return "";
    if (openedLocally) return m.reader_offline_local_copy();
    if (!mayEdit || !session) return m.common_reconnecting();
    return persistence.local
      ? m.reader_offline_kept_locally()
      : m.reader_offline_reconnecting();
  });

  // What the document is called, which is what the rendered page is titled.
  // The title it was published under wins; a document that never had one is
  // named by its own first heading, the way `publish` names one -- the heading
  // of the file the preview draws, since a chapter's heading names the chapter.
  async function headingOf(tree) {
    return doc.title || (await renderers.titleOf(tree)) || m.common_untitled();
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

  // The manifest, and whether the last read of it succeeded, belong to the
  // timeline; the comparison below owns everything about a selected version.
  const timeline = createTimeline({
    slug: SLUG,
    key: KEY,
    // Open on what the restore replaced: the version just before the
    // restore's own row. The compare-with setting moves to Latest, since that
    // is what is shown: against now, that is the change the restore made, and
    // its Restore button is the undo. With no such version, it is the Latest
    // row alone.
    onrestored: () => {
      const newest = [...timeline.state.labels].sort((a, b) => b.seq - a.seq);
      const beforeSha = newest[0]?.why === "restore" ? newest[1]?.sha || "" : "";
      return historySource.setCompareWith("latest", beforeSha);
    },
    disposed: () => readerDisposed,
  });
  const labels = $derived(timeline.state.labels);
  // The loaded timeline pages plus the saved versions they have not reached yet
  // (saved versions can be older than any loaded page), so that a comparison can
  // order any endpoint the Saved tab offers. Deduplicated by sha, the loaded
  // page's own entry winning.
  const comparisonLabels = $derived.by(() => {
    const seen = new Set(labels.map(point => point.sha));
    const extra = timeline.state.saved.filter(point => {
      if (seen.has(point.sha)) return false;
      seen.add(point.sha);
      return true;
    });
    return [...labels, ...extra];
  });
  const historyProblem = $derived(timeline.state.problem);
  // Selection is a user-interface fact, not proof that the source could be
  // read: a failed comparison reports itself in the workspace and cannot make
  // a clicked version appear unselected.
  // Reads the timeline when it has not been read yet, and throws when it still
  // cannot be read.
  async function loadTimelineOrThrow() {
    if (timeline.state.loaded) return;
    await timeline.load();
    if (!timeline.state.loaded) {
      throw new Error(timeline.state.problem || m.history_source_label_load_failed());
    }
  }
  const historySource = createHistorySource({
    read: sha => history.read(SLUG, sha, keyHeaders(KEY)),
    labels: () => comparisonLabels,
    // The newest version, or CURRENT when the timeline is known to hold none.
    // Throws when the timeline cannot be read.
    newest: async () => {
      await loadTimelineOrThrow();
      return timeline.state.labels.at(-1)?.sha ?? CURRENT;
    },
    // The version before this one in the whole history, reading older pages when
    // it is not loaded yet. The saved list is not used: it skips unsaved versions.
    previous: async (sha) => {
      await loadTimelineOrThrow();
      const at = () => timeline.state.labels.findIndex(point => point.sha === sha);
      while (at() <= 0 && !timeline.state.complete && !readerDisposed) {
        await timeline.loadMore();
        if (timeline.state.moreProblem) throw new Error(timeline.state.moreProblem);
      }
      const index = at();
      return index > 0 ? timeline.state.labels[index - 1].sha : index === 0 ? NOTHING : CURRENT;
    },
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
        if (readerDisposed || !session || session.joined === false) throw new Error(m.reader_still_connecting());
        return session.tree();
      })();
    },
  });
  const historySelectedSha = $derived(historySource.selected);
  $effect(() => () => historySource.dispose());
  // On a phone only one pane shows. Picking a version needs the timeline, so
  // show it while a selection is open. When the choice is made or cancelled,
  // show the source again, but only while the comparison is still open: close()
  // also resets the mode, and by then the reader may have left History for
  // somewhere else, which must not be overridden. close() clears `newer`, so an
  // empty `newer` means the comparison is gone.
  let previousHistoryMode = "normal";
  $effect(() => {
    const mode = historySource.mode;
    const before = previousHistoryMode;
    previousHistoryMode = mode;
    if (!compact || mode === before) return;
    if (before === "normal") untrack(() => arrangement.showMobileView("sidebar"));
    else if (mode === "normal" && historySource.newer) untrack(() => arrangement.showMobileView("source"));
  });
  // Which file the reader arrived asking for, when the landing page's search
  // found the project by one of its files. Honoured once the directory has
  // arrived, and once only.
  const ARRIVED_FILE = new URLSearchParams(location.search).get("file") || "";

  const loadHistory = () => timeline.load();

  // The Authors tab of the history panel. The panel says when the tab is on
  // or off; while it is on, the editor colours the text by who wrote it. The
  // map from peer to name is fetched once and then kept, so turning the tab
  // on again is instant. A collaborator who joined after the fetch is not in
  // the map, and the editor says so, which is what the refetch below is for.
  let authorsView = $state(false);
  let authorsOnly = $state(null);
  let authorsMap = $state(null);
  let authorsLoading = $state(false);
  let authorsError = $state("");
  let authorsSummary = $state(null);
  let authorsRequests = 0;
  // When the map was last asked for, whether or not it arrived. A refetch
  // waits this long after the last one, so a failing server is not asked on
  // every redraw.
  let authorsFetchedAt = 0;
  const AUTHORS_REFETCH_MS = 10000;

  async function loadAuthorsMap() {
    const request = ++authorsRequests;
    authorsFetchedAt = Date.now();
    authorsLoading = true;
    authorsError = "";
    try {
      const { loadAuthors } = await import("../lib/authorship.js");
      const map = await loadAuthors(SLUG, authHeaders(KEY));
      if (request === authorsRequests) authorsMap = map;
    } catch (error) {
      if (request === authorsRequests) authorsError = error.message;
    } finally {
      if (request === authorsRequests) authorsLoading = false;
    }
  }

  // The timeline and the source workspace share one selection. Selecting a
  // version shows it beside the other end the compare-with setting picks. The
  // document pane still shows the document as it stands.
  function viewPoint(sha) {
    beginComparison("history");
    const pending = historySource.view(sha);
    if (compact) arrangement.showMobileView("source");
    return pending;
  }

  // A comparison is the editor area showing a version or a changed file in
  // place of the source. Choosing one starts it and saves the editor area as it
  // is; leaving its panel, or Close, ends it and puts that back. Opening a panel
  // shows only its list.
  const comparison = createComparison();
  const comparing = $derived(comparison.kind);
  function editorAreaView() {
    return {
      editing,
      layout,
      preferredPane,
      mobileView,
      viewing,
      openFile: ws.openFile,
      figure: ws.figure,
      caret: editor && !ws.figure ? { file: ws.openFile, at: editor.caret() } : null,
    };
  }
  function beginComparison(kind) {
    comparison.begin(kind, editorAreaView);
  }
  function endComparison() {
    if (comparison.kind === "history") historySource.close();
    const view = comparison.end();
    if (view) void restoreEditorArea(view);
  }
  async function restoreEditorArea(view) {
    // Editing does not end once it has begun, so a source pane that was closed
    // is closed by the arrangement and the face that hide it.
    arrangement.state.layout = view.editing ? view.layout : "document";
    arrangement.state.preferredPane = view.editing ? view.preferredPane : "document";
    arrangement.state.mobileView = view.mobileView === "source" && !view.editing ? "document" : view.mobileView;
    viewing = view.viewing;
    // A file deleted while the comparison was open is not put back: the
    // workspace has already chosen another, and the editor cannot show a file
    // the project no longer holds.
    const present = (id) => Boolean(id) && ws.files.some((file) => file.id === id);
    if (present(view.openFile)) ws.openFile = view.openFile;
    if (!view.figure || present(view.figure.id)) ws.figure = view.figure;
    // The caret is put back once the editor is mounted again. A Git comparison
    // unmounts it while it is open.
    if (!view.caret || !view.editing || !present(view.caret.file)) return;
    await tick();
    if (readerDisposed) return;
    editor?.goToIn(view.caret.file, view.caret.at);
  }

  const restoreBusy = $derived(timeline.state.restoreBusy);
  const restoreMoved = $derived(timeline.state.restoreMoved);
  const restoreName = $derived(timeline.describe(timeline.state.restoreSha));
  // What restoring is about to discard, read here and handed over, so that
  // the question and the answer are asked of the same project.
  const projectSignature = () => {
    const tree = session?.tree?.();
    return JSON.stringify({ texts: tree?.texts || {}, files: tree?.files || {} });
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

  /// A version downloads as a zip built here from that version's source and
  /// figures, the same way the project download is built, so nothing needs to
  /// be prepared on the server first. `downloadingSha` is the version being
  /// fetched; asking for another version supersedes it.
  let downloadingSha = $state("");
  let downloadProblem = $state("");
  // `YYYY-MM-DD-HHMM` in local time, or the start of the version's id when
  // its time is missing or not a date.
  function versionStamp(at, sha) {
    const when = at ? new Date(at) : null;
    if (!when || Number.isNaN(when.getTime())) return sha.slice(0, 8);
    const pad = (n) => String(n).padStart(2, "0");
    return `${when.getFullYear()}-${pad(when.getMonth() + 1)}-${pad(when.getDate())}-${pad(when.getHours())}${pad(when.getMinutes())}`;
  }
  async function downloadArchive(sha) {
    if (downloadingSha === sha) return;
    downloadingSha = sha;
    downloadProblem = "";
    try {
      const point = await history.read(SLUG, sha, keyHeaders(KEY));
      const digests = {};
      for (const [path, file] of Object.entries(point.files || {})) {
        if (file?.kind !== "text" && file?.digest) digests[path] = file.digest;
      }
      const files = await projectFiles({ tree: { texts: point.texts || {}, digests }, folders: [], gather: gatherFigures });
      if (downloadingSha !== sha) return; // superseded by a later request
      saveBlob(await archive(files), `${SLUG}-${versionStamp(point.at, sha)}.zip`);
    } catch (error) {
      if (downloadingSha === sha) downloadProblem = error.message || m.readerb_version_download_failed();
    } finally {
      if (downloadingSha === sha) downloadingSha = "";
    }
  }

  // The document as a renderer takes it: every text in it and the figures by
  // digest. The file the render builds is named by `treeNow` below.
  //
  // Before a session arrives there is no source tree to render.
  // The file manager always operates on the live directory. In particular,
  // its list remains live while the document pane is showing a label, so
  // downloads must use the same source as that list.
  function liveTreeNow() {
    if (!session) return projection.tree();
    const tree = session.tree();
    // CodeMirror owns the active Loro binding and exposes the text it is
    // displaying. During a local transaction its view can be one tick ahead
    // of the directory observer, so use that current source for snapshots
    // taken by the preview scheduler.
    const activeText = editing && openTextPath && editor?.text?.(openFile) != null
      ? String(editor.text(openFile))
      : null;
    return activeText == null ? tree : { ...tree, texts: { ...tree.texts, [openTextPath]: activeText } };
  }

  // The tree a render builds: the live tree with the file the preview shows
  // named as its entry. That entry is chosen per render and is no setting of
  // the project.
  function treeNow() {
    const tree = liveTreeNow();
    if (!previewPath || !(previewPath in tree.texts)) return tree;
    return { ...tree, main: previewPath };
  }

  // Painting the preview is sending it to the frame: the draft is a document,
  // and a document belongs on the documents origin, not in this page. The
  // agent republishes its text from there, which re-anchors every comment
  // against what was just typed.
  const schedule = createSourceSchedule({
    editing: () => editing,
    format: () => sourceFormat,
    latexDebounce: () => latex.debounce(),
    paint: () => paintPreview(),
    retarget: () => target.update(),
  });
  const renderCoordinator = createRenderCoordinator({
    navigation: () => target.navigation,
    source: () => schedule.generation,
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

  /// A loadable URL for a figure in the open project, for the editor's visual markup.
  async function figureUrlFor(path) {
    const digest = treeNow().digests?.[path];
    if (!digest) return null;
    const held = await figures.gather(SLUG, { [path]: digest }, authHeaders(KEY));
    return held.urls[path] ?? null;
  }

  // Quitting exits the native app, which ends every window. The app asks each
  // window and exits.
  async function quit() {
    try {
      await localQuarto.shellAction("quit");
    } catch (error) {
      say(error.message || m.readerb_quit_failed(), { kind: "problem", id: "reader:quit" });
    }
  }
  // What the comparison bar says is compared.
  const comparisonSays = $derived(comparing === "history" ? m.readerb_version_history() : "");
  // A web project's Git panel: the last push.
  const webGit = createWebGit();
  $effect(() => () => webGit?.dispose());
  // The desktop app's connection of this project to a git repository, for an
  // editor. It is made once the session says the person may edit, which is
  // after this script runs.
  let webConnection = $state(null);
  $effect(() => {
    if (!hasDesktop || !mayEdit) return;
    const connection = createWebConnection({
      project: `${location.origin}/docs/${SLUG}`,
      flush: async () => {
        // A push captures the project, so the editor's edits must reach the
        // server first. A session that is reconnecting is waited for, briefly.
        const deadline = Date.now() + 10000;
        while (!readerDisposed && (!persistence.joined || persistence.pending) && Date.now() < deadline) {
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        if (!persistence.joined || persistence.pending) throw new Error(m.readerb_edits_unsaved());
      },
      signedIn: async () => Boolean((await whoami())?.provider),
      approve: (userCode) => post("/api/auth/device/approve", { user_code: userCode }),
    });
    webConnection = connection;
    return () => {
      connection.dispose();
      webConnection = null;
    };
  });
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
    return renderers.formatOf(path) || (path && path === previewPath ? sourceFormat : "");
  });
  // The outline of the open file. Typst reads the typst module's tree, which
  // is empty until the module loads; the revision brings the outline back then.
  const outlineHeadings = $derived.by(() => {
    void outlineRevision;
    if (panel !== "outline" || !mayEdit) return [];
    return extractOutline(outlineSource, outlineFormat);
  });
  // Word counts for each heading in the Outline panel.
  const outlineSections = $derived.by(() => panel === "outline" && mayEdit
    ? sectionWords(outlineSource, outlineFormat, outlineHeadings) : []);
  // The Word count dialog: the open file's document with its includes, and
  // the selection in the open file. A preview lock does not change the count.
  // Nothing is counted while the dialog is shut.
  let wordCountOpen = $state(false);
  const wordCount = $derived.by(() => {
    void outlineRevision;
    if (!wordCountOpen || !mayEdit || !toolbarPath) return null;
    const texts = liveTreeNow().texts;
    const root = previewTarget({ open: toolbarPath, locked: null, texts }).path || toolbarPath;
    const range = editor?.selection?.();
    return {
      document: documentWords(root, texts),
      selection: range && range.file === openFile ? countWordsIn(editor.text(), outlineFormat, range.from, range.to) : null,
    };
  });
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
  // What the preview frame shows, and what it has shown since it last loaded.
  // The facts and their resets live in painted.svelte.js.
  const painted = createPainted();
  // Whether the frame shows any page at all since it last loaded, even one
  // of another file. The viewer swaps new pages in only once they are drawn,
  // so such a frame can stay in sight while it draws the next one.
  let frameHasPage = $state(false);
  // A delivered PDF the viewer has not confirmed within this long is shown
  // anyway. The confirmation is what keeps the frame out of sight, and a
  // viewer that never sends it must not leave the pane waiting for ever.
  const DRAWN_GRACE_MS = 4000;
  $effect(() => {
    if (!pdfOutput || !painted.state.shown || painted.state.drawn) return;
    const timer = setTimeout(() => painted.drawn(), DRAWN_GRACE_MS);
    return () => clearTimeout(timer);
  });
  // The renderer engine being downloaded for the preview, or "". LaTeX
  // reports its own loading phase and is never named here.
  let engineLoading = $state("");
  function warmEngine(format, source = "") {
    const wanted = format;
    if (!["latex", "html", "text"].includes(format)) engineLoading = format;
    void renderers.warm(format, source).then(() => {
      if (engineLoading === wanted) engineLoading = "";
    });
  }

  const errorCount = $derived(diagnostics.filter((d) => d.severity !== "warning" && d.severity !== "info").length);
  const warningCount = $derived(diagnostics.filter((d) => d.severity === "warning").length);
  const diagnosticBadge = $derived(
    errorCount && warningCount
      ? m.readerb_count_both({ errors: m.readerb_count_errors({ count: errorCount }), warnings: m.readerb_count_warnings({ count: warningCount }) })
      : errorCount
        ? m.readerb_count_errors({ count: errorCount })
        : warningCount
          ? m.readerb_count_warnings({ count: warningCount })
          : "",
  );

  // What the rail says about a panel beyond its name: a fuller label for a
  // screen reader, counts to print under the icon, a dot to mark it. Written
  // here because this is where a diagnostic and a tracked change mean
  // something; the rail draws whatever it is handed.
  const badges = $derived({
    diagnostics: {
      says: diagnosticBadge ? m.readerb_diagnostics_says({ badge: diagnosticBadge }) : "",
      counts: diagnostics.length
        ? [
            errorCount ? { tone: "errors", of: errorCount } : null,
            warningCount ? { tone: "warnings", of: warningCount } : null,
          ].filter(Boolean)
        : [],
    },
    changes: {},
    collaboration: unreadChat ? { says: m.readerb_collaboration_unread(), dot: true } : {},
    agent: agentWaitingBadge({ waiting: agentWaiting, visible: agentInView }),
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
    const id = session?.idOf(item.file || previewPath);
    return files.find((file) => file.id === id && file.kind === "text") || null;
  }

  async function openDiagnostic(item) {
    const file = diagnosticFile(item);
    if (!file) return;
    rememberPlace(file.id);
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
  // not conditioned on the source pane being open: the output starts on the
  // person's default preview for this format (HTML unless they chose PDF in
  // Settings), so somebody who only reads the document is not left on a paged
  // fallback.
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
  const buildUserId = $derived(buildPreferenceUser(me));
  const buildSettings = createBuildSettings({
    slug: SLUG,
    origin: location.origin,
    interrupt: stopRendering,
    paint: () => void paintPreview(),
  });
  const buildPreferences = $derived(buildSettings.state.preferences);
  // Where the toggle is offered: in the desktop app, for every editable
  // document whose format is set to this computer.
  const runCodeOffered = $derived(mayEdit && hasDesktop && buildPreferences.backend === "local");
  const latexOutput = $derived(buildSettings.state.latexOutput);
  const typstOutput = $derived(buildSettings.state.typstOutput);
  const markdownOutput = $derived(buildPreferences.output === "pdf" ? "pdf" : "html");
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
  // Nothing can be drawn here, so there is nothing to wait for.
  const noCompiler = $derived(pdfOutput && !painted.state.shown && !renderState.failure && !compilesHere);

  // The LaTeX compile status: idle, running, or failed, with progress and
  // messages. Used by Diagnostics and the preview's progress line.
  let latexState = $state.raw(latex.status());
  const latexPhase = $derived(latexState.phase);
  $effect(() => latex.subscribe((next) => (latexState = next)));

  // A build that failed and said nothing a diagnostic could carry must
  // still be reported somewhere.
  const buildFailure = $derived(
    sourceFormat === "latex" && !latexHtmlPreview
      ? (latexPhase === "failed" ? latexState.message || m.readerb_build_failed() : "")
      : renderState.failure ? renderState.failureReason || m.readerb_build_failed() : "",
  );

  // A Quarto document is drawn as the Markdown it is until Run code is on,
  // so the desktop app says what turns the real render on.
  const quartoNeedsRunCode = $derived(sourceFormat === "quarto" && hasDesktop && mayEdit && !localExecution);

  const localAppDiagnostics = $derived([
    quartoNeedsRunCode ? { severity: "warning", message: m.readerb_run_code_for_render() } : null,
  // An item with no message is dropped. An empty error would show as a blank
  // entry and would hide the failed render's own message.
  ].filter((item) => item?.message).map((item) => ({ ...item, source: "local-app" })));
  // Backups that are off say nothing. Only a failing backup is reported.
  const backupDiagnostics = $derived.by(() => {
    const data = backupStatus.active ? backupStatus.data : null;
    if (!data?.enabled) return [];
    const message = data.needs_login ? m.readerb_backup_lost_access()
      : data.error ? m.readerb_backup_failing({ error: data.error })
      : !data.destination_set ? m.readerb_backup_no_destination()
      : "";
    return message ? [{ severity: "warning", message, source: "backups" }] : [];
  });
  $effect(() => {
    void localAppDiagnostics;
    void backupDiagnostics;
    void previewNoteItems;
    void buildFailure;
    void editing;
    untrack(() => diagnosticsController.refresh());
  });

  // What the PDF frame is showing, as the frame itself reports it: whether a
  // page is drawn at all, the zoom mode in force, and the scale it came out
  // at. The header's controls are drawn from this rather than from what was
  // last asked for, so the percentage beside them is the one on screen -- a
  // frame too narrow for "150%" draws it smaller, and says so.
  //
  // A format with no paged frame never sends it, which is exactly how the
  // controls stay out of the header for flowing HTML.
  let viewerView = $state(null);
  // Counts the frame's requests for the search row; the controls watch it.
  let findAsk = $state(0);
  // Whether PDF pages are inverted while the application is dark. Remembered
  // like the other habits of the person reading, and sent to each new frame
  // once it reports a page drawn.
  let invertPages = $state(read(PDF_INVERT, false) === true);
  let invertSent = false;
  // The desktop app's page zoom as far as WebKit leaves it out of
  // devicePixelRatio (1 everywhere else), so the frame can draw sharp.
  // Nothing is sent while it is 1 and never was anything else.
  let pageZoom = $state(1);
  let pageZoomChanged = false;
  let densitySent = false;
  // Untracked: the listener's first call runs inside the effect, and reading
  // pageZoom there would resubscribe on every change.
  $effect(() => untrack(() => onPageZoom((next) => {
    if (next === pageZoom) return;
    if (pageZoom !== 1 || next !== 1) pageZoomChanged = true;
    pageZoom = next;
    if (viewerView) tell({ type: "viewer-density", zoom: next });
  })));
  // Whether the application is dark right now; the invert item is only
  // offered then, because the frame ignores it in a light theme.
  let darkNow = $state(false);
  $effect(() => onMode((next) => (darkNow = next === "dark")));
  let viewerTool = $state("select");
  framePreview = createFramePreview({
    slug: SLUG,
    getDocsOrigin: () => docsOrigin,
    framePath: () => framePath,
    setSource: (source) => (frameSrc = source),
    send: tell,
    onNavigate: () => {
      live.reset();
      frameReady = false;
      frameHasPage = false;
      // A new frame has nothing drawn in it and no tool set; it says so once
      // it loads. Until then the controls go away rather than describing the
      // document that just left.
      viewerView = null;
      painted.navigated();
      viewerTool = "select";
      invertSent = false;
      densitySent = false;
      findAsk = 0;
      renderCoordinator.invalidate();
      renderStatus.clearDocx();
      frameOverlays.resetAnnotations();
    },
    onDelivered: (payload) => {
      painted.delivered();
      // An HTML page is on screen the moment it is handed over.
      if (payload.kind === "html") frameHasPage = true;
    },
  });


  // Instant preview updates: a small LaTeX edit is shown in the HTML preview
  // before the compile that confirms it.
  const live = createLivePreview({ send: (message) => tell(message) });
  const liveActive = () => !readerDisposed && Boolean(session) && sourceFormat === "latex" && latexOutput === "html"
    && readLivePatch(buildScope(), "latex");
  // Every .tex text as the editor shows it now, by path. Only the sources are
  // read: no figures, no assets.
  function liveTexts() {
    const out = Object.create(null);
    for (const file of session.list()) {
      if (file.kind !== "text" || !/\.tex$/i.test(file.path)) continue;
      out[file.path] = file.path === openTextPath && editing && editor?.text?.(openFile) != null
        ? String(editor.text(openFile))
        : String(session.textOf(file.id)?.toString?.() ?? "");
    }
    return out;
  }

  const navigateFrame = (force = false) => framePreview.navigate(force);
  const replayPreview = () => paintsTheFrame && framePreview.replay();
  const frameLoaded = () => {
    const first = framePreview.markReady();
    frameReady = true;
    if (first) replayPreview();
  };

  // The kind of frame follows the tree being displayed, including a
  // historical tree. This effect is also what navigates when the file the
  // preview shows changes from Markdown/Typst/HTML to LaTeX or back.
  $effect(() => {
    void docsOrigin;
    void framePath;
    untrack(() => navigateFrame());
  });

  function refreshFramedPage() {
    if (paintsTheFrame || !session || !docsOrigin || !previewPath) return;
    framePreview.refresh(String(session.textOf(session.idOf(previewPath))?.toString?.() ?? ""));
  }

  const compiling = $derived(Boolean(
    (sourceFormat === "latex" && !latexHtmlPreview && ["loading", "compiling", "browser-biber"].includes(latexPhase))
      || renderState.compiling,
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
    // The page this file last drew stands in for an empty pane until the
    // render replaces it.
    lastPage: lastCachedPage,
    rememberPreview: cachePage,
    onRendered: (identity, projectDigest) => {
      renderedIdentity = identity;
      if (identity && liveActive()) live.compiled(liveTexts());
      else live.reset();
      if (!mayEdit) renderedProjectDigest = projectDigest || "";
    },
    onEmpty: () => {
      painted.reset({ empty: true });
      renderStatus.resetFailure();
    },
    facts: () => ({
      disposed: readerDisposed,
      navigation: target.navigation,
      source: schedule.generation,
      everPainted: painted.ever,
      format: sourceFormat,
      hasSession: Boolean(session),
      paintsTheFrame,
      latexOutput,
      typstOutput,
      buildPreferences,
      localExecution,
      projectDigest: mayEdit ? "" : projection.state.project?.project_digest || "",
    }),
    tree: () => treeNow(),
    gather: (digests) => gatherFigures(digests),
    heading: (tree) => headingOf(tree),
    takeManual: () => {
      const manual = manualCompile;
      manualCompile = false;
      return manual;
    },
    manualPending: () => manualCompile,
    onsynctex: (parsed) => { painted.synctex = parsed; },
    send: (message) => tell(message),
    clearDebounce: () => {
      schedule.cancel();
    },
    refreshFrame: () => refreshFramedPage(),
    debug: (...parts) => {
      try {
        if (localStorage.getItem("librepaper-latex-debug")) console.debug(...parts);
      } catch { /* diagnostics are optional */ }
    },
  });

  function sourceChanged() {
    if (readerDisposed) return;
    schedule.bump();
    painted.state.empty = false;
    refreshReview();
    outlineRevision += 1;
    if (sourceFormat === "quarto" && session && previewPath) {
      const main = previewPath;
      const parsed = quarto.parseQuarto(String(session.textOf(session.idOf(main))?.toString?.() ?? ""), { path: main });
      const nextDiagnostics = parsed.diagnostics.map((item) => diagnosticContext(item, { main, texts: { [main]: parsed.source } }, ""));
      const diagnosticsGeneration = schedule.generation;
      // Loro can notify this observer from inside CodeMirror's update
      // listener. Editor.setDiagnostics dispatches another update, which
      // CodeMirror rejects while the first one is still in progress. Source
      // invalidation stays synchronous, but the editor repaint waits until
      // the current transaction has returned and is skipped if newer text
      // arrived before then.
      queueMicrotask(() => {
        if (readerDisposed || diagnosticsGeneration !== schedule.generation) return;
        diagnosticsController.render(nextDiagnostics);
      });
    }
    // The keystroke, which is what the diagnostic wait is measured from.
    diagnosticPainter.typed();
    if (liveActive()) live.edited(liveTexts(), previewPath || treeNow().main);
    else live.reset();
    schedule.changed();
  }

  /* ------------------------------------------------------- keeping in step */

  let stepTimer = null;
  function outlineTextChanged() {
    // CodeMirror has applied the local or remote transaction by this point.
    // The source observer can run before its caret has been mapped.
    outlineActiveFrom = editor?.caret?.() ?? null;
    noteSourceSelection();
  }
  // Whether the source has a selection, which "Attach to selection" waits on.
  // Read wherever the editor reports a caret, a text change or a file change:
  // those are the ways a selection moves.
  let sourceSelected = $state(false);
  let sourcePassageNow = $state(null);
  function noteSourceSelection() {
    const range = editor?.selection?.();
    sourceSelected = Boolean(range);
    sourcePassageNow = range ? sourcePassage(editor.text(), range.from, range.to, session?.paths?.get(range.file) || "") : null;
  }
  // Tasks on a passage take it from the source, where selecting is precise and
  // the text is what the server finds. The rendered selection serves only
  // readers without the source.
  const agentSelection = $derived.by(() => {
    if (sourcePassageNow) return { source: sourcePassageNow, exact: sourcePassageNow.exact };
    return !mayEdit && pending && !pending.whole ? pending : null;
  });
  function outlineCaretChanged() {
    outlineTextChanged();
    followCaret();
  }

  // Set while a click in the preview is placing the caret: the echo of that
  // move must not scroll the preview to the caret it was just clicked at.
  let previewPlacing = false;
  function followCaret() {
    if (previewPlacing || !linked || !editing || docText === null) return;
    clearTimeout(stepTimer);
    stepTimer = setTimeout(showCaretInDocument, 120);
  }

  // Shows the document at the place the caret is in the source, whether or not
  // the two are kept in step.
  function showCaretInDocument() {
    if (!editing || docText === null || !editor) return;
    // The format of the file being edited, which is not always the
    // document's: a .bib beside a .tex has comments of its own kind, and
    // stripping .tex comments out of it would blank the wrong runs.
    const openPath = session?.paths?.get(openFile) || "";
    const format = openPath ? renderers.formatOf(openPath) : sourceFormat;
    if (format === "latex" && painted.synctex) {
      const path = session?.paths?.get(openFile) || "";
      const precise = painted.synctex.forward(path, synctexLineAt(editor.text(), editor.caret()));
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
  }

  function followDocumentClick(offset, pdfPoint = null) {
    if (!linked || !editing || docText === null || !editor) return;
    // The words double-clicked in the document may belong to any file: a reader
    // double-clicking a paragraph of chapter three is asking for chapter three, not
    // for the file that happens to be on screen. So the whole directory is
    // searched, and the file the words are in is opened.
    const tree = treeNow();
    if (followPdfClick(pdfPoint)) return;
    const found = sync.sourcePlaceInTree(docText, offset, tree, {
      open: session?.paths?.get(openFile) || "",
      formatOf: renderers.previewFormatOf,
    });
    if (!found) return;
    const id = session.idOf(found.path);
    rememberPlace(id, found.at);
    if (id) {
      ws.openFile = id;
      editor.goToIn(id, found.at);
    } else {
      editor.goTo(found.at);
    }
  }

  function followPdfClick(pdfPoint) {
    if (!linked || !editing || !editor || sourceFormat !== "latex" ||
        !painted.synctex || !pdfPoint) return false;
    const precise = painted.synctex.inverse(Number(pdfPoint.page), Number(pdfPoint.x), Number(pdfPoint.y));
    const id = precise && session.idOf(precise.path);
    if (!id) return false;
    rememberPlace(id);
    ws.openFile = id;
    editor.openAt(id, precise.line, 1);
    return true;
  }

  // A plain click in the preview puts the source caret where the words clicked
  // are in the source, and scrolls the source to show it. The keyboard stays
  // with the document: the source is moved, not focused. Only the open file is
  // searched, so a click never switches files, and only on a wide window where
  // the source is beside the document -- a phone shows one pane at a time and a
  // click there is for reading.
  function followPreviewClick(offset, pdfPoint = null) {
    if (!linked || compact || !editing || comparing !== "" || !editor || docText === null || !openFile) return;
    if (effectiveLayout !== "split" && effectiveLayout !== "source") return;
    const text = editor.text();
    if (typeof text !== "string") return;
    let at = null;
    // SyncTeX is only for LaTeX. Any other format, or a SyncTeX point that does
    // not resolve to the open file, falls back to the text the click carried.
    const precise = pdfPoint && sourceFormat === "latex" && painted.synctex
      ? painted.synctex.inverse(Number(pdfPoint.page), Number(pdfPoint.x), Number(pdfPoint.y))
      : null;
    if (precise && session.idOf(precise.path) === openFile) {
      // SyncTeX gives a line and no column, so the start of that line is the place.
      at = synctexOffsetOfLine(text, precise.line);
    } else {
      if (offset === null || offset === undefined) return;
      const openPath = session?.paths?.get(openFile) || "";
      const format = openPath ? renderers.formatOf(openPath) : sourceFormat;
      // Exact words only: the widening search a double-click allows would, on
      // every single click, now and then land somewhere nearby instead.
      at = sync.sourcePlaceFor(docText, Number(offset) || 0, text, format, { exact: true });
    }
    if (at === null || at === undefined) return;
    previewPlacing = true;
    try {
      editor.goTo(at, { focus: false });
    } finally {
      previewPlacing = false;
    }
  }

  function setLinked(on) {
    linked = on;
    if (!localDataForgotten()) write(LINKED, on);
  }

  function setVisual(on) {
    visual = on;
  }

  /* ------------------------------------------------------------------ panes */

  // Which side the source is on, which keys the editor answers to and how
  // wide the panes are: one reader's habits rather than anything about a
  // document. The preferences own them, and own writing them down -- see
  // `reader/preferences.svelte.js` for why those are one act and not two.
  const preferences = createPreferences();
  const prefs = preferences.state;
  // Visual mode starts from the Editor setting on every open. The top-bar switch changes it for this session only.
  let visual = $state(prefs.visual);
  // Which panes and which panel are on screen, on a desktop and on a phone.
  // `reader/arrangement.svelte.js` owns them and what they add up to.
  const arrangement = createArrangement({
    editing: () => editing,
    comparing: () => comparing,
    sourceSide: () => prefs.sourceSide,
    sizes: () => prefs.sizes,
    home: () => home,
    blocked: (name) => DEMO && DEMO_OFF.includes(name),
    leaving: (name) => { if (comparison.kind && comparison.kind !== name) endComparison(); },
    loadHistory: () => loadHistory(),
  });
  const layout = $derived(arrangement.state.layout);
  const panel = $derived(arrangement.state.panel);
  const mobileView = $derived(arrangement.state.mobileView);
  const preferredPane = $derived(arrangement.state.preferredPane);
  const moreOpen = $derived(arrangement.state.moreOpen);
  const mobileCommand = $derived(arrangement.state.mobileCommand);
  const keyboardOpen = $derived(arrangement.state.keyboardOpen);
  const width = $derived(arrangement.state.width);
  const unit = $derived(arrangement.state.unit);
  const compact = $derived(arrangement.compact);
  const sourceOpen = $derived(arrangement.sourceOpen);
  const activeMobileView = $derived(arrangement.activeMobileView);
  const splitTight = $derived(arrangement.splitTight);
  const adapted = $derived(arrangement.adapted);
  const effectiveLayout = $derived(arrangement.effectiveLayout);
  const panes = $derived(arrangement.panes);
  const shown = $derived(arrangement.shown);
  let collaborationTab = $state("comments");
  let moreOpener = null;
  function openMore(event) {
    moreOpener = event?.currentTarget ?? null;
    arrangement.state.moreOpen = true;
  }
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
  const completion = $derived(prefs.completion);
  const brackets = $derived(prefs.brackets);
  const latexAssist = $derived(prefs.latexAssist);
  const proofing = $derived(prefs.proofing);
  const dialect = $derived(prefs.dialect);
  const check = $derived(prefs.check);
  const sizes = $derived(prefs.sizes);

  // Settings open as a dialog from the navbar menu or the sidebar. Every entry
  // point opens the same dialog on the category it is about.
  let settingsOpen = $state(untrack(() => initialSettings === "tools"));
  let settingsCategory = $state(untrack(() => initialSettings || "general"));
  // An entry to scroll to once Settings shows, such as a section's id.
  let settingsEntry = $state("");
  let saveTemplateDialog = $state(null);

  async function openSettings(category = "general", entry = "") {
    // The editor category carries the shortcut table, and the table greys what
    // the source cannot currently do, so the editor is asked before the page
    // that shows the answer is drawn.
    refreshEditAvailability();
    settingsCategory = category;
    settingsEntry = entry;
    try { await lazy.settings.load(); } catch { lazyFailed(); return; }
    settingsOpen = true;
  }
  $effect(() => { if (settingsOpen) lazy.settings.load().catch(() => {}); });
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
  // A panel's code is fetched when the panel is first mounted.
  $effect(() => {
    for (const id of mountedPanels) {
      if (id === "agent" || id === "git") lazy[id].load().catch(() => {});
      if (id === "history") {
        lazy.history.load().catch(() => {});
        lazy.historyWorkspace.load().catch(() => {});
      }
    }
  });
  $effect(() => {
    if (comparing === "history") lazy.historyWorkspace.load().catch(() => {});
  });
  // The tabs this browser is offered. Which reader may see which panel is
  // written with the panels, in lib/panels.js; this only says who is asking.
  const tabs = $derived(
    tabsFor({ role: doc.role, mayEdit, editing, canSeeSharing: DEMO || canSeeSharing, canPublish, desktop: hasDesktop })
      .map((tab) => (DEMO && DEMO_OFF.includes(tab.id) ? { ...tab, disabled: true } : tab)),
  );
  // The robot buttons hand work to the agent. The agent runs only in the
  // desktop app, so in a browser they say so rather than disappear.
  const agentOffered = $derived(!DEMO && tabs.some((tab) => tab.id === "agent"));
  // Where the column goes back to when what it showed is taken away: the
  // files, for every role that is offered them, which is every role that has
  // opened a project. Before the document has answered nobody is offered
  // anything, and the column is not drawn.
  const home = $derived(
    tabs.some((tab) => tab.id === "files") ? "files"
    : tabs.some((tab) => tab.id === "collaboration") ? "collaboration"
    : "",
  );
  // The height of the demo banner, taken off the workspace so the page does not scroll.
  let demoBannerHeight = $state(0);
  // The first time a project is opened by someone it was shared with, one line
  // says what the link allows. Shown once per project in this browser, and
  // until dismissed; an owner, or a role not yet known, is shown nothing.
  const shareIntroWasSeen = DEMO || shareIntroSeen(SLUG);
  let shareIntroDismissed = $state(false);
  let shareIntroHeight = $state(0);
  const shareIntro = $derived(shareIntroWasSeen || shareIntroDismissed ? null : shareIntroKind(doc.role));
  // Taken off the workspace only while the line is shown, as the height is
  // not reset when the banner is removed.
  // clientHeight is a whole number, rounded down from the banner's true height;
  // one more pixel keeps the workspace from ending a fraction too tall, which
  // would let the page scroll.
  const shareIntroOffset = $derived(shareIntro ? shareIntroHeight + 1 : 0);
  $effect(() => {
    if (shareIntro) markShareIntroSeen(SLUG);
  });
  // The policy links come from /api/me; fetched only while the banner that shows them is up.
  let shareIntroLegal = $state(undefined);
  $effect(() => {
    if (!shareIntro) return;
    whoami().then((who) => (shareIntroLegal = who?.legal));
  });

  // Kept current for the background work that consults it.
  $effect(() => {
    const track = () => {
      hidden = document.visibilityState === "hidden";
      if (!hidden && !mayEdit) projection.resume();
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

  // The source and the document are kept as a share of what they have between
  // them; the comment column is kept in pixels. Two units because they are two
  // different kinds of pane: half a window stays half when the window changes,
  // and a comment card wants the same readable width whatever the screen is.
  let guide = $state({ shown: false, left: 0, held: false });
  let grabbing = $state(false);
  $effect(() => { agentInView = panel === "agent" && shown.comments; });

  /* --------------------------------------------------------- the phone shell */

  // The layers the phone opens over the document, its header and its bars.
  // `reader/phone.svelte.js` owns what they add up to and what each row does;
  // the effects that tie them to the browser stay here.
  const phone = createPhone({
    arrangement,
    comparing: () => comparing,
    comparisonSays: () => comparisonSays,
    settingsOpen: () => settingsOpen,
    closeSettings: () => { settingsOpen = false; },
    endComparison: () => endComparison(),
    barHidden: () => barHidden,
    tabs: () => tabs,
    badges: () => badges,
    participants: () => participants,
    role: () => ({ mayEdit, mayChat, me }),
    title: () => doc.title,
    source: () => ({ format: sourceFormat, compilesHere, toolbarPath, editing, editorReady: Boolean(editor), figureShown: Boolean(shownFigure) }),
    demo: () => ({ on: DEMO, hint: DEMO_HINT }),
    hasDesktop: () => hasDesktop,
    exportable: () => exportable,
    canExportComments: () => canExportComments,
    can: () => can,
    editGroups: () => editGroups,
    outputs: () => ({ displayedFormat, latexOutput, typstOutput, markdownOutput }),
    view: () => ({ linked, visual, runCodeOffered, localExecution }),
    insertMenu: () => insertMenu,
    refreshEditAvailability: () => refreshEditAvailability(),
    chooseFileCommand: (id) => chooseFileCommand(id),
    chooseEditCommand: (id) => chooseEditCommand(id),
    chooseViewCommand: (id) => chooseViewCommand(id),
    toggleLocalExecution: () => toggleLocalExecution(),
    openSettings: () => openSettings(),
    runCommand: (id) => runCommand(id),
    signOut: () => signOut(),
    signInHref: () => signInHref(),
    openPanel: (name) => openPanel(name),
    setCollaborationTab: (tab) => { collaborationTab = tab; },
  });
  const overlayView = $derived(phone.overlayView);
  const overlayDepth = $derived(phone.overlayDepth);
  const mobileTitle = $derived(phone.mobileTitle);
  const modeSwitchShown = $derived(phone.modeSwitchShown);
  const keyboardBarShown = $derived(phone.keyboardBarShown);
  const phoneChrome = $derived(phone.chrome);
  const mobileCommandGroups = $derived(phone.commandGroups);
  const moreSections = $derived(phone.moreSections);
  const tabsShown = $derived(phone.tabsShown);
  const tabActive = $derived(phone.tabActive);
  const tabBarShown = $derived(phone.tabBarShown);
  let overlays = null;
  $effect(() => {
    overlays = createOverlayHistory({
      onpop: (depth) => phone.popped(depth),
    });
    return () => { overlays?.dispose(); overlays = null; };
  });
  $effect(() => { overlays?.sync(overlayDepth); });
  // A resize to desktop must not leave a hidden overlay behind it.
  $effect(() => {
    if (compact) return;
    arrangement.state.moreOpen = false;
    arrangement.state.mobileCommand = "";
    arrangement.state.keyboardOpen = false;
  });
  // The keyboard hides the mode switch and narrows the workspace. When it
  // comes up the caret is brought back into sight above it.
  $effect(() => compact
    ? watchKeyboard((open) => {
        arrangement.state.keyboardOpen = open;
        if (open) requestAnimationFrame(() => editor?.revealCaret?.());
      })
    : undefined);
  $effect(() => {
    const now = mobileCommand;
    untrack(() => phone.commandChanged(now));
  });

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
    split: { icon: "columns-2", says: m.reader_split(), next: "document" },
    source: { icon: "panel-left", says: m.common_editor(), next: "split" },
    document: { icon: "file-text", says: m.common_preview(), next: "source" },
  };

  let editAvailability = $state({});
  // Every item is named by its command in lib/commands.js, the same name the
  // palette gives it.
  const editGroups = [
    [["undo", labelOf("undo")], ["redo", labelOf("redo")]],
    [["cut", labelOf("cut")], ["copy", labelOf("copy")], ["paste", labelOf("paste")]],
    [["select-all", labelOf("select-all")]],
    [["find", labelOf("find")], ["replace", labelOf("replace")], ["word-count", labelOf("word-count")]],
    [["bold", labelOf("bold")], ["italic", labelOf("italic")]],
    [["comment", labelOf("comment")]],
  ];

  // What the keyboard already does, named beside the item the way a desktop
  // menu names it: the menu is where people go to learn the shortcut, not
  // only to avoid it. Read from lib/commands.js rather than written here, so a
  // menu cannot name a key the window has stopped listening for.
  //
  // In Vim or Emacs mode the editing keys belong to the mode, and a menu
  // naming them would be lying about what the next keystroke does; the
  // workspace's own keys are not the mode's and stay named.
  const modalEditor = $derived(keys === "vim" || keys === "emacs");
  const MENU_KEYS = $derived(menuKeys({ modalEditor, apple: APPLE }));

  function chooseEditCommand(command) {
    if (command === "word-count") return openWordCount();
    arrangement.showMobileView("source");
    const target = editor;
    // Restore source focus after the menu finishes closing.
    setTimeout(async () => {
      if (editor !== target || !mayEdit) return;
      try { await target?.editCommand(command); }
      catch (error) { say(error.message || m.readerb_clipboard_refused(), { kind: "problem", id: "reader:clipboard" }); }
    }, 0);
  }

  function cycleLayout() {
    arrangement.state.layout = ARRANGEMENTS[layout].next;
    if (compact) arrangement.showMobileView(layout === "source" ? "source" : "document");
  }

  const putSourceOn = (side) => preferences.setSourceSide(side);

  const setKeys = (next) => preferences.setKeys(next);
  const setCompletion = (on) => preferences.setCompletion(on);
  const setBrackets = (on) => preferences.setBrackets(on);
  const setLatexAssist = (on) => preferences.setLatexAssist(on);
  const setProofing = (value) => preferences.setProofing(value);
  const setDialect = (value) => preferences.setDialect(value);
  const setCheck = (on) => preferences.setCheck(on);
  // The Editor setting also sets the session value, so the change shows at once.
  const setVisualDefault = (on) => { preferences.setVisual(on); visual = on; };

  // What ":q" in Vim mode asks for: the document alone, set directly rather
  // than reached by cycling.
  const showDocumentAlone = () => { arrangement.state.layout = "document"; };

  // Everything the layout menu offers, named by what was chosen. The menu
  // reports the value of the line rather than each line calling back, so this
  // is the one place those names are read.
  // The source text size, from the View menu, the palette or the keyboard.
  // The same size the pinch and the editor keys change.
  function changeTextSize(id) {
    if (id === "text-bigger") return setTextSize(textSize() + 1);
    if (id === "text-smaller") return setTextSize(textSize() - 1);
    return setTextSize(DEFAULT_SIZE);
  }

  function chose(what) {
    if (what.startsWith("layout-")) {
      arrangement.state.layout = what.slice(7);
      if (compact) arrangement.showMobileView(layout === "source" ? "source" : "document");
      return;
    }
    if (what === "side-left" || what === "side-right") return putSourceOn(what.slice(5));
    if (what.startsWith("ratio-")) return setSize(PANES.editor, Number(what.slice(6)));
    if (what === "linked") return setLinked(!linked);
    if (what === "visual-markup") return setVisual(!visual);
    if (TEXT_SIZE_COMMANDS.includes(what)) return changeTextSize(what);
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
    if (value === "lock-preview") return lockPreview(openFile);
    if (value === "unlock-preview") return unlockPreview();
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
    if (value === "settings") return openSettings();
    if (value === "quit") return void quit();
    if (value === "compile") return compileNow();
    if (value === "download") return downloadTree();
    if (value === "download-source") return downloadSource();
    if (value === "download-comments") return downloadComments();
    if (value === "download-pdf" || value === "download-html" || value === "download-docx") return downloadAs(value.slice(9));
    if (value === "share" || value === "history" || value === "git") return openPanel(value);
    if (value === "save-template") return saveTemplateDialog?.open();
    if (!["new-file", "new-folder", "upload"].includes(value)) return;
    await openPanel("files");
    await tick();
    if (value === "upload") fileList?.choose();
    else fileList?.start(value === "new-file" ? "file" : "folder");
  }

  // Whether the desktop app builds exports with the command for this format:
  // the same facts the preview routes a job by, so the menu offers what the
  // export makes. The browser route is taken for any file it has no command for.
  const exportsLocally = $derived(localExecution && hasDesktop && mayEdit && buildPreferences.backend === "local");

  // Which exports the File menu can honour, for the file being previewed.
  const exportable = $derived({
    pdf: exportAvailability(displayedFormat, "pdf", { local: exportsLocally }),
    html: exportAvailability(displayedFormat, "html", { local: exportsLocally }),
    docx: exportAvailability(displayedFormat, "docx", { local: exportsLocally }),
  });

  const docxDownload = $derived(renderState.docxArtifact && renderState.docxArtifact.source === schedule.generation && renderState.docxArtifact.navigation === target.navigation ? renderState.docxArtifact : null);

  // The identity of the source the frame was last drawn from, so an export
  // can tell whether the page on screen is the current document.
  let renderedIdentity = "";
  let exporting = false;
  // Zag closes a sub-menu only through its own trigger, so the Download list
  // is shut by hand whenever the bar stops showing File.
  let downloadOpen = $state(false);
  $effect(() => { if (menubar.opened !== "file") downloadOpen = false; });

  // Exports are made in this browser from the live source. A page or PDF the
  // frame already holds is reused when it is the current document. LibrePaper
  // never fetches or uploads a generated result for an export.
  async function downloadAs(target) {
    if (exporting) return;
    if (target === "docx" && docxDownload) {
      const file = docxFile(docxDownload.bytes, SLUG);
      saveBlob(file.blob, file.name);
      return;
    }
    exporting = true;
    const id = "reader:export";
    say(m.readerb_preparing_export({ format: target.toUpperCase() }), { id });
    try {
      // Everything that names the document is read before the first await, so
      // a file switched to while figures load cannot pair one source with
      // another format.
      const tree = capturePreviewTree(treeNow());
      const format = displayedFormat;
      const preview = framePreview.preview();
      const local = exportsLocally;
      if (Object.keys(tree.digests || {}).length) {
        const { held, missing } = await gatherFigures(tree.digests);
        if (target !== "html" && missing.length) {
          throw new Error(m.readerc_figures_missing({ count: missing.length, missing: missing.join(", ") }));
        }
        tree.assets = held.assets;
        tree.urls = { ...(tree.urls || {}), ...held.urls };
      }
      const file = await exportFile({
        target,
        format,
        tree,
        title: await headingOf(tree),
        slug: SLUG,
        project: SLUG,
        preview,
        previewCurrent: Boolean(renderedIdentity) && renderedIdentity === await renderers.snapshotDigest(tree),
        render: renderers.render,
        run: pandoc,
        local,
      });
      saveBlob(file.blob, file.name);
      if (!local && unrunCells(format, tree) > 0) {
        say(m.readerc_cells_not_run(), { id });
      } else {
        toaster.dismiss(id);
      }
    } catch (error) {
      say(error.message || m.readerc_export_failed({ target: target.toUpperCase() }), { kind: "problem", id });
    } finally {
      exporting = false;
    }
  }

  // Every thread of the project as Markdown. The server pages comments, so
  // this reads the same pages the sidebar does, to the end, and then adds what
  // only this browser knows: whether a passage is still in the document and
  // where it sits.
  async function downloadComments() {
    if (exporting) return;
    exporting = true;
    const id = "reader:export";
    say(m.readerc_comments_preparing(), { id });
    try {
      const read = async (path) => {
        const response = await fetch(`/api/documents/${SLUG}/comments${path}`, { headers: authHeaders(KEY) });
        const body = await response.json().catch(() => ({}));
        if (!response.ok) throw new Error(body.error || m.common_comments_could_not_be_read({ status: response.status }));
        return body;
      };
      const loaded = await loadAllComments({
        fetchPage: (cursor) => read(cursor ? `?cursor=${encodeURIComponent(cursor)}` : ""),
        fetchReplies: (comment, cursor) => read(`/${comment.id}/replies?cursor=${encodeURIComponent(cursor)}`),
      });
      // Threads on pages the sidebar has not loaded are placed the way the
      // sidebar places its own, on copies so the sidebar's list is untouched.
      const held = new Map(comments.map((item) => [String(item.id), item]));
      const unheld = loaded.filter((item) => !held.has(String(item.id))).map((item) => ({ ...item }));
      // Without the page text there is nothing to place them against, as for
      // the sidebar: they stay unplaced rather than read as detached.
      if (docText !== null) {
        if (mayEdit && session) {
          const bodies = new Map();
          for (const item of unheld) item.attachment = deriveAttachment(session.doc, session.anchors, item, bodies);
        }
        anchorComments(unheld);
      }
      for (const item of unheld) held.set(String(item.id), item);
      const threads = loaded.map((item) => {
        const seen = held.get(String(item.id));
        return {
          id: item.id,
          creator: item.creator,
          created: item.created,
          body: item.body,
          resolved: Boolean(item.resolved),
          replies: item.replies,
          whole: aboutWholeDocument(item),
          quote: shownSelector(item).exact,
          orphaned: Boolean(seen?.orphaned),
          position: seen?.start ?? null,
        };
      });
      const markdown = commentsMarkdown({
        title: await headingOf(capturePreviewTree(treeNow())),
        url: `${location.origin}/docs/${SLUG}`,
        exportedAt: new Date(),
        comments: threads,
      });
      saveBlob(new Blob([markdown], { type: "text/markdown" }), `${SLUG}-comments.md`);
      toaster.dismiss(id);
    } catch (error) {
      say(error.message || m.readerc_comments_export_failed(), { kind: "problem", id });
    } finally {
      exporting = false;
    }
  }

  // Open a panel from the menu: unlike the activity bar, choosing an item
  // that is already open leaves it open rather than closing the column.
  function openPanel(name) {
    if (DEMO && DEMO_OFF.includes(name)) return Promise.resolve();
    if (!visitedPanels.includes(name)) visitedPanels = [...visitedPanels, name];
    // Only one thing is ever open over the document on a phone.
    arrangement.state.moreOpen = false;
    arrangement.state.mobileCommand = "";
    const opening = arrangement.showPanel(name);
    if (compact) arrangement.showMobileView("sidebar");
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
    retarget: () => target.update(),
    onarrived: (file) => openTheFile(file),
  });
  const ws = workspace.state;
  const files = $derived(ws.files);
  // Where the editor was before each jump, for Back. Another project is another
  // page, so the places last as long as this one. A session that is replaced
  // on a reconnect keeps its file ids, and a place whose file is gone is skipped.
  const jumps = createJumps();
  const fileExists = (id) => Boolean(session?.paths?.get(id));
  // Whether there is a text to edit yet. The directory arrives after the
  // session joins, and the editor has nothing to bind to before then.
  const sourceBound = $derived(Boolean(session && persistence.joined && files.some((file) => file.kind === "text")));
  const folders = $derived(ws.folders);
  const openFile = $derived(ws.openFile);
  const rules = $derived(ws.rules);
  const shownFigure = $derived(ws.figure);

  // The Authors view needs the history panel open and a source editor on
  // screen to colour. Without one, the tab says there is no file to show.
  const authorsShown = $derived(sourceOpen && !comparing && !shownFigure && sourceBound && Boolean(openFile));
  const authorsActive = $derived(authorsView && panel === "history" && authorsShown);

  // Turning the view on fetches the map the first time. Turning it off drops
  // any isolated author, so the next time the tab is on, everyone is shown.
  // Read untracked: the fetch sets state the effect would otherwise follow,
  // and a failed fetch would then be retried in a loop.
  $effect(() => {
    const active = authorsActive;
    untrack(() => {
      if (!active) {
        authorsOnly = null;
        return;
      }
      if (!authorsMap && !authorsLoading) void loadAuthorsMap();
    });
  });

  // When the editor finds text from a peer the map does not name, the map is
  // fetched again, once the last fetch is ten seconds old. The timer belongs
  // to the summary it was started for, so a newer summary replaces it.
  $effect(() => {
    if (!authorsActive || !authorsSummary?.missing) return;
    const wait = Math.max(0, authorsFetchedAt + AUTHORS_REFETCH_MS - Date.now());
    const timer = setTimeout(() => untrack(() => void loadAuthorsMap()), wait);
    return () => clearTimeout(timer);
  });
  const figureUrl = $derived(ws.figureUrl);
  // Test whether a file path can be displayed as an image in the figure view.
  const isDisplayableImage = (path) => /\.(?:png|jpg|jpeg|gif|svg|webp)$/i.test(path);
  let outlineActiveFrom = $state(null);
  let outlineRevision = $state(0);
  // The outline can be shown with no editor open, so it also follows the typst
  // module's arrival itself.
  $effect(() => onTypstSyntax(() => { outlineRevision += 1; }));
  $effect(() => onOrgSyntax(() => { outlineRevision += 1; }));
  // The path of the file the preview is locked on. The lock itself is held by
  // the target, by identity, so a rename keeps it.
  const lockedPath = $derived(files.find((file) => file.id === lockedFile)?.path || "");
  const toolbarPath = $derived(files.find((file) => file.id === openFile)?.path || "");
  const openTextPath = $derived(files.find((file) => file.id === openFile && file.kind === "text")?.path || "");
  const previewNoteItems = $derived(previewNotes({
    openPath: toolbarPath,
    openIsText: files.find((file) => file.id === openFile)?.kind === "text",
    target: { path: previewPath, reason: previewReason },
  }));
  const editorFormat = $derived(toolbarPath ? renderers.formatOf(toolbarPath) : sourceFormat);
  // The visual layer draws only the formats in VISUAL_FORMATS, so the toggle is offered for those alone.
  // The visual layer reads Quarto as Markdown.
  const visualAvailable = $derived(VISUAL_FORMATS.has(editorFormat === "quarto" ? "markdown" : editorFormat));
  // The formatting row under the top bar: only while the source is open and editable in visual mode.
  const formatBarShown = $derived(visual && sourceOpen && mayEdit && visualAvailable);
  // At compact the bar is drawn only while the keyboard is open, so its height is taken off the workspace only then.
  const formatBarVisible = $derived(formatBarShown && (!compact || keyboardOpen));
  // Why the pane shows what it shows, when it is not simply the open file.
  const previewChip = $derived(Boolean(previewPath) && (previewReason === "locked" || (previewReason === "inside" && toolbarPath)));
  // What the card says when the open file has nothing the preview could draw.
  // Empty when there is no card.
  const previewCard = $derived.by(() => {
    if (previewReason !== "none" || projectEmpty || !openTextPath) return "";
    const name = basename(openTextPath);
    const format = renderers.formatOf(openTextPath);
    if (!format) return m.common_no_own_preview({ name });
    return format === "latex"
      ? m.readerc_preview_part_latex({ name, cmd: "\\documentclass" })
      : m.readerc_preview_part({ name });
  });
  const canLockPreview = $derived(Boolean(openTextPath && aloneSet.has(openTextPath) && lockedFile !== openFile));
  // The server checks every path again, but a project opens only with
  // the rules, so a failed load is tried again until it lands. The rules of the
  // last visit stand in until then, and are applied once rather than on every
  // try.
  let configRetry = null;
  let rulesRemembered = false;
  function loadRules() {
    configOrRemembered()
      .then(({ config: answer, remembered }) => {
        if (remembered) configRetry = setTimeout(loadRules, 30_000);
        if (remembered && rulesRemembered) return;
        rulesRemembered = remembered;
        workspace.setRules(answer || {});
        if (typeof answer?.latexMirror === "string") latex.at(answer.latexMirror);
        if (sourceFormat === "latex") void renderers.warm("latex");
      })
      .catch(() => {
        configRetry = setTimeout(loadRules, 30_000);
      });
  }
  loadRules();
  $effect(() => () => clearTimeout(configRetry));

  // Reading the directory into the list is the workspace's; what is left
  // here is the preview target, which is a rendering question rather than a
  // directory one.
  const refreshFiles = () => workspace.refresh();

  // A project with no file at all, as opposed to one whose directory has not
  // arrived yet. The directory is read from the session itself so that a
  // stale file list cannot report a project as empty.
  const projectEmpty = $derived(mayEdit
    ? Boolean(session && persistence.joined && files.length === 0 && session.list().length === 0)
    : Boolean(projection.state.project && !Object.keys(projection.state.project.tree?.files || projection.state.project.texts || {}).length));
  $effect(() => { if (projectEmpty) untrack(() => target.clear()); });

  // The preview follows the open file, and follows the lock when there is one.
  $effect(() => {
    void openFile;
    void lockedFile;
    untrack(() => target.update());
  });

  // The eye in Files, the View menu and the card all lock the preview, and
  // none of them opens the file. A phone shows the preview it just moved.
  function lockPreviewOn(id) {
    viewing = false;
    target.lock(id);
    if (!compact && layout === "source") arrangement.state.layout = "split";
    if (compact) arrangement.showMobileView("document");
  }

  function lockPreview(id) {
    const file = files.find((entry) => entry.id === id);
    if (!file || file.kind !== "text" || !aloneSet.has(file.path)) return;
    lockPreviewOn(id);
  }

  function unlockPreview() {
    target.unlock();
  }

  // A file added, renamed or removed: the list is redrawn and the document is
  // rendered again, because every one of those changes what a compiler would
  // produce. An edit to the text of any file is a source change too, because
  // it can alter the render of whichever file the preview is drawing.
  function filesChanged(events, active = session) {
    if (!active || active !== session) return;
    // Nested text edits change the preview, but not the file list. Which
    // events are the directory's is the session's to say: the list moves with
    // four maps, not one.
    if (active.directoryChanged(events)) refreshFiles();
    if (needsSourceRefresh(events)) {
      sourceChanged();
      schedule.scheduleTarget();
    }
  }

  function refreshPeers() {
    workspace.refreshPeers();
    participants = ws.participants;
  }

  // Whether the document pane is showing the open file in place of the
  // preview. Only somebody with no source pane ever does: for them a file
  // that cannot be previewed has nowhere else to be shown.
  let viewing = $state(false);
  const heldFile = $derived(!mayEdit && viewing ? files.find((file) => file.id === openFile) || null : null);

  // A failure is reported once the render has settled as failed. One that a
  // retry is already superseding is not.
  const failedBeforeRender = $derived(renderState.failure && !painted.state.shown && !compiling);
  // The one waiting display. It stands in for the pane until the page for the
  // current target is drawn, whatever the format. A stand-in page from the
  // cache is drawn like any other, so a recompile under one never lands here.
  const pageIsUp = $derived(pdfOutput ? painted.state.drawn : painted.state.shown);
  const waiting = $derived(Boolean(
    shown.document && previewPath && compilesHere && !pageIsUp
      && !documentNeedsSignIn && !heldFile && !projection.state.unreadable && !projectEmpty && !previewCard
      && !failedBeforeRender && !painted.state.empty,
  ));
  const waitStage = $derived(waitStageOf({
    pageDelivered: pdfOutput && painted.state.shown,
    latexPhase: sourceFormat === "latex" && !latexHtmlPreview ? latexPhase : "",
    latexScope: sourceFormat === "latex" && !latexHtmlPreview ? String(latexState.progress?.scope || "") : "",
    engineLoading: engineLoading && engineLoading === sourceFormat ? engineLoading : "",
    compiling,
  }));
  const waitProgress = $derived(sourceFormat === "latex" && !latexHtmlPreview && latexState.progress?.total ? latexState.progress : null);
  // The block is held to reading speed as a whole: it does not appear for a
  // quick render and does not blink off between stages once it is up.
  let waitingShown = $state(false);
  const steadyWaiting = createSteadyBusy({ onchange: (shown) => (waitingShown = shown) });
  // A delivered PDF in a frame that already shows pages is swapped in by the
  // viewer once it is drawn, so there is nothing to wait for on screen.
  const waitingNeeded = $derived(waiting && !(frameHasPage && pdfOutput && painted.state.shown));
  $effect(() => {
    steadyWaiting.set(waitingNeeded);
  });
  $effect(() => () => steadyWaiting.stop());
  // What keeps the frame out of sight: the block is up, or the frame has
  // nothing on it yet, which would show the viewer's own empty page.
  const waitingUp = $derived(waitingShown || (waitingNeeded && !frameHasPage));

  // Somebody who cannot edit has no source pane. Choosing a file opens it as
  // it does for an editor, and the preview follows it there or into the file
  // that includes it. A figure, or a text with no preview anywhere, is shown
  // read only in place of the preview until the reader goes back to it.
  function viewTheFile(file) {
    if (openFile !== file.id) outlineActiveFrom = null;
    workspace.show(file);
    const texts = projection.state.project?.texts || {};
    viewing = file.kind !== "text" || previewTarget({ open: file.path, locked: lockedPath, texts }).reason === "none";
    if (compact) arrangement.showMobileView("document");
  }

  function backToPreview() {
    viewing = false;
    ws.figure = null;
    const previewed = files.find((file) => file.kind === "text" && file.path === previewPath);
    if (previewed) ws.openFile = previewed.id;
  }

  // On a phone, a tap in Files shows the preview when the preview would show
  // that file. A figure, a file with no preview of its own, or a preview
  // locked on another file opens the source instead.
  function previewsOnTap(file) {
    if (file.kind !== "text") return false;
    const target = previewTarget({ open: file.path, locked: lockedPath, texts: liveTreeNow().texts });
    return Boolean(target.path) && (!lockedPath || lockedPath === file.path);
  }

  function openTheFile(file, { fromList = false } = {}) {
    if (!mayEdit) {
      viewTheFile(file);
      return;
    }
    if (openFile !== file.id) outlineActiveFrom = null;
    workspace.show(file);
    // Choosing a file is asking to see it, so an arrangement with no source
    // pane makes room for one.
    if (mayEdit && !compact && layout === "document") arrangement.state.layout = "split";
    if (mayEdit) arrangement.showMobileView(fromList && compact && previewsOnTap(file) ? "document" : "source");
  }

  // An outline entry is a source navigation command. It may be the first
  // source command in a document-only or compact view, so mount the lazy
  // editor before asking it to move the caret.
  async function openOutlineHeading(heading) {
    if (!mayEdit || !session || !heading) return;
    const id = openFile;
    const file = files.find((entry) => entry.id === id && entry.kind === "text");
    if (!file) return;
    rememberPlace(id, heading.from);
    outlineActiveFrom = heading.from;
    const revision = outlineRevision;
    openTheFile(file);
    if (!editing) await startEditing();
    if (readerDisposed || id !== openFile) return;
    arrangement.showMobileView("source");
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
  const addFigure = (file, path) => {
    if (DEMO) return Promise.reject(new Error(m.readerc_demo_no_uploads()));
    return workspace.addFigure(file, path);
  };

  function insertContext() {
    if (!mayEdit) return null;
    const context = editor?.getInsertContext?.();
    if (!context) return null;
    return { ...context, files: [...context.files, ...workspace.assets()] };
  }
  function applyInsertion(result, context, options) {
    if (!mayEdit || !editor?.applyInsertResult?.(result, context, options)) {
      throw new Error(m.readerc_insertion_changed());
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
    if (projectEmpty) {
      say(m.readerc_download_no_files(), { kind: "problem", id: "reader:download-project" });
      return;
    }
    try {
      // The tree and the directory are read here, before anything is awaited,
      // and handed over as values: `projectFiles` cannot then be cut from a
      // tree newer than the one whose assets it fetched.
      const files = await projectFiles({ tree: liveTreeNow(), folders: [...folders], gather: gatherFigures });
      // Named for the document rather than for any one file: what is being
      // downloaded is the directory, and the slug is what a person knows it by.
      saveBlob(await archive(files), `${SLUG}.zip`);
    } catch (error) {
      say(error.message || m.readerc_download_project_failed(), { kind: "problem", id: "reader:download-project" });
    }
  }

  const addDroppedText = (file, path) => workspace.addDroppedText(file, path);

  async function downloadEntry(entry) {
    try {
      const chosen = await entryDownload({ entry, tree: liveTreeNow(), folders: [...folders], gather: gatherFigures });
      saveBlob(chosen.files ? await archive(chosen.files) : new Blob([chosen.bytes]), chosen.name);
    } catch (error) { say(error.message || m.readerc_download_entry_failed(), { kind: "problem", id: "reader:download-entry" }); }
  }

  // The file open in the editor, as written, not the project archive.
  function downloadSource() {
    if (!toolbarPath) return;
    return downloadEntry({ kind: "file", path: toolbarPath });
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
      arrangement.showPanel("files");
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

  // A document is shown once the server has answered for it.
  function showDocument(found) {
    doc = found;
    document.title = `${found.title} · LibrePaper`;
    // The server refuses a documents host equal to the app host, so the
    // page only has to refuse to invent one: falling back to the app's own
    // origin would hand the sandboxed frame that origin.
    const separate = Boolean(found.docs_origin);
    docsOrigin = separate ? found.docs_origin : null;
    // The frame is an empty page with the agent in it, on the documents
    // origin. What goes into it is what this browser renders -- or, for a
    // LaTeX document, a PDF an editor's browser compiled, which needs a
    // frame that can draw one. Set after `prepare`, which is what settles
    // the format and so which frame this document wants.
    void lifecycle.prepare(found);
    if (!separate) {
      renderStatus.failed(NO_DOCS_ORIGIN);
      say(NO_DOCS_ORIGIN, { kind: "problem", id: "reader:no-docs-origin" });
    }
  }

  $effect(() => {
    if (!DEMO) markViewed(SLUG);
    pendingChat = createPendingChat({
      send: (message) => collaboration?.sendLive(message) || { ok: false },
    });
    const boot = createReaderBoot({
      slug: SLUG,
      key: KEY,
      documentUrl: DEMO ? "/api/try" : undefined,
      onIdentity: (who) => {
        me = who;
        if (who.name || who.id) session?.rename(who.name || "", who.provider ? who.id || "" : "");
      },
      onDocument: showDocument,
      // A document answers a stranger exactly as a missing one does, which
      // tells a stranger nothing -- and tells an owner who has not signed in
      // nothing either. That is what this line is for: the page was opened
      // at a real URL, so the honest thing to say is both. A stopped server or
      // a failing one is not a missing document.
      onError: (error) => {
        if (error?.status === 401) {
          documentNeedsSignIn = true;
          doc = { title: m.readerc_title_sign_in() };
        } else if (error?.status === 404) {
          doc = { title: m.readerc_title_not_found() };
          say(
            me.providers?.length && !identity
              ? m.readerc_doc_missing_sign_in()
            : m.readerc_doc_missing(),
            { kind: "problem", id: "reader:not-found" },
          );
        } else if (error?.status) {
          doc = { title: m.readerc_title_unavailable() };
          say(
            m.readerc_server_open_failed({ status: error.status }),
            { kind: "problem", id: "reader:not-found" },
          );
        } else {
          doc = { title: m.readerc_title_unavailable() };
          say(
            m.readerc_server_unreachable(),
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
      void flushAll();
      renderCoordinator.invalidate();
      target.bump();
      renderers.cancelPreview();
      clearTimeout(reviewTimer);
      schedule.dispose();
      projection.dispose();
      boot.dispose();
      framePreview.dispose();
      stopLatex();
      pendingChat?.dispose();
      changesUnsubscribe?.();
      changesUnsubscribe = null;
      followAnnotations?.();
      followAnnotations = null;
      lifecycle.close();
    };
  });

  // Account scope and the desktop app are the only triggers for reading
  // backup status. A signed-out page or a browser never makes a request.
  $effect(() => backupStatusStore.setScope(DEMO ? undefined : me?.id, hasDesktop));
  $effect(() => () => backupStatusStore.reset());

  // Every project an editor opens is kept in this browser, so it can be opened
  // again without a connection. A new project reports local before anything is
  // stored, so a failed attempt is retried on the next report instead of being
  // given up for the rest of the page load. The inputs are read before any
  // early return, so the effect stays subscribed to them. It waits for the
  // editor module too: on a first visit that import may not have gone through
  // the service worker yet, and the shell cached without it cannot edit.
  let localCopyRunning = false;
  let localCopyRemembered = false;
  $effect(() => {
    const ready = !DEMO && mayEdit && !openedLocally && persistence.local && Editor;
    if (!ready || localCopyRunning || localCopyRemembered) return;
    localCopyRunning = true;
    const server = location.origin;
    const identity = { server, documentId: doc.document_id, slug: SLUG, createdAt: doc.created_at };
    (async () => {
      const record = await rememberProject({ ...doc, slug: SLUG, server });
      // The page shell is cached with the local copy, so the landing page can
      // list local projects when the server cannot be reached. Until it is
      // cached the record has no shellReady, and the next attempt tries again.
      // A record cached by an older build has no shellReady for this one, so
      // it is cached again and picks up chunks that have since become lazy.
      if (!record.shellReady) {
        // The offline copy only stores code already loaded, so load the lazy panels first.
        if (await prefetchLazyPanels() && await cacheCurrentShell()) await markShellReady(identity);
      }
      localCopyRemembered = true;
      localCopyRunning = false;
    })().catch(() => {
      localCopyRunning = false;
    });
  });

  // Ctrl-S reports the durability the server has confirmed for local work.
  // Typing is saved automatically, but a buffered edit is not durable yet.
  // What it says, and why it avoids the word "saved", is the opening's.
  function reportPersistence() {
    const message = lifecycle.report();
    if (message) say(message, { id: "reader:persistence" });
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
  /* ------------------------------------------------------------- the keyboard */

  let commandPalette = $state(false);

  // What the workspace can currently offer, as facts rather than rules. The
  // rules are in lib/commands.js, which decides from these whether a command
  // can be asked for; this is the only place that knows both.
  // Anyone offered the collaboration panel can see the comments, so the same
  // test decides who is offered their export.
  const canExportComments = $derived(tabs.some((tab) => tab.id === "collaboration"));

  const commandContext = $derived({
    mayEdit, editing, compact,
    panels: tabs.filter((tab) => !tab.disabled).map((tab) => tab.id),
    preview: shown.document,
    compilable: mayEdit && sourceFormat === "latex" && compilesHere,
    canLockPreview,
    previewLocked: Boolean(lockedPath),
    downloads: {
      pdf: exportable.pdf.ok,
      html: exportable.html.ok,
      docx: exportable.docx.ok,
      comments: canExportComments,
      source: Boolean(toolbarPath),
    },
    edit: editAvailability,
    canClearChat: chatState.canClear,
    canComment: mayChat,
    // In Vim or Emacs mode Alt is the editor's, so the Alt menu chords stand down in the source.
    modalEditor: keys === "vim" || keys === "emacs",
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
    if (id === "report-problem") return openSettings("general", "diagnostics-report");
    if (id === "word-count") return openWordCount();
    if (EDIT_COMMANDS.includes(id)) return chooseEditCommand(id);
    if (id === "palette") { refreshEditAvailability(); try { await lazy.commandPalette.load(); } catch { lazyFailed(); return; } commandPalette = true; return; }
    if (id.startsWith("menu-")) return openMenu(id.slice("menu-".length));
    if (id === "shortcuts") return openSettings("editor");
    if (id === "focus-files") return focusPanel("files");
    if (id === "focus-outline") return focusPanel("outline");
    if (id === "symbols" || id === "math") return openPalette(id);
    // A template command opens the Math overlay on its template.
    if (id.startsWith("math-")) return openPalette("math", id.slice("math-".length));
    if (id === "focus-comments") return focusPanel("collaboration");
    if (id === "comment-document") return commentOnDocument();
    if (id === "chat-clear") return askClearChat();
    if (id === "focus-preview") return focusPreview();
    if (id === "layout-cycle") return cycleLayout();
    if (LAYOUT_COMMANDS.includes(id)) return chose(id);
    if (id === "visual-markup") return setVisual(!visual);
    if (id === "lock-preview" || id === "unlock-preview") return chooseViewCommand(id);
    if (TEXT_SIZE_COMMANDS.includes(id)) return changeTextSize(id);
    return chooseFileCommand(id);
  }

  // Alt+F and the rest open a word of the bar, with the keyboard on its first
  // line. A word that is not on the bar now (Edit with no source, say) stays shut.
  function openMenu(id) {
    if (!document.querySelector(`.menubar [data-menubar="${id}"]:not(:disabled)`)) return;
    menubar.show(id);
    menubar.enter();
  }

  const EDIT_COMMANDS = [
    "undo", "redo", "cut", "copy", "paste", "select-all", "find", "replace", "bold", "italic",
    "link", "comment", "numbered-list", "bulleted-list", "heading-1", "heading-2", "heading-3", "heading-4", "heading-5", "heading-6",
  ];
  const LAYOUT_COMMANDS = ["linked", "layout-source", "layout-document", "layout-split", "side-left", "side-right"];
  const TEXT_SIZE_COMMANDS = ["text-bigger", "text-smaller", "text-reset"];

  // The count reads the source, so the dialog is offered only where the source is open.
  function openWordCount() {
    if (mayEdit && editing) wordCountOpen = true;
  }

  // The palettes open over the editor. Each one inserts at the caret the
  // editor had when it opened, so there is nothing to open without a source.
  async function openPalette(name, template = "") {
    if (!can[name]) return;
    try { await lazy[name].load(); } catch { lazyFailed(); return; }
    if (!can[name]) return;
    symbolsOpen = name === "symbols";
    mathOpen = name === "math";
    await tick();
    if (template) mathPalette?.openTemplate?.(template);
  }

  // A palette that has just inserted closes and hands the keyboard to the
  // editor, after the dialog has returned focus to its opener, so typing goes
  // on where the insertion landed. Escape leaves focus with the opener. On a
  // touch device the editor is not refocused, since that raises the virtual
  // keyboard.
  function closePalette() {
    symbolsOpen = false;
    mathOpen = false;
    if (!matchMedia("(pointer: coarse)").matches) requestAnimationFrame(() => editor?.focus?.());
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
    if (compact) arrangement.showMobileView("document");
    preview?.focus();
  }

  function shortcut(event) {
    // One Escape, one thing: the swatches, then the bar. Nothing here
    // touches a dialog -- those close themselves -- and nothing happens when
    // neither is up, so Escape stays the browser's.
    if (event.key === "Escape") {
      if (palette) { palette = false; return; }
      if (bar.shown) { bar = { ...bar, shown: false }; return; }
      // A comparison closes on Escape, unless the focus is in a field, or the
      // editable side of a merge, where Escape is somebody else's.
      const typing = event.target instanceof Element && event.target.closest("input, textarea, select, [contenteditable='true']");
      // An Escape a dialog or a menu already took is theirs, not the comparison's.
      if (comparing && !typing && !event.defaultPrevented && !captured(event.target)) endComparison();
      return;
    }
    // Back is not in the registry: its chord is Ctrl+Minus on a Mac, which the
    // registry does not read. It is refused in a dialog, and in a field that
    // takes text; the source is a field too, and Back is for the source.
    if (isBackChord(event, APPLE)) {
      // Without the source on screen the chord is the browser's.
      if (!editing || !shown.source) return;
      if (captured(event.target)) return;
      if (writing(event.target) && !event.target.closest(".cm-editor")) return;
      event.preventDefault();
      goBack();
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

<svelte:window bind:innerWidth={arrangement.state.width} onresize={() => (arrangement.state.unit = measure())} onkeydown={shortcut} onbeforeunload={beforeUnload} onpagehide={() => session?.leave()} />

{#snippet faceSwitch()}
  <div class="face-switch" role="group" aria-label={m.readerd_workspace_view()}>
    <IconButton icon="eye" label={m.common_preview()} pressed={shown.document}
                onclick={() => arrangement.showMobileView("document")} />
    {#if sourceOpen}
      <IconButton icon="writing-hand" label={m.common_editor()} pressed={shown.source}
                  onclick={() => arrangement.showMobileView("source")} />
    {/if}
  </div>
{/snippet}
{#snippet visualModeControl()}
  {#if sourceOpen && mayEdit && visualAvailable}
    <label class="bar-toggle" title={m.readerd_visual_mode_hint()}>
      <button type="button" role="switch" class="switch" aria-checked={visual} aria-label={m.common_visual_mode()} data-state={visual ? "checked" : "unchecked"} onclick={() => setVisual(!visual)}>
        <span class="switch-thumb" data-state={visual ? "checked" : "unchecked"}></span>
      </button>
      <span class="bar-toggle-label">{m.common_visual_mode()}</span>
    </label>
  {/if}
{/snippet}
{#snippet localExecutionControl()}
  {#if runCodeOffered}
    <label class="bar-toggle" title={m.readerd_run_code_hint()}>
      <button type="button" role="switch" class="switch" aria-checked={localExecution} aria-label={m.readerd_run_code()} data-state={localExecution ? "checked" : "unchecked"} onclick={toggleLocalExecution}>
        <span class="switch-thumb" data-state={localExecution ? "checked" : "unchecked"}></span>
      </button>
      <span class="bar-toggle-label">{m.readerd_run_code()}</span>
    </label>
  {/if}
{/snippet}

{#if DEMO}
  <div class="demo-banner" role="status" bind:clientHeight={demoBannerHeight}>
    <span>{m.readerd_demo_banner()}</span>
    <span class="demo-banner-actions">
      <a class="btn btn-sm lp-control-brand" href="/signin">{m.common_sign_in()}</a>
      <button type="button" class="btn btn-sm lp-control-neutral" onclick={() => location.reload()}>{m.readerd_start_over()}</button>
    </span>
  </div>
{/if}

<!-- The top of the page, above the top bar. Renders nothing unless the
     desktop app and this page disagree. -->
<DesktopMismatch />

<Nav {me} phoneMenu phoneShell={compact} phoneBare={tabsShown} hidden={barHidden && !compact}>
  {#snippet tools()}
    {#if compact}
      <!-- The phone has no tools in its header: More is in the bottom bar. -->
    {:else}
      <!-- Between the phone and the split's narrow point, the face choice stays
           in the top bar. -->
      {#if adapted && sourceOpen}
        {@render faceSwitch()}
      {/if}
      <!-- The build's state is told by the preview pane's progress line and
           by Diagnostics, so the bar only carries the local execution switch. -->
      {@render visualModeControl()}
      {@render localExecutionControl()}
      <!-- Shown only while tracked changes are being recorded, so a reviewer
           who has closed the Changes panel can still see that edits are tracked. -->
      {#if tracking}
        <button type="button" class="tracking-pill" title={m.reader_tracking_open()} onclick={() => void focusPanel("changes")}>
          <Icon name="pencil" size={14} />
          <span>{m.reader_tracking_on()}</span>
        </button>
      {/if}
      <div class="presence" role="group" aria-label={connected ? m.readerd_people_connected({ count: peers }) : connectionNote} title={connected ? m.readerd_people_connected({ count: peers }) : connectionNote}>
        {#each participants.slice(0, 3) as person (person.key)}
          <button type="button" class="presence-jump" title={person.name} aria-label={m.common_go_to_cursor({ name: person.name })} onclick={() => jumpToPerson(person)}>
            <Avatar name={person.name} key={person.key} colour={person.colour} size={6} />
          </button>
        {/each}
        {#if participants.length > 3}<span class="presence-more">+{participants.length - 3}</span>{/if}
        {#if !connected && !DEMO}<span class="presence-offline"><span class="presence-offline-dot" aria-hidden="true"></span>{m.common_offline()}</span>{/if}
      </div>
    {/if}
  {/snippet}
  {#snippet menus()}
    <Menus role={{ mayEdit, me, canSeeSharing, canPublish, canExportComments }} demo={DEMO} demoHint={DEMO_HINT} {tabs} {hasDesktop}
      source={{ format: sourceFormat, compilesHere, toolbarPath, editing, editorReady: Boolean(editor), figureShown: Boolean(shownFigure) }}
      {exportable}
      view={{ compact, activeMobileView, layout, sourceSide, editorShare: sizes[PANES.editor.key], linked, visual }}
      outputs={{ displayedFormat, latexOutput, typstOutput, markdownOutput }}
      {can} keys={MENU_KEYS} {editGroups} bind:downloadOpen
      onfile={(command) => void chooseFileCommand(command)}
      onedit={chooseEditCommand}
      oneditopen={refreshEditAvailability}
      onview={chooseViewCommand}
      onhelp={(id) => void runCommand(id)}>
      {#snippet insert()}
        <InsertMenu bind:this={insertMenu} getContext={insertContext} oninsert={applyInsertion} onupload={uploadInsertAsset} onpreview={previewInsertAsset} onpalette={(id) => void openPalette(id)} keys={{ symbols: MENU_KEYS.symbols, math: MENU_KEYS.math }} oncancel={(context) => editor?.releaseInsertContext?.(context)} onfocus={() => editor?.focus?.()} onzoterosearch={localQuarto.desktop() ? (query) => editor?.zoteroSearch?.(query) : null} onzoteroimport={(items) => editor?.importZoteroItems?.(items)} disabled={!mayEdit || !editor} demo={DEMO} />
      {/snippet}
    </Menus>
  {/snippet}
  {#snippet children()}
    {#if compact}
      <!-- The header only shows over a view that is not a tab (a panel, a
           command list, a comparison): Back closes it, and the title names it. -->
      <button type="button" class="mobile-header-button" aria-label={m.common_back()} onclick={() => phone.back()}>
        <Icon name="arrow-left" size={20} />
      </button>
    {/if}
    <span id="docTitle" class="nav-document truncate" class:mobile-title={compact} class:mobile-title-open={overlayView}
          title={toolbarPath || doc.title || ""}>
      {compact ? mobileTitle : toolbarPath ? basename(toolbarPath) : doc.title || "LibrePaper"}
    </span>
  {/snippet}
</Nav>

<!-- The view the phone is on, said when it changes. -->
{#if compact}
  <div class="sr-only" role="status" aria-live="polite">{mobileTitle}</div>
{/if}

{#if formatBarVisible}
  <FormatToolbar active={formatActive} hidden={barHidden && !compact} {modalEditor} available={can} onformat={(id) => editor?.formatSelection?.(id)} oninsert={(id) => insertMenu?.insertAction?.(id)} oncommand={(id) => void runCommand(id)} />
{/if}

{#if compact && tabsShown}
  <!-- No header on a tab screen: this keeps the top safe area painted. -->
  <div class="phone-safe-top" aria-hidden="true"></div>
{/if}

{#if compact && modeSwitchShown}
  <ModeSwitch mode={shown.source ? "source" : "document"} collapsed={barHidden} onchange={(view) => arrangement.showMobileView(view)} />
{/if}

{#if shareIntro}
  <div class="share-intro" role="status" bind:clientHeight={shareIntroHeight}>
    <span class="share-intro-text">{shareIntro === "edit" ? m.readerd_share_intro_edit() : shareIntro === "comment" ? m.readerd_share_intro_comment() : m.readerd_share_intro_view_ask()}</span>
    <LegalNotice legal={shareIntroLegal} />
    <button type="button" class="btn btn-sm lp-control-neutral" onclick={() => (shareIntroDismissed = true)}>{m.toasts_dismiss()}</button>
  </div>
{/if}

<main id="main" tabindex="-1" class="reader" class:editing={shown.source} class:no-preview={!shown.document}
      class:no-comments={!shown.comments} class:source-right={sourceSide === "right"}
      class:mobile-document={activeMobileView === "document"} class:mobile-source={activeMobileView === "source"}
      class:mobile-sidebar={activeMobileView === "sidebar"} class:adapted={compact} class:phone-tabs={tabsShown} class:bar-hidden={barHidden && !compact}
      style="height: calc(100dvh - var(--librepaper-bar-visible, var(--librepaper-bar)) - var(--librepaper-format-used, 0px) - {demoBannerHeight + shareIntroOffset}px - var(--librepaper-phone-chrome, 0px));{compact ? ` --librepaper-phone-chrome: calc(${phoneChrome});` : ''} --librepaper-format-used: {formatBarVisible ? 'var(--librepaper-format-bar)' : '0px'}; --librepaper-activity: {px(ACTIVITY_WIDTH, unit)}px; --librepaper-editor: {pixels(PANES.editor, panes)}px; --librepaper-sidebar: {pixels(PANES.sidebar, panes)}px">
  <!-- What this page is. The title is in the bar, where it is a span beside
       the menus rather than a heading, so the one heading a screen reader
       looks for first is here instead: said once, seen by nobody. -->
  <h1 class="sr-only">{doc.title || m.readerd_untitled_document()}</h1>
  <!-- Each panel, wired where its state lives. The column renders whichever
       of these its rail has selected; none of it travels through the column
       as props, so a panel can grow a field without the column hearing of it. -->
  {#snippet filesPanel()}
    <Files bind:this={fileList} {files} {folders} open={openFile} preview={previewPath} locked={lockedPath}
      {mayEdit} {rules} onopen={(file) => openTheFile(file, { fromList: true })} onadd={addFile}
      onmkdir={(path) => workspace.addFolder(path)} onrelocate={relocateFiles}
      ondelete={deleteFiles} onduplicate={(entry, path) => workspace.duplicate(entry, path)}
      onfigure={addFigure} ontext={addDroppedText} demo={DEMO}
      ondownload={downloadTree} ondownloaditem={downloadEntry}
      previewable={(path) => aloneSet.has(path)} onlock={(entry) => lockPreview(entry.id)} onunlock={unlockPreview} />
  {/snippet}

  <!-- The figure being looked at: wherever it is shown, it is shown this way. -->
  {#snippet figureBody()}
    <div class="figureview">
      {#if !isDisplayableImage(shownFigure.path) && !shownFigure.path.toLowerCase().endsWith(".pdf")}
        <div class="notyet">
          <p class="lp-text-secondary text-sm">{m.readerd_figure_not_displayable({ name: basename(shownFigure.path) })}</p>
        </div>
      {:else if !figureUrl}
        <p>{m.readerd_figure_fetching({ path: shownFigure.path })}</p>
      {:else if shownFigure.path.toLowerCase().endsWith(".pdf")}
        <object data={figureUrl} type="application/pdf" title={shownFigure.path}>
          <p>{shownFigure.path}</p>
        </object>
      {:else}
        <img src={figureUrl} alt={shownFigure.path} />
      {/if}
    </div>
  {/snippet}

  {#snippet outlinePanel()}
    <Outline headings={outlineHeadings} counts={outlineSections} activeFrom={outlineActiveFrom} onselect={openOutlineHeading} />
  {/snippet}

  {#snippet agentPanel()}
    {#if lazy.agent.component}
    {@const Agent = lazy.agent.component}
    <Agent open={panel === "agent" && shown.comments} slug={SLUG} link={linkFor(SLUG)} canShare={doc.role === "owner"}
      path={session?.paths?.get(openFile) || ""} selection={agentSelection}
      revision={agentSelection?.revision || ""} request={assistantRequest}
      {comments} changes={changeRows} {diagnostics} onreview={reviewAssistantResults} onpreview={previewAssistant}
      onsettings={() => openSettings("agents")} userName={identity || doc.commenting_as || m.common_you()}
      bind:waiting={agentWaiting} />
    {/if}
  {/snippet}

  {#snippet collaborationPanel()}
    {#if compact}<Presence {participants} onjump={jumpToPerson} />{/if}
    <Collaboration messages={chatState.messages} more={chatState.more} loadingOlder={chatState.loadingOlder}
      loadError={chatState.loadError} canClear={chatState.canClear} onloadolder={liveChat.loadOlder}
      onclear={askClearChat} {connected} canPost={mayChat} onsend={sendLiveChat}
      {unreadChat} bind:tab={collaborationTab} {comments} {identity}
      commentingAs={doc.commenting_as || m.common_anonymous()} {canModerate}
      canComment={mayChat}
      onreveal={revealAnnotation} selected={selectedAnnotation} onresolve={resolve}
      ondelete={askDelete} ondeletemany={askDeleteMany} onreply={reply} onedit={edit}
      onagent={agentOffered ? askCommentAssistant : undefined}
      onattach={mayEdit ? attachComment : undefined} attachable={sourceSelected}
      {composing} {needsLogin} signInHref={signInHref()}
      page={annotations.state.page}
      onloadmore={() => void annotations.loadMore()}
      onloadreplies={(comment) => void annotations.loadReplies(comment)}
      oncommentsend={sendDraft} oncommentcancel={cancelDraft} oncommentdocument={commentOnDocument} />
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
      onclose={() => arrangement.showPanel("")} />
  {/snippet}

  {#snippet gitPanel()}
    {#if lazy.git.component}
    {@const Git = lazy.git.component}
    <Git open={panel === "git" && shown.comments}
      web={webGit} slug={SLUG} linkKey={KEY} desktop={hasDesktop}
      connection={webConnection} />
    {/if}
  {/snippet}

  {#snippet diagnosticsPanel()}
    <Diagnostics {diagnostics}
      entry={previewPath}
      canOpen={(item) => Boolean(diagnosticFile(item))} onopen={openDiagnostic}
      provenance={renderState.lastLatexResult?.provenance || null}
      attempts={renderState.lastLatexResult?.attempts || []}
      onagent={agentOffered ? askDiagnostic : undefined}
      log={renderState.lastLatexResult?.log || renderState.lastLog || ""}
      logOpen={renderState.lastLatexResult ? undefined : false} />
  {/snippet}

  {#snippet historyPanel()}
    {#if lazy.history.component}
    {@const History = lazy.history.component}
    <History {labels} viewing={historySelectedSha} older={historySource.older} newer={historySource.newer} canEdit={mayEdit}
      problem={historyProblem} loaded={timeline.state.loaded} complete={timeline.state.complete}
      loadingMore={timeline.state.loadingMore} moreProblem={timeline.state.moreProblem}
      saved={timeline.state.saved} savedLoaded={timeline.state.savedLoaded}
      savedLoading={timeline.state.savedLoading} savedProblem={timeline.state.savedProblem}
      onview={(sha) => historySource.mode !== "normal" ? historySource.choose(sha) : viewPoint(sha)}
      selecting={historySource.mode}
      oncancelselect={() => historySource.cancelSelect()}
      onname={nameLabel} onretry={loadHistory}
      onloadmore={() => timeline.loadMore()}
      onloaduntil={(iso) => timeline.loadUntil(iso)}
      onloadsaved={() => timeline.loadSaved()}
      authors={authorsView ? {
        rows: authorsSummary?.rows ?? [],
        since: authorsSummary?.since ?? null,
        loading: authorsLoading && !authorsMap,
        error: authorsError,
        noFile: !authorsShown,
      } : null}
      authorsOnly={authorsOnly}
      onauthorsview={(on) => (authorsView = on)}
      onisolate={(key) => (authorsOnly = key)} />
    {/if}
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
    label={m.common_document()}
    panels={{ files: filesPanel, outline: outlinePanel, agent: agentPanel,
              collaboration: collaborationPanel, changes: changesPanel,
              share: sharePanel, git: gitPanel, diagnostics: diagnosticsPanel, history: historyPanel }}
    panes={panes} sidebarPane={PANES.sidebar}
    onselectpanel={(name) => arrangement.selectPanel(name)} ondrop={dropped}
    onsize={(size) => setSize(PANES.sidebar, size)} onguide={(where) => (guide = where)}
    ongrab={(on) => { grabbing = on; guide = { ...guide, shown: on }; }}
  >
    {#snippet controls()}
      <!-- Back out to the projects. The rail it returns to is this rail, in
           the same place, which is why this is a plain link and not a
           gesture: the page changes, the furniture does not. -->
      <IconButton icon="home" label={m.readerd_all_projects()} href="/" />
      <IconButton icon="help" label={m.common_documentation()} href={DOCUMENTATION_URL} external />
      {#if editing}
        <IconButton icon={ARRANGEMENTS[layout].icon}
          label={m.readerd_layout_switch({ current: ARRANGEMENTS[layout].says, next: ARRANGEMENTS[ARRANGEMENTS[layout].next].says })}
          onclick={cycleLayout} />
      {/if}
      <IconButton icon="settings" label={m.common_settings()} tone="plain"
                  onclick={() => openSettings()} />
    {/snippet}
  </Sidebar>

  {#if sourceOpen}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section id="reader-source" class="editorpane" class:away={!shown.source} class:comparing={comparing !== ""}
             role={compact && modeSwitchShown ? "tabpanel" : undefined} aria-labelledby={compact && modeSwitchShown ? "mode-tab-source" : undefined}
             ondragover={(event) => event.preventDefault()} ondrop={dropped}>
      <!-- A figure has no editor. Choosing one shows it: an image as itself,
           a PDF through the browser's own viewer, which shows the first page
           without this application carrying a PDF renderer of its own. -->
      {#if comparing}
        <ComparisonBar says={comparisonSays} onclose={() => { endComparison(); if (compact) arrangement.showMobileView("sidebar"); }} />
      {/if}
      {#if comparing === "history"}
        {#if lazy.historyWorkspace.component}
          {@const HistoryWorkspace = lazy.historyWorkspace.component}
          <HistoryWorkspace source={historySource} versions={labels} canEdit={mayEdit} onrestore={restoreLabel}
            ondownload={downloadArchive}
            downloading={downloadingSha} downloadProblem={downloadProblem} />
        {/if}
      {:else if shownFigure}
        {@render figureBody()}
      {:else if Editor && sourceBound}
        <Editor bind:this={editor} {session} format={editorFormat} file={openFile} entry={previewPath} {keys} {completion} {brackets} {latexAssist} {proofing} {dialect} {check} {visual} imageUrl={figureUrlFor} {rules} {tracking} me={trackingMe} {selectedChange} editable={mayEdit}
                  annotating={mayEdit && mayChat} onannotate={annotateSource}
                  onbibliography={bibliographyAnalyzed} onchange={outlineTextChanged} oncaret={outlineCaretChanged} onsave={reportPersistence} onquit={showDocumentAlone}
                  oncomment={(id) => focusAnnotation(comments.find((item) => String(item.id) === String(id))?.id, { focus: false })}
                  onchangeselect={selectChangeFromSource}
                  onformatstate={(state) => { formatActive = state; refreshEditAvailability(); }}
                  authorship={authorsActive && authorsMap ? { authors: authorsMap, only: authorsOnly } : null}
                  onauthorship={(summary) => (authorsSummary = summary)}
                  onfilechange={(id) => { ws.openFile = id; outlineActiveFrom = null; ws.figure = null; noteSourceSelection(); }} />
      {/if}
    </section>
  {/if}

  <!-- A separator only where there are two things to separate. -->
  {#if shown.source && shown.document}
    <Grip pane={PANES.editor} label={m.readerd_grip_split()} panes={panes}
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
  {#if heldFile}
    <!-- A reader has no source pane, so a file that is not the preview is
         shown here, read only, where the preview would be. The frame below is
         only hidden, which is why going back does not render it again. -->
    <section class="latexpane fileview" aria-label={heldFile.path}>
      <div class="fileview-head">
        <span class="fileview-path">{heldFile.path}</span>
        <button class="btn btn-sm lp-control-tonal-brand" onclick={backToPreview}>{m.readerd_back_to_preview()}</button>
      </div>
      {#if heldFile.kind === "asset" && shownFigure}
        {@render figureBody()}
      {:else}
        <pre class="fileview-text">{projection.state.project?.texts?.[heldFile.path] ?? ""}</pre>
      {/if}
    </section>
  {:else if documentNeedsSignIn}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">{m.readerd_sign_in_to_open()}</h2>
        <p class="lp-text-secondary text-sm">
          {m.readerd_sign_in_to_open_body()}
        </p>
        <div class="notyet-actions">
          <a class="btn btn-sm lp-control-brand" href={signInHref()}>{m.common_sign_in()}</a>
        </div>
      </div>
    </section>
  {:else if shown.document && projection.state.unreadable}
    <!-- No projection ever arrived, so there is no tree to compile and no
         renderer card below has anything to say. This is the one state that
         is not "not yet" -- the server itself has nothing to hand this
         browser -- so it gets its own plain message instead of a blank pane
         or a spinner that `settled` silently ended (§2.2). -->
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">{m.readerd_cannot_show()}</h2>
        <p class="lp-text-secondary text-sm">{projection.state.unreadable}</p>
        <div class="notyet-actions">
          <button class="btn btn-sm lp-control-tonal-brand" onclick={() => void projection.refresh()}>{m.common_try_again()}</button>
        </div>
      </div>
    </section>
  {:else if shown.document && projectEmpty}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">{m.readerd_nothing_to_preview()}</h2>
        <p class="lp-text-secondary text-sm">
          {m.readerd_project_no_files()}
          {#if mayEdit}{m.readerd_add_one_from_files()}{/if}
        </p>
        {#if mayEdit}
          <div class="notyet-actions">
            <button class="btn btn-sm lp-control-tonal-brand" onclick={() => arrangement.openFilesPanel()}>{m.readerd_open_files()}</button>
          </div>
        {/if}
      </div>
    </section>
  {:else if shown.document && previewCard}
    <!-- The open file has nothing the preview could draw: no renderer, or it
         is part of a larger document that no single file stands for. -->
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">{m.readerd_nothing_to_preview()}</h2>
        <p class="lp-text-secondary text-sm">{previewCard}</p>
        {#if previewChoices.length}
          <p class="lp-text-secondary text-sm">{m.readerd_preview_inside()}</p>
          <div class="notyet-actions">
            {#each previewChoices as includer (includer)}
              <button class="btn btn-sm lp-control-tonal-brand" onclick={() => lockPreviewOn(session ? session.idOf(includer) || "" : includer)}>{m.common_lock_preview_on({ name: includer })}</button>
            {/each}
          </div>
        {:else}
          <p class="lp-text-secondary text-sm">{m.readerd_lock_preview_hint()}</p>
        {/if}
      </div>
    </section>
  {:else if shown.document && failedBeforeRender}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">{m.readerd_could_not_render()}</h2>
        {#if latexHtmlPreview}
          <p class="lp-text-secondary text-sm">
            {m.readerd_html_convert_failed()}
          </p>
          <p class="lp-text-secondary text-sm">
            <Icon name="triangle-alert" /> {m.readerd_diagnostics_converter()}
          </p>
          <div class="notyet-actions">
            <button class="btn btn-sm lp-control-brand" onclick={() => void setLatexOutput("pdf")}>{m.readerd_show_pdf()}</button>
            <button class="btn btn-sm lp-control-tonal-brand" onclick={() => arrangement.showPanel("diagnostics")}>{m.readerd_open_diagnostics()}</button>
          </div>
        {:else if renderState.failureReason}
          <p class="lp-text-secondary text-sm">
            {m.readerd_compiler_no_output({ kind: pdfOutput ? "PDF" : m.readerd_html_preview_kind() })}
          </p>
          <pre class="lp-text-secondary text-xs">{renderState.failureReason}</pre>
        {:else}
          <p class="lp-text-secondary text-sm">
            {#if !compilesHere}
              {m.readerd_browser_cannot_render_source({ format: sourceFormat === "latex" ? "LaTeX" : sourceFormat === "typst" ? "Typst" : sourceFormat === "quarto" ? "Quarto/Markdown" : sourceFormat === "org" ? "Org" : "Markdown" })} {sourceFormat === "latex" ? m.readerd_configure_latex() : m.readerd_desktop_can_render()}
            {:else}
              {m.readerd_fix_errors()}
            {/if}
          </p>
          {#if !compilesHere && sourceFormat !== "latex"}<DesktopOnly feature={m.readerd_this_preview()} />{/if}
        {/if}
      </div>
    </section>
  {:else if shown.document && noCompiler}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">{m.readerd_not_rendered()}</h2>
        <p class="lp-text-secondary text-sm">
          {m.readerd_browser_cannot_render_paged({ kind: sourceFormat === "typst" ? "Typst" : m.readerd_paged() })}
        </p>
        {#if session && previewPath}
          <section aria-label={m.readerd_source_of({ path: previewPath })}>
            <h3 class="h6">{m.readerd_source_heading({ path: previewPath })}</h3>
            <pre>{String(session.textOf(session.idOf(previewPath))?.toString?.() ?? "")}</pre>
          </section>
        {/if}
      </div>
    </section>
  {/if}

  <!-- Kept mounted whatever the arrangement: taking the frame out of the tree
       would reload the document and lose the reader's place in it. -->
  {#snippet previewControls()}
    {#if viewerView}
      <PreviewControls view={viewerView} tool={viewerTool} busy={previewBusy} dark={darkNow} {findAsk}
        onpresent={() => preview?.present()}
        send={(message) => {
          // The one message that is also a preference: remember what was chosen.
          if (message.type === "viewer-invert") {
            invertPages = message.invert;
            write(PDF_INVERT, message.invert);
          }
          return tell(message);
        }}
        onmode={(mode) => {
          // Optimistic, so the select does not snap back while the page is
          // redrawn; the frame corrects it with the scale it really used.
          viewerView = { ...viewerView, mode };
          tell({ type: "viewer-scale", mode });
        }}
        ontool={(next) => {
          viewerTool = next;
          tell({ type: "viewer-tool", tool: next });
        }}
        ondownload={can["download-pdf"] ? () => void downloadAs("pdf") : null} />
    {/if}
  {/snippet}
  {#snippet waitingCover()}
    <PreviewWaiting stage={waitStage} progress={waitProgress} />
  {/snippet}
  {#snippet emptyCover()}
    <p class="lp-text-secondary text-sm">{m.readerd_document_empty()}</p>
  {/snippet}
  {#snippet previewNotice()}
    {#if previewReason === "locked"}
      <span class="notice-chip">
        <span>{m.readerd_locked_path({ path: previewPath })}</span>
        <button type="button" aria-label={m.common_unlock_the_preview()} title={m.common_unlock_the_preview()} onclick={unlockPreview}><Icon name="x" /></button>
      </span>
    {:else}
      <span class="notice-chip"><span>{m.readerd_shown_inside({ name: basename(toolbarPath), path: previewPath })}</span></span>
    {/if}
  {/snippet}
  <Preview bind:this={preview} id="reader-document" role={compact && modeSwitchShown ? "tabpanel" : undefined}
           labelledby={compact && modeSwitchShown ? "mode-tab-document" : undefined}
           src={frameSrc} {docsOrigin} onmessage={fromFrame} onload={frameLoaded} {grabbing}
           controls={previewControls}
           notice={previewChip ? previewNotice : undefined}
           cover={painted.state.empty ? emptyCover : waitingUp ? waitingCover : undefined}
           busy={previewBusy && !waitingUp}
           progress={!waitingUp ? waitProgress : null}
           away={documentNeedsSignIn || Boolean(heldFile) || !shown.document || noCompiler || failedBeforeRender || Boolean(projection.state.unreadable) || Boolean(previewCard)} />

  <!-- Shown only while a separator is dragged: a line that follows the pointer
       so the split can be seen moving without the iframe reflowing on every
       pointermove. -->
  {#if guide.shown}<div class="grip-guide" class:held={guide.held} style="left: {guide.left}px"></div>{/if}

  <!-- The File, Edit, View and Insert commands as a list over the workspace.
       Everything under it stays mounted, the editor included. -->
  {#if compact && mobileCommand}
    <CommandView groups={mobileCommandGroups} onchoose={(id) => phone.chooseCommand(id)} />
  {/if}
</main>

{#if tabBarShown}
  <TabBar active={tabActive}
          filesShown={tabs.some((tab) => tab.id === "files")} filesDisabled={Boolean(tabs.find((tab) => tab.id === "files")?.disabled)}
          commentsShown={tabs.some((tab) => tab.id === "collaboration")} commentsDot={Boolean(badges.collaboration?.dot)}
          {moreOpen} offlineNote={!connected && !DEMO ? connectionNote : ""}
          ondocument={() => phone.showDocument()} onfiles={() => void openPanel("files")} oncomments={() => phone.showComments()} onmore={openMore}
          hidden={barHidden} onreveal={() => (barHidden = false)} />
{/if}
{#if compact && keyboardBarShown}
  <KeyboardBar onundo={() => void chooseEditCommand("undo")} onredo={() => void chooseEditCommand("redo")}
               oninsert={() => phone.openCommand("insert")} onmore={() => openMore()} />
{/if}
{#if compact}
  <MoreSheet bind:open={arrangement.state.moreOpen} sections={moreSections} onchoose={(id) => phone.chooseMoreRow(id)} finalFocusEl={() => moreOpener} />
{/if}

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
                    aria-label={m.readerd_choose_highlight_colour()} title={m.readerd_choose_highlight_colour()}
                    style="--ink: {highlightColor}"
                    onclick={() => (palette = !palette)}></button>
            {#if palette}
              <div class="highlight-colors" role="group" aria-label={m.readerd_highlight_colour_group()}>
                {#each HIGHLIGHT_COLORS as color}
                  <button type="button" class:selected={highlightColor === color} class="color-swatch" style="background:{color}"
                    aria-label={m.readerd_highlight_in({ color: colorName(color) })} onclick={() => highlightWith(color)}></button>
                {/each}
                <label class="custom-color" title={m.readerd_choose_highlight_colour()}>
                  <span class="sr-only">{m.readerd_custom_highlight_colour()}</span>
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
{#if lazy.commandPalette.component}
  {@const CommandPalette = lazy.commandPalette.component}
  <CommandPalette bind:open={commandPalette} context={commandContext} apple={APPLE}
                  onrun={(id) => void runCommand(id)} />
{/if}

<!-- The palettes open over the editor rather than in the sidebar. Each is
     mounted only while open, so a form left half filled does not come back. -->
<Modal bind:open={symbolsOpen} title={m.common_insert_symbol()} compact sheet
       initialFocus={() => symbolPalette?.focusTarget?.()} onEscapeKeyDown={(event) => paletteEscape(symbolPalette, event)}>
  {#if symbolsOpen && lazy.symbols.component}
    {@const SymbolPalette = lazy.symbols.component}
    <SymbolPalette bind:this={symbolPalette} format={editorFormat} getContext={insertContext} oninsert={(result, context) => applyInsertion(result, context, { focus: false })} oncancel={(context) => editor?.releaseInsertContext?.(context)} />
  {/if}
</Modal>
<Modal bind:open={mathOpen} title={m.common_math()} wider
       initialFocus={() => mathPalette?.focusTarget?.()} onEscapeKeyDown={(event) => paletteEscape(mathPalette, event)}>
  {#if mathOpen && lazy.math.component}
    {@const MathPalette = lazy.math.component}
    <MathPalette bind:this={mathPalette} format={editorFormat} getContext={insertContext} oninsert={applyInsertion} onfocus={closePalette} oncancel={(context) => editor?.releaseInsertContext?.(context)} />
  {/if}
</Modal>

<!-- Word counts for the open file, read on open and as the text changes. -->
<Modal bind:open={wordCountOpen} title={m.common_word_count()}
  confirm={{ label: m.common_close(), cancel: false, onclick: () => (wordCountOpen = false) }}>
  {#snippet children()}
    <WordCount count={wordCount} />
  {/snippet}
</Modal>

<!-- The preferences dialog is shared by the navbar menu and sidebar button. -->
{#if lazy.settings.component}
  {@const SettingsDialog = lazy.settings.component}
  <SettingsDialog bind:open={settingsOpen} bind:category={settingsCategory} bind:entry={settingsEntry}
                  remoteConnected={connected} remoteNote={connectionNote}
                  {keys} onkeys={setKeys}
                  {completion} oncompletion={setCompletion} {brackets} onbrackets={setBrackets} {latexAssist} onlatexassist={setLatexAssist} {proofing} onproofing={setProofing} {dialect} ondialect={setDialect} {check} oncheck={setCheck}
                  visual={prefs.visual} onvisual={setVisualDefault}
                  commands={commandContext}
                  userId={buildUserId}
                  onbuildpreferences={(format, next) => { if (format === sourceFormat) setBuildPreferences(next); }}
                  account={me} />
{/if}

<SaveTemplateDialog bind:this={saveTemplateDialog} slug={SLUG} title={doc.title} />

<!-- Restoring is an editor's action, so a reader is given no dialog for it. -->
{#if mayEdit}
  <Modal bind:open={timeline.state.restoring} title={m.readerd_restore_version_title()}
    description={restoreMoved
      ? m.readerd_restore_moved({ name: restoreName })
      : m.readerd_restore_description({ name: restoreName })}
    confirm={{
      label: restoreBusy ? m.readerd_restoring() : restoreMoved ? m.readerd_restore_anyway() : m.readerd_restore_version(),
      disabled: restoreBusy || !mayEdit,
      onclick: confirmRestore,
    }}>
    {#if historyProblem}
      <p class="lp-text-error text-sm" role="alert">{historyProblem}</p>
    {/if}
  </Modal>
{/if}

<DeleteCommentsDialog bind:this={deleteCommentsDialog} ondelete={(comment) => annotations.delete(comment)} />

<ClearChatDialog bind:this={clearChatDialog} onclear={() => collaboration?.send({ type: "chat-clear" })} />

<IdentityDialog bind:this={identityDialog} name={doc.commenting_as || m.common_anonymous()} signInHref={signInHref} oncontinue={() => openDraft()} />

<Toasts />

<style>
  .demo-banner, .share-intro { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--spacing) calc(var(--spacing) * 3); padding: calc(var(--spacing) * 1.5) var(--panel-padding, calc(var(--spacing) * 4)); border-bottom: 1px solid var(--color-pane-edge); background: var(--color-subtle); color: var(--color-text); font-size: var(--text-sm); }
  .demo-banner-actions { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 2); }
  .editorpane.comparing { grid-template-rows: auto minmax(0, 1fr); }
  .bar-toggle { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); cursor: pointer; font-size: var(--text-sm); white-space: nowrap; }
  .nav-document {
    display: block;
    max-width: min(38vw, 20rem);
    color: var(--color-text-secondary);
    font-size: var(--text-sm);
  }
  /* Adapted widths above the phone keep the two faces grouped in the bar. */
  .face-switch { display: inline-flex; align-items: center; gap: 2px; padding: 2px; border-radius: var(--radius-container); background: var(--color-subtle); }
  /* The phone's header over a non-tab view: a back control and the title,
     the control a thumb-sized square with no fill of its own. */
  .mobile-header-button { position: relative; display: inline-grid; grid-auto-flow: column; flex: none; place-items: center; gap: .125rem; min-width: 2.75rem; height: 2.75rem; padding: 0; border: 0; border-radius: var(--radius-base); background: none; color: var(--color-text); font-size: var(--text-xs); line-height: 1; cursor: pointer; }
  .mobile-header-button:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .phone-safe-top { height: env(safe-area-inset-top); background: var(--color-shell); }
  /* The title takes the room the buttons leave. Over a panel or a list it is
     that view's name; over the document it is the quiet file name. */
  .nav-document.mobile-title { flex: 1 1 auto; min-width: 0; max-width: none; color: var(--color-text); font-size: .875rem; }
  .nav-document.mobile-title-open { font-size: 1rem; font-weight: 500; }
  @media (max-width: 600px) {
    /* Nobody else here yet: an empty group would still take a gap. */
    .presence:empty { display: none; }
  }
  .tracking-pill { display: inline-flex; flex: none; align-items: center; gap: calc(var(--spacing) * 1.5); min-height: 1.75rem; padding: 0 calc(var(--spacing) * 2.5); border: 1px solid var(--color-divider); border-radius: 999px; background: transparent; color: var(--color-text-secondary); font-size: var(--text-xs); white-space: nowrap; cursor: pointer; }
  .tracking-pill:hover { background: var(--color-row-hover); color: var(--color-text); }
  .tracking-pill:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  .presence { display: inline-flex; flex: none; align-items: center; gap: calc(var(--spacing) * .5); color: var(--color-text-secondary); font-size: var(--text-xs); }
  .presence-offline { display: inline-flex; align-items: center; gap: calc(var(--spacing) * .75); color: var(--color-text-secondary); font-size: var(--text-xs); }
  .presence-offline-dot { width: .5rem; height: .5rem; flex: 0 0 .5rem; border-radius: 50%; background: var(--color-error-solid); }
  .presence-jump { display: inline-flex; flex: none; padding: 0; border: 0; border-radius: 50%; background: none; cursor: pointer; }
  .presence-jump:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  .presence-jump + .presence-jump { margin-left: calc(var(--spacing) * -1.5); box-shadow: 0 0 0 2px var(--color-shell); }
  .presence-more { display: inline-grid; place-items: center; min-width: 1.5rem; height: 1.5rem; margin-left: calc(var(--spacing) * -1.5); border-radius: 50%; background: var(--color-divider); color: var(--color-text-secondary); font-size: .65rem; }
  /* A heading over a run of items. Aligned with the names rather than with
     the panel edge, so the column the items make starts once. */
  @media (max-width: 767px) {
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
  .color-swatch { width:1.5rem; height:1.5rem; border:2px solid transparent; border-radius:50%; }
  .color-swatch.selected { border-color:var(--color-text); box-shadow:0 0 0 1px var(--color-brand); }
  .custom-color { display:grid; place-items:center; width:1.5rem; height:1.5rem; }
  .custom-color input { width:1.5rem; height:1.5rem; padding:0; border:0; background:transparent; }
  .pending-recovery { flex-shrink: 0; max-height: 35%; overflow-y: auto; border-top: 1px solid var(--color-border); background: var(--color-subtle); }
</style>
