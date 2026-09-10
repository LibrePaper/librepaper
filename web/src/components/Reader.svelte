<script>
  // One document: the source beside it, the page itself, and everything said
  // about it.
  import { anchorAll, anchorAllSources, flatten } from "../lib/anchor.js";
  import * as sync from "../lib/sync.js";
  import * as renderers from "../lib/renderers.js";
  import * as quarto from "../lib/engines/quarto.js";
  import * as diagnosticsRule from "../lib/diagnostics.js";
  import * as figures from "../lib/figures.js";
  import * as history from "../lib/history.js";
  import { createHistoryController } from "../lib/reader/history.svelte.js";
  import * as passages from "../lib/passages.js";
  import * as suggestions from "../lib/suggestions.js";
  import { diagnosticContext } from "../lib/assistant-review.js";
  import { capturePreviewTree, previewCandidate } from "../lib/assistant-preview.js";
  import { attribution, authorIndex, itemsFor } from "../lib/redlines.js";
  import { orphanState } from "../lib/orphan.js";
  import * as latex from "../lib/latex.js";
  import * as localQuarto from "../lib/latex/local.js";
  import { checkPlacement, basename, inside } from "../lib/file-manager.js";
  import { snapshotDigest } from "../lib/tree-digest.js";
  import { createAnnotations } from "../lib/reader/annotations.js";
  import { createReaderBoot } from "../lib/reader/boot.js";
  import { createPendingChat } from "../lib/reader/chat.js";
  import { createReaderCollaboration } from "../lib/reader/collaboration.js";
  import { needsSourceRefresh } from "../lib/reader/source-events.js";
  import PendingAnnotations from "./PendingAnnotations.svelte";
  import {
    SHELL_HEADERS,
    config as loadConfig,
    keyHeaders,
    signInHref,
    uploadAsset,
  } from "../lib/api.js";
  import {
    KEYMAP,
    LAYOUT,
    LINKED,
    PANEL,
    SOURCE_SIDE,
    linkFor,
    markViewed,
    read,
    takeKeyFromFragment,
    write,
  } from "../lib/storage.js";
  import { ACTIVITY_WIDTH, DOCUMENT_MIN, GRIP, LAYOUTS, PANES, RATIOS, clamp, pixels, remember, showing, stored } from "../lib/panes.js";

  import { tick, untrack } from "svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import CopyLink from "./CopyLink.svelte";
  import Share from "./Share.svelte";
  import Modal from "./Modal.svelte";
  import Toasts from "./Toasts.svelte";
  import DictationDownload from "./DictationDownload.svelte";
  import DictationPill from "./DictationPill.svelte";
  import Row from "./layout/Row.svelte";
  import { problem as toastProblem, said as toastSaid, unsay as toastUnsay } from "../lib/toast.svelte.js";
  import { availableDownloads, inlineBlobUrls, saveBlob } from "../lib/reader/downloads.js";
  import { getDictation } from "../lib/dictation/service.js";
  import { targetForActiveElement, textareaTarget } from "../lib/dictation/targets.js";
  import Preview from "./Preview.svelte";
  import Grip from "./Grip.svelte";
  import Collaboration from "./Collaboration.svelte";
  import Changes from "./Changes.svelte";
  import { loadRenderOptions, saveRenderOptions, parseRenderOptions } from "../lib/quarto-options.js";
  import Agent from "./Agent.svelte";
  import History from "./History.svelte";
  import Diagnostics from "./Diagnostics.svelte";
  import SettingsDialog from "./settings/SettingsDialog.svelte";
  import LatexStatus from "./LatexStatus.svelte";
  import Files from "./Files.svelte";
  import { renderedNoteText } from "../lib/latex/status-text.js";
  import { createPreviewApi } from "../lib/reader/preview-api.js";
  import { createFramePreview } from "../lib/reader/frame-preview.js";
  import { createRenderingStore } from "../lib/reader/rendering-store.js";
  import { createLocalPreview } from "../lib/reader/local-preview.js";
  import { HIGHLIGHT_COLORS } from "../lib/annotation-colors.js";
  import { documentResultsIdentity } from "../lib/engines/identity.js";
  import DictationButton from "./DictationButton.svelte";
  import InsertMenu from "./InsertMenu.svelte";

  const SLUG = location.pathname.split("/").pop();

  // A one-time migration: the old distribution chooser kept its choice under
  // this key, in this browser, forever. The engine initializes automatically --
  // there is nothing left to remember here, and a stale entry is only ever
  // read by code that no longer exists.
  try {
    localStorage.removeItem("librepaper-latex");
  } catch {
    // Storage can be unavailable (private browsing, a locked-down profile);
    // there is nothing to migrate away from in that case either.
  }

  // The key a reader arrived with, taken out of the fragment before anything
  // asks the server a question. A fragment never leaves the browser, so this
  // is the one part of the URL a link key can safely travel in; from here it
  // is kept under the slug and presented on every request for this document.
  const KEY = takeKeyFromFragment(SLUG);
  const previewApi = createPreviewApi({ slug: SLUG, key: KEY, shellHeaders: SHELL_HEADERS, keyHeaders });

  /* ------------------------------------------------------------ the document */

  let doc = $state({});
  let readerDisposed = false;
  let docsOrigin = $state(null);
  let frameSrc = $state(null);
  let me = $state({});
  // The displayed name, since this is what goes on a comment and what the
  // reader is shown commenting as. A Google account's handle is its email and
  // belongs on neither.
  let identity = $derived(me.name || "");
  let canModerate = $derived(Boolean(doc.can_moderate));
  let connected = $state(true);
  let liveChat = $state([]);
  let unreadChat = $state(false);
  let pendingChat;
  let mayChat = $derived(["commenter", "editor", "owner"].includes(doc.role));
  // Sharing is the owner's; seeing who else is in the room is anyone's who is
  // named on the document. A reader who arrived by link is offered neither,
  // which is most of the point of a blind review.
  let canSeeSharing = $derived(Boolean(doc.can_see_sharing));

  // Whether this browser's work is safe, which is a different question from
  // whether the socket is up. `pending` counts the updates the server has not
  // yet said it has written; `local` says the document is in this browser's
  // own storage, which is what makes a reload safe while the socket is down.
  let persistence = $state({ pending: 0, local: false, joined: false });

  /* --------------------------------------------------------------- anchoring */

  let comments = $state([]);
  let unconfirmed = $state([]);
  const annotations = createAnnotations({
    slug: SLUG,
    list: () => comments,
    update: (next) => (comments = next),
    anchor: anchorComments,
    repaint: applyHighlights,
    send: (message) => collaboration?.send(message),
    changed: (items) => (unconfirmed = items),
  });
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
  // Calepin run the same `createLocalPreview` lifecycle (`../lib/reader/local-preview.js`)
  // against the same local app; only the reactive mirrors below -- what the
  // template reads -- are per engine.
  let quartoPreview = $state(null);
  let quartoPreviewStarting = $state(false);
  let quartoLiveSyncTimer = null;
  // Whether the bridge says Quarto is currently re-rendering the live
  // preview -- carried on every response of the page route (200, 304, 404
  // alike), never just the ones with new HTML. Drives the "Rendering…" badge
  // and the faster poll cadence while true.
  let quartoRendering = $state(false);
  let calepinPreview = $state(null);
  let calepinPreviewStarting = $state(false);
  let calepinSyncTimer = null;
  let calepinRendering = $state(false);
  // Front-matter derived defaults (format, profile, parameters), remembered
  // per document. There is no UI to change these any more -- nothing
  // rendered is ever uploaded, so there is no render job to point them at --
  // but the live preview controller still hands them to Quarto's own preview.
  let quartoOptions = $state(loadRenderOptions(SLUG));
  let quartoBindingId = $state("");
  let localAppStatus = $state(localQuarto.status());
  // Which of Quarto's own live preview, or this browser's own Markdown
  // draft, a Quarto document shows. One person's choice, remembered per
  // document, in this browser.
  const QUARTO_PREVIEW_MODE_KEY = `librepaper-quarto-preview:${SLUG}`;
  let quartoPreviewMode = $state(read(QUARTO_PREVIEW_MODE_KEY, "quarto") === "markdown" ? "markdown" : "quarto");
  // Which of the browser's own Typst rendering, or Calepin running the same
  // document's chunks on this computer through the local app, a Typst
  // document shows. Same shape of choice as Quarto's, remembered separately.
  const TYPST_PREVIEW_MODE_KEY = `librepaper-typst-preview:${SLUG}`;
  let typstPreviewMode = $state(read(TYPST_PREVIEW_MODE_KEY, "typst") === "calepin" ? "calepin" : "typst");

  function setQuartoPreviewMode(mode) {
    quartoPreviewMode = mode === "markdown" ? "markdown" : "quarto";
    write(QUARTO_PREVIEW_MODE_KEY, quartoPreviewMode);
    // A user gesture may open the pairing popup when the app is reachable
    // but not yet paired; the mode's own effect starts the preview once it
    // is connected.
    if (quartoPreviewMode === "quarto") void ensureLocalApp();
  }

  function setTypstPreviewMode(mode) {
    typstPreviewMode = mode === "calepin" ? "calepin" : "typst";
    write(TYPST_PREVIEW_MODE_KEY, typstPreviewMode);
    if (typstPreviewMode === "calepin") void ensureLocalApp();
  }

  function quartoTargetFormat(tree = null) {
    if (quartoOptions.format !== "default") return quartoOptions.format;
    const main = tree?.main || session?.mainPath?.() || "main.qmd";
    const source = tree?.texts?.[main] || session?.textOf?.(session.mainId?.())?.toString?.() || session?.text?.toString?.() || "";
    const value = quarto.parseQuarto(source, { path: main }).metadata?.format;
    const named = typeof value === "string" ? value : value && typeof value === "object" ? Object.keys(value)[0] : "html";
    const format = String(named || "html").trim().toLowerCase().split(/[+:]/, 1)[0];
    return ["html", "pdf", "docx", "revealjs"].includes(format) ? format : "html";
  }
  function quartoRenderContext(tree = null) {
    return {
      format: quartoTargetFormat(tree),
      profiles: quartoOptions.profile ? [quartoOptions.profile] : [],
      parameters: { ...quartoOptions.parameters },
    };
  }

  const tell = (message, transfer) => preview?.tell(message, transfer);
  // Initialized after the derived frame kind is available. The controller's
  // callbacks still update the small bits of component state used by the
  // template and annotation code.
  let framePreview;
  let renderingStore;

  // The agent repaints the whole document on every "regions" or "highlight"
  // message, so a call that changes nothing is not free even though it looks
  // idempotent. Each is sent only when its payload actually differs from the
  // last one sent -- reset when the frame republishes its text, since the
  // agent's DOM was rebuilt then and needs the full repaint regardless.
  let lastRegions = null;
  let lastHighlight = null;
  let lastRedlines = null;
  // The annotation singled out last, from either side: a card clicked in the
  // sidebar or a mark clicked in the document. Its card wears a ring and the
  // frame rings its passage, and both stay until another one is chosen.
  let selectedAnnotation = $state("");
  let lastSelected = null;
  function applySelection() {
    if (!frameReady || selectedAnnotation === lastSelected) return;
    lastSelected = selectedAnnotation;
    tell({ type: "select", id: selectedAnnotation });
  }
  $effect(() => { void selectedAnnotation; applySelection(); });

  function applyHighlights() {
    if (!frameReady) return;
    const regions = JSON.stringify(
      comments
        .filter((comment) => comment.region && !comment.output_anchor)
        .map((comment) => ({
          id: comment.id,
          point: Boolean(comment.point),
          digest: comment.region.image_digest,
          index: comment.region.image_index,
          x: comment.region.x,
          y: comment.region.y,
          w: comment.region.w,
          h: comment.region.h,
          motivation: comment.motivation,
          resolved: Boolean(comment.resolved),
        })),
    );
    if (regions !== lastRegions) {
      lastRegions = regions;
      tell({ type: "regions", regions: JSON.parse(regions) });
    }

    const highlight = JSON.stringify(
      comments
        .filter((comment) => !comment.orphaned && comment.start != null)
        .map((comment) => ({
          id: comment.id,
          point: Boolean(comment.point),
          start: comment.start,
          end: comment.end,
          motivation: comment.motivation,
          resolved: Boolean(comment.resolved),
          // Only meaningful for a suggestion, but sent for every comment: the
          // frame paints them only where `motivation` is `editing`, and a
          // constant shape here keeps the JSON comparison above from firing
          // on fields that never change.
          proposed: comment.proposed ?? "",
          outcome: comment.outcome || "",
          color: comment.color || undefined,
        })),
    );
    if (highlight !== lastHighlight) {
      lastHighlight = highlight;
      tell({ type: "highlight", ranges: JSON.parse(highlight) });
    }
  }

  // The "Show in document" toggle in the history panel. Items are sent only
  // while the toggle is on, the history panel is the one showing, the
  // format has text to paint into, and the panel actually has a diff to
  // show -- every other state means an empty list, which is what clears
  // whatever was painted before. `who` is computed once for the whole
  // comparison (the checkpoints between the baseline and the compare point,
  // or the baseline and the live document when there is no compare point),
  // since redlines describe one span, not one hunk at a time.
  function applyRedlines() {
    if (!frameReady) return;
    const showable = historyRedlines && panel === "history" && !redlinesDisabledReason &&
      historyBaseline && Array.isArray(historyChanges);
    // Each hunk already carries its own author in `who` when the history
    // controller's chained attribution ran (`hunk.who`, read by `itemsFor`
    // in preference to the range-level fallback below); `author` turns that
    // name into the colour index the frame paints with, the same index
    // `History.svelte`'s timeline dot uses for the same author.
    const items = showable
      ? (() => {
          const fallback = attribution(checkpoints, historyBaseline.sha, historyComparePoint?.sha || null);
          const authors = authorIndex(checkpoints);
          return itemsFor(historyChanges, fallback).map((item) => ({
            ...item,
            author: authors.has(item.who) ? authors.get(item.who) : undefined,
          }));
        })()
      : [];
    const payload = JSON.stringify(items);
    if (payload !== lastRedlines) {
      lastRedlines = payload;
      tell({ type: "redlines", items: JSON.parse(payload) });
    }
  }

  // Whether a comment's passage is lost is answered from two anchors, not
  // one: the rendered quotation, which is what the highlight and the click
  // target are drawn from, and the source quotation, which is the anchor of
  // record. A comment is orphaned only when neither finds its passage; when
  // only the source still has it, the card says so instead and a click on it
  // goes to the source rather than nowhere. A region has no source anchor and
  // is never in either state.
  function applyAnchorFlags(comment) {
    if (comment.output_anchor) {
      comment.orphaned = false;
      comment.inSourceOnly = false;
      comment.start = comment.end = comment.sourceStart = null;
      return;
    }
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
    const renderAnchors = list.filter((comment) => !comment.region && !comment.output_anchor);
    anchorAll(docText || "", renderAnchors, docText === null ? null : docView);
    anchorAllSources(treeNow(), list.filter((comment) => !comment.output_anchor));
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
    if (!mayEdit || !session || viewing || docText === null) return;
    const tree = treeNow();
    const open = session?.paths?.get(openFile) || "";
    for (const comment of comments) {
      if (comment.source || comment.region || comment.point || comment.output_anchor || comment.pending || comment.temp_id) continue;
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
    comments = comments;
    applyHighlights();
    tracePassages();
    // The frame's own `ready` is what re-anchors after the source changes --
    // `session.watchSource` schedules a repaint, and every repaint ends here
    // -- so a comment newly findable in the source is caught by the same
    // pass, not by a second path. Batched a tick out so the paint above is
    // never delayed by a socket round trip.
    setTimeout(backfillSourceAnchors, 0);
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
        tell({ type: "tool", tool });
        // Whatever was painted before is gone with the rebuilt DOM.
        lastRegions = lastHighlight = lastRedlines = lastSelected = null;
        reanchor();
        applySelection();
        revealPendingHistory();
        // A repaint rebuilds the frame's document from scratch, so redlines
        // need resending here just as highlights do in `reanchor` -- the
        // `computeHistoryChanges` branch below covers the case where the
        // hunks themselves are stale, but the common case is the same hunks
        // painted onto a freshly built DOM.
        applyRedlines();
        if (panel === "history" && historyBaseline && (!viewing || historyComparePoint)) void computeHistoryChanges();
        if (first) {
          replayPreview();
          void paintPreview();
        }
        break;
      case "selection":
        showSelection(message.selector, message.rect);
        break;
      case "region":
        // A rectangle drawn on a figure anchors the same way a quotation
        // does, but it has no words to look up in the source: a region has
        // no source anchor and never will.
        pending = { exact: "", prefix: "", suffix: "", position: null, region: message.region, source: null };
        placeBar(message.rect);
        break;
      case "regions-unplaceable": {
        const ids = new Set((message.ids || []).map(String));
        if (!ids.size) break;
        for (const comment of comments) {
          if (comment.region && ids.has(String(comment.id))) {
            comment.regionUnplaceable = true;
            comment.regionUnplaceableReason = String(message.reason || "figure-unavailable");
          }
        }
        comments = comments;
        break;
      }
      case "regions-placeable": {
        const ids = new Set((message.ids || []).map(String));
        if (!ids.size) break;
        for (const comment of comments) {
          if (comment.region && ids.has(String(comment.id))) {
            delete comment.regionUnplaceable;
            delete comment.regionUnplaceableReason;
          }
        }
        comments = comments;
        break;
      }
      case "caret":
        followDocumentClick(Number(message.offset) || 0);
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
      if (docText !== null && !point) {
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
    const tree = treeNow();
    selectionRevision = viewing?.sha ? Promise.resolve(viewing.sha) : snapshotDigest(tree);
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
    const tree = capturePreviewTree(treeNow());
    if (Object.keys(tree.digests || {}).length) {
      const held = await figures.gather(SLUG, tree.digests, { ...SHELL_HEADERS, ...keyHeaders(KEY) });
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
    if (!suggestion) collaborationTab = plainHighlight ? "highlights" : "comments";
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
      if (id) { openFile = id; editor.goToIn(id, comment.sourceStart); }
      else editor.goTo(comment.sourceStart);
    }
  }

  function placeBar(rect) {
    if (rect && !matchMedia("(max-width:760px)").matches) {
      const frameRect = document.querySelector(".viewport").getBoundingClientRect();
      bar = {
        shown: true,
        left: Math.max(8, Math.min(innerWidth - 260, frameRect.left + rect.left + (rect.right - rect.left) / 2 - 125)),
        top: Math.max(65, frameRect.top + rect.top - 42),
      };
      return;
    }
    bar = { ...bar, shown: true };
  }

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
  let commentBodyField = $state(null);

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
    if (!pending || !mayChat) return;
    // The server determines the author when it acknowledges the submission.
    annotations.comment(pending, { motivation, body, proposed, color: motivation === "highlighting" ? highlightColor : undefined }, identity || doc.commenting_as || "Anonymous");
    pending = null;
  }

  function submitDialog(event) {
    event.preventDefault();
    const motivation = tool === "region" || tool === "point" ? "commenting" : tool;
    submitAnnotation({
      motivation,
      body: draft.body,
      proposed: motivation === "editing" ? draft.proposed : undefined,
    });
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
    comments = comments;
    collaboration?.send({ type: action, comment_id: comment.id, request_id: crypto.randomUUID() });
  }

  async function rejectConfirmed(comment) {
    const response = await fetch(`/api/documents/${SLUG}/comments`, {
      method: "POST", headers: { ...SHELL_HEADERS, ...keyHeaders(KEY), "Content-Type": "application/json" },
      body: JSON.stringify({ type: "reject", comment_id: comment.id, request_id: crypto.randomUUID() }),
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
      historyController.closeFileDiff();
      mergeTarget = {
        path,
        oldText,
        newText: tree.texts?.[path] ?? "",
        liveText: id ? session.textOf(id) : null,
        awareness: session.awareness,
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

  let collaboration = null;

  function sendLiveChat(text) {
    if (!collaboration || !connected || !mayChat) return Promise.resolve(false);
    return pendingChat?.send(text) || Promise.resolve(false);
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
      outbox.reconcile(event.comments);
      comments = event.comments;
      commentsReady = true;
      reanchor();
      if (panel === "history" && !historyBaseline) void loadHistory();
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
        comments = comments;
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
          headers: { ...SHELL_HEADERS, ...keyHeaders(KEY) },
        })
          .then((response) => response.json())
          .then((data) => receive({ type: "hello", comments: data.comments }))
          .catch(() => {});
      }
      toastProblem(event.message);
      return;
    }

    // The shared document: the state of the session as it stands, one more
    // change to it, what the server has written, who else is in it, or where
    // their carets are. A reader receives all of this too -- that is how they
    // see the current text -- and sends none of it.
    if (event.type === "y-state") {
      session
        ?.start(event)
        .then(() => paintPreview())
        .catch((error) => say(error.message || "could not open the document", true));
      peers = event.count || 1;
      return;
    }
    if (event.type === "y-update") {
      session?.apply(event.update);
      return;
    }
    if (event.type === "y-awareness") {
      session?.applyAwareness(event.update);
      return;
    }
    if (event.type === "y-ack") {
      // The server has written this far. Relaying was never durability; this
      // is, and it is what the badge is allowed to speak from.
      session?.acknowledge(event.seq || 0);
      return;
    }
    if (event.type === "y-peers") {
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
      comments = comments;
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
  // Which text the editor is bound to. It changes once on a document migrated
  // from before there were directories: the words arrive in the retired text
  // and the session then holds them in a file. Keying the component on this is
  // what binds it to the file rather than to what the file used to be.
  let sourceEpoch = $state(0);
  let mayEdit = $state(false);
  let sourceFormat = $state("");
  let peers = $state(1);
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
  const atRisk = $derived(Boolean(mayEdit && persistence.pending && !persistence.local));

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
    localQuarto.configure({ project: SLUG, origin: location.origin });
    let status = await localQuarto.retry();
    if (status.state === "unreachable") {
      // A registered `librepaper://` handler starts the app; without one
      // this is a no-op and the retry below says so.
      localQuarto.openApp();
      await new Promise((resolve) => setTimeout(resolve, 1500));
      status = await localQuarto.retry();
    }
    if (status.state === "unauthorized" || status.state === "reachable") {
      try { status = await localQuarto.pairViaApp(); }
      catch (error) { say(error.message, true); return false; }
    }
    if (status.state !== "connected") {
      say(status.instructions || "Local LibrePaper is unavailable.", true);
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
  let viewing = $state(null);

  // Whether this browser should be running Quarto's own live preview rather
  // than showing its own draft rendering: Quarto preview mode chosen, paired,
  // connected, editable, and not looking at history. `editing` (the source
  // pane) is not required -- an editor who has not opened it yet still gets
  // the live pane the moment they are able to edit.
  const quartoLiveActive = $derived(
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && mayEdit && !viewing &&
      localAppStatus.state === "connected",
  );

  // Whether this browser should be showing a Typst document's chunks run by
  // Calepin on this computer rather than this browser's own Typst rendering:
  // the calepin mode chosen, paired, connected, the calepin command itself
  // found, editable, and not looking at history.
  const calepinActive = $derived(
    sourceFormat === "typst" && typstPreviewMode === "calepin" && mayEdit && !viewing &&
      localAppStatus.state === "connected" && localQuarto.calepinAvailable(),
  );

  const quartoPreviewController = createLocalPreview({
    local: localQuarto,
    engine: "quarto",
    label: "Quarto",
    publish: (payload) => framePreview.publish(payload),
    say,
    treeNow,
    entrypointOf: (tree) => tree.main,
    optionsOf: (tree) => {
      const context = quartoRenderContext(tree);
      return { format: context.format, profile: context.profiles[0] || null, parameters: context.parameters };
    },
    jobOf: () => ({ binding: quartoBindingId }),
    isDisposed: () => readerDisposed,
    onRunningChange: (session) => (quartoPreview = session),
    onRenderingChange: (value) => (quartoRendering = value),
    onStartingChange: (value) => (quartoPreviewStarting = value),
    onEnded: () => void paintPreview(),
  });

  const calepinPreviewController = createLocalPreview({
    local: localQuarto,
    engine: "calepin",
    label: "Calepin",
    publish: (payload) => framePreview.publish(payload),
    say,
    treeNow,
    entrypointOf: (tree) => tree.main,
    optionsOf: () => ({ format: "pdf" }),
    jobOf: () => ({ binding: quartoBindingId }),
    isDisposed: () => readerDisposed,
    onRunningChange: (session) => (calepinPreview = session),
    onRenderingChange: (value) => (calepinRendering = value),
    onStartingChange: (value) => (calepinPreviewStarting = value),
    onEnded: () => void paintPreview(),
  });

  // Drives the whole automatic mode for each engine: starts the managed
  // preview the moment its conditions are met, and tears it down (falling
  // back to the ordinary rendering, no retry) the moment any of them stop
  // holding.
  $effect(() => { void quartoPreviewController.reconcile(quartoLiveActive); });
  $effect(() => { void calepinPreviewController.reconcile(calepinActive); });

  // The render options, applied from Settings: kept for this document, and
  // the live preview -- which reads them only as it starts -- restarted so
  // the page shows the new format rather than the old one until the next
  // reconnect. A preview still starting reads the options after its
  // workspace sync, so it picks them up on its own.
  async function applyRenderOptions(next) {
    quartoOptions = parseRenderOptions(next);
    saveRenderOptions(SLUG, quartoOptions);
    if (!quartoPreview || !quartoLiveActive) return;
    await stopLivePreview();
    void startLivePreview();
  }

  const historyController = createHistoryController({
    slug: SLUG,
    headers: () => keyHeaders(KEY),
    comments: () => comments,
    live: () => ({ session, text: docText }),
    viewing: () => viewing,
    sourceFormat: () => sourceFormat,
    mayEdit: () => mayEdit,
    editing: () => editing,
    onRedlines: () => applyRedlines(),
    onMerge: async (target, current) => {
      if (!target) {
        mergeTarget = null;
        return;
      }
      const component = (await import("./MergeEditor.svelte")).default;
      if (!current() || !mayEdit || !editing) return;
      MergeEditor = component;
      mergeTarget = target;
    },
  });
  let checkpoints = $derived(historyController.checkpoints);
  let historyNavigationProblem = $state("");
  let historyProblem = $derived(historyNavigationProblem || historyController.problem);
  let historyBaseline = $derived(historyController.baseline);
  let historyComparePoint = $derived(historyController.target);
  let historyChanges = $derived(historyController.changes);
  let historyChangedPaths = $derived(historyController.changedPaths);
  let historyRedlines = $derived(historyController.redlines);
  // The PDF viewer exposes the same text offsets as the HTML frame.
  const redlinesDisabledReason = "";
  function setHistoryRedlines(on) {
    historyController.setRedlines(Boolean(on) && !redlinesDisabledReason);
  }
  let fileDiff = $derived(historyController.fileDiff);
  $effect(() => () => historyController.dispose());
  // Kept here, not read off the pill, so Escape can stop dictation from
  // anywhere in the reader (SPEC-dictation.md 4.8) even while the pill has
  // not mounted yet or has scrolled out of view.
  let dictationSnapshot = $state({ state: "idle", progress: null, model: null, device: null, reason: null, speaking: false });
  $effect(() => getDictation().subscribe((value) => { dictationSnapshot = value; }));
  let navigationGeneration = 0;
  // Which checkpoint the reader arrived asking for, out of the link somebody
  // sent them. Read once, because after that the panel is where the answer is.
  const ARRIVED_AT = new URLSearchParams(location.search).get("at") || "";
  // And which file, when the landing page's search found the project by one
  // of its files. Honoured once the directory has arrived, and once only.
  const ARRIVED_FILE = new URLSearchParams(location.search).get("file") || "";
  let arrivedFileOpened = false;

  const loadHistory = () => {
    historyNavigationProblem = "";
    return historyController.load();
  };
  const chooseHistoryBaseline = (sha) => {
    historyNavigationProblem = "";
    return historyController.chooseBaseline(sha);
  };
  const chooseHistoryTarget = (sha) => {
    historyNavigationProblem = "";
    return historyController.chooseTarget(sha);
  };
  const computeHistoryChanges = (point) => historyController.computeChanges(point);

  // Stepping through the changes. A change is named by its offset into the
  // text the frame published -- the same offset its redlines were painted at
  // -- so finding it is asking the frame to scroll there. The frame has to
  // be showing the compare end of the range for the offset to mean anything;
  // when it is not, the step waits until it is.
  let pendingHistoryReveal = null;
  function revealPendingHistory() {
    const pending = pendingHistoryReveal;
    if (!pending || docText === null || (viewing?.sha || "") !== pending.sha) return;
    pendingHistoryReveal = null;
    tell({ type: "locate", start: pending.hunk.position, length: pending.hunk.length || (pending.hunk.insert || "").length });
  }

  async function revealHistoryHunk(hunk) {
    if (!hunk || typeof hunk.position !== "number") return;
    const sha = historyComparePoint?.sha || "";
    pendingHistoryReveal = { hunk, sha };
    if ((viewing?.sha || "") !== sha) {
      if (sha) await showCheckpoint(sha);
      else backToNow();
    } else {
      revealPendingHistory();
    }
  }

  // The timeline's two gestures. Clicking a row shows the document as it was
  // then, with the changes since the baseline painted into it: the row is
  // the compare end of the range. The row's "compare since" action makes it
  // the start instead. Either way the document pane and the range agree,
  // which is what lets the painted offsets be trusted.
  async function viewPoint(sha) {
    showMobileView("document");
    if (!sha) {
      backToNow();
      await chooseHistoryTarget("");
      return;
    }
    await showCheckpoint(sha);
    if ((viewing?.sha || "") !== sha) return;
    historyNavigationProblem = "";
    await historyController.compareTo(sha);
  }

  async function compareSince(sha) {
    await chooseHistoryBaseline(sha);
    // The baseline moved past the compare end, so the range now runs to the
    // live document, and the pane has to show the live document too.
    if (viewing && !historyComparePoint) backToNow();
  }

  async function restoreCheckpoint(sha) {
    if (!mayEdit || !sha) return;
    try {
      const response = await fetch(`/api/documents/${SLUG}/restore`, {
        method: "POST",
        headers: { ...SHELL_HEADERS, ...keyHeaders(KEY), "content-type": "application/json" },
        body: JSON.stringify({ sha }),
      });
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(payload.error || "that checkpoint could not be restored");
      mergeTarget = null;
      backToNow();
      await loadHistory();
    } catch (error) {
      toastProblem(error.message || "that checkpoint could not be restored");
    }
  }

  const openCheckpointFile = (point, path) => historyController.openCheckpointFile(point, path);
  const openFileDiff = (path) => historyController.openFileDiff(path);

  // A checkpoint as a renderer takes it. Its texts came with it; its figures
  // did not, because a figure is served immutably by its digest and the ones
  // this checkpoint used may well be the ones on the screen already.
  function checkpointTree(point) {
    const digests = {};
    for (const [path, file] of Object.entries(point.files || {})) {
      if (file.kind !== "text") digests[path] = file.sha;
    }
    return { main: point.main, texts: point.texts || {}, digests, files: point.files || {} };
  }

  async function showCheckpoint(sha) {
    const mine = ++navigationGeneration;
    historyController.invalidateChanges();
    renderingStore?.invalidate();
    issued += 1;
    dropHeldRendering();
    try {
      const point = await history.checkpoint(SLUG, sha, keyHeaders(KEY));
      if (mine !== navigationGeneration) return;
      viewing = point;
      write(`librepaper-history-baseline:${SLUG}`, sha);
      historyNavigationProblem = "";
    } catch (error) {
      if (mine !== navigationGeneration) return;
      historyNavigationProblem = error.message || "that checkpoint could not be read";
      return;
    }
    if (mine === navigationGeneration) await paintPreview();
  }

  function backToNow() {
    const wasCheckpoint = Boolean(viewing) || frameShowsCheckpoint;
    navigationGeneration += 1;
    renderingStore?.invalidate();
    issued += 1;
    dropHeldRendering();
    if (!wasCheckpoint) return;
    viewing = null;
    frameShowsCheckpoint = false;
    // A live HTML page is an active document, while a checkpoint was inert
    // HTML painted into the shell. Source equality cannot tell those states
    // apart, so leaving history always reloads the live page and reruns its
    // scripts.
    if (!editing && sourceFormat === "html") {
      navigateFrame(true);
    } else {
      void paintPreview();
    }
  }

  async function nameCheckpoint(sha, given) {
    try {
      await history.label(SLUG, sha, given, keyHeaders(KEY));
    } catch (error) {
      toastProblem(error.message || "that checkpoint could not be named");
      return;
    }
    await loadHistory();
    // The bar over the document says what it is showing by name, so a rename
    // of the checkpoint on the screen has to reach it too.
    if (viewing?.sha === sha) viewing = { ...viewing, label: given };
    // Naming a checkpoint is the editor saying "this one", so a rendering
    // waiting for the text to stay quiet is stored now rather than later.
    if (given) renderingStore?.flushHeld();
  }

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
  let went = $state({});
  let replacements = $state({});
  let passageTraceGeneration = 0;
  let lastPassageTrace = null;

  async function tracePassages() {
    if (viewing || docText === null || !session) return;
    const lost = comments.filter((comment) => comment.orphaned && !comment.region);
    const ids = lost.map((comment) => comment.id + ":" + comment.revision).join("|");
    if (lastPassageTrace?.source === sourceGeneration && lastPassageTrace?.visible === docText && lastPassageTrace?.ids === ids) return;
    const visible = docText;
    const source = sourceGeneration;
    lastPassageTrace = { source, visible, ids };
    const mine = ++passageTraceGeneration;
    const currentTree = session.tree();
    const nextWent = {};
    const nextReplacements = {};
    if (lost.length && !checkpoints.length) await loadHistory();
    const list = checkpoints;
    for (const comment of lost) {
      if (mine !== passageTraceGeneration || viewing || source !== sourceGeneration || visible !== docText) return;
      try {
        const point = await passages.wentAt(SLUG, comment, list, keyHeaders(KEY));
        if (point) nextWent[comment.id] = point;
        if (!comment.revision) continue;
        const oldText = comment.source
          ? await passages.sourceTextAt(SLUG, comment.revision, comment.source.path, keyHeaders(KEY))
          : await passages.textAt(SLUG, comment.revision, keyHeaders(KEY));
        const current = comment.source ? currentTree.texts[comment.source.path] ?? "" : visible;
        const replacement = await passages.replacementAt(oldText, current, comment.source || comment);
        if (replacement !== null) nextReplacements[comment.id] = replacement;
      } catch {
        // A missing checkpoint or rendering cannot establish a replacement.
        if (mine === passageTraceGeneration) lastPassageTrace = null;
      }
    }
    if (mine === passageTraceGeneration && !viewing && source === sourceGeneration && visible === docText) {
      went = nextWent;
      replacements = nextReplacements;
    }
  }

  // What the bar over the document calls what it is showing: the name somebody
  // gave the moment, or the digest, which is the name it has anyway.
  const viewingName = $derived(
    !viewing ? "" : viewing.label || history.shortSha(viewing.sha),
  );

  // The document as a renderer takes it: every text in it, the figures by
  // digest, and which file is the document. A compiler given only the main
  // file produces the error a reader would otherwise be shown.
  //
  // Two moments have no directory to give. A session that has not arrived yet
  // is empty, and a document the server has not migrated is still one text
  // under its old name. Both are answered the same way, and with the same
  // name the server would give them -- `main_path_for` in room.rs -- so that
  // what is rendered before the maps land and what is rendered after are the
  // same document under the same title.
  // The file manager always operates on the live directory. In particular,
  // its list remains live while the document pane is showing a checkpoint, so
  // downloads must use the same source as that list.
  function liveTreeNow() {
    if (!session) return { main: "", texts: {}, digests: {} };
    const tree = session.tree();
    if (tree.main) {
      // CodeMirror owns the active Yjs binding and exposes the text it is
      // displaying. During a local transaction its view can be one tick ahead
      // of the directory observer, so use that current source for snapshots
      // taken by the preview scheduler.
      const activeText = typeof editing !== "undefined" && editing && typeof editor !== "undefined" && editor?.text?.() != null && (!openFile || session.paths?.get?.(openFile) === tree.main)
        ? String(editor.text())
        : null;
      return activeText == null ? tree : { ...tree, texts: { ...tree.texts, [tree.main]: activeText } };
    }
    const named =
      { typst: "main.typ", markdown: "main.md", quarto: "main.qmd", html: "main.html", latex: "main.tex" }[sourceFormat] ||
      "main.txt";
    return {
      main: named,
      texts: { [named]: session.text.toString() },
      digests: {},
    };
  }

  function treeNow() {
    // A checkpoint picked out of the timeline is shown in the document pane in
    // place of the live text. Everything downstream -- the render, the frame,
    // the agent, the anchoring -- is the same as for the live document,
    // because to all of it a checkpoint is just another directory.
    if (viewing) return checkpointTree(viewing);
    return liveTreeNow();
  }

  // Painting the preview is sending it to the frame: the draft is a document,
  // and a document belongs on the documents origin, not in this page. The
  // agent republishes its text from there, which re-anchors every comment
  // against what was just typed.
  let issued = 0;
  let painted = 0;
  let sourceGeneration = 0;
  // Keep one render in flight and coalesce requests into the latest tree.
  // This bounds the worker queue while typing, and keeps each LaTeX PDF
  // tied to the snapshot digest of the tree that produced it.
  let previewPaintBusy = false;
  let previewPaintQueued = false;
  let previewTimer = null;

  // What the last compile said. When it is painted is `diagnostics.js`'s
  // rule, and the wait it counts is from the keystroke rather than from the
  // render that noticed the error, so a slow render does not add its own
  // length to it. From clean to red at reading speed, from red to clean at
  // typing speed.
  let diagnostics = $state([]);
  let renderDiagnostics = [];
  let bibliographyDiagnostics = [];
  const diagnosticPainter = diagnosticsRule.painter({
    paint: (list) => paintDiagnostics(list),
  });
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

  // A reader is told nothing. They cannot fix it, the author is looking at the
  // error at that moment, and a reader shown a red badge for a typo in
  // somebody else's editing session learns to stop reading while a document is
  // being worked on. What a reader gets instead is the last page that
  // compiled, which is what `everPainted` below keeps on the screen.
  function paintDiagnostics(list) {
    if (!editing) return;
    renderDiagnostics = list;
    paintCombinedDiagnostics();
  }

  function paintCombinedDiagnostics() {
    if (!editing) return;
    const seen = new Set();
    diagnostics = [...renderDiagnostics, ...bibliographyDiagnostics].filter((item) => {
      const key = JSON.stringify([item.file, item.line, item.column, item.message]);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
    editor?.setDiagnostics?.(diagnostics);
  }

  function bibliographyAnalyzed(result, request) {
    bibliographyDiagnostics = (result?.diagnostics || []).map((item) =>
      diagnosticContext(item, { main: request?.main || "", texts: request?.texts || {} }, ""));
    paintCombinedDiagnostics();
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
    viewing ? renderers.formatOf(viewing.main) || sourceFormat : sourceFormat,
  );
  const paintsTheFrame = $derived(Boolean(viewing) || editing || displayedFormat !== "html");

  /* -------------------------------------------------------------- LaTeX */

  // Paged source formats have no HTML to paint, so their frame is the PDF
  // viewer on the documents origin rather than the empty shell. Everything
  // else about the frame is the same: same origin, same CSP, same channel.
  const pdfOutput = $derived(renderers.producesPdf(displayedFormat));
  const framePath = $derived(renderers.outputKind(displayedFormat) === "pdf" ? "pdf" : "raw");

  // How long the last compile took, and whether one is running now. Paged
  // formats expose the same short-lived loading state; the elapsed time is
  // especially useful for LaTeX, whose compiler can take seconds.
  let compiling = $state(false);
  let lastCompile = $state(0);
  let pdfFailure = $state(false);
  // Why, when the Diagnostics list has nothing to say: the compiler's own
  // words when it threw, or the tail of its log when it produced neither a
  // PDF nor an error the parser could name. Empty when the list says it.
  let pdfFailureReason = $state("");

  // The project's LaTeX settings, mirrored into state because the Yjs `meta`
  // map they live in is not itself reactive: the Compiler settings need to redraw
  // when a settings change arrives from another collaborator, not only when
  // this browser writes one. Kept current by the `meta.observe` handler set
  // up by `configureLatex` when this project enters LaTeX mode.
  let latexSettingsState = $state({ engine: "auto", release: null });
  let latexObservedSession = null;
  let latexSettingsObserver = null;
  let latexConfiguration = 0;

  function stopLatex() {
    latexConfiguration += 1;
    if (latexObservedSession) {
      latexObservedSession.meta.unobserve(latexSettingsObserver);
      latex.cancel();
    }
    latexObservedSession = null;
    latexSettingsObserver = null;
  }

  function configureLatex(format) {
    if (format !== "latex" || !session) {
      stopLatex();
      return;
    }
    if (latexObservedSession === session) return;
    stopLatex();
    const active = session;
    const configuration = latexConfiguration;
    latexSettingsState = active.latexSettings();
    latex.configure({
      project: SLUG,
      settings: latexSettingsState,
      mayCompile: mayEdit && renderers.available("latex"),
    });
    latexObservedSession = active;
    latexSettingsObserver = (event) => {
      if (configuration !== latexConfiguration || session !== active) return;
      const changed = [...event.changes.keys.keys()];
      if (!changed.includes("latex.engine") && !changed.includes("latex.release")) return;
      latexSettingsState = active.latexSettings();
      latex.setSettings(latexSettingsState);
      void paintPreview();
    };
    active.meta.observe(latexSettingsObserver);
    // Only an editor pins the mirror's default into the shared project.
    // A response from an earlier format or session must not change this one.
    if (mayEdit && !latexSettingsState.release) {
      latex.releases().then((info) => {
        if (configuration === latexConfiguration && session === active && mayEdit
            && info?.default && !active.latexSettings().release) {
          active.setLatexSettings({ release: info.default });
        }
      }).catch(() => {
        // A compile reports an unavailable mirror through its normal status.
      });
    }
  }

  // The most recent LaTeX compile result -- success or failure -- kept whole
  // for Diagnostics' "Compiled with" block and "Earlier attempts" list
  // (docs/specs/latex-compiler.md: "Preserve both attempts' logs when a browser failure
  // led to a local attempt."). Null for every other format.
  let lastLatexResult = $state(null);

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
  // below), and the loading itself is what the status line under the toolbar
  // reports. Everybody else -- a reader, or anyone on a deployment with no
  // mirror -- is shown what the server kept.
  const compilesHere = $derived(
    editing && mayEdit && (
      sourceFormat === "typst"
        ? renderers.compilerAvailable("typst")
      : sourceFormat === "latex" && renderers.compilerAvailable("latex")
    ),
  );
  const unrendered = $derived(pdfOutput && !compilesHere && !everPaintedShown);
  const failedBeforeRender = $derived(pdfOutput && compilesHere && pdfFailure && !everPaintedShown);

  // A paged compile that is running says so, and says how long the last one
  // took once there has been one. Before the first, there is no honest number
  // to give. LaTeX has its own, richer status line -- `LatexStatus.svelte`,
  // fed straight from `latex.subscribe` -- so this badge is Typst's alone.
  const compileBadge = $derived(
    !compiling ? "" : lastCompile ? `compiling… (last took ${lastCompile.toFixed(1)}s)` : "compiling…",
  );

  // Whether `LatexStatus` has anything to draw. It draws nothing while the
  // engine is idle, and the status row must know that, or a LaTeX document
  // would wear an empty strip under the toolbar until its first compile.
  let latexPhase = $state(latex.status().phase);
  $effect(() => latex.subscribe((next) => (latexPhase = next.phase)));

  // Quarto preview mode chosen, but not yet paired with the local app on
  // this computer: the pane shows the draft, and the status row carries the
  // one-line explanation and a way to connect.
  const quartoNeedsLocalApp = $derived(
    sourceFormat === "quarto" && quartoPreviewMode === "quarto" && mayEdit && !viewing &&
      localAppStatus.state !== "connected",
  );

  // Whether the status row under the toolbar has a reason to exist.
  const statusRow = $derived(Boolean(
    connectionNote
      || renderedNote
      || (editing && (peers > 1 || (sourceFormat === "latex" ? latexPhase !== "idle" : compileBadge)))
      || quartoRendering || quartoNeedsLocalApp,
  ));

  let frameShowsCheckpoint = false;
  // The kind of the payload the frame was last handed -- "pdf", "html", or
  // "" since the last navigation. `framePreview.preview()` knows the same,
  // but not reactively, and the File menu's download items follow this.
  let deliveredKind = $state("");
  framePreview = createFramePreview({
    slug: SLUG,
    getDocsOrigin: () => docsOrigin,
    framePath: () => framePath,
    api: previewApi,
    setSource: (source) => (frameSrc = source),
    send: tell,
    onNavigate: () => {
      frameReady = false;
      renderingStore?.invalidate();
      issued += 1;
      renderedSha = null;
      deliveredKind = "";
      lastRegions = lastHighlight = null;
    },
    onDelivered: (payload) => {
      deliveredKind = payload.kind;
      if (payload.kind === "pdf") renderedSha = payload.sha || null;
      frameShowsCheckpoint = Boolean(viewing);
      everPainted = true;
      everPaintedShown = true;
    },
  });

  const navigateFrame = (force = false) => framePreview.navigate(force);
  const replayPreview = () => paintsTheFrame && framePreview.replay();

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

  // What a paged document's frame is showing: the checkpoint the stored
  // rendering was compiled from, when that checkpoint was taken, and whether
  // it is the text as it stands. Null until the server has been asked.
  let rendering = $state(null);
  // The SHA whose bytes are in the frame, so a poll that finds the same
  // rendering costs one small request rather than a PDF.
  let renderedSha = null;

  // The line under the badge for a LaTeX document, and the whole of what makes
  // storing a derived thing honest. A rendering is named by the digest of the
  // source it was compiled from, so there are exactly three things to say: it
  // is the text as it stands, it is older than the text and here is when
  // (with what compiled it, when the server kept that), or nobody has
  // rendered this yet. The wording for the last two comes from
  // `renderedNoteText`, shared with checks/latex-reader.mjs.
  const renderedNote = $derived(
    !pdfOutput || (compilesHere && !viewing)
      ? ""
      : !rendering
        ? "not yet rendered"
        : renderedNoteText(rendering),
  );

  renderingStore = createRenderingStore({
    api: previewApi,
    getViewing: () => viewing,
    getSourceGeneration: () => sourceGeneration,
    getNavigationGeneration: () => navigationGeneration,
    getRenderedSha: () => renderedSha,
    getPreview: () => framePreview.preview(),
    deliver: (payload) => framePreview.publish(payload),
    onRendering: (value) => (rendering = value),
    onMissing: (value) => {
      const hadPages = Boolean(framePreview.preview() || renderedSha || everPaintedShown);
      rendering = value;
      framePreview.clear();
      renderedSha = null;
      frameShowsCheckpoint = false;
      everPainted = false;
      everPaintedShown = false;
      if (hadPages) navigateFrame(true);
    },
    schedulePreview: () => void paintPreview(),
  });

  const paintRendering = () => renderingStore.paint();
  const holdRendering = (...args) => renderingStore.hold(...args);
  const dropHeldRendering = () => renderingStore.dropHeld();

  async function paintPreview() {
    if (readerDisposed) return;
    clearTimeout(previewTimer);
    previewTimer = null;
    renderingStore.cancelPoll();
    // The live preview owns the pane while it is running (or starting): the
    // frame shows Quarto's own page, kept current by `syncQuartoLive` and
    // the page poller, not by anything painted here.
    // `typeof` rather than a bare read: checks/reader-races.mjs runs this
    // function's body in isolation against a context that does not declare
    // these names for its non-Quarto (and some Quarto) cases.
    // Keyed to a preview actually running or starting, not to whether this
    // browser is paired: a browser whose preview failed to start falls
    // through to the draft below, rather than leaving the pane at whatever
    // it showed last.
    if (sourceFormat === "quarto" && typeof quartoLiveActive !== "undefined" && quartoLiveActive &&
        typeof quartoPreview !== "undefined" && (quartoPreview || quartoPreviewStarting)) {
      return;
    }
    // Not live: nothing rendered is ever uploaded, so there is no shared
    // bundle to fall back to -- a Quarto document not showing its own live
    // preview shows this browser's Markdown draft, the same as every other
    // draft format, painted below.
    // A paged document is compiled in an editor's browser and nowhere else,
    // so everybody else is shown the PDF the server kept from the last one
    // who did. See `docs/specs/latex.md`.
    const outputIsPdf = pdfOutput;
    if (outputIsPdf && (Boolean(viewing) || !compilesHere)) {
      await paintRendering();
      return;
    }
    // An editor's first look at a document somebody has already rendered is
    // that rendering, painted before the compile that will replace it: a
    // compile takes seconds and its first attempt can fail, and a pane that
    // says nothing until then is worse than the pages that already exist.
    // Only while nothing has been painted; after that the last page that
    // compiled stays up, as the spec says. The poll paintRendering leaves
    // behind is for a browser that waits on somebody else's compile, and
    // this one compiles for itself.
    if (outputIsPdf && !everPainted && !viewing) {
      await paintRendering();
      if (readerDisposed) return;
      renderingStore.cancelPoll();
    }
    if (!paintsTheFrame) {
      refreshFramedPage();
      return;
    }
    const mine = ++issued;
    const tree = treeNow();
    const snapshotViewing = viewing;
    const snapshotNavigation = navigationGeneration;
    const snapshotSource = sourceGeneration;
    const format = renderers.formatOf(tree.main);
    const paged = renderers.producesPdf(format);
    const slow = format === "latex";
    if (previewPaintBusy) {
      previewPaintQueued = true;
      return;
    }
    previewPaintBusy = true;
    try {
      // The figures, if this document has any. A figure not yet here is
      // awaited before the first compile that needs it, and the page that is
      // already up stays up meanwhile: rendering without them would produce a
      // document with holes in it and replace it a moment later, which reads
      // as a flicker rather than as progress.
      if (Object.keys(tree.digests || {}).length) {
        const held = await figures.gather(SLUG, tree.digests, { ...SHELL_HEADERS, ...keyHeaders(KEY) });
        if (
          mine <= painted ||
          snapshotNavigation !== navigationGeneration ||
          ((slow || format === "quarto") && snapshotSource !== sourceGeneration) ||
          tree.main !== treeNow().main
        ) return;
        if (paged) {
          const missing = Object.keys(tree.digests).filter(
            (path) => !Object.prototype.hasOwnProperty.call(held.assets, path),
          );
          if (missing.length) {
            throw new Error(`could not fetch figure${missing.length === 1 ? "" : "s"}: ${missing.join(", ")}`);
          }
        }
        tree.assets = held.assets;
        // Authored figure URLs and cached Quarto output assets are both part
        // of this render. Keep both inventories when the figure collector
        // returns its authenticated blob URLs.
        tree.urls = { ...(tree.urls || {}), ...held.urls };
      }
      // A LaTeX compile takes seconds rather than milliseconds, so the pane
      // says one is running. The last page that compiled stays up under it:
      // an author who is typing has something to look at, which is the whole
      // difference between this and a pane that blanks for four seconds.
      if (paged) {
        compiling = true;
        if (!everPainted) pdfFailure = false;
      }
      // What a rendering compiled now will be stored as. Asked before the
      // compile rather than after, because a compile takes seconds and the
      // text may move meanwhile: what comes out is of the text as it was, and
      // a name the text has moved past is refused by the server.
      // Checkpoint SHAs are already canonical server tree digests. For live
      // text, hash this exact immutable tree, after asset bytes have arrived
      // so asset sizes agree with the server's TreeEntry values.
      const renderingName = snapshotViewing?.sha || (await snapshotDigest(tree, tree.assets || {}));
      if (readerDisposed) return;
      // A manual compile is asked for once; the flag is read here, at the
      // one call site that reaches the compiler, and cleared immediately so
      // it cannot linger onto an edit's ordinary debounced compile.
      // `typeof` rather than a bare read: checks/reader-races.mjs runs this
      // function's body in isolation, pulled out of the component by a text
      // marker, against a context that supplies only the variables each test
      // needs -- `manualCompile` among them only here, where it is read, not
      // there. `typeof` is the one operator that does not throw on a name a
      // context never declared; the assignment below is unconditional
      // because assigning an undeclared name is not an error.
      const manual = typeof manualCompile === "boolean" && manualCompile;
      manualCompile = false;
      let rendered;
      try {
        const title = await headingOf(tree);
        if (readerDisposed) return;
        rendered = await renderers.render(tree, title, manual ? { manual: true } : undefined);
      } finally {
        // Only the newest compile owns the badge. An older one finishing
        // afterwards must not turn the spinner off under a newer one.
        if (paged && mine > painted) compiling = false;
      }
      if (readerDisposed) return;
      if (format === "latex") lastLatexResult = rendered;
      const { html, pdf, synctex, diagnostics: said, seconds, log, provenance } = rendered;
      const contextualDiagnostics = (said || []).map((item) => diagnosticContext(item, tree, renderingName));
      // An in-flight preview may finish after another keystroke: HTML and
      // Typst may show that intermediate progress while the queued render
      // catches up. Navigation and main-file changes still invalidate it;
      // LaTeX keeps its strict source guard.
      if (
        mine <= painted ||
        snapshotNavigation !== navigationGeneration ||
        ((slow || format === "quarto") && snapshotSource !== sourceGeneration) ||
        tree.main !== treeNow().main
      ) return;
      painted = mine;
      if (paged && seconds) lastCompile = seconds;
      // A render carries `html` or `pdf`, and the reader posts whichever it
      // has. The bytes are transferred rather than copied: a PDF is megabytes
      // and this page has no further use for it once the frame has it.
      if (pdf) {
        pdfFailure = false;
        pdfFailureReason = "";
        const buffer = pdf.buffer ? pdf.buffer.slice(pdf.byteOffset, pdf.byteOffset + pdf.byteLength) : pdf;
        const preview = { kind: "pdf", sha: renderingName, bytes: new Uint8Array(buffer.slice(0)) };
        // Held for the readers, from a copy: the hand-over to the frame below
        // empties this page's own.
        if (renderingName && snapshotSource === sourceGeneration) {
          holdRendering(
            renderingName,
            buffer.slice(0),
            synctex,
            !snapshotViewing && snapshotNavigation === navigationGeneration,
            provenance || null,
          );
        }
        framePreview.publish(preview);
        if (snapshotSource === sourceGeneration) {
          diagnosticPainter.rendered({ page: "", diagnostics: contextualDiagnostics });
        }
        return;
      }
      if (typeof html === "string") {
        // The page is what the document says now, so every error said about an
        // earlier state of it is cleared at once. The warnings that came with
        // this page are painted on the same slow schedule the errors are, so
        // that a font name half typed does not flash a badge on every
        // keystroke.
        framePreview.publish({ kind: "html", html });
        if (snapshotSource === sourceGeneration) {
          diagnosticPainter.rendered({ page: html, diagnostics: contextualDiagnostics });
        }
        return;
      }
      // No page: the last one that compiled stays up, and what is said is that
      // it does not compile now, and where -- once the typing has stopped.
      if (snapshotSource !== sourceGeneration) return;
      diagnosticPainter.rendered({ page: null, diagnostics: contextualDiagnostics });
      if (paged) {
        pdfFailure = true;
        // A log the parser found nothing in is still the only account there
        // is of what happened, and its last lines are where an engine says
        // why it stopped.
        pdfFailureReason = said?.length
          ? ""
          : rendered.failure?.message || (log || "").trim().split("\n").slice(-12).join("\n") ||
            "the compiler produced no PDF and no log";
        if (pdfFailureReason) console.error("latex: could not render:", pdfFailureReason);
      }
      // Unless nothing was ever painted, which is what someone who opens the
      // editor on a document that does not compile sees. Then the frame shows
      // the engine's page saying so, with the list on it, styled like a
      // document rather than like a crash.
      if (!everPainted) {
        const page = await renderers
          .failurePage(await headingOf(tree), format)
          .catch(() => null);
        if (!readerDisposed && page && mine >= painted && snapshotNavigation === navigationGeneration) tell({ type: "preview", html: page });
      }
    } catch (error) {
      // Not a document that did not compile: a renderer that could not be
      // fetched, which is this page's problem rather than the author's.
      const currentSnapshot = snapshotNavigation === navigationGeneration &&
        (!paged || snapshotSource === sourceGeneration);
      if (mine > painted && currentSnapshot) {
        if (paged) {
          pdfFailure = true;
          pdfFailureReason = error.message || "could not render";
          console.error("latex: could not render:", error);
        }
        say(error.message || "could not render", true);
      }
    } finally {
      previewPaintBusy = false;
      if (previewPaintQueued) {
        previewPaintQueued = false;
        void paintPreview();
      }
    }
  }

  // Editors refresh at a bounded cadence even during continuous typing.
  // Readers wait for a pause so they do not see every half-written word.
  const READER_DEBOUNCE = 1000;

  function sourceChanged() {
    if (readerDisposed) return;
    sourceGeneration += 1;
    if (typeof quartoLiveActive !== "undefined" && quartoLiveActive && typeof quartoPreview !== "undefined" && quartoPreview) {
      clearTimeout(quartoLiveSyncTimer);
      quartoLiveSyncTimer = setTimeout(() => void syncQuartoLive(), 500);
    }
    if (sourceFormat === "quarto" && session) {
      const main = session.mainPath() || "main.qmd";
      const parsed = quarto.parseQuarto(session.textOf(session.mainId())?.toString?.() || session.text.toString(), { path: main });
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
    const outputIsPdf = pdfOutput;
    // The keystroke, which is what the diagnostic wait is measured from.
    diagnosticPainter.typed();
    // Typst has a bounded 300 ms preview cadence like the other fast source
    // formats. Only LaTeX owns its longer compiler debounce; postponing this
    // timer for every Typst keystroke would starve the PDF indefinitely.
    if (editing && sourceFormat !== "latex" && previewTimer !== null) return;
    clearTimeout(previewTimer);
    if (outputIsPdf && !compilesHere) {
      // The text has moved, so what is in the frame is a rendering of an
      // earlier version. That is known here rather than asked: the rendering
      // is named by the digest of the source it was compiled from.
      if (rendering?.current) rendering = { ...rendering, current: false };
      renderingStore.schedulePoll();
      return;
    }
    // A rendering waiting for the text to stay quiet is of a text that did
    // not.
    if (outputIsPdf) dropHeldRendering();
    // A LaTeX compile takes seconds, so it waits for the source to be quiet
    // for longer -- `latex.DEBOUNCE`, which is that module's number and not
    // one written twice. A reader watching somebody else type waits longer
    // still, and the longer of the two wins.
    const wait =
      sourceFormat === "latex"
        ? Math.max(latex.DEBOUNCE, editing ? 0 : READER_DEBOUNCE)
        : editing
          ? 300
          : READER_DEBOUNCE;
    previewTimer = setTimeout(paintPreview, wait);
  }

  /* ------------------------------------------------------- keeping in step */

  // Said when the lock has nowhere to go: the words at the caret, and the words
  // around them, are in neither the document nor the source. That is rare now
  // that it looks beside the line as well as at it -- a formula, a blank line
  // and a fenced block all resolve to the prose next to them -- so when it does
  // happen it is worth one plain line rather than an alarm. Nothing is broken:
  // the lock is on and the next move will try again.
  const NO_MATCH = "nothing to jump to here";

  function lost(yes) {
    if (!yes) {
      toastUnsay(`reader:${NO_MATCH}`);
      return;
    }
    // Not a problem: the editor is in the state it was in, and the reader has
    // lost nothing. It is a fact about where the caret happens to be.
    say(NO_MATCH);
  }

  let stepTimer = null;
  function followCaret() {
    if (!linked || !editing || docText === null) return;
    clearTimeout(stepTimer);
    stepTimer = setTimeout(() => {
      // The format of the file being edited, which is not always the
      // document's: a .bib beside a .tex has comments of its own kind, and
      // stripping .tex comments out of it would blank the wrong runs.
      const format = renderers.formatOf(session?.paths?.get(openFile) || "") || sourceFormat;
      const place = sync.documentPlaceFor(editor.text(), editor.caret(), docText, format);
      if (place) {
        tell({ type: "locate", start: place.at, length: place.length });
        lost(false);
        return;
      }
      // The words at the caret are not findable in the document: a formula, a
      // table cell, a heading that renders as something else. Said rather than
      // ignored, because a lock that silently does nothing is
      // indistinguishable from one that is broken.
      lost(true);
    }, 120);
  }

  function followDocumentClick(offset) {
    if (!linked || !editing || docText === null || !editor) return;
    // The words clicked in the document may belong to any file: a reader
    // clicking a paragraph of chapter three is asking for chapter three, not
    // for the file that happens to be on screen. So the whole directory is
    // searched, and the file the words are in is opened.
    const tree = treeNow();
    const found = sync.sourcePlaceInTree(docText, offset, tree, {
      open: session?.paths?.get(openFile) || "",
      formatOf: renderers.formatOf,
    });
    if (!found) {
      lost(true);
      return;
    }
    lost(false);
    const id = session.idOf(found.path);
    if (id) {
      openFile = id;
      editor.goToIn(id, found.at);
    } else {
      editor.goTo(found.at);
    }
  }

  function setLinked(on) {
    linked = on;
    write(LINKED, on);
  }

  /* ------------------------------------------------------------------ panes */

  // How the window is divided, and which side the source is on. Both are one
  // reader's habit rather than anything about a document, so both are
  // remembered and an editor reopened lands where they left it.
  let layout = $state(LAYOUTS.includes(read(LAYOUT, "split")) ? read(LAYOUT, "split") : "split");
  let sourceSide = $state(read(SOURCE_SIDE, "left") === "right" ? "right" : "left");
  // Which keys the editor answers to. A preference of the person at this
  // browser, not of the document, and nobody's default but their own; set
  // from the Settings panel, beside the rest of this browser's preferences.
  let keys = $state(["vim", "emacs"].includes(read(KEYMAP, "default")) ? read(KEYMAP, "default") : "default");

  // The column at the left, and what is in it: the files, the comments or the
  // history, or "" for closed. One value rather than a switch per panel,
  // because the column shows one thing at a time. An editor's first visit
  // opens on the files -- the shape of the project is what a project space
  // starts with -- and every visit after that opens where they left it.
  //
  // Somebody who came by a read or a comment link is shown the document and
  // its comments first. History is available to compare review rounds; files
  // and editor settings remain in the editor workspace.
  const TABS = [
    { id: "files", says: "Files", editorOnly: true },
    { id: "collaboration", says: "Collaboration" },
    { id: "changes", says: "Changes", editorOnly: false },
    { id: "agent", says: "Agent" },
    // History is readable by link-holders too: reviewers need the “since”
    // view even when they cannot edit or restore the live source.
    { id: "history", says: "History" },
    { id: "diagnostics", says: "Diagnostics", editOnly: true },
    { id: "share", says: "Share", sharingOnly: true },
  ];
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
  const PANELS = ["", ...TABS.map((tab) => tab.id)];
  const storedPanel = read(PANEL, null);
  const migratedPanel = storedPanel === "comments" || storedPanel === "chat" ? "collaboration" : storedPanel;
  let panel = $state(PANELS.includes(migratedPanel) ? migratedPanel : "files");
  let collaborationTab = $state(storedPanel === "chat" ? "chat" : "comments");
  const chatVisible = $derived(panel === "collaboration" && collaborationTab === "chat" && shown.comments);
  $effect(() => { if (chatVisible) unreadChat = false; });
  // Mount panels on their first visit and retain them across view changes.
  // This preserves scroll positions, expanded folders and unsent chat drafts.
  let visitedPanels = $state([]);
  let workspaceBannerHeight = $state(0);
  $effect(() => {
    if (settled && shown.comments && panel && !visitedPanels.includes(panel)) visitedPanels = [...visitedPanels, panel];
  });
  // Narrow screens show one workspace view at a time. This is independent of
  // the desktop split, so widening the window restores the reader's layout.
  const MOBILE_VIEW = "librepaper-mobile-view";
  let mobileView = $state(["document", "source", "sidebar"].includes(read(MOBILE_VIEW, "document"))
    ? read(MOBILE_VIEW, "document") : "document");
  let preferredPane = $state(read(LAYOUT, "split") === "source" ? "source" : "document");
  function showMobileView(view) {
    if (view === "source" && !editing) view = "document";
    mobileView = view;
    if (view !== "sidebar") preferredPane = view;
    if (!compact && view !== "sidebar" && layout !== "split") {
      layout = view;
      write(LAYOUT, layout);
    }
    write(MOBILE_VIEW, view);
    if (view === "sidebar" && !panel) showPanel(home);
  }
  function selectPanel(name) {
    const next = panel === name && (!compact || activeMobileView === "sidebar") ? "" : name;
    showPanel(next);
    if (width <= 760) showMobileView(next ? "sidebar" : "document");
  }
  // The tabs this browser is offered. `editorOnly` waits on the role the
  // document answers with; `editOnly` on the source pane being open.
  const tabs = $derived(
    TABS.filter(
      (tab) =>
        (!tab.editorOnly || mayEdit) && (!tab.editOnly || editing) && (!tab.sharingOnly || canSeeSharing),
    ),
  );
  // Where the column goes back to when what it showed is taken away: the
  // files for an editor, the comments for everybody else.
  const home = $derived(mayEdit ? "files" : "collaboration");
  // Whether the document has said who this browser is. Until it has, the
  // column is drawn empty rather than as one audience's and then the other's.
  let settled = $state(false);

  // Showing a panel; "" closes the column. Leaving the timeline is leaving it:
  // what the document pane shows goes back to the text as it stands, because
  // a page nobody can see the history behind is a page with no way back.
  // Opening it is what fetches the manifest.
  function showPanel(name, remembered = true) {
    if (panel === "history" && name !== "history") {
      const hadCheckpoint = Boolean(viewing);
      backToNow();
      if (!hadCheckpoint) navigationGeneration += 1;
    }
    panel = name;
    if (compact) mobileView = name ? "sidebar" : "document";
    if (remembered) write(PANEL, name);
    // Leaving the history panel clears whatever redlines were painted; the
    // history panel itself is what `applyRedlines` reads to decide that.
    applyRedlines();
    return name === "history" ? loadHistory() : Promise.resolve();
  }

  // The source and the document are kept as a share of what they have between
  // them; the comment column is kept in pixels. Two units because they are two
  // different kinds of pane: half a window stays half when the window changes,
  // and a comment card wants the same readable width whatever the screen is.
  let sizes = $state({
    [PANES.editor.key]: stored(PANES.editor),
    [PANES.sidebar.key]: stored(PANES.sidebar),
  });
  let guide = $state({ shown: false, left: 0, held: false });
  let grabbing = $state(false);
  // The width the panes are divided out of. Bound rather than read when
  // something happens to ask: it is what every size below is measured against,
  // and a measurement taken once is a layout that is right until the window
  // moves.
  let width = $state(innerWidth);
  const compact = $derived(width <= 760);
  const activeMobileView = $derived(mobileView === "source" && !editing ? "document"
    : mobileView === "sidebar" && !panel ? "document" : mobileView);
  const splitTight = $derived(width < PANES.editor.min + DOCUMENT_MIN + GRIP
    + (panel ? PANES.sidebar.min + GRIP : ACTIVITY_WIDTH));
  const effectiveLayout = $derived(compact
    ? (activeMobileView === "source" ? "source" : "document")
    : layout === "split" && splitTight ? preferredPane : layout);

  // What every measurement below is made against.
  // The column holds one of three things -- the files, the comments or the
  // timeline -- so what the layout needs to know is whether it is there, not
  // which of them is in it.
  const panes = $derived({
    layout: effectiveLayout,
    comments: Boolean(panel),
    editing,
    sourceSide,
    sizes,
    width,
  });
  const shown = $derived(compact ? {
    source: editing && activeMobileView === "source",
    document: activeMobileView === "document",
    comments: activeMobileView === "sidebar",
  } : showing(panes));

  // What the reader asked for is kept; what fits is worked out again every
  // time it is needed. Writing the fitted size back would make a narrow window
  // permanent -- drag the window in and the split is squeezed, drag it out and
  // it stays squeezed, because what was asked for is gone.
  function setSize(pane, size) {
    sizes[pane.key] = clamp(pane, panes, size);
    remember(pane, sizes[pane.key]);
  }

  // The three arrangements, in the order the button walks through them. The
  // icon is the one it is in rather than the one it is going to: the button is
  // as much a statement of where you are as a way of leaving.
  const ARRANGEMENTS = {
    split: { icon: "columns-2", says: "Source and document", next: "source" },
    source: { icon: "panel-left", says: "Source only", next: "document" },
    document: { icon: "file-text", says: "Document only", next: "split" },
  };

  function cycleLayout() {
    layout = ARRANGEMENTS[layout].next;
    write(LAYOUT, layout);
  }

  function putSourceOn(side) {
    sourceSide = side;
    write(SOURCE_SIDE, side);
  }

  function setKeys(next) {
    keys = next;
    write(KEYMAP, next);
  }

  // What ":q" in Vim mode asks for: the document alone, set directly rather
  // than reached by cycling, and remembered like any other choice of layout.
  function showDocumentAlone() {
    layout = "document";
    write(LAYOUT, layout);
  }

  // Everything the layout menu offers, named by what was chosen. The menu
  // reports the value of the line rather than each line calling back, so this
  // is the one place those names are read.
  function chose(what) {
    if (what.startsWith("layout-")) {
      layout = what.slice(7);
      write(LAYOUT, layout);
      if (compact) showMobileView(layout === "source" ? "source" : "document");
      return;
    }
    if (what === "side-left" || what === "side-right") return putSourceOn(what.slice(5));
    if (what.startsWith("ratio-")) return setSize(PANES.editor, Number(what.slice(6)));
    if (what === "linked") return setLinked(!linked);
  }

  function chooseToolCommand(value) {
    if (value === "settings") return openSettings();
    if (value === "compile") return compileNow();
    if (value === "preview-markdown") return void setQuartoPreviewMode("markdown");
    if (value === "preview-quarto") return void setQuartoPreviewMode("quarto");
  }

  // The File menu. Its first three items are what the Files panel's toolbar
  // does, reached without first switching layouts and opening the panel; the
  // rest open the panels a person looks for under File. The Files panel is
  // mounted on its first visit and draws the name field it focuses, so the
  // panel is opened and the DOM given a turn before the panel is asked.
  const FILE_COMMANDS = ["new-file", "new-folder", "upload", "download-pdf", "download-html", "download", "share", "history"];
  async function chooseFileCommand(value) {
    if (value === "download") return downloadTree();
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
    rendering,
    displayedFormat,
  }));

  // The rendering on screen, as a file. A PDF is the bytes the frame was
  // handed, or the stored rendering the server holds when this browser has
  // not been handed one yet. An HTML page is the one this browser painted,
  // with its figures written in, or for an authored HTML document the text
  // itself, which is the rendering.
  async function downloadRendering(kind) {
    try {
      if (kind === "pdf") {
        const preview = framePreview.preview();
        let bytes = preview?.kind === "pdf" ? preview.bytes : null;
        if (!bytes) {
          const found = rendering?.sha ? rendering : await previewApi.latest().then((r) => (r.ok ? r.json() : null)).catch(() => null);
          if (!found?.sha) throw new Error("This document has not been rendered yet.");
          const response = await previewApi.rendering(found.sha).catch(() => null);
          if (!response?.ok) throw new Error("The rendering could not be fetched.");
          bytes = new Uint8Array(await response.arrayBuffer());
        }
        saveBlob(new Blob([bytes], { type: "application/pdf" }), `${SLUG}.pdf`);
        return;
      }
      let html = null;
      if (displayedFormat === "html") {
        const tree = treeNow();
        html = tree.texts?.[tree.main] ?? null;
      } else {
        const preview = framePreview.preview();
        html = preview?.kind === "html" ? await inlineBlobUrls(preview.html) : null;
      }
      if (typeof html !== "string") throw new Error("This document has not been rendered yet.");
      saveBlob(new Blob([html], { type: "text/html" }), `${SLUG}.html`);
    } catch (error) {
      say(error.message || "Could not download the rendering.", true);
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

  // The one menu a narrow screen has stands in for all of them.
  function chooseCompactCommand(value) {
    if (FILE_COMMANDS.includes(value)) return void chooseFileCommand(value);
    if (value.startsWith("layout-") || value.startsWith("side-") || value.startsWith("ratio-") || value === "linked") return chose(value);
    return chooseToolCommand(value);
  }

  /* ------------------------------------------------------------------- boot */

  /* ------------------------------------------------------------- the files */

  // The directory as the list shows it, and where everyone's caret is. Held
  // as state rather than derived, because what they are derived from is a
  // CRDT that changes outside Svelte's knowledge.
  let files = $state([]);
  let folders = $state([]);
  let openFile = $state("");
  let handledFileTransactions = new WeakSet();
  const toolbarPath = $derived(files.find((file) => file.id === openFile)?.path || "");
  let peersByFile = $state(new Map());
  // The deployment's rules, which say what a path may be and what may sit at
  // one. Fetched rather than compiled in, so a deployment that widens its
  // extension lists widens them here too.
  let rules = $state({});
  loadConfig()
    .then((answer) => (rules = answer || {}))
    .catch(() => {
      /* the server checks every path again; this only explains it sooner */
    });

  // Reading the directory into the list. Deliberately does not paint: the
  // first call happens while the session is still being joined, before the
  // frame has even navigated to the documents origin, and a paint sent then
  // is a postMessage to a window that is not there yet. What paints is a
  // *change* -- `filesChanged` below -- and the first paint of all is the one
  // the arriving text triggers, as it always was.
  function refreshFiles() {
    if (!session) return;
    const previousFormat = sourceFormat;
    const previousFigure = shownFigure;
    const previousFiles = files;
    files = session.list();
    folders = session.folders();
    if (previousFigure) {
      const moved = files.filter((file) => file.kind === "asset" && file.sha === previousFigure.sha
        && !previousFiles.some((previous) => previous.path === file.path));
      const current = files.find((file) => file.kind === "asset" && file.path === previousFigure.path)
        || (moved.length === 1 ? moved[0] : null);
      shownFigure = current || null;
      if (current && openFile === previousFigure.id) openFile = current.id;
    }
    // A file that went away under this browser -- somebody else deleted it --
    // leaves the editor showing something that is not there any more, so it
    // falls back to the document itself.
    if (openFile && !files.some((file) => file.id === openFile)) openFile = "";
    if (!openFile) openFile = session.mainId();
    // The file the link named, if the project has one by that name. An
    // editor's source pane opens on it; a reader has no pane to open it in
    // and no list to mark it in, so for them the link is to the document.
    if (ARRIVED_FILE && mayEdit && !arrivedFileOpened && files.length) {
      arrivedFileOpened = true;
      const named = files.find((file) => file.path === ARRIVED_FILE);
      if (named) openTheFile(named);
    }
    // The main file's name is the document's format, and it can change: a
    // document whose main file becomes a .typ is a typst document from that
    // moment.
    const format = renderers.formatOf(session.mainPath());
    if (format && format !== sourceFormat) {
      sourceFormat = format;
      configureLatex(format);
      if (mayEdit) renderers.warm(format);
      // A main-file rename can keep the same output kind (Typst -> LaTeX is
      // still PDF), so framePath alone is not enough to invalidate the old
      // page. Drop all replayable state before the new format gets a chance to
      // render, and reload the viewer even when both formats use pdf.js.
      if (previousFormat && previousFormat !== format) {
        navigationGeneration += 1;
        renderingStore?.reset();
        issued += 1;
        dropHeldRendering();
        framePreview.clear();
        rendering = null;
        renderedSha = null;
        frameShowsCheckpoint = false;
        everPainted = false;
        everPaintedShown = false;
        pdfFailure = false;
        pdfFailureReason = "";
        if (docsOrigin) navigateFrame(true);
      }
    }
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
    // Nested text edits change the preview, but not the file list.
    if (!Array.isArray(events) || events.some((event) => event.target === active.files)) refreshFiles();
    if (needsSourceRefresh(events, active.text, handledFileTransactions)) sourceChanged();
  }

  function refreshPeers() {
    if (session) peersByFile = session.whereEveryoneIs();
  }

  function openTheFile(file) {
    // A figure has no editor: choosing one shows it. The id of an asset is
    // its path, since its bytes are not in the shared document and there is
    // nothing else to key it by.
    openFile = file.id;
    shownFigure = file.kind === "asset" ? file : null;
    // Choosing a file is asking to see it, so an arrangement with no source
    // pane makes room for one. Remembered like any other choice of layout.
    if (mayEdit && !compact && layout === "document") {
      layout = "split";
      write(LAYOUT, layout);
    }
    if (mayEdit) showMobileView("source");
  }

  // The figure being looked at, when the chosen file is one. Held rather than
  // derived because the bytes it needs are fetched.
  let shownFigure = $state(null);
  let figureUrl = $state("");
  $effect(() => {
    const wanted = shownFigure;
    if (!wanted) {
      figureUrl = "";
      return;
    }
    figures
      .gather(SLUG, { [wanted.path]: wanted.sha }, { ...SHELL_HEADERS, ...keyHeaders(KEY) })
      .then((held) => {
        if (shownFigure === wanted) figureUrl = held.urls[wanted.path] || "";
      })
      .catch(() => {
        if (shownFigure === wanted) figureUrl = "";
      });
  });

  function addFile(path) {
    if (!mayEdit) throw new Error("This project is read-only.");
    path = checkPlacement(rules, { kind: "text", path }, session.list(), session.folders());
    openFile = session.addText(path, "");
    paintPreview();
  }

  function relocateFiles(entries, destination, rename) {
    const plan = session.relocate(entries, destination, rules, rename);
    if (plan.files.some((file) => file.path !== file.previousPath)) {
      say("Files moved. References in source files are not changed automatically.");
    }
  }

  function deleteFiles(entries) {
    session.removeEntries(entries);
    paintPreview();
  }

  function makeMain(file) {
    if (file.kind !== "text") return;
    session.setMain(file.id);
    paintPreview();
  }

  /// A figure: the bytes go to the store and the name goes into the shared
  /// document, in that order. The name is this browser's to give; the bytes
  /// are the server's to keep, under their own digest.
  ///
  /// The two are separate requests, which is why the server keeps a figure
  /// nothing refers to for an hour: between them there is a moment when the
  /// bytes are stored and nothing names them.
  async function addFigure(file, path = file.name) {
    if (!mayEdit) throw new Error("This project is read-only.");
    const activeSession = session;
    path = checkPlacement(rules, { kind: "asset", path }, activeSession.list(), activeSession.folders());
    const { sha } = await uploadAsset(SLUG, file, KEY);
    // An upload yields to other editors; recheck before installing its name.
    if (session !== activeSession || !mayEdit) throw new Error("The editing session changed during upload.");
    checkPlacement(rules, { kind: "asset", path }, session.list(), session.folders());
    session.putAsset(path, sha);
    paintPreview();
    return path;
  }

  // InsertMenu captures the active editor target before opening a dialog. The
  // editor owns the CRDT anchors; the reader only supplies project metadata
  // and reports any generator notes to the existing toast channel.
  function insertContext() {
    return editor?.getInsertContext?.() || null;
  }
  function applyInsertion(result, context) {
    if (!result?.text) {
      for (const note of result?.notes || []) say(note, true);
      return;
    }
    if (!editor?.applyInsertResult?.(result, context)) {
      say("The insertion target is no longer available.", true);
      return;
    }
    for (const note of result.notes || []) say(note, true);
  }
  async function uploadInsertAsset(file) {
    const path = await addFigure(file);
    return path;
  }

  /// The whole directory, as a zip. Built here rather than by a route,
  /// because everything it needs is already in this browser: the texts are in
  /// the shared document and the figures were fetched to render them, so
  /// asking the server to assemble what is already here would be a round trip
  /// to be told what we know.
  async function downloadTree() {
    try {
      const tree = liveTreeNow();
      const currentFolders = [...folders];
      const files = { ...tree.texts };
      for (const path of currentFolders) files[`${path}/`] = new Uint8Array();
      if (Object.keys(tree.digests || {}).length) {
        const held = await figures.gather(SLUG, tree.digests, {
          ...SHELL_HEADERS,
          ...keyHeaders(KEY),
        });
        const missing = Object.keys(tree.digests).filter(
          (path) => !Object.prototype.hasOwnProperty.call(held.assets, path),
        );
        if (missing.length) {
          throw new Error(`could not download ${missing.join(", ")}`);
        }
        Object.assign(files, held.assets);
      }
      // Loaded when it is asked for. A reader who never downloads a document
      // should not carry the code that would have built one.
      const { zip } = await import("../lib/zip.js");
      // Named for the document rather than for its main file: what is being
      // downloaded is the directory, and the slug is what a person knows it by.
      saveBlob(zip(files), `${SLUG}.zip`);
    } catch (error) {
      say(error.message || "could not download the project", true);
    }
  }

  // A text dropped or chosen is read and added as a file. Its bytes are
  // words, so they belong in the shared document rather than in the store.
  async function addDroppedText(file, path = file.name) {
    if (!mayEdit) throw new Error("This project is read-only.");
    const activeSession = session;
    const text = await file.text();
    if (session !== activeSession || !mayEdit) throw new Error("The editing session changed during upload.");
    path = checkPlacement(rules, { kind: "text", path }, session.list(), session.folders());
    openFile = session.addText(path, text);
    paintPreview();
  }

  async function downloadEntry(entry) {
    try {
      const tree = liveTreeNow();
      const currentFolders = [...folders];
      const selected = (path) => entry.kind === "folder" ? inside(path, entry.path) : path === entry.path;
      const content = Object.fromEntries(Object.entries(tree.texts).filter(([path]) => selected(path)));
      const digests = Object.fromEntries(Object.entries(tree.digests || {}).filter(([path]) => selected(path)));
      if (Object.keys(digests).length) {
        const held = await figures.gather(SLUG, digests, { ...SHELL_HEADERS, ...keyHeaders(KEY) });
        if (Object.keys(digests).some((path) => !Object.prototype.hasOwnProperty.call(held.assets, path))) throw new Error("Could not download all selected files.");
        Object.assign(content, held.assets);
      }
      let blob;
      if (entry.kind === "folder") {
        for (const path of currentFolders) if (path === entry.path || selected(path)) content[path + "/"] = new Uint8Array();
        content[entry.path + "/"] = new Uint8Array();
        const { zip } = await import("../lib/zip.js");
        blob = zip(content);
      } else {
        if (!(entry.path in content)) throw new Error("This file is no longer available.");
        blob = new Blob([content[entry.path]]);
      }
      saveBlob(blob, basename(entry.path) + (entry.kind === "folder" ? ".zip" : ""));
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

  function startCollaboration(document_) {
    collaboration?.close();
    collaboration = createReaderCollaboration({
      slug: SLUG,
      key: KEY,
      getIdentity: () => identity,
      getCanEdit: () => mayEdit,
      onMessage: receive,
      onConnected: (up) => {
        connected = up;
        if (!up) {
          pendingChat?.disconnect();
          outbox.disconnected();
        }
      },
      onPeers: (count) => (peers = Math.max(peers, count)),
      onState: (state_) => (persistence = state_),
      onSession: (active) => {
        session = active;
        handledFileTransactions = new WeakSet();
        refreshFiles();
      },
      onSource: (active) => {
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
    // The role the document's own endpoint answers with, which is where every
    // affordance comes from: the editor at `editor` and above. `can_edit` is
    // the same answer in the older shape, for a server that predates the role.
    const ROLES = ["reader", "commenter", "editor", "owner"];
    const allowed = document_.role
      ? ROLES.indexOf(document_.role) >= ROLES.indexOf("editor")
      : document_.can_edit === undefined
        ? document_.can_moderate
        : document_.can_edit;
    // The backend's explicit engine/draft pair is authoritative when present.
    // Legacy source_format=quarto documents infer Quarto through the adapter
    // helper, while a bundle without an engine discriminator remains Quarto.
    let resultsIdentity;
    try {
      resultsIdentity = documentResultsIdentity(document_);
    } catch (error) {
      say(error.message, true);
      settled = true;
      return;
    }
    const format = resultsIdentity.execution_engine === "quarto"
      ? "quarto"
      : document_.source_format
        ? resultsIdentity.draft_format
        : "html";
    sourceFormat = format;
    if (format === "quarto") {
      localQuarto.configure({ project: SLUG, origin: location.origin });
      quartoBindingId = localQuarto.bindingId();
      // A pairing this browser already holds is verified now, so the
      // workspace banner shows a connected preview without a first failed
      // attempt to start one.
      void localQuarto.probe();
    }
    // A document is shown by output kind. Paged documents use stored PDFs when
    // this deployment has no browser compiler, so opening a Typst paper never
    // depends on downloading Typst WASM. Compiler availability only controls
    // whether an authorized editor compiles locally.
    const list = Array.isArray(document_.renderers) ? document_.renderers : ["markdown"];
    renderers.offerLatex(list.includes("latex"));
    if (!renderers.outputKind(format)) {
      say(`${format} documents are read where their renderer is built`, true);
      settled = true;
      return;
    }
    mayEdit = Boolean(allowed);
    // A panel remembered from an editor's visit is not one a link-holder is
    // offered. Coerced without being remembered: the preference is this
    // browser's, and an editor coming back to their own document keeps it.
    if (panel && !tabs.some((tab) => tab.id === panel)) showPanel(home, false);
    settled = true;
    // Typst is loaded automatically for editors. Readers use the stored PDF
    // and must remain usable on a deployment with no Typst module at all.
    if (mayEdit) renderers.warm(format);
    startCollaboration(document_);
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
    // Somebody sent a link to a moment rather than to the document. Opening it
    // opens the panel too, so that what is on the screen is explained by
    // something every reader with a live share link can see and leave.
    if (ARRIVED_AT) {
      showPanel("history", false).then(() => {
        // The history request may outlive the panel. Do not enter a
        // checkpoint after the reader has explicitly left history.
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
    const stopQuartoStatus = localQuarto.subscribe((status) => { localAppStatus = status; });
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
      issued += 1;
      navigationGeneration += 1;
      clearTimeout(previewTimer);
      previewTimer = null;
      boot.dispose();
      passages.clearPassageCache();
      stopQuartoPreviewPagePoll();
      if (quartoPreview) void localQuarto.stopQuartoPreview(quartoPreview.id).catch(() => {});
      framePreview.dispose();
      renderingStore?.dispose();
      stopLatex();
      pendingChat?.dispose();
      collaboration?.close();
    };
  });

  // Ctrl-S is what a hand does after typing a paragraph, and there is nothing
  // for it to do: the document is already durable. What it must not do is
  // claim that pending writes are saved, so it says what is actually true.
  function reportPersistence() {
    // Never claim pending or disconnected writes have reached the server.
    if (!connected || persistence.pending) return;
    say("saved on the server");
  }

  // There is no save, so a close is almost never worth interrupting: the
  // document is durable, and what is not yet on the server is in this
  // browser. The one case left is work that has reached neither.
  function beforeUnload(event) {
    if (atRisk) event.preventDefault();
  }

  // Ctrl-Shift-D (Cmd on macOS) toggles dictation into whatever text input
  // has focus, including the editor -- SPEC-dictation.md 4.8. It works in
  // both reading and editing mode, since comments and chat exist in both;
  // Escape below stops it from anywhere, including a pane that has scrolled
  // the pill out of view.
  async function toggleDictation() {
    const dictation = getDictation();
    if (dictation.state !== "idle" && dictation.state !== "unavailable") {
      await dictation.stop();
      return;
    }
    const target = targetForActiveElement(document, (element) => (editor?.contains?.(element) ? editor : null));
    if (!target) {
      toastSaid("Click into a text field first");
      return;
    }
    if (target.kind === "editor" && editor?.vimMode?.() === "normal") {
      toastProblem("Enter insert mode to dictate into the editor");
      return;
    }
    await dictation.start(target);
  }

  // The arrangement is changed often enough to be worth a key. Ctrl-\ is what
  // an editor usually puts a split on, and nothing here or in CodeMirror wants
  // it.
  function shortcut(event) {
    if (!event.isComposing && (event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "d") {
      event.preventDefault();
      toggleDictation();
      return;
    }
    if (event.key === "Escape" && dictationSnapshot.state !== "idle" && dictationSnapshot.state !== "unavailable") {
      event.preventDefault();
      getDictation().stop();
      return;
    }
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

<svelte:window bind:innerWidth={width} onkeydown={shortcut} onbeforeunload={beforeUnload} onpagehide={() => session?.leave()} />

{#snippet fileItems()}
  {#if !viewing}
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
  <Menu.Item value="download" class="menuitem">Download project</Menu.Item>
  <hr class="hr my-1" />
  {#if canSeeSharing}<Menu.Item value="share" class="menuitem">Share…</Menu.Item>{/if}
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

{#snippet toolItems()}
  {#if sourceFormat === "quarto" && !viewing}
    <!-- Nothing rendered is ever uploaded: choosing "Quarto preview" runs
         the document's code with Quarto on this computer, through the local
         app, and shows its own page here; "Markdown preview" never runs any
         code. The two are exclusive, with a check mark on whichever is
         active. Local app settings remain reachable from Settings… below. -->
    <Menu.Item value="preview-markdown" class="menuitem">
      <span class="w-4">{quartoPreviewMode === "markdown" ? "✓" : ""}</span>Markdown preview
    </Menu.Item>
    <Menu.Item value="preview-quarto" class="menuitem">
      <span class="w-4">{quartoPreviewMode === "quarto" ? "✓" : ""}</span>Quarto preview
    </Menu.Item>
    <hr class="hr my-1" />
  {:else if sourceFormat === "latex" && !viewing}
    {#if compilesHere}<Menu.Item value="compile" class="menuitem">Compile now</Menu.Item>{/if}
  {/if}
  <Menu.Item value="settings" class="menuitem">Settings…</Menu.Item>
{/snippet}

<Nav {me}>
  {#snippet menus()}
    {#if mayEdit}
      <div class="desktop-workspace-menu">
        <Menu onSelect={(chosen) => void chooseFileCommand(chosen.value)}>
          <Menu.Trigger class="menubar-item">File</Menu.Trigger>
          <ExplorerMenu>{@render fileItems()}</ExplorerMenu>
        </Menu>
      </div>
    {/if}
    {#if editing && mayEdit && !viewing}
      <InsertMenu getContext={insertContext} oninsert={applyInsertion} onupload={uploadInsertAsset} />
    {/if}
    {#if editing}
      <div class="desktop-workspace-menu">
        <Menu onSelect={(chosen) => chose(chosen.value)}>
          <Menu.Trigger class="menubar-item">View</Menu.Trigger>
          <ExplorerMenu>{@render layoutItems()}</ExplorerMenu>
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
    {#if editing || mayEdit}
      <div class="compact-workspace-menu">
        <Menu onSelect={(chosen) => chooseCompactCommand(chosen.value)}>
          <Menu.Trigger class="menubar-item" aria-label="File, view and tools">Menu</Menu.Trigger>
          <ExplorerMenu>
            {#if mayEdit}{@render fileItems()}<hr class="hr my-1" />{/if}
            {#if editing}{@render layoutItems()}<hr class="hr my-1" />{/if}
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
  {/snippet}
  {#snippet tools()}
    <CopyLink href={linkFor(SLUG)} label="Copy the link to this document" />
  {/snippet}
</Nav>

<div bind:clientHeight={workspaceBannerHeight}>
{#if viewing}
  <div class="workspace-banner preset-tonal-warning" role="region" aria-label="Historical version">
    <span title={new Date(viewing.at).toLocaleString()}>Showing {viewingName}</span>
    <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={backToNow}>Back to now</button>
    {#if mayEdit}<button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => restoreCheckpoint(viewing.sha)}>Restore this version</button>{/if}
    <CopyLink href={checkpointLink(viewing.sha)} label="Copy the link to this version" />
  </div>
{/if}
<!-- The status row. Everything the document has to say about its own state
     -- the connection, what rendering is on screen, who else is here, how a
     compile is going -- used to be badges on the bar, in the smallest type
     on the page and clipped to an ellipsis as soon as the bar ran out of
     room, which on a laptop it always did. Here it has the whole width, the
     page's own type size, and room to wrap; a LaTeX failure can carry its
     hint and its buttons on one readable line. The row is absent, not empty,
     when there is nothing to say. -->
{#if statusRow}
  <div class="workspace-banner workspace-status" role="status" aria-label="Document status">
    {#if connectionNote}<span class="status-warning">{connectionNote}</span>{/if}
    {#if renderedNote}<span>{renderedNote}</span>{/if}
    {#if editing}
      {#if peers > 1}<span>{peers} people editing</span>{/if}
      {#if sourceFormat === "latex"}
        <LatexStatus onconnect={() => openSettings("local")} onretrybrowser={() => void paintPreview()} />
      {:else if compileBadge}
        <span><span class="spinner" aria-hidden="true"></span>{compileBadge}</span>
      {/if}
    {/if}
    {#if sourceFormat === "quarto"}
      {#if quartoRendering}
        <span title="Quarto is re-rendering the live preview."><span class="spinner" aria-hidden="true"></span>Rendering the live preview…</span>
      {/if}
      {#if quartoNeedsLocalApp}
        <span class="status-warning">Quarto preview needs the local app on this computer.</span>
        <button type="button" class="btn btn-sm preset-tonal-primary" onclick={ensureLocalApp}>Connect</button>
      {/if}
    {/if}
  </div>
{/if}

</div>

<main class="reader" class:editing={shown.source} class:no-preview={!shown.document}
      class:no-comments={!shown.comments} class:source-right={sourceSide === "right"}
      class:mobile-document={activeMobileView === "document"} class:mobile-source={activeMobileView === "source"}
      class:mobile-sidebar={activeMobileView === "sidebar"} class:adapted={compact || (splitTight && layout === "split")}
      style="height: calc(100dvh - var(--librepaper-bar) - {workspaceBannerHeight}px); --librepaper-activity: {ACTIVITY_WIDTH}px; --librepaper-editor: {pixels(PANES.editor, panes)}px; --librepaper-sidebar: {pixels(PANES.sidebar, panes)}px">
  <!-- The column, first: the files, the comments or the history, chosen by
       the activity bar. A file dropped anywhere on it joins the project. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
    <aside class="sidebar" class:collapsed={!shown.comments} ondragover={(event) => event.preventDefault()} ondrop={dropped}>
      <div class="sidebar-activity">
        <div class="activity-sections" role="group" aria-label="Sidebar sections">
          {#each tabs as tab (tab.id)}
            {#if tab.id === "diagnostics"}
              <!-- The counts sit under the icon, in the colour of what they
                   count, so the bar says at a glance whether the document
                   compiles; the words go to the tooltip and the screen reader. -->
              <div class="activity-diagnostics">
                <IconButton icon="triangle-alert"
                  label={diagnosticBadge ? `${tab.says}: ${diagnosticBadge}` : tab.says}
                  pressed={panel === tab.id}
                  onclick={() => selectPanel(tab.id)} />
                {#if diagnostics.length}
                  <span class="activity-counts" aria-hidden="true">
                    {#if errorCount}<span class="activity-count errors">{errorCount}</span>{/if}
                    {#if warningCount}<span class="activity-count warnings">{warningCount}</span>{/if}
                  </span>
                {/if}
              </div>
            {:else}
              <IconButton
                icon={tab.id === "files" ? "folder" : tab.id === "collaboration" ? "comment" : tab.id === "changes" ? "pencil" : tab.id === "agent" ? "bot" : tab.id === "history" ? "history" : tab.id === "share" ? "users" : "sliders"}
                label={tab.says} pressed={panel === tab.id}
                onclick={() => selectPanel(tab.id)} />
            {/if}
          {/each}
        </div>

      </div>
      {#if settled}
      <div class="sidebar-content">
      {#each tabs.filter((tab) => visitedPanels.includes(tab.id) || (tab.id === "collaboration" && unconfirmed.length)) as tab (tab.id)}
      <div class="panel-slot" hidden={panel !== tab.id || !shown.comments}>
      {#if panel === tab.id && ["collaboration", "changes"].includes(tab.id) && unconfirmed.length}
        <div class="pending-recovery">
          <PendingAnnotations items={unconfirmed}
            onretry={(id) => outbox.retry(id, (message) => collaboration?.send(message))}
            ondiscard={discardAnnotation} />
        </div>
      {/if}
      {#if tab.id === "files" && mayEdit}
        <Files bind:this={fileList} {files} {folders} open={openFile} peers={peersByFile}
               {mayEdit} {rules} onopen={openTheFile} onadd={addFile}
               onmkdir={(path) => session.addFolder(path, rules)} onrelocate={relocateFiles}
               ondelete={deleteFiles} onduplicate={(entry, path) => session.duplicateEntry(entry, path, rules)} onmain={makeMain}
               onfigure={addFigure} ontext={addDroppedText}
               ondownload={downloadTree} ondownloaditem={downloadEntry} />
      {:else if tab.id === "agent"}
        <Agent slug={SLUG} link={linkFor(SLUG)} canShare={doc.role === "owner"} path={session?.paths?.get(openFile) || ""}
               selection={pending} revision={pending?.revision || ""} request={assistantRequest}
               {comments} {diagnostics} oncommenttask={askCommentAssistant} ondiagnostictask={askDiagnostic}
               onreview={reviewAssistantResults} onpreview={previewAssistant} />
      {:else if tab.id === "collaboration"}
        <Collaboration messages={liveChat} {connected} canPost={mayChat}
          onsend={sendLiveChat} {unreadChat} bind:tab={collaborationTab}
          {comments} {figureAt} {identity} commentingAs={doc.commenting_as || "Anonymous"} {canModerate} {tool} {went} {replacements}
          canComment={mayChat} hasFigures={figureAt.length > 0} ontool={chooseTool}
          onreveal={revealAnnotation} selected={selectedAnnotation}
          onresolve={resolve} ondelete={askDelete} ondeletemany={askDeleteMany} onreply={reply} />
      {:else if tab.id === "changes"}
        <Changes {comments} {figureAt} {identity} commentingAs={doc.commenting_as || "Anonymous"} {canModerate} {tool} {went} {replacements}
          canComment={mayChat} ontool={chooseTool} onreveal={revealAnnotation} selected={selectedAnnotation}
          onresolve={resolve} ondelete={askDelete} ondeletemany={askDeleteMany} onreply={reply}
          onaccept={(comment) => decideSuggestion(comment, "accept")} onreject={(comment) => decideSuggestion(comment, "reject")}
          onrejectconfirmed={rejectConfirmed}
          onhistory={() => { setHistoryRedlines(true); void showPanel("history"); }} />
      {:else if tab.id === "share" && canSeeSharing}
        <Share open={panel === "share" && shown.comments} inline slug={SLUG} onclose={() => showPanel("")} />
      {:else if tab.id === "diagnostics"}
        <Diagnostics {diagnostics} main={session?.mainPath() || ""}
                     canOpen={(item) => Boolean(diagnosticFile(item))} onopen={openDiagnostic}
                     provenance={lastLatexResult?.provenance || null} attempts={lastLatexResult?.attempts || []} />
      {:else if tab.id === "history"}
        <History {checkpoints} viewing={viewing?.sha || null} canEdit={mayEdit}
                 problem={historyProblem}
                 baseline={historyBaseline} changes={historyChanges}
                 changedPaths={historyChangedPaths}
                 redlines={historyRedlines} onredlines={setHistoryRedlines}
                 {fileDiff}
                 target={historyComparePoint}
                 onview={viewPoint} oncompare={compareSince}
                 onback={() => viewPoint("")} onname={nameCheckpoint}
                 onrestore={restoreCheckpoint} oncopy={checkpointLink}
                 onstep={revealHistoryHunk}
                 oncheckpointfile={openCheckpointFile}
                 onfilediff={openFileDiff} onclosefilediff={historyController.closeFileDiff} />
      {/if}
      </div>
      {/each}
      </div>
      {/if}
    </aside>
  {#if shown.comments && !compact}
    <Grip pane={PANES.sidebar} label="Resize the left-hand column" panes={panes}
          onsize={(size) => setSize(PANES.sidebar, size)}
          onguide={(where) => (guide = where)}
          ongrab={(on) => { grabbing = on; guide = { ...guide, shown: on }; }} />
  {/if}

  {#if editing}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="editorpane" class:away={!shown.source}
             ondragover={(event) => event.preventDefault()} ondrop={dropped}>
      <!-- A figure has no editor. Choosing one shows it: an image as itself,
           a PDF through the browser's own viewer, which shows the first page
           without this application carrying a PDF renderer of its own. -->
      {#if mergeTarget && MergeEditor}
        <MergeEditor path={mergeTarget.path} oldText={mergeTarget.oldText} newText={mergeTarget.newText}
                     liveText={mergeTarget.liveText} awareness={mergeTarget.awareness}
                     editable={mergeTarget.editable !== false && mayEdit && editing}
                     targetLabel={mergeTarget.targetLabel}
                     note={mergeTarget.note || ""}
                     onlive={historyComparePoint ? async () => { const path = mergeTarget.path; await chooseHistoryTarget(""); await openFileDiff(path); } : null}
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
      {:else if Editor}
        {#key sourceEpoch}
          <Editor bind:this={editor} {session} format={sourceFormat} file={openFile} {keys}
                  editable={mayEdit}
                  onbibliography={bibliographyAnalyzed} oncaret={followCaret} onsave={reportPersistence} onquit={showDocumentAlone}
                  onfilechange={(id) => { openFile = id; shownFigure = null; }} />
        {/key}
      {/if}
    </section>
  {/if}

  <!-- A separator only where there are two things to separate. -->
  {#if shown.source && shown.document}
    <Grip pane={PANES.editor} label="Split between the source and the document" panes={panes}
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
       the engine on its own, automatically, and the status line under the
       toolbar carries the loading and failure states. Every paged format
       still gets an explicit not-yet-rendered state until a stored PDF
       arrives -- readers never load a compiler merely to read an existing
       artifact. -->
  {#if shown.document && failedBeforeRender}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">Could not render</h2>
        {#if pdfFailureReason}
          <p class="text-surface-700-300 text-sm">
            The compiler produced no PDF, and Diagnostics has nothing to show for it. What it said:
          </p>
          <pre class="text-surface-700-300 text-xs">{pdfFailureReason}</pre>
        {:else}
          <p class="text-surface-700-300 text-sm">
            Fix the errors in Diagnostics to produce a PDF preview.
          </p>
        {/if}
      </div>
    </section>
  {:else if shown.document && unrendered}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">Not yet rendered</h2>
        <p class="text-surface-700-300 text-sm">
          This {sourceFormat === "typst" ? "Typst" : "paged"} document has no stored PDF yet. When an editor compiles it, its pages appear here.
        </p>
      </div>
    </section>
  {/if}

  <!-- Kept mounted whatever the arrangement: taking the frame out of the tree
       would reload the document and lose the reader's place in it. -->
  <Preview bind:this={preview} src={frameSrc} {docsOrigin} onmessage={fromFrame} {grabbing}
           away={!shown.document || unrendered || failedBeforeRender} />

  <nav class="mobile-pane-nav" aria-label="Workspace view">
    <IconButton icon="book" label="Document" pressed={shown.document}
                onclick={() => showMobileView("document")} />
    {#if editing}
      <IconButton icon="file-text" label="Source" pressed={shown.source}
                  onclick={() => showMobileView("source")} />
    {/if}
    {#each compact ? tabs : [] as tab (tab.id)}
      <IconButton
        icon={tab.id === "files" ? "folder" : tab.id === "collaboration" ? "comment" : tab.id === "changes" ? "pencil" : tab.id === "agent" ? "bot" : tab.id === "history" ? "history" : tab.id === "diagnostics" ? "triangle-alert" : tab.id === "share" ? "users" : "sliders"}
        label={tab.says} pressed={shown.comments && panel === tab.id}
        onclick={() => selectPanel(tab.id)} />
    {/each}
  </nav>

  <!-- Shown only while a separator is dragged: a line that follows the pointer
       so the split can be seen moving without the iframe reflowing on every
       pointermove. -->
  {#if guide.shown}<div class="grip-guide" class:held={guide.held} style="left: {guide.left}px"></div>{/if}
</main>

{#if bar.shown && mayChat}
  <div
    id="selectionbar"
    class="flex gap-1"
    style="display: flex; left: {bar.left}px; top: {bar.top}px"
  >
    {#if tool === "highlighting"}
      <div class="highlight-colors" role="group" aria-label="Highlight color">
        {#each HIGHLIGHT_COLORS as color}
          <button type="button" class:selected={highlightColor === color} class="color-swatch" style="background:{color}"
            aria-label="Use {color} highlight" aria-pressed={highlightColor === color}
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
                latexSettings={latexSettingsState} onlatexsettings={(next) => session?.setLatexSettings(next)}
                bindingId={quartoBindingId} onbindingid={(id) => { quartoBindingId = id; localQuarto.setBindingId(id); }}
                preview={quartoPreview} options={quartoOptions} {viewing}
                onapplyoptions={applyRenderOptions} />

<Modal bind:open={commenting} title={tool === "editing" ? "Suggest a change" : "Add comment"}>
  {#snippet children()}
    <form id="commentForm" class="flex flex-col gap-3" onsubmit={submitDialog}>
      <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3 text-sm">
        {pending?.output_anchor ? `Saved result: ${pending.exact}${pending.region ? " (selected region)" : ""}` : pending?.point ? "Comment at this point" : pending?.region ? `Figure ${pending.region.image_index + 1}` : `“${pending?.exact ?? ""}”`}
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
               rather than clicking Accept -- see `docs/specs/track-changes.md`. -->
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
          <textarea class="textarea" rows="5" maxlength="5000" required autofocus bind:value={draft.body}
            bind:this={commentBodyField}
          ></textarea>
        </label>
        <Row justify="end">
          <DictationButton target={() => textareaTarget(commentBodyField)} label="Dictate comment" size="btn-icon-sm" />
        </Row>
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

<DictationDownload />
<Toasts />
<DictationPill />

<style>
  .nav-document {
    display: block;
    max-width: min(38vw, 20rem);
    color: var(--color-surface-700-300);
    font-size: var(--text-sm);
  }
  .sidebar { flex-direction: row; }
  .sidebar.collapsed { flex: 0 0 var(--librepaper-activity); }
  .sidebar-activity {
    display: flex;
    flex: none;
    flex-direction: column;
    align-items: center;
    gap: var(--spacing);
    width: var(--librepaper-activity);
    padding-block: calc(var(--spacing) * 3);
  }
  .activity-sections {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--spacing);
  }
  .compact-workspace-menu { display: none; }
  .workspace-banner { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 2); padding: calc(var(--spacing) * 2) calc(var(--spacing) * 4); border-bottom: 1px solid var(--color-divider); }
  /* The status row wears the page's own type, not a badge's: what it says is
     meant to be read across the room, and an offline warning in particular is
     not something to squint at. Each item is one inline group, so a spinner
     stays beside its words when the row wraps. */
  .workspace-status { color: var(--color-surface-700-300); }
  .workspace-status > * { display: inline-flex; align-items: center; gap: var(--spacing); }
  .workspace-status .status-warning { color: var(--color-warning-600-400); font-weight: 500; }
  @media (max-width: 600px) {
    .desktop-workspace-menu { display: none; }
    .compact-workspace-menu { display: block; }
  }
  .activity-diagnostics {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }
  .activity-counts {
    display: flex;
    gap: 4px;
    font-size: 0.625rem;
    line-height: 1;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .activity-count.errors { color: var(--color-error-500); }
  .activity-count.warnings { color: var(--color-warning-500); }
  .sidebar-content {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .panel-slot { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; min-height: 0; overflow: hidden; }
  .panel-slot[hidden] { display: none; }
  .pending-recovery { flex-shrink:0; max-height:40%; overflow-y:auto; padding:var(--spacing); }
  .highlight-colors { display:flex; align-items:center; gap:3px; padding:2px; border-radius:4px; background:var(--color-surface-100-900); }
  .color-swatch { width:1.25rem; height:1.25rem; border:2px solid transparent; border-radius:50%; }
  .color-swatch.selected { border-color:var(--color-surface-900-100); box-shadow:0 0 0 1px var(--color-primary-500); }
  .custom-color input { width:1.35rem; height:1.35rem; padding:0; border:0; background:transparent; }
  @media (max-width: 760px) {
    .sidebar, .sidebar.collapsed { flex-direction: column; }
    .sidebar-activity { display: none; }
    .activity-sections { flex-direction: row; }
    .sidebar-content { overflow: hidden; }
  }
</style>
