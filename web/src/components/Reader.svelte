<script>
  // One document: the source beside it, the page itself, and everything said
  // about it.
  import { anchorAll, anchorAllSources, anchorOne, flatten } from "../lib/anchor.js";
  import * as sync from "../lib/sync.js";
  import * as renderers from "../lib/renderers.js";
  import * as diagnosticsRule from "../lib/diagnostics.js";
  import * as figures from "../lib/figures.js";
  import * as history from "../lib/history.js";
  import { createHistoryController } from "../lib/reader/history.svelte.js";
  import * as passages from "../lib/passages.js";
  import * as suggestions from "../lib/suggestions.js";
  import { diagnosticContext } from "../lib/assistant-review.js";
  import { attribution, itemsFor } from "../lib/redlines.js";
  import { orphanState } from "../lib/orphan.js";
  import * as latex from "../lib/latex.js";
  import { checkPlacement, basename, inside } from "../lib/file-manager.js";
  import { snapshotDigest } from "../lib/tree-digest.js";
  import { createAnnotations } from "../lib/reader/annotations.js";
  import { createReaderBoot } from "../lib/reader/boot.js";
  import { createPendingChat } from "../lib/reader/chat.js";
  import { createReaderCollaboration } from "../lib/reader/collaboration.js";
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
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import ControlGroup from "./ControlGroup.svelte";
  import CopyLink from "./CopyLink.svelte";
  import Share from "./Share.svelte";
  import Modal from "./Modal.svelte";
  import Toasts from "./Toasts.svelte";
  import Row from "./layout/Row.svelte";
  import { problem as toastProblem } from "../lib/toast.svelte.js";
  import Preview from "./Preview.svelte";
  import Grip from "./Grip.svelte";
  import Comments from "./Comments.svelte";
  import Agent from "./Agent.svelte";
  import Chat from "./Chat.svelte";
  import History from "./History.svelte";
  import Diagnostics from "./Diagnostics.svelte";
  import Settings from "./Settings.svelte";
  import LatexStatus from "./LatexStatus.svelte";
  import Files from "./Files.svelte";
  import { renderedNoteText } from "../lib/latex/status-text.js";
  import { createPreviewApi } from "../lib/reader/preview-api.js";
  import { createFramePreview } from "../lib/reader/frame-preview.js";
  import { createRenderingStore } from "../lib/reader/rendering-store.js";

  const SLUG = location.pathname.split("/").pop();

  // A one-time migration: the old distribution chooser kept its choice under
  // this key, in this browser, forever. WasmTex initializes automatically --
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

  function applyHighlights() {
    if (!frameReady) return;
    const regions = JSON.stringify(
      comments
        .filter((comment) => comment.region)
        .map((comment) => ({
          id: comment.id,
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
    const items = showable
      ? itemsFor(historyChanges, attribution(checkpoints, historyBaseline.sha, historyComparePoint?.sha || null))
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
    anchorAll(docText || "", list.filter((comment) => !comment.region), docText === null ? null : docView);
    anchorAllSources(treeNow(), list);
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
      if (comment.source || comment.region || comment.pending || comment.temp_id) continue;
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
        // Whatever was painted before is gone with the rebuilt DOM.
        lastRegions = lastHighlight = lastRedlines = null;
        reanchor();
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
        document.getElementById("comment-" + message.id)?.scrollIntoView({ behavior: "smooth", block: "center" });
        break;
    }
  }

  /* --------------------------------------------------------------- selection */

  let tool = $state("commenting");
  let pending = $state(null);
  let assistantRequest = $state(null);
  let selectionRevision = Promise.resolve("");
  let bar = $state({ shown: false, left: 0, top: 0 });

  function showSelection(selector, rect) {
    if (!selector || !selector.exact) {
      bar = { ...bar, shown: false };
      pending = null;
      return;
    }
    pending = {
      exact: String(selector.exact),
      prefix: String(selector.prefix || ""),
      suffix: String(selector.suffix || ""),
      // A hint, not a claim: the server keeps it, and anchoring uses it only
      // to choose between passages the context cannot separate.
      position: Number.isInteger(selector.position) && selector.position >= 0 ? selector.position : null,
    };
    // The anchor of record, cut from the source at the same moment: a best
    // effort taken here, in the commenter's browser, while the words just
    // selected are still fresh. A page with no text yet, or a phrase the
    // source-matching heuristic cannot place, leaves this null -- which the
    // server reads as "no source anchor yet" rather than as a failure.
    let source = null;
    try {
      if (docText !== null) {
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

  async function askAssistant() {
    if (!pending?.exact) return;
    const captured = { ...pending, source: pending.source ? { ...pending.source } : null };
    const revision = await selectionRevision.catch(() => "");
    assistantRequest = { id: crypto.randomUUID(), selection: captured, revision };
    bar = { ...bar, shown: false };
    showPanel("agent");
    if (width <= 760) showMobileView("sidebar");
  }

  function askDiagnostic(item) {
    assistantRequest = { id: crypto.randomUUID(), diagnostic: { ...item }, revision: item.revision || "" };
    showPanel("agent");
    if (width <= 760) showMobileView("sidebar");
  }

  async function reviewAssistantResults({ suggestions: ids = [], pass = "" }) {
    const matches = comments.filter((comment) => comment.motivation === "editing" &&
      (ids.includes(comment.id) || (pass && comment.pass === pass)));
    const first = matches.find((comment) => !comment.resolved) || matches[0];
    if (!first) { toastProblem("These suggestions are no longer available."); return; }
    showPanel("comments");
    if (width <= 760) showMobileView("sidebar");
    await tick();
    const card = document.getElementById(`comment-${first.id}`);
    card?.scrollIntoView({ block: "nearest" });
    card?.querySelector("button")?.focus({ preventScroll: true });
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
    tell({ type: "tool", tool: which });
  }

  /* -------------------------------------------------------------- annotating */

  let commenting = $state(false);
  let identifying = $state(false);
  let deleting = $state(false);
  let draft = $state({ body: "", proposed: "" });
  let pendingDelete = null;

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
    annotations.comment(pending, { motivation, body, proposed }, identity || doc.commenting_as || "Anonymous");
    pending = null;
  }

  function submitDialog(event) {
    event.preventDefault();
    const motivation = tool === "region" ? "commenting" : tool;
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
    pendingDelete = comment;
    deleting = true;
  }

  function confirmDelete() {
    const comment = pendingDelete;
    pendingDelete = null;
    deleting = false;
    if (!comment) return;
    annotations.delete(comment);
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
      if (!liveChat.some((message) => message.id === event.id)) liveChat = [...liveChat, event].slice(-200);
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
  let session = $state(null);
  let editing = $state(false);
  // Which text the editor is bound to. It changes once on a document migrated
  // from before there were directories: the words arrive in the retired text
  // and the session then holds them in a file. Keying the component on this is
  // what binds it to the file rather than to what the file used to be.
  let sourceEpoch = $state(0);
  let mayEdit = $state(false);
  let sourceFormat = $state("");
  let state = $state(""); // what the editor is saying about itself
  let problem = $state(false);
  let peers = $state(1);
  let linked = $state(read(LINKED, false) === true);

  // Routine saving stays quiet; losing the connection still needs a warning.
  const persistenceBadge = $derived.by(() => {
    if (!mayEdit || !session) return "";
    if (!connected) {
      return persistence.local ? "offline, changes kept in this browser" : "offline";
    }
    return "";
  });

  // A close is only worth interrupting when the work has reached neither this
  // browser's storage nor the server.
  const atRisk = $derived(Boolean(mayEdit && persistence.pending && !persistence.local));

  function say(text, isProblem = false) {
    state = text;
    problem = isProblem;
  }

  // What the document is called, which is what the rendered page is titled.
  // The title it was published under wins; a document that never had one is
  // named by its own first heading, the way `publish` names one -- the *main*
  // file's first heading, since a chapter's heading names the chapter.
  async function headingOf(tree) {
    return doc.title || (await renderers.titleOf(tree)) || "Untitled";
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
  // Typst and LaTeX render to a PDF drawn by a browser VM -- the frame has
  // no text there for a mark to land on, so the toggle stays off and says
  // why rather than silently doing nothing.
  const redlinesDisabledReason = $derived(
    sourceFormat === "typst" || sourceFormat === "latex"
      ? "Redlines cannot be shown in a document rendered to PDF."
      : "",
  );
  function setHistoryRedlines(on) {
    historyController.setRedlines(Boolean(on) && !redlinesDisabledReason);
  }
  let fileDiff = $derived(historyController.fileDiff);
  $effect(() => () => historyController.dispose());
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

  let pendingHistoryReveal = null;
  function revealPendingHistory() {
    const pending = pendingHistoryReveal;
    if (!pending || docText === null || (viewing?.sha || "") !== pending.sha) return;
    const found = anchorOne(docText, pending.hunk, docView);
    if (!found) return;
    pendingHistoryReveal = null;
    tell({ type: "locate", start: found.start, length: found.end - found.start });
  }

  async function revealHistoryHunk(hunk) {
    if (!hunk.exact) return;
    const sha = historyComparePoint?.sha || "";
    pendingHistoryReveal = { hunk, sha };
    if ((viewing?.sha || "") !== sha) {
      if (sha) await showCheckpoint(sha);
      else backToNow();
    } else {
      revealPendingHistory();
    }
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
    if (tree.main) return tree;
    const named =
      { typst: "main.typ", markdown: "main.md", html: "main.html", latex: "main.tex" }[sourceFormat] ||
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
  // map they live in is not itself reactive: Settings.svelte needs to redraw
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
  // (docs/specs/wasmtex.md: "Preserve both attempts' logs when a browser failure
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
  // WasmTex automatically the first time it is asked to compile (`paintPreview`
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

  let frameShowsCheckpoint = false;
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
      lastRegions = lastHighlight = null;
    },
    onDelivered: (payload) => {
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
          (slow && snapshotSource !== sourceGeneration) ||
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
        tree.urls = held.urls;
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
        (slow && snapshotSource !== sourceGeneration) ||
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
          .failurePage(await headingOf(tree), sourceFormat)
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
      if (state === NO_MATCH) say("");
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
  let keys = $state(read(KEYMAP, "default") === "vim" ? "vim" : "default");

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
    { id: "comments", says: "Comments" },
    { id: "chat", says: "Chat" },
    { id: "agent", says: "Agent" },
    // History is readable by link-holders too: reviewers need the “since”
    // view even when they cannot edit or restore the live source.
    { id: "history", says: "History" },
    { id: "diagnostics", says: "Diagnostics", editOnly: true },
    { id: "share", says: "Share", sharingOnly: true },
    { id: "settings", says: "Settings", editorOnly: true },
  ];
  const PANELS = ["", ...TABS.map((tab) => tab.id)];
  let panel = $state(PANELS.includes(read(PANEL, null)) ? read(PANEL, null) : "files");
  // Mount panels on their first visit and retain them across view changes.
  // This preserves scroll positions, expanded folders and unsent chat drafts.
  let visitedPanels = $state([]);
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
  const home = $derived(mayEdit ? "files" : "comments");
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
    if (what === "side-left" || what === "side-right") return putSourceOn(what.slice(5));
    if (what.startsWith("ratio-")) return setSize(PANES.editor, Number(what.slice(6)));
    if (what === "linked") return setLinked(!linked);
  }

  /* ------------------------------------------------------------------- boot */

  /* ------------------------------------------------------------- the files */

  // The directory as the list shows it, and where everyone's caret is. Held
  // as state rather than derived, because what they are derived from is a
  // CRDT that changes outside Svelte's knowledge.
  let files = $state([]);
  let folders = $state([]);
  let openFile = $state("");
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
  // what a compiler would produce.
  function filesChanged(events) {
    // Nested text edits change the preview, but not the file list.
    if (!Array.isArray(events) || events.some((event) => event.target === session.files)) refreshFiles();
    sourceChanged();
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
      const url = URL.createObjectURL(zip(files));
      const link = document.createElement("a");
      link.href = url;
      // Named for the document rather than for its main file: what is being
      // downloaded is the directory, and the slug is what a person knows it by.
      link.download = `${SLUG}.zip`;
      link.click();
      // Revoked on a later turn: revoking it now would race the download the
      // click has only just started.
      setTimeout(() => URL.revokeObjectURL(url), 10_000);
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
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = basename(entry.path) + (entry.kind === "folder" ? ".zip" : "");
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 10_000);
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
        refreshFiles();
      },
      onSource: (active) => {
        if (!active.mainId()) sourceChanged();
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
    // A document published before HTML was a source format has one anyway: the
    // page itself, through the identity renderer.
    const format = document_.source_format || "html";
    sourceFormat = format;
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
    // actually starts loading WasmTex. `session.latexSettings()` needs the
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
        if (panel === "history") void showCheckpoint(ARRIVED_AT);
      });
    } else if (panel === "history") {
      // The column reopened where it was left, and this panel has to fetch
      // what it shows.
      loadHistory();
    }
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
      issued += 1;
      navigationGeneration += 1;
      clearTimeout(previewTimer);
      previewTimer = null;
      boot.dispose();
      passages.clearPassageCache();
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
    setTimeout(() => {
      if (state === "saved on the server") say("");
    }, 2000);
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

<svelte:window bind:innerWidth={width} onkeydown={shortcut} onbeforeunload={beforeUnload} onpagehide={() => session?.leave()} />

{#snippet layoutControl()}
  <!-- Right-click, or hold, for which side the source is on: an order set once
       does not belong in a control flipped hourly. -->
  <Menu onSelect={(chosen) => chose(chosen.value)}>
    <Menu.ContextTrigger>
      {#snippet element(attributes)}
        <span {...attributes} class="contents">
          <IconButton
            icon={ARRANGEMENTS[layout].icon}
            label="Layout: {ARRANGEMENTS[layout].says}. Click for {ARRANGEMENTS[
              ARRANGEMENTS[layout].next
            ].says}."
            pressed={layout !== "split"}
            onclick={cycleLayout}
          />
        </span>
      {/snippet}
    </Menu.ContextTrigger>
    <Menu.Positioner class="z-50">
      <Menu.Content class="card bg-surface-50-950 w-52 p-1 shadow-xl">
        {#each [["left", "Source on left"], ["right", "Source on right"]] as [side, says]}
          <Menu.Item value="side-{side}" class="menuitem">
            <span class="w-4">{sourceSide === side ? "✓" : ""}</span>
            {says}
          </Menu.Item>
        {/each}
        <hr class="hr my-1" />
        <!-- The same ratios a drag sticks to, for anyone who never finds that
             it does. -->
        {#each RATIOS as ratio}
          <Menu.Item value="ratio-{ratio.share}" class="menuitem">
            <span class="w-4">{sizes[PANES.editor.key] === ratio.share ? "✓" : ""}</span>
            {ratio.says}
          </Menu.Item>
        {/each}
        <hr class="hr my-1" />
        <!-- A preference rather than a mode: set once, and only about this
             arrangement. It is also available from Settings. -->
        <Menu.Item value="linked" class="menuitem">
          <span class="w-4">{linked ? "✓" : ""}</span>
          Keep in step
        </Menu.Item>
      </Menu.Content>
    </Menu.Positioner>
  </Menu>
{/snippet}

<Nav {me} documentation={false}>
  {#snippet children()}
    <span id="docTitle" class="nav-document truncate" title={toolbarPath || doc.title || ""}>
      {toolbarPath ? basename(toolbarPath) : doc.title || "LibrePaper"}
    </span>
  {/snippet}
  {#snippet status()}
    {#if !connected}<small class="badge preset-tonal-warning" title="Reconnecting">reconnecting…</small>{/if}
    {#if viewing}<small class="badge preset-tonal-warning" title={new Date(viewing.at).toLocaleString()}>Showing {viewingName}</small>{/if}
    {#if renderedNote}<small class="badge preset-tonal-surface" title={renderedNote}>{renderedNote}</small>{/if}
    {#if editing}
      {#if persistenceBadge}<small class="badge preset-tonal-warning" title={persistenceBadge}>{persistenceBadge}</small>{/if}
      {#if peers > 1}<small class="badge preset-tonal-secondary">{peers} editing</small>{/if}
      {#if sourceFormat === "latex"}
        <LatexStatus onconnect={() => showPanel("settings")} onretrybrowser={() => void paintPreview()} />
      {:else if compileBadge}
        <small class="badge preset-tonal-surface" title={compileBadge}><span class="spinner" aria-hidden="true"></span>{compileBadge}</small>
      {/if}
      {#if state}<small class="badge {problem ? 'preset-tonal-error' : 'preset-tonal-surface'}" title={state}>{state}</small>{/if}
    {/if}
  {/snippet}
  {#snippet tools()}
    <Row gap={2}>
      <div class="mobile-workspace-tools">
        {#if editing}
          <ControlGroup label="Layout">
            {#snippet children()}{@render layoutControl()}{/snippet}
          </ControlGroup>
        {/if}
        <IconButton icon="help" label="Documentation" href="/documentation" />
      </div>
      {#if viewing}
        <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={backToNow}>Back to now</button>
        {#if mayEdit}<button type="button" class="btn btn-sm preset-tonal-primary" onclick={() => restoreCheckpoint(viewing.sha)}>Restore this version</button>{/if}
        <CopyLink href={checkpointLink(viewing.sha)} label="Copy the link to this version" />
      {/if}
      {#if editing && sourceFormat === "latex" && compilesHere}
        <!-- Automatic compilation covers every edit; this is only for asking
             again right now -- after fixing an error, or after connecting
             local LibrePaper -- without waiting for the debounce or typing a
             fresh keystroke. No "play" icon exists in Icon.svelte's set, so
             this reuses "check": the label carries the meaning. -->
        <IconButton icon="check" label="Compile now" title="Compile now" onclick={compileNow} />
      {/if}
      {#if !canSeeSharing}
        <CopyLink href={linkFor(SLUG)} label="Copy the link to this document" />
      {/if}
    </Row>
  {/snippet}
</Nav>

<main class="reader" class:editing={shown.source} class:no-preview={!shown.document}
      class:no-comments={!shown.comments} class:source-right={sourceSide === "right"}
      class:mobile-document={activeMobileView === "document"} class:mobile-source={activeMobileView === "source"}
      class:mobile-sidebar={activeMobileView === "sidebar"} class:adapted={compact || (splitTight && layout === "split")}
      style="--librepaper-activity: {ACTIVITY_WIDTH}px; --librepaper-editor: {pixels(PANES.editor, panes)}px; --librepaper-sidebar: {pixels(PANES.sidebar, panes)}px">
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
                icon={tab.id === "files" ? "folder" : tab.id === "comments" ? "comment" : tab.id === "chat" ? "message-square" : tab.id === "agent" ? "bot" : tab.id === "history" ? "history" : tab.id === "share" ? "users" : "sliders"}
                label={tab.says} pressed={panel === tab.id}
                onclick={() => selectPanel(tab.id)} />
            {/if}
          {/each}
        </div>
        <div class="activity-utilities" role="group" aria-label="Workspace controls">
          {#if editing}{@render layoutControl()}{/if}
          <IconButton icon="help" label="Documentation" href="/documentation" />
        </div>
      </div>
      {#if settled}
      <div class="sidebar-content">
      {#each tabs.filter((tab) => visitedPanels.includes(tab.id) || (tab.id === "comments" && unconfirmed.length)) as tab (tab.id)}
      <div class="panel-slot" hidden={panel !== tab.id || !shown.comments}>
      {#if tab.id === "files" && mayEdit}
        <Files bind:this={fileList} {files} {folders} open={openFile} peers={peersByFile}
               {mayEdit} {rules} onopen={openTheFile} onadd={addFile}
               onmkdir={(path) => session.addFolder(path, rules)} onrelocate={relocateFiles}
               ondelete={deleteFiles} onduplicate={(entry, path) => session.duplicateEntry(entry, path, rules)} onmain={makeMain}
               onfigure={addFigure} ontext={addDroppedText}
               ondownload={downloadTree} ondownloaditem={downloadEntry} />
      {:else if tab.id === "agent"}
        <Agent slug={SLUG} link={linkFor(SLUG)} path={session?.paths?.get(openFile) || ""}
               selection={pending} revision={pending?.revision || ""} request={assistantRequest}
               {comments} onreview={reviewAssistantResults} />
      {:else if tab.id === "chat"}
        <Chat messages={liveChat} {connected} canPost={mayChat}
              onsend={sendLiveChat} />
      {:else if tab.id === "share" && canSeeSharing}
        <Share open={panel === "share" && shown.comments} inline slug={SLUG} onclose={() => showPanel("")} />
      {:else if tab.id === "settings" && mayEdit}
        <Settings {keys} {linked} {sourceSide} ratio={sizes[PANES.editor.key]}
                  {sourceFormat} {mayEdit} latexSettings={latexSettingsState}
                  onkeys={setKeys} onlinked={setLinked} onside={putSourceOn}
                  onratio={(share) => setSize(PANES.editor, share)}
                  onlatexsettings={(next) => session?.setLatexSettings(next)} />
      {:else if tab.id === "diagnostics"}
        <Diagnostics {diagnostics} main={session?.mainPath() || ""}
                     canOpen={(item) => Boolean(diagnosticFile(item))} onopen={openDiagnostic}
                     onask={askDiagnostic}
                     provenance={lastLatexResult?.provenance || null} attempts={lastLatexResult?.attempts || []} />
      {:else if tab.id === "history"}
        <History {checkpoints} viewing={viewing?.sha || null} canEdit={mayEdit}
                 problem={historyProblem}
                 baseline={historyBaseline} changes={historyChanges}
                 changedPaths={historyChangedPaths}
                 redlines={historyRedlines} onredlines={setHistoryRedlines}
                 {redlinesDisabledReason}
                 {fileDiff}
                 currentLabel={historyComparePoint?.label || (historyComparePoint ? history.shortSha(historyComparePoint.sha) : "now")}
                 target={historyComparePoint}
                 onbaseline={chooseHistoryBaseline}
                 ontarget={chooseHistoryTarget}
                 onshow={(sha) => { showMobileView("document"); return showCheckpoint(sha); }} onback={backToNow} onname={nameCheckpoint}
                 onrestore={restoreCheckpoint} oncopy={checkpointLink}
                 onreveal={revealHistoryHunk}
                 oncheckpointfile={openCheckpointFile}
                 onfilediff={openFileDiff} onclosefilediff={historyController.closeFileDiff} />
      {:else}
        <Comments {comments} {figureAt} {identity} commentingAs={doc.commenting_as || "Anonymous"} {canModerate} {tool} {went} {replacements}
                  canComment={mayChat}
                  hasFigures={figureAt.length > 0}
                  ontool={chooseTool}
                  onreveal={async (comment) => {
                    // The rendered anchor, when there is one, reveals on the
                    // page as it always did. A comment found only in the
                    // source has nothing there to reveal, and goes to the
                    // source instead -- the same jump a click in the document
                    // makes in `followDocumentClick`.
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
                      if (id) {
                        openFile = id;
                        editor.goToIn(id, comment.sourceStart);
                      } else {
                        editor.goTo(comment.sourceStart);
                      }
                    }
                  }}
                  onresolve={resolve} ondelete={askDelete} onreply={reply}
                  onaccept={(comment) => decideSuggestion(comment, "accept")}
                  onrejectconfirmed={rejectConfirmed}
                  onreject={(comment) => decideSuggestion(comment, "reject")}>
          {#snippet pending()}
            <PendingAnnotations items={unconfirmed}
              onretry={(id) => outbox.retry(id, (message) => collaboration?.send(message))}
              ondiscard={discardAnnotation} />
          {/snippet}
        </Comments>
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
       WasmTex on its own, automatically, and the status line under the
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
        icon={tab.id === "files" ? "folder" : tab.id === "comments" ? "comment" : tab.id === "chat" ? "message-square" : tab.id === "agent" ? "bot" : tab.id === "history" ? "history" : tab.id === "diagnostics" ? "triangle-alert" : tab.id === "share" ? "users" : "sliders"}
        label={tab.says} pressed={shown.comments && panel === tab.id}
        onclick={() => selectPanel(tab.id)} />
    {/each}
  </nav>

  <!-- Shown only while a separator is dragged: a line that follows the pointer
       so the split can be seen moving without the iframe reflowing on every
       pointermove. -->
  {#if guide.shown}<div class="grip-guide" class:held={guide.held} style="left: {guide.left}px"></div>{/if}
</main>

{#if bar.shown}
  <div
    id="selectionbar"
    class="flex gap-1"
    style="display: flex; left: {bar.left}px; top: {bar.top}px"
  >
    {#if mayChat}<button class="btn btn-sm preset-filled-primary-500 shadow-lg" onclick={barClicked}>
      {tool === "highlighting" ? "Highlight" : tool === "region" ? "Box" : tool === "editing" ? "Suggest" : "Comment"}
    </button>{/if}
    {#if pending?.exact}<button class="btn btn-sm preset-filled-primary-500 shadow-lg" onclick={() => void askAssistant()}>Ask assistant</button>{/if}
  </div>
{/if}

<!-- What a selection becomes, once the reader has said what to call it and
     what they think of it. -->
<Modal bind:open={commenting} title={tool === "editing" ? "Suggest a change" : "Add comment"}>
  {#snippet children()}
    <form id="commentForm" class="flex flex-col gap-3" onsubmit={submitDialog}>
      <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3 text-sm">
        {pending?.region ? `Figure ${pending.region.image_index + 1}` : `“${pending?.exact ?? ""}”`}
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
          ></textarea>
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

<!-- Deleting a thread cannot be undone, so it is confirmed. -->
<Modal
  bind:open={deleting}
  title="Delete comment?"
  description="This removes the comment and its replies for everyone. It cannot be undone."
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
  .activity-sections, .activity-utilities {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--spacing);
  }
  .activity-utilities {
    margin-top: auto;
    padding-top: calc(var(--spacing) * 2);
    border-top: 1px solid var(--color-surface-200-800);
  }
  .mobile-workspace-tools { display: none; align-items: center; gap: var(--spacing); }
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
  @media (max-width: 760px) {
    .sidebar, .sidebar.collapsed { flex-direction: column; }
    .sidebar-activity { display: none; }
    .activity-sections { flex-direction: row; }
    .activity-utilities { display: none; }
    .mobile-workspace-tools { display: flex; }
    .sidebar-content { overflow: hidden; }
  }
</style>
