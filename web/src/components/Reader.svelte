<script>
  // One document: the source beside it, the page itself, and everything said
  // about it.
  import { anchorAll, flatten } from "../lib/anchor.js";
  import * as sync from "../lib/sync.js";
  import * as renderers from "../lib/renderers.js";
  import * as diagnosticsRule from "../lib/diagnostics.js";
  import * as collab from "../lib/collab.js";
  import * as figures from "../lib/figures.js";
  import * as history from "../lib/history.js";
  import * as passages from "../lib/passages.js";
  import * as latex from "../lib/latex.js";
  import { openRoom } from "../lib/room.js";
  import {
    SHELL_HEADERS,
    config as loadConfig,
    keyHeaders,
    me as whoami,
    signInHref,
    uploadAsset,
  } from "../lib/api.js";
  import {
    AUTHOR,
    LAYOUT,
    LINKED,
    SOURCE_SIDE,
    linkFor,
    markViewed,
    read,
    takeKeyFromFragment,
    write,
  } from "../lib/storage.js";
  import { LAYOUTS, PANES, RATIOS, clamp, pixels, remember, showing, stored } from "../lib/panes.js";

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
  import Sidebar from "./Sidebar.svelte";
  import History from "./History.svelte";
  import Files from "./Files.svelte";

  const SLUG = location.pathname.split("/").pop();

  // The key a reader arrived with, taken out of the fragment before anything
  // asks the server a question. A fragment never leaves the browser, so this
  // is the one part of the URL a link key can safely travel in; from here it
  // is kept under the slug and presented on every request for this document.
  const KEY = takeKeyFromFragment(SLUG);

  /* ------------------------------------------------------------ the document */

  let doc = $state({});
  let docsOrigin = $state(null);
  let frameSrc = $state(null);
  let me = $state({});
  // The displayed name, since this is what goes on a comment and what the
  // reader is shown commenting as. A Google account's handle is its email and
  // belongs on neither.
  let identity = $derived(me.name || "");
  let canModerate = $derived(Boolean(doc.can_moderate));
  let connected = $state(true);
  // Sharing is the owner's; seeing who else is in the room is anyone's who is
  // named on the document. A reader who arrived by link is offered neither,
  // which is most of the point of a blind review.
  let sharingOpen = $state(false);
  let canSeeSharing = $derived(Boolean(doc.can_see_sharing));
  let visibility = $state("");

  // Whether this browser's work is safe, which is a different question from
  // whether the socket is up. `pending` counts the updates the server has not
  // yet said it has written; `local` says the document is in this browser's
  // own storage, which is what makes a reload safe while the socket is down.
  let persistence = $state({ pending: 0, local: false, joined: false });

  /* --------------------------------------------------------------- anchoring */

  let comments = $state([]);
  let commentsReady = false;
  let frameReady = false;
  let docText = null; // the joined visible text, invariant across repaints
  let docView = null; // flatten(docText), so anchoring does not redo it per call
  let figureAt = $state([]); // text offset of each figure, by its index

  let preview = $state(null);
  const tell = (message, transfer) => preview?.tell(message, transfer);

  // The agent repaints the whole document on every "regions" or "highlight"
  // message, so a call that changes nothing is not free even though it looks
  // idempotent. Each is sent only when its payload actually differs from the
  // last one sent -- reset when the frame republishes its text, since the
  // agent's DOM was rebuilt then and needs the full repaint regardless.
  let lastRegions = null;
  let lastHighlight = null;

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
        })),
    );
    if (highlight !== lastHighlight) {
      lastHighlight = highlight;
      tell({ type: "highlight", ranges: JSON.parse(highlight) });
    }
  }

  function reanchor() {
    if (!frameReady || !commentsReady || docText === null) return;
    // A region annotation is placed by the agent, not by text matching, so it
    // is never orphaned for want of a quotation.
    anchorAll(docText, comments.filter((comment) => !comment.region), docView);
    comments = comments;
    applyHighlights();
    tracePassages();
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
        const first = !frameReady;
        frameReady = true;
        // Whatever was painted before is gone with the rebuilt DOM.
        lastRegions = lastHighlight = null;
        reanchor();
        if (first) paintPreview();
        break;
      case "selection":
        showSelection(message.selector, message.rect);
        break;
      case "region":
        // A rectangle drawn on a figure anchors the same way a quotation does.
        pending = { exact: "", prefix: "", suffix: "", position: null, region: message.region };
        placeBar(message.rect);
        break;
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
    placeBar(rect);
  }

  function placeBar(rect) {
    if (rect && !matchMedia("(max-width:760px)").matches) {
      const frameRect = document.querySelector(".viewport").getBoundingClientRect();
      bar = {
        shown: true,
        left: Math.min(innerWidth - 105, frameRect.left + rect.left + (rect.right - rect.left) / 2 - 42),
        top: Math.max(65, frameRect.top + rect.top - 42),
      };
      return;
    }
    bar = { ...bar, shown: true };
  }

  function chooseTool(which) {
    tool = which;
    tell({ type: "tool", tool: which });
  }

  /* -------------------------------------------------------------- annotating */

  let commenting = $state(false);
  let identifying = $state(false);
  let deleting = $state(false);
  let draft = $state({ body: "", tags: "", creator: read(AUTHOR, "Anonymous") });
  let pendingDelete = null;

  function barClicked() {
    if (!pending) return;
    bar = { ...bar, shown: false };
    if (tool === "highlighting") {
      // No dialog: the passage is the whole annotation.
      submitAnnotation({ motivation: "highlighting", body: "", tags: [] });
      return;
    }
    if (!identity && me.providers?.length) {
      identifying = true;
      return;
    }
    commenting = true;
  }

  function submitAnnotation({ motivation, body, tags }) {
    if (!pending) return;
    const creator = identity || draft.creator;
    if (!identity) write(AUTHOR, creator);
    const temp_id = crypto.randomUUID();
    const optimistic = {
      id: temp_id,
      temp_id,
      seq: Number.MAX_SAFE_INTEGER,
      ...pending,
      motivation,
      body,
      tags,
      creator: creator || "Anonymous",
      created: new Date().toISOString(),
      resolved: false,
      resolved_at: null,
      replies: [],
      pending: true,
    };
    // Drawn before the round trip; the broadcast reconciles it by temp_id.
    anchorAll(docText || "", [optimistic], docText === null ? null : docView);
    comments = [...comments, optimistic];
    applyHighlights();
    room?.send({ type: "comment", ...pending, motivation, body, tags, creator, temp_id });
    pending = null;
  }

  function submitDialog(event) {
    event.preventDefault();
    submitAnnotation({
      motivation: tool === "region" ? "commenting" : tool,
      body: draft.body,
      // "methods, typo" becomes ["methods", "typo"]. The server normalises
      // again, so this only has to be reasonable.
      tags: draft.tags.split(",").map((tag) => tag.trim()).filter(Boolean),
    });
    draft = { ...draft, body: "", tags: "" };
    commenting = false;
  }

  function resolve(comment) {
    // Optimistic: flip locally, then tell the room. The broadcast that comes
    // back is idempotent with what we already drew.
    comment.resolved = !comment.resolved;
    comments = comments;
    applyHighlights();
    room?.send({ type: "resolve", comment_id: comment.id, resolved: comment.resolved });
  }

  function askDelete(comment) {
    pendingDelete = comment;
    deleting = true;
  }

  function confirmDelete() {
    const comment = pendingDelete;
    pendingDelete = null;
    deleting = false;
    if (!comment) return;
    comments = comments.filter((item) => item !== comment);
    applyHighlights();
    room?.send({ type: "delete", comment_id: comment.id });
  }

  function reply(comment, body, name) {
    const temp_id = crypto.randomUUID();
    comment.replies = [
      ...comment.replies,
      { id: temp_id, body, creator: name || "Anonymous", created: new Date().toISOString(), temp_id },
    ];
    comments = comments;
    room?.send({ type: "reply", comment_id: comment.id, body, creator: name, temp_id });
  }

  /* -------------------------------------------------------------------- room */

  let room = null;

  function receive(event) {
    if (event.type === "hello") {
      comments = event.comments;
      commentsReady = true;
      reanchor();
      return;
    }
    if (event.type === "error") {
      // Roll the optimistic row back.
      if (event.temp_id) {
        comments = comments
          .filter((comment) => comment.temp_id !== event.temp_id)
          .map((comment) => ({
            ...comment,
            replies: comment.replies.filter((reply) => reply.temp_id !== event.temp_id),
          }));
        applyHighlights();
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
        document.title = `${event.title} · Komodoc`;
      }
      return;
    }

    if (event.type === "comment") {
      const local = comments.find((comment) => comment.temp_id === event.temp_id);
      // Broadcasts carry no `deletable` field, so the caller's own comment,
      // reconciled here from its optimistic placeholder, stays deletable by
      // this browser regardless of what the server sent back.
      if (local) Object.assign(local, event.comment, { temp_id: undefined, pending: false, deletable: true });
      else if (!comments.some((comment) => comment.id === event.comment.id)) {
        anchorAll(docText || "", [event.comment], docText === null ? null : docView);
        comments = [...comments, event.comment];
      }
      comments = comments;
      applyHighlights();
      return;
    }
    if (event.type === "reply") {
      const comment = comments.find((item) => item.id === event.comment_id);
      if (!comment) return;
      const local = comment.replies.find((reply) => reply.temp_id === event.temp_id);
      if (local) Object.assign(local, event.reply, { temp_id: undefined });
      else if (!comment.replies.some((reply) => reply.id === event.reply.id)) {
        comment.replies = [...comment.replies, event.reply];
      }
      comments = comments;
      return;
    }
    if (event.type === "delete") {
      comments = comments.filter((item) => item.id !== event.comment_id);
      applyHighlights();
      return;
    }
    if (event.type === "resolve") {
      const comment = comments.find((item) => item.id === event.comment_id);
      if (!comment) return;
      comment.resolved = event.resolved;
      comment.resolved_at = event.resolved_at;
      comments = comments;
      applyHighlights();
    }
  }

  /* ----------------------------------------------------- reading and editing */

  // CodeMirror is a third of a megabyte, and most people who open a document
  // are here to read it. The editor component is fetched when one is actually
  // opened, so a reader never pays for it. The session is not the editor: a
  // reader joins it too, because that is where the text comes from.
  let Editor = $state(null);
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

  // What the toolbar says about durability. There is no save, so there is
  // nothing to say while everything the socket carried has been written; the
  // only things worth saying are that work is on its way, that it is being
  // kept here for now, or that it is in neither place yet.
  const persistenceBadge = $derived.by(() => {
    if (!mayEdit || !session) return "";
    if (!connected) {
      return persistence.local ? "offline, changes kept in this browser" : "offline";
    }
    return persistence.pending ? "saving…" : "";
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
  let historyOpen = $state(false);
  let checkpoints = $state([]);
  let historyProblem = $state("");
  // The checkpoint being shown in the document pane, whole -- its tree and its
  // texts -- or null for the document as it stands.
  let viewing = $state(null);
  // Which checkpoint the reader arrived asking for, out of the link somebody
  // sent them. Read once, because after that the panel is where the answer is.
  const ARRIVED_AT = new URLSearchParams(location.search).get("at") || "";

  async function loadHistory() {
    try {
      checkpoints = await history.load(SLUG, keyHeaders(KEY));
      historyProblem = "";
    } catch (error) {
      historyProblem = error.message || "the history could not be read";
    }
  }

  // Closing the timeline is leaving it: what the document pane shows goes back
  // to the text as it stands, because a page nobody can see the history behind
  // is a page with no way back.
  function closeHistory() {
    historyOpen = false;
    backToNow();
  }

  async function toggleHistory() {
    if (historyOpen) return closeHistory();
    historyOpen = true;
    commentsOpen = false;
    await loadHistory();
  }

  // A checkpoint as a renderer takes it. Its texts came with it; its figures
  // did not, because a figure is served immutably by its digest and the ones
  // this checkpoint used may well be the ones on the screen already.
  function checkpointTree(point) {
    const digests = {};
    for (const [path, file] of Object.entries(point.files || {})) {
      if (file.kind !== "text") digests[path] = file.sha;
    }
    return { main: point.main, texts: point.texts || {}, digests };
  }

  async function showCheckpoint(sha) {
    try {
      viewing = await history.checkpoint(SLUG, sha, keyHeaders(KEY));
      historyProblem = "";
    } catch (error) {
      historyProblem = error.message || "that checkpoint could not be read";
      return;
    }
    await paintPreview();
  }

  function backToNow() {
    if (!viewing) return;
    viewing = null;
    paintPreview();
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
  // The comments already looked into, so a repaint does not search again for
  // an answer that has not changed.
  const traced = new Set();

  // A document with nothing orphaned costs nothing at all: no manifest, no
  // renders. The first passage that has gone is what buys the manifest, and
  // every comment after it is answered from the same list.
  async function tracePassages() {
    const lost = comments.filter(
      (comment) => comment.orphaned && !comment.region && !traced.has(comment.id),
    );
    if (!lost.length || viewing) return;
    for (const comment of lost) traced.add(comment.id);
    if (!checkpoints.length) await loadHistory();
    const list = checkpoints;
    if (!list.length) return;
    for (const comment of lost) {
      try {
        const point = await passages.wentAt(SLUG, comment, list, keyHeaders(KEY));
        if (point) went = { ...went, [comment.id]: point };
      } catch {
        // A checkpoint that cannot be read or rendered is one this cannot say
        // anything about, and the card falls back to saying the passage is
        // not in the document -- which is still true and still useful.
      }
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
  function treeNow() {
    // A checkpoint picked out of the timeline is shown in the document pane in
    // place of the live text. Everything downstream -- the render, the frame,
    // the agent, the anchoring -- is the same as for the live document,
    // because to all of it a checkpoint is just another directory.
    if (viewing) return checkpointTree(viewing);
    if (!session) return { main: "", texts: {}, digests: {} };
    const tree = session.tree();
    if (tree.main) return tree;
    const named =
      { typst: "main.typ", markdown: "main.md", html: "main.html", latex: "main.tex" }[sourceFormat] ||
      "main.txt";
    return { main: named, texts: { [named]: session.text.toString() }, digests: {} };
  }

  // Painting the preview is sending it to the frame: the draft is a document,
  // and a document belongs on the documents origin, not in this page. The
  // agent republishes its text from there, which re-anchors every comment
  // against what was just typed.
  let issued = 0;
  let painted = 0;
  let previewTimer = null;

  // What the last compile said. When it is painted is `diagnostics.js`'s
  // rule, and the wait it counts is from the keystroke rather than from the
  // render that noticed the error, so a slow render does not add its own
  // length to it. From clean to red at reading speed, from red to clean at
  // typing speed.
  let diagnostics = $state([]);
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
    diagnostics = list;
    editor?.setDiagnostics?.(list);
  }

  // Clicking the badge goes to the first thing the compiler complained about,
  // and again to the next, round the list.
  function goToDiagnostic() {
    editor?.nextDiagnostic?.();
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
  // A private document is the other exception, and in the other direction: the
  // documents origin shares no cookie with this one, so it has no identity to
  // check `private` against and never serves such a document's bytes. What
  // arrives there is the empty shell, so this page paints it whatever its
  // format -- and an HTML document's own scripts do not run, because painting
  // sets innerHTML. A document that needs them is one to share by link.
  //
  // And a checkpoint is always painted, whatever the format: the frame is
  // served from the live document, so there is nothing on the documents origin
  // that is the document as it was on Tuesday.
  const paintsTheFrame = $derived(
    Boolean(viewing) || editing || sourceFormat !== "html" || visibility === "private",
  );

  /* -------------------------------------------------------------- LaTeX */

  // A LaTeX document has no HTML to paint, so its frame is the PDF viewer on
  // the documents origin rather than the empty shell. Everything else about
  // the frame is the same: same origin, same CSP, same agent, same channel.
  const framePath = $derived(sourceFormat === "latex" ? "pdf" : "raw");

  // Whether a compiler has been chosen in this browser. Not a promise and not
  // a fetch: the card is drawn from this before anything is downloaded.
  let latexReady = $state(false);
  // How long the last compile took, and whether one is running now. Both only
  // exist for LaTeX, where a compile takes seconds and silence would read as
  // a preview that had stopped working.
  let compiling = $state(false);
  let lastCompile = $state(0);

  // The card is offered to somebody who can act on it and to nobody else. A
  // reader is never asked to download a compiler to read a paper: what they
  // get is "not yet rendered" and the source, until a stored rendering makes
  // that unnecessary.
  // Reopened from the toolbar to switch distribution, which is the only way
  // back to it once one has been chosen.
  let cardOpen = $state(false);
  const showsCard = $derived(
    sourceFormat === "latex" && editing && mayEdit && (!latexReady || cardOpen),
  );
  const unrendered = $derived(sourceFormat === "latex" && !mayEdit && !everPaintedShown);

  // A distribution is loaded: the card goes, and the document is compiled at
  // once rather than on the next keystroke. This is the only place a compile
  // is started other than an edit, and it is the person asking for one.
  function latexChosen() {
    latexReady = true;
    cardOpen = false;
    paintPreview();
  }

  // A LaTeX compile that is running says so, and says how long the last one
  // took once there has been one. Before the first, there is no honest number
  // to give.
  const compileBadge = $derived(
    !compiling ? "" : lastCompile ? `compiling… (last took ${lastCompile.toFixed(1)}s)` : "compiling…",
  );

  // What the frame was showing the last time it was loaded, so a reload
  // happens when the document has changed and not merely because somebody's
  // caret moved. Seeded on the first join: the frame was served from the same
  // live document a moment earlier.
  let framedSource = null;
  let framedGeneration = 0;

  function refreshFramedPage() {
    if (paintsTheFrame || !session || !docsOrigin) return;
    const source = session.text.toString();
    if (framedSource === null) {
      framedSource = source;
      return;
    }
    if (source === framedSource) return;
    framedSource = source;
    // A new URL is what makes the frame load again. The response is `no-store`
    // and the path is the document's own, so this is the same page from the
    // same origin under the same CSP -- the scripts it carries run exactly as
    // they did on the first load.
    frameSrc = `${docsOrigin}/${framePath}/${SLUG}/?v=${++framedGeneration}`;
  }

  async function paintPreview() {
    if (!paintsTheFrame) {
      refreshFramedPage();
      return;
    }
    const mine = ++issued;
    const tree = treeNow();
    try {
      // The figures, if this document has any. A figure not yet here is
      // awaited before the first compile that needs it, and the page that is
      // already up stays up meanwhile: rendering without them would produce a
      // document with holes in it and replace it a moment later, which reads
      // as a flicker rather than as progress.
      if (Object.keys(tree.digests || {}).length) {
        const held = await figures.gather(SLUG, tree.digests, { ...SHELL_HEADERS, ...keyHeaders(KEY) });
        if (mine <= painted) return;
        tree.assets = held.assets;
        tree.urls = held.urls;
      }
      // A LaTeX compile takes seconds rather than milliseconds, so the pane
      // says one is running. The last page that compiled stays up under it:
      // an author who is typing has something to look at, which is the whole
      // difference between this and a pane that blanks for four seconds.
      const slow = renderers.formatOf(tree.main) === "latex";
      if (slow) compiling = true;
      let rendered;
      try {
        rendered = await renderers.render(tree, await headingOf(tree));
      } finally {
        // Only the newest compile owns the badge. An older one finishing
        // afterwards must not turn the spinner off under a newer one.
        if (slow && mine > painted) compiling = false;
      }
      const { html, pdf, diagnostics: said, seconds } = rendered;
      // A slower render that resolves late must not paint over a newer one.
      if (mine <= painted) return;
      painted = mine;
      if (slow && seconds) lastCompile = seconds;
      // A render carries `html` or `pdf`, and the reader posts whichever it
      // has. The bytes are transferred rather than copied: a PDF is megabytes
      // and this page has no further use for it once the frame has it.
      if (pdf) {
        const buffer = pdf.buffer ? pdf.buffer.slice(pdf.byteOffset, pdf.byteOffset + pdf.byteLength) : pdf;
        tell({ type: "preview", pdf: buffer }, [buffer]);
        everPainted = true;
        everPaintedShown = true;
        diagnosticPainter.rendered({ page: "", diagnostics: said || [] });
        return;
      }
      if (html !== null) {
        // The page is what the document says now, so every error said about an
        // earlier state of it is cleared at once. The warnings that came with
        // this page are painted on the same slow schedule the errors are, so
        // that a font name half typed does not flash a badge on every
        // keystroke.
        tell({ type: "preview", html });
        everPainted = true;
        everPaintedShown = true;
        diagnosticPainter.rendered({ page: html, diagnostics: said || [] });
        return;
      }
      // No page: the last one that compiled stays up, and what is said is that
      // it does not compile now, and where -- once the typing has stopped.
      diagnosticPainter.rendered({ page: null, diagnostics: said || [] });
      // Unless nothing was ever painted, which is what someone who opens the
      // editor on a document that does not compile sees. Then the frame shows
      // the engine's page saying so, with the list on it, styled like a
      // document rather than like a crash.
      if (!everPainted) {
        const page = await renderers
          .failurePage(await headingOf(tree), sourceFormat)
          .catch(() => null);
        if (page && mine >= painted) tell({ type: "preview", html: page });
      }
    } catch (error) {
      // Not a document that did not compile: a renderer that could not be
      // fetched, which is this page's problem rather than the author's.
      if (mine > painted) say(error.message || "could not render", true);
    }
  }

  // Sixty milliseconds for an editor: short enough to read as live -- the
  // renderer takes single-digit milliseconds -- and long enough that a burst
  // of typing is one render. A second for a reader, who is watching somebody
  // else type and should never be shown a word half written.
  const READER_DEBOUNCE = 1000;

  function sourceChanged() {
    // The keystroke, which is what the diagnostic wait is measured from.
    diagnosticPainter.typed();
    clearTimeout(previewTimer);
    // A LaTeX compile takes seconds, so it waits for the source to be quiet
    // for longer -- `latex.DEBOUNCE`, which is that module's number and not
    // one written twice. A reader watching somebody else type waits longer
    // still, and the longer of the two wins.
    const wait =
      sourceFormat === "latex"
        ? Math.max(latex.DEBOUNCE, editing ? 0 : READER_DEBOUNCE)
        : editing
          ? 60
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
  let commentsOpen = $state(true);
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

  // What every measurement below is made against.
  // The column to the right holds one of two things -- the comments or the
  // timeline -- so what the layout needs to know is whether it is there, not
  // which of them is in it.
  const panes = $derived({
    layout,
    comments: commentsOpen || historyOpen,
    editing,
    sourceSide,
    sizes,
    width,
  });
  const shown = $derived(showing(panes));

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

  // Everything the layout menu offers, named by what was chosen. The menu
  // reports the value of the line rather than each line calling back, so this
  // is the one place those names are read.
  function chose(what) {
    if (what === "side-left" || what === "side-right") return putSourceOn(what.slice(5));
    if (what.startsWith("ratio-")) return setSize(PANES.editor, Number(what.slice(6)));
    if (what === "linked") return setLinked(!linked);
  }

  function toggleComments() {
    commentsOpen = !commentsOpen;
    // The two share a column, so showing one puts the other away rather than
    // stacking them or splitting what is already the narrowest pane.
    if (commentsOpen && historyOpen) closeHistory();
  }

  /* ------------------------------------------------------------------- boot */

  // Joining the session is what shows the document: there is no stored page to
  // load, so a reader renders the text with the same module the editor
  // previews with, on a longer timer. Editing is not a second connection; it
  // is the source pane unfolding over the document this page already holds.
  function joinSession(document_) {
    session = collab.join({
      send: (message) => room.send(message),
      onPeers: (count) => (peers = Math.max(peers, count)),
      onState: (state_) => (persistence = state_),
      name: identity || read(AUTHOR, "Anonymous"),
      slug: SLUG,
      mayEdit,
    });
    session.watchSource(sourceChanged);
    // The text the editor is bound to is not the text it was bound to when a
    // migrated document's maps arrive. Re-keying the component is what makes
    // it bind again; the words do not change, only which type holds them.
    session.onSwap(() => (sourceEpoch += 1));
    // The directory, and who is in which file. Both change under this browser
    // rather than because of it, so both are watched rather than recomputed
    // after each of this browser's own actions.
    session.onFiles(filesChanged);
    session.awareness.on("change", refreshPeers);
    refreshFiles();
    room.send(session.open());
    void document_;
  }

  /* ------------------------------------------------------------- the files */

  // The directory as the list shows it, and where everyone's caret is. Held
  // as state rather than derived, because what they are derived from is a
  // CRDT that changes outside Svelte's knowledge.
  let files = $state([]);
  let openFile = $state("");
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
    files = session.list();
    // A file that went away under this browser -- somebody else deleted it --
    // leaves the editor showing something that is not there any more, so it
    // falls back to the document itself.
    if (openFile && !files.some((file) => file.id === openFile)) openFile = "";
    if (!openFile) openFile = session.mainId();
    // The main file's name is the document's format, and it can change: a
    // document whose main file becomes a .typ is a typst document from that
    // moment.
    const format = renderers.formatOf(session.mainPath());
    if (format && format !== sourceFormat) {
      sourceFormat = format;
      renderers.warm(format);
    }
  }

  // A file added, renamed, removed, or made the main one: the list is redrawn
  // and the document is rendered again, because every one of those changes
  // what a compiler would produce.
  function filesChanged() {
    refreshFiles();
    paintPreview();
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
    openFile = session.addText(path, "");
    paintPreview();
  }

  function renameFile(id, path) {
    session.renameFile(id, path);
  }

  function removeFile(file) {
    // The main file is the document. Removing it would leave nothing to
    // render, so the list does not offer it and this does not do it.
    if (file.id === session.mainId()) return;
    if (file.kind === "asset") session.removeAsset(file.path);
    else session.removeFile(file.id);
    if (openFile === file.id) openFile = session.mainId();
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
  async function addFigure(file) {
    try {
      const { sha } = await uploadAsset(SLUG, file, KEY);
      session.putAsset(file.name, sha);
      paintPreview();
    } catch (error) {
      say(error.message || "could not add that figure", true);
    }
  }

  /// The whole directory, as a zip. Built here rather than by a route,
  /// because everything it needs is already in this browser: the texts are in
  /// the shared document and the figures were fetched to render them, so
  /// asking the server to assemble what is already here would be a round trip
  /// to be told what we know.
  async function downloadTree() {
    const tree = treeNow();
    const files = { ...tree.texts };
    if (Object.keys(tree.digests || {}).length) {
      const held = await figures.gather(SLUG, tree.digests, {
        ...SHELL_HEADERS,
        ...keyHeaders(KEY),
      });
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
  }

  // A text dropped or chosen is read and added as a file. Its bytes are
  // words, so they belong in the shared document rather than in the store.
  async function addDroppedText(file) {
    try {
      openFile = session.addText(file.name, await file.text());
      paintPreview();
    } catch (error) {
      say(error.message || "could not read that file", true);
    }
  }

  // Dropping a file on the source pane does what the controls in the list do:
  // a text becomes a file, a figure is uploaded, and a name that is neither is
  // refused by the same rules.
  let fileList = $state(null);
  function dropped(event) {
    if (!mayEdit) return;
    event.preventDefault();
    const chosen = [...(event.dataTransfer?.files || [])];
    if (chosen.length) fileList?.offer(chosen);
  }

  // The source pane. Nothing is fetched here and nothing is seeded: the text
  // is already in the session, and this only shows it.
  async function startEditing() {
    if (editing || !mayEdit) return;
    Editor = (await import("./Editor.svelte")).default;
    editing = true;
    paintPreview();
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
    // A document is shown here only if this deployment can render what it was
    // written in. Markdown and HTML always; typst when its renderer was built.
    const list = Array.isArray(document_.renderers) ? document_.renderers : ["markdown"];
    // LaTeX is the one renderer that is a property of the deployment rather
    // than of the build: the compiler is behind `--latex`, not in the binary,
    // and a deployment with no mirror has nowhere to send this browser for
    // one. Told before the question below is asked.
    renderers.offerLatex(list.includes("latex"));
    latexReady = format === "latex" && Boolean(latex.chosen());
    if (!list.includes(format) || !renderers.available(format)) {
      say(`${format} documents are read where their renderer is built`, true);
      return;
    }
    mayEdit = Boolean(allowed);
    renderers.warm(format);
    joinSession(document_);
    // A document its author may edit opens ready to be worked on: that is what
    // they came for.
    if (mayEdit) startEditing();
    // Somebody sent a link to a moment rather than to the document. Opening it
    // opens the panel too, so that what is on the screen is explained by
    // something the reader can see and leave.
    if (ARRIVED_AT) {
      toggleHistory().then(() => showCheckpoint(ARRIVED_AT));
    }
  }

  // The socket is up or down. A socket that comes back has to rejoin: the
  // server hands the document out on `y-open` and nothing else, so without
  // this the changes made on either side of the gap never reach the other.
  function reconnected(up) {
    connected = up;
    if (!up) {
      session?.disconnected();
      return;
    }
    if (session) room.send(session.open());
  }

  $effect(() => {
    markViewed(SLUG);
    room = openRoom(SLUG, { onMessage: receive, onConnected: reconnected, key: KEY });

    whoami().then((who) => {
      me = who;
      if (who.name) {
        draft.creator = who.name;
        session?.rename(who.name);
      }
    });

    fetch(`/api/documents/${SLUG}`, { headers: keyHeaders(KEY) })
      .then((response) => (response.ok ? response.json() : Promise.reject(new Error("not found"))))
      .then((found) => {
        doc = found;
        visibility = found.visibility || "link";
        document.title = `${found.title} · Komodoc`;
        docsOrigin = found.docs_origin || location.origin;
        // The frame is an empty page with the agent in it, on the documents
        // origin. What goes into it is what this browser renders.
        // Set after `prepare`, which is what settles the format and so which
        // frame this document wants: a LaTeX document is a PDF and gets the
        // viewer, everything else gets the empty shell.
        prepare(found);
        frameSrc = `${docsOrigin}/${framePath}/${SLUG}/`;
      })
      // A private document answers a stranger exactly as a missing one does,
      // which tells a stranger nothing -- and tells a named reader who has not
      // signed in nothing either. That is what this line is for: the page was
      // opened at a real link, so the honest thing to say is both.
      .catch(() => {
        doc = { title: "Document not found" };
        say(
          me.providers?.length && !identity
            ? "not found — sign in, if this was shared with you"
            : "not found",
          true,
        );
      });

    return () => {
      // The session on the server ends when the last person in it
      // disconnects, which the socket closing does on its own; this is only
      // this browser letting go of its half.
      session?.leave();
      room?.close();
    };
  });

  // Ctrl-S is what a hand does after typing a paragraph, and there is nothing
  // for it to do: the document is already durable. What it must not do is
  // claim that pending writes are saved, so it says what is actually true.
  function reportPersistence() {
    if (!connected) {
      say(persistence.local ? "offline, changes kept in this browser" : "offline", true);
      return;
    }
    say(persistence.pending ? "saving\u2026" : "saved on the server");
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

<Nav {me}>
  {#snippet children()}
    <span id="docTitle" class="text-surface-600-400 truncate text-sm">{doc.title ?? ""}</span>
    <!-- Silent while the socket is up: it only has something to say when the
         live updates have stopped. -->
    {#if !connected}
      <small class="badge preset-tonal-warning whitespace-nowrap">reconnecting…</small>
    {/if}
  {/snippet}

  {#snippet tools()}
    <Row gap={3}>
      <!-- What you are looking at. Two controls rather than three switches:
           one arrangement of the source and the document, which only means
           anything while editing, and the comments, which are a column that is
           either there or not. -->
      <ControlGroup label="Layout">
        {#snippet children()}
          {#if editing}
            <!-- Right-click, or hold, for which side the source is on: an
                 order set once does not belong in a control flipped hourly. -->
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
                  <!-- The same ratios a drag sticks to, for anyone who never
                       finds that it does. -->
                  {#each RATIOS as ratio}
                    <Menu.Item value="ratio-{ratio.share}" class="menuitem">
                      <span class="w-4">{sizes[PANES.editor.key] === ratio.share ? "✓" : ""}</span>
                      {ratio.says}
                    </Menu.Item>
                  {/each}
                  <hr class="hr my-1" />
                  <!-- A preference rather than a mode: set once, and only about
                       this arrangement. It does not earn a place in the bar. -->
                  <Menu.Item value="linked" class="menuitem">
                    <span class="w-4">{linked ? "✓" : ""}</span>
                    Keep in step
                  </Menu.Item>
                </Menu.Content>
              </Menu.Positioner>
            </Menu>
          {/if}
          <IconButton
            icon="history"
            label="Show or hide the history"
            title="History"
            pressed={historyOpen}
            onclick={toggleHistory}
          />
          <IconButton
            icon="message-square"
            label="Show or hide the comments"
            title="Comments"
            pressed={commentsOpen}
            onclick={toggleComments}
          />
        {/snippet}
      </ControlGroup>
      <!-- The bar over an old version. It says which one is showing, because a
           document that is not the current one and does not say so is a way to
           quote something that was withdrawn a month ago; and it offers the
           way out, and the link that puts somebody else where the reader is.
           Naming lives on every row of the panel rather than only on this one,
           which is the same offer in a better place. -->
      {#if viewing}
        <small class="badge preset-tonal-warning whitespace-nowrap">
          Showing {viewingName} · {new Date(viewing.at).toLocaleString()}
        </small>
        <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                onclick={backToNow}>
          Back to now
        </button>
        <CopyLink href={checkpointLink(viewing.sha)} label="Copy the link to this version" />
      {/if}
      {#if editing}
        <!-- There is no save. What the toolbar says instead is whether this
             browser's work has reached the server, which is a different
             question from whether the socket is open and the only one worth
             answering. It is empty when there is nothing to say. -->
        {#if persistenceBadge}
          <small class="badge preset-tonal-warning whitespace-nowrap">{persistenceBadge}</small>
        {/if}
        <!-- Said only when there is more than one person editing. -->
        {#if peers > 1}
          <small class="badge preset-tonal-secondary whitespace-nowrap">{peers} editing</small>
        {/if}
        <!-- What the compiler said, counted. Clicking it goes to the first
             thing it complained about, and again to the next. -->
        <!-- A compile takes seconds, so the pane says one is running, and
             after the first says how long the last one took. Nothing is said
             between compiles: the page on screen is the answer. -->
        {#if compileBadge}
          <small class="badge preset-tonal-surface whitespace-nowrap">
            <span class="spinner" aria-hidden="true"></span>
            {compileBadge}
          </small>
        {/if}
        {#if sourceFormat === "latex" && latexReady}
          <IconButton
            icon="book"
            label="Choose a different TeX distribution"
            title="TeX distribution"
            pressed={cardOpen}
            onclick={() => (cardOpen = !cardOpen)}
          />
        {/if}
        {#if diagnosticBadge}
          <button type="button" onclick={goToDiagnostic}
                  title="Go to the next problem"
                  class="badge whitespace-nowrap {errorCount ? 'preset-tonal-error' : 'preset-tonal-warning'}">
            {diagnosticBadge}
          </button>
        {/if}
        {#if state}
          <small class="badge whitespace-nowrap {problem ? 'preset-tonal-error' : 'preset-tonal-surface'}">
            {state}
          </small>
        {/if}
      {/if}
      <!-- Sharing is the copy-link button grown up: copying the link is still
           the first thing inside it. Somebody with no place on the document
           gets the plain copy button, which is all it ever was for them. -->
      {#if canSeeSharing}
        <IconButton
          icon="users"
          label="Share this document"
          title="Share"
          onclick={() => (sharingOpen = true)}
        />
      {:else}
        <CopyLink href={linkFor(SLUG)} label="Copy the link to this document" />
      {/if}
    </Row>
  {/snippet}
</Nav>

<Share
  bind:open={sharingOpen}
  slug={SLUG}
  onvisibility={(chosen) => {
    // Making a document private changes where its bytes come from, so the
    // frame is reloaded rather than left showing what it was served before.
    visibility = chosen;
    if (docsOrigin) frameSrc = `${docsOrigin}/${framePath}/${SLUG}/?v=${++framedGeneration}`;
  }}
/>

<main class="reader" class:editing={shown.source} class:no-preview={!shown.document}
      class:no-comments={!shown.comments} class:source-right={sourceSide === "right"}
      style="--komodoc-editor: {pixels(PANES.editor, panes)}px; --komodoc-sidebar: {pixels(PANES.sidebar, panes)}px">
  {#if shown.source}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="editorpane" ondragover={(event) => event.preventDefault()} ondrop={dropped}>
      <!-- The directory, above the file being edited. A document with one
           file still has a list: it is one line, and it is where the control
           to add a second one lives. -->
      {#if files.length}
        <Files bind:this={fileList} {files} open={openFile} peers={peersByFile}
               {mayEdit} {rules} onopen={openTheFile} onadd={addFile}
               onrename={renameFile} onremove={removeFile} onmain={makeMain}
               onfigure={addFigure} ontext={addDroppedText}
               ondownload={downloadTree} />
      {/if}
      <!-- A figure has no editor. Choosing one shows it: an image as itself,
           a PDF through the browser's own viewer, which shows the first page
           without this application carrying a PDF renderer of its own. -->
      {#if shownFigure}
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
          <Editor bind:this={editor} {session} format={sourceFormat} file={openFile}
                  onchange={sourceChanged} oncaret={followCaret} onsave={reportPersistence} />
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
       Two states and two audiences: an editor who has not chosen a compiler
       gets the card, and a reader gets told plainly that nobody has compiled
       this yet. A reader is never shown the card -- nobody is asked to
       download a TeX distribution in order to read a paper. -->
  {#if shown.document && showsCard}
    <section class="latexpane">
      {#await import("./LatexCard.svelte") then { default: LatexCard }}
        <LatexCard onchosen={latexChosen} onerror={(why) => say(why, true)} />
      {/await}
    </section>
  {:else if shown.document && unrendered}
    <section class="latexpane">
      <div class="notyet">
        <h2 class="h4">Not yet rendered</h2>
        <p class="text-surface-700-300 text-sm">
          This is a LaTeX document, and no editor has compiled it in a browser
          yet. When one does, its pages appear here.
        </p>
        <button type="button" class="btn preset-tonal-surface" onclick={downloadTree}>
          Download the source
        </button>
      </div>
    </section>
  {/if}

  <!-- Kept mounted whatever the arrangement: taking the frame out of the tree
       would reload the document and lose the reader's place in it. -->
  <Preview bind:this={preview} src={frameSrc} {docsOrigin} onmessage={fromFrame} {grabbing}
           away={!shown.document || showsCard || unrendered} />

  {#if shown.comments}
    <Grip pane={PANES.sidebar} label="Resize the right-hand column" panes={panes}
          onsize={(size) => setSize(PANES.sidebar, size)}
          onguide={(where) => (guide = where)}
          ongrab={(on) => { grabbing = on; guide = { ...guide, shown: on }; }} />
    {#if historyOpen}
      <History {checkpoints} viewing={viewing?.sha || null} canEdit={mayEdit}
               problem={historyProblem}
               onshow={showCheckpoint} onback={backToNow} onname={nameCheckpoint} />
    {:else}
      <Sidebar {comments} {figureAt} {identity} {canModerate} {tool} {went}
               hasFigures={figureAt.length > 0}
               ontool={chooseTool}
               onreveal={(comment) => tell({ type: "reveal", id: comment.id })}
               onresolve={resolve} ondelete={askDelete} onreply={reply} />
    {/if}
  {/if}

  <!-- Shown only while a separator is dragged: a line that follows the pointer
       so the split can be seen moving without the iframe reflowing on every
       pointermove. -->
  {#if guide.shown}<div class="grip-guide" class:held={guide.held} style="left: {guide.left}px"></div>{/if}
</main>

{#if bar.shown}
  <button
    id="selectionbar"
    class="btn btn-sm preset-filled-primary-500 shadow-lg"
    style="display: block; left: {bar.left}px; top: {bar.top}px"
    onclick={barClicked}
  >
    {tool === "highlighting" ? "Highlight" : tool === "region" ? "Box" : "Comment"}
  </button>
{/if}

<!-- What a selection becomes, once the reader has said what to call it and
     what they think of it. -->
<Modal bind:open={commenting} title="Add comment">
  {#snippet children()}
    <form id="commentForm" class="flex flex-col gap-3" onsubmit={submitDialog}>
      <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3 text-sm">
        {pending?.region ? `Figure ${pending.region.image_index + 1}` : `“${pending?.exact ?? ""}”`}
      </blockquote>
      {#if identity}
        <p class="text-surface-600-400 text-sm">
          commenting as {me.provider === "github" ? `@${identity}` : identity}
        </p>
      {:else if me.comments_need_login}
        <p class="text-sm">
          <a class="anchor" href={signInHref()}>Sign in</a> to comment on this document.
        </p>
      {:else}
        <label class="label">
          <span class="label-text">Name</span>
          <input class="input" maxlength="80" bind:value={draft.creator} />
        </label>
      {/if}
      <label class="label">
        <span class="label-text">Comment</span>
        <!-- svelte-ignore a11y_autofocus -->
        <textarea class="textarea" rows="5" maxlength="5000" required autofocus bind:value={draft.body}
        ></textarea>
      </label>
      <label class="label">
        <span class="label-text">Tags <small class="text-surface-500">optional, comma separated</small></span>
        <input class="input" placeholder="methods, typo, citation" bind:value={draft.tags} />
      </label>
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
      onclick={() => { identifying = false; commenting = true; }}
    >
      Enter a name
    </button>
  {/snippet}
</Modal>

<Toasts />
