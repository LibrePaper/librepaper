<script>
  // One document: the source beside it, the page itself, and everything said
  // about it.
  import { anchorAll, flatten } from "../lib/anchor.js";
  import * as sync from "../lib/sync.js";
  import * as renderers from "../lib/renderers.js";
  import * as diagnosticsRule from "../lib/diagnostics.js";
  import * as collab from "../lib/collab.js";
  import { openRoom } from "../lib/room.js";
  import { me as whoami, signInHref } from "../lib/api.js";
  import { AUTHOR, LAYOUT, LINKED, SOURCE_SIDE, markViewed, read, write } from "../lib/storage.js";
  import { LAYOUTS, PANES, RATIOS, clamp, pixels, remember, showing, stored } from "../lib/panes.js";

  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import Nav from "./Nav.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import ControlGroup from "./ControlGroup.svelte";
  import CopyLink from "./CopyLink.svelte";
  import Modal from "./Modal.svelte";
  import Toasts from "./Toasts.svelte";
  import Row from "./layout/Row.svelte";
  import { problem as toastProblem } from "../lib/toast.svelte.js";
  import Preview from "./Preview.svelte";
  import Grip from "./Grip.svelte";
  import Sidebar from "./Sidebar.svelte";

  const SLUG = location.pathname.split("/").pop();

  /* ------------------------------------------------------------ the document */

  let doc = $state({});
  let docsOrigin = $state(null);
  let frameSrc = $state(null);
  let me = $state({});
  let identity = $derived(me.login || "");
  let canModerate = $derived(Boolean(doc.can_moderate));
  let connected = $state(true);

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
  const tell = (message) => preview?.tell(message);

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
  }

  function fromFrame(message) {
    switch (message.type) {
      case "ready":
        docText = typeof message.text === "string" ? message.text : "";
        docView = flatten(docText);
        // Where each figure sits in that text, so a note on a figure can be
        // ordered against the notes on passages.
        figureAt = Array.isArray(message.images) ? message.images.map(Number) : [];
        frameReady = true;
        // Whatever was painted before is gone with the rebuilt DOM.
        lastRegions = lastHighlight = null;
        reanchor();
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
    if (!identity && me.can_sign_in) {
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
        fetch(`/api/documents/${SLUG}/comments`)
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
  // named by its own first heading, the way `publish` names one.
  async function headingOf(source) {
    return doc.title || (await renderers.titleOf(source, sourceFormat)) || "Untitled";
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
  const paintsTheFrame = $derived(editing || sourceFormat !== "html");

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
    frameSrc = `${docsOrigin}/raw/${SLUG}/?v=${++framedGeneration}`;
  }

  async function paintPreview() {
    if (!paintsTheFrame) {
      refreshFramedPage();
      return;
    }
    const mine = ++issued;
    const source = session ? session.text.toString() : "";
    try {
      const { html, diagnostics: said } = await renderers.render(
        source,
        await headingOf(source),
        sourceFormat,
      );
      // A slower render that resolves late must not paint over a newer one.
      if (mine <= painted) return;
      painted = mine;
      if (html !== null) {
        // The page is what the document says now, so every error said about an
        // earlier state of it is cleared at once. The warnings that came with
        // this page are painted on the same slow schedule the errors are, so
        // that a font name half typed does not flash a badge on every
        // keystroke.
        tell({ type: "preview", html });
        everPainted = true;
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
          .failurePage(await headingOf(source), sourceFormat)
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
    previewTimer = setTimeout(paintPreview, editing ? 60 : READER_DEBOUNCE);
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
      const place = sync.documentPlaceFor(editor.text(), editor.caret(), docText, sourceFormat);
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
    const at = sync.sourcePlaceFor(docText, offset, editor.text(), sourceFormat);
    if (at === null) {
      lost(true);
      return;
    }
    lost(false);
    editor.goTo(at);
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
  const panes = $derived({ layout, comments: commentsOpen, editing, sourceSide, sizes, width });
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
    session.text.observe(sourceChanged);
    room.send(session.open());
    void document_;
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
    room = openRoom(SLUG, { onMessage: receive, onConnected: reconnected });

    whoami().then((who) => {
      me = who;
      if (who.login) {
        draft.creator = who.login;
        session?.rename(who.login);
      }
    });

    fetch(`/api/documents/${SLUG}`)
      .then((response) => (response.ok ? response.json() : Promise.reject(new Error("not found"))))
      .then((found) => {
        doc = found;
        document.title = `${found.title} · Komodoc`;
        docsOrigin = found.docs_origin || location.origin;
        // The frame is an empty page with the agent in it, on the documents
        // origin. What goes into it is what this browser renders.
        frameSrc = `${docsOrigin}/raw/${SLUG}/`;
        prepare(found);
      })
      .catch(() => (doc = { title: "Document not found" }));

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
            icon="message-square"
            label="Show or hide the comments"
            title="Comments"
            pressed={commentsOpen}
            onclick={toggleComments}
          />
        {/snippet}
      </ControlGroup>
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
      <CopyLink label="Copy the link to this document" />
    </Row>
  {/snippet}
</Nav>

<main class="reader" class:editing={shown.source} class:no-preview={!shown.document}
      class:no-comments={!shown.comments} class:source-right={sourceSide === "right"}
      style="--komodoc-editor: {pixels(PANES.editor, panes)}px; --komodoc-sidebar: {pixels(PANES.sidebar, panes)}px">
  {#if shown.source}
    <section class="editorpane">
      {#if Editor}
        <Editor bind:this={editor} {session} format={sourceFormat}
                onchange={sourceChanged} oncaret={followCaret} onsave={reportPersistence} />
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

  <!-- Kept mounted whatever the arrangement: taking the frame out of the tree
       would reload the document and lose the reader's place in it. -->
  <Preview bind:this={preview} src={frameSrc} {docsOrigin} onmessage={fromFrame} {grabbing}
           away={!shown.document} />

  {#if shown.comments}
    <Grip pane={PANES.sidebar} label="Resize the comment pane" panes={panes}
          onsize={(size) => setSize(PANES.sidebar, size)}
          onguide={(where) => (guide = where)}
          ongrab={(on) => { grabbing = on; guide = { ...guide, shown: on }; }} />
    <Sidebar {comments} {figureAt} {identity} {canModerate} {tool}
             hasFigures={figureAt.length > 0}
             ontool={chooseTool}
             onreveal={(comment) => tell({ type: "reveal", id: comment.id })}
             onresolve={resolve} ondelete={askDelete} onreply={reply} />
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
        <p class="text-surface-600-400 text-sm">commenting as @{identity}</p>
      {:else if me.comments_need_login}
        <p class="text-sm">
          <a class="anchor" href={signInHref()}>Sign in with GitHub</a> to comment on this document.
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
      Sign in with GitHub
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
