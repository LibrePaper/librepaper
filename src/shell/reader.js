import { anchorAll, flatten } from "./anchor.js";

const SLUG = location.pathname.split("/").pop();
const frame = document.getElementById("docframe");
const bar = document.getElementById("selectionbar");
const box = document.getElementById("comments");
const countEl = document.getElementById("count");
const connEl = document.getElementById("conn");
const dialog = document.getElementById("commentDialog");

let comments = [];
let pending = null;
let socket = null;
let backoff = 500;
let docText = null; // joined visible text, invariant across highlight renders
let docView = null; // flatten(docText), cached so anchoring does not redo it per call
let figureAt = []; // text offset of each figure, by its index in the document
let frameReady = false;
let commentsReady = false;
let identity = ""; // GitHub login, when signed in; comments are signed with it
let canModerate = false; // true when the caller owns this document (GET /api/documents/{slug})

// A browser cannot set a custom header on a cross-origin request without a
// CORS preflight, which the server never grants -- so this header is proof,
// to the server, that a state-changing request came from this page and not
// from a hostile document on the sibling docs host.
const SHELL_HEADERS = { "X-Komodoc-Client": "shell" };

// The Delete button is only ever real for: a comment the server says this
// caller may delete, one the caller just posted and is still waiting to be
// confirmed (temp_id/pending), or a caller who owns the document outright.
function canDelete(comment) {
  return Boolean(comment.deletable) || Boolean(comment.temp_id) || Boolean(comment.pending) || canModerate;
}

// One card per comment, kept across renders so a rebuild never wipes an open
// reply draft, an expanded quotation, or scroll position. Keyed by the
// comment object itself (not its id): `receive` mutates a comment in place
// when the server confirms it (Object.assign onto the same object), so the
// object's identity survives an id change and the card follows it for free.
const cards = new Map();

/* ----------------------------------------------------------------- frame */

// The document is on its own origin, so nothing here can touch it. agent.js,
// injected into the document, does the DOM work and reports back. Anchoring
// stays on this side: the agent sends text, this sends back offsets to paint.
//
// Everything arriving from the frame is untrusted. The agent shares an origin
// with the document, and a hostile document can rewrite it.

let docsOrigin = null; // learned from the API, and the only origin we accept

function tell(message) {
  if (!docsOrigin || !frame.contentWindow) return;
  frame.contentWindow.postMessage({ komodoc: true, ...message }, docsOrigin);
}

addEventListener("message", (event) => {
  if (!docsOrigin || event.origin !== docsOrigin || event.source !== frame.contentWindow) return;
  const message = event.data;
  if (!message || message.komodoc !== true) return;

  switch (message.type) {
    case "ready":
      // The whole visible text of the document, in one string.
      docText = typeof message.text === "string" ? message.text : "";
      // Computed once here, not once per anchorAll call.
      docView = flatten(docText);
      // Where each figure sits in that text, so a note on a figure can be
      // ordered against the notes on passages.
      figureAt = Array.isArray(message.images) ? message.images.map(Number) : [];
      frameReady = true;
      // The agent's DOM was just rebuilt and rescanned, so whatever was
      // painted before is gone; the next applyHighlights must repaint in full.
      lastRegions = lastHighlight = null;
      reanchor();
      break;

    case "selection":
      showSelection(message.selector, message.rect);
      break;

    case "region":
      // A rectangle drawn on a figure, which anchors the same way a quotation
      // does: it becomes what the next annotation is about.
      pending = { exact: "", prefix: "", suffix: "", position: null, region: message.region };
      placeBar(message.rect);
      break;

    case "focus":
      document
        .getElementById("comment-" + message.id)
        ?.scrollIntoView({ behavior: "smooth", block: "center" });
      break;
  }
});

// Hand the agent the positions to paint. It knows nothing about anchoring.
//
// The agent repaints the whole document on every "regions" or "highlight"
// message, so a call that changes nothing is not free even though it looks
// idempotent. Each is sent only when its payload actually differs from the
// last one sent -- reset when the frame republishes its text, since the
// agent's DOM was rebuilt then and needs the full repaint regardless.
let lastRegions = null;
let lastHighlight = null;

function applyHighlights() {
  if (!frameReady) return;
  // Annotations on figures are placed by the agent from the image identifiers,
  // since there is no text for this side to anchor against.
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
  // A region annotation is placed by the agent, not by text matching, so it is
  // never orphaned for want of a quotation.
  anchorAll(docText, comments.filter((comment) => !comment.region), docView);
  render();
  applyHighlights();
}

/* -------------------------------------------------------------- selection */

// The agent reports a selector and where it sits inside the frame; the button
// goes over it, in this document's coordinates.
function showSelection(selector, rect) {
  if (!selector || !selector.exact) {
    bar.style.display = "none";
    pending = null;
    return;
  }
  pending = {
    exact: String(selector.exact),
    prefix: String(selector.prefix || ""),
    suffix: String(selector.suffix || ""),
    // A hint, not a claim: the server keeps it and anchoring uses it only to
    // choose between passages the context cannot separate.
    position: Number.isInteger(selector.position) && selector.position >= 0 ? selector.position : null,
  };
  placeBar(rect);
}

// The button goes over whatever was chosen, in this document's coordinates.
function placeBar(rect) {
  if (rect && !matchMedia("(max-width:760px)").matches) {
    const frameRect = frame.getBoundingClientRect();
    bar.style.left =
      Math.min(innerWidth - 105, frameRect.left + rect.left + (rect.right - rect.left) / 2 - 42) + "px";
    bar.style.top = Math.max(65, frameRect.top + rect.top - 42) + "px";
  }
  bar.style.display = "block";
}
/* --------------------------------------------------------------- sidebar */

function element(tag, text) {
  const el = document.createElement(tag);
  el.textContent = text;
  return el;
}

// <mark> is Pico's highlight, which is what a small status label wants to be.
function mark(text, motivation) {
  const el = element("mark", text);
  // The label wears the same hue the passage does, so the sidebar and the
  // document agree about what kind of annotation this is.
  if (motivation) el.dataset.motivation = motivation;
  return el;
}

// A passage can be a paragraph long, which would bury the comment made about
// it. The sidebar shows the opening words and expands on request.
const QUOTE_WORDS = 8;

// `card` is where "expanded or not" lives, since the quote itself gets
// rebuilt whenever anything else about the comment changes (see updateStatic)
// and a plain closure variable would forget the reader's click at that point.
function quoteOf(exact, card) {
  const quote = document.createElement("blockquote");
  const words = exact.split(/\s+/);
  if (words.length <= QUOTE_WORDS + 2) {
    quote.textContent = "“" + exact + "”";
    return quote;
  }

  const short = words.slice(0, QUOTE_WORDS).join(" ");
  const text = document.createElement("span");
  const toggle = document.createElement("a");
  toggle.href = "#";

  const draw = () => {
    text.textContent = card.quoteOpen ? "“" + exact + "”" : "“" + short;
    toggle.textContent = card.quoteOpen ? " less" : "… ”";
  };
  toggle.onclick = (event) => {
    event.preventDefault();
    event.stopPropagation(); // the card itself scrolls to the highlight
    card.quoteOpen = !card.quoteOpen;
    draw();
  };

  draw();
  quote.append(text, toggle);
  return quote;
}

// The confirmation is a modal dialog whose OK button is the default and has
// focus, so Enter confirms and Escape cancels.
const deleteDialog = document.getElementById("deleteDialog");
function confirmDelete() {
  return new Promise((resolve) => {
    deleteDialog.returnValue = "";
    deleteDialog.showModal();
    deleteDialog.addEventListener(
      "close",
      () => resolve(deleteDialog.returnValue === "ok"),
      { once: true },
    );
  });
}

const stamp = (value) => value.replace("T", " ").slice(0, 16) + " UTC";

// Filtering by tag: an empty set shows everything, and an annotation matches
// if it carries every tag chosen.
let chosenTags = new Set();

function toggleTag(tag) {
  chosenTags.has(tag) ? chosenTags.delete(tag) : chosenTags.add(tag);
  render();
}

function drawTagFilter() {
  const filter = document.getElementById("tagFilter");
  const all = new Set();
  for (const comment of comments) for (const tag of comment.tags || []) all.add(tag);

  filter.innerHTML = "";
  filter.hidden = all.size === 0;
  for (const tag of [...all].sort()) {
    const chip = element("button", tag);
    chip.type = "button";
    chip.className = chosenTags.has(tag) ? "tag chosen" : "tag";
    chip.onclick = () => toggleTag(tag);
    filter.appendChild(chip);
  }
}

// The sidebar reads in document order: an annotation is about a place in the
// text, so the column follows the page rather than the order things were
// written. A note on a figure sorts by where that figure sits in the text.
// Anything that could not be anchored has no place to sort by, so it goes to
// the end, in the order it was made.
function place(comment) {
  if (comment.region) {
    const at = figureAt[comment.region.image_index];
    if (Number.isFinite(at)) return at;
  }
  return Number.isFinite(comment.start) ? comment.start : Infinity;
}

// Everything that goes into the static part of a card (the part rebuilt
// wholesale when it changes) -- resolved/pending, replies and id are handled
// elsewhere, since those can change on their own without touching this.
function cardSignature(comment) {
  return JSON.stringify([
    comment.exact,
    comment.region,
    comment.replacement,
    comment.body,
    comment.tags,
    comment.motivation,
    comment.creator,
    comment.created,
    comment.orphaned,
  ]);
}

// The quote, marks, suggestion, body, tags and byline: whatever the reader
// cannot type into and cannot leave mid-edit, so rebuilding it from scratch
// on change costs nothing worth preserving.
function updateStatic(card, comment) {
  const wrap = card.staticWrap;
  wrap.innerHTML = "";
  if (comment.orphaned) wrap.appendChild(mark("Needs re-anchoring"));
  // The motivation is the W3C annotation type. Commenting is the default,
  // so only the others are worth showing.
  if (comment.motivation && comment.motivation !== "commenting") {
    wrap.appendChild(mark(comment.motivation, comment.motivation));
  }
  if (comment.region) {
    const where = element("blockquote", `Figure ${comment.region.image_index + 1}`);
    where.className = "figureref";
    wrap.appendChild(where);
  } else {
    wrap.appendChild(quoteOf(comment.exact, card));
  }
  // A suggested edit reads as what it proposes, not as a remark about it.
  if (comment.replacement) {
    const suggestion = element("p", comment.replacement);
    suggestion.className = "suggestion";
    wrap.appendChild(suggestion);
  }
  if (comment.body) wrap.appendChild(element("p", comment.body));
  for (const tag of comment.tags || []) {
    const chip = element("button", tag);
    chip.type = "button";
    chip.className = "tag";
    chip.onclick = (event) => {
      event.stopPropagation();
      toggleTag(tag);
    };
    wrap.appendChild(chip);
  }
  wrap.appendChild(element("small", comment.creator + " · " + stamp(comment.created)));
}

// Built once per comment and reused after that. `card.comment` is a
// reference into `comments`, and `receive` mutates that object in place
// (Object.assign), so handlers below always see the current id/resolved/etc
// without this needing to be told about it.
function makeCard(comment) {
  const el = document.createElement("article");

  const staticWrap = document.createElement("div");
  el.appendChild(staticWrap);

  const repliesList = document.createElement("ul");
  repliesList.hidden = true;
  el.appendChild(repliesList);

  // Not role="group": that is Pico's segmented control, which joins its
  // buttons into one shape. These are two separate actions.
  const actions = document.createElement("div");
  actions.className = "actions";
  const resolveBtn = document.createElement("button");
  const replyButton = document.createElement("button");
  replyButton.textContent = "Reply";
  const deleteButton = document.createElement("button");
  deleteButton.textContent = "Delete";
  actions.append(resolveBtn, replyButton, deleteButton);
  el.appendChild(actions);

  const card = {
    el,
    staticWrap,
    repliesList,
    repliesDrawn: [], // reply objects already rendered as <li>, in order
    actions,
    resolveBtn,
    form: null, // created lazily, on first "Reply" click
    nameInput: null,
    bodyInput: null,
    quoteOpen: false,
    sig: null,
    comment,
    deleteButton,
  };

  resolveBtn.onclick = (event) => {
    event.stopPropagation();
    // Optimistic: flip locally, then tell the room. The broadcast that
    // comes back is idempotent with what we already drew.
    const target = card.comment;
    target.resolved = !target.resolved;
    render();
    applyHighlights();
    send({ type: "resolve", comment_id: target.id, resolved: target.resolved });
  };

  deleteButton.onclick = async (event) => {
    event.stopPropagation();
    if (!(await confirmDelete())) return;
    // Optimistic, like resolve: drop it locally, then tell the room.
    const target = card.comment;
    comments = comments.filter((item) => item !== target);
    render();
    applyHighlights();
    send({ type: "delete", comment_id: target.id });
  };

  replyButton.onclick = (event) => {
    event.stopPropagation();
    ensureForm(card);
    card.form.hidden = !card.form.hidden;
    if (!card.form.hidden) card.bodyInput.focus();
  };

  el.onclick = (event) => {
    const target = card.comment;
    if (!target.orphaned && !event.target.closest("button,input,textarea")) {
      tell({ type: "reveal", id: target.id });
    }
  };

  return card;
}

// Lazy, so a document with hundreds of comments does not carry hundreds of
// unused inputs, textareas and buttons that nobody ever clicked into.
function ensureForm(card) {
  if (card.form) return;
  const form = document.createElement("form");
  form.hidden = true;
  const name = document.createElement("input");
  name.placeholder = "Name";
  name.value = identity || localStorage.getItem("komodoc-author") || "Anonymous";
  name.maxLength = 80;
  // Signed in, the reply is signed by the account; there is nothing to type.
  name.hidden = Boolean(identity);
  const body = document.createElement("textarea");
  body.placeholder = "Reply";
  body.rows = 2;
  body.maxLength = 5000;
  body.required = true;
  const submit = document.createElement("button");
  submit.type = "submit";
  submit.textContent = "Add reply";
  form.append(name, body, submit);
  form.onclick = (event) => event.stopPropagation();
  form.onsubmit = (event) => {
    event.preventDefault();
    const comment = card.comment;
    if (!identity) localStorage.setItem("komodoc-author", name.value);
    const temp_id = crypto.randomUUID();
    comment.replies.push({
      id: temp_id,
      body: body.value,
      creator: name.value || "Anonymous",
      created: new Date().toISOString(),
      temp_id,
    });
    body.value = "";
    render();
    send({
      type: "reply",
      comment_id: comment.id,
      body: comment.replies[comment.replies.length - 1].body,
      creator: name.value,
      temp_id,
    });
  };
  card.el.appendChild(form);
  card.form = form;
  card.nameInput = name;
  card.bodyInput = body;
}

// Reuses cards across renders instead of wiping and rebuilding the whole
// column, so an open reply draft, an expanded quotation and the scroll
// position all survive a render triggered by someone else's comment.
function updateCard(card, comment) {
  card.el.id = "comment-" + comment.id;

  const sig = cardSignature(comment);
  if (card.sig !== sig) {
    updateStatic(card, comment);
    card.sig = sig;
  }

  // resolved/pending can flip on their own, independent of everything above.
  card.el.className = comment.resolved || comment.pending ? "resolved" : "";
  card.resolveBtn.textContent = comment.resolved ? "Reopen" : "Resolve";
  // Deletability can change too: an optimistic comment gets `deletable: true`
  // once the server confirms it, and canModerate can arrive after the first render.
  card.deleteButton.hidden = !canDelete(comment);

  // Replies are matched by object, not id: a reply's id changes from temp_id
  // to the server's (the same Object.assign-in-place pattern as a comment).
  // They are normally append-only, but a server error rolls an optimistic
  // reply back out of the list, so the list is redrawn whenever what is drawn
  // stops matching what is there.
  const drawn = card.repliesDrawn;
  const same =
    drawn.length <= comment.replies.length && drawn.every((reply, i) => reply === comment.replies[i]);
  if (!same) {
    card.repliesList.innerHTML = "";
    drawn.length = 0;
  }
  for (const reply of comment.replies.slice(drawn.length)) {
    const item = document.createElement("li");
    item.appendChild(element("span", reply.body));
    item.appendChild(document.createElement("br"));
    item.appendChild(element("small", reply.creator + " · " + stamp(reply.created)));
    card.repliesList.appendChild(item);
    drawn.push(reply);
  }
  card.repliesList.hidden = comment.replies.length === 0;

  // The name field is the one part of an open form that identity arriving
  // (or changing) should still touch.
  if (card.form) {
    card.nameInput.hidden = Boolean(identity);
    if (identity) card.nameInput.value = identity;
  }
}

function render() {
  drawTagFilter();
  const shown = comments
    .filter((comment) => [...chosenTags].every((tag) => (comment.tags || []).includes(tag)))
    .sort((a, b) => place(a) - place(b) || a.seq - b.seq);

  // A card survives being hidden by the tag filter (its draft and state stay
  // in `cards`); it is only ever dropped once its comment is gone for good.
  const alive = new Set(comments);
  for (const [comment, card] of cards) {
    if (!alive.has(comment)) {
      card.el.remove();
      cards.delete(comment);
    }
  }

  const elements = shown.map((comment) => {
    let card = cards.get(comment);
    if (!card) {
      card = makeCard(comment);
      cards.set(comment, card);
    }
    updateCard(card, comment);
    return card.el;
  });

  // Drop whatever is currently in `box` but not in this render's list (a
  // comment now filtered out, or deleted), then walk the rest into order.
  // Only a card that is out of place is moved: moving a node detaches and
  // reattaches it, which would blur a reply someone is in the middle of
  // typing every time another comment arrived.
  const keep = new Set(elements);
  for (const child of [...box.children]) {
    if (!keep.has(child)) box.removeChild(child);
  }
  elements.forEach((el, i) => {
    if (box.children[i] !== el) box.insertBefore(el, box.children[i] || null);
  });

  const open = comments.filter((comment) => !comment.resolved).length;
  countEl.textContent = comments.length ? `${open} open · ${comments.length} total` : "";
  // The instruction is onboarding, not a caption: it goes once there is
  // something in the column to read.
  document.getElementById("hint").hidden = comments.length > 0;
}

/* ------------------------------------------------------------------ wire */

// A socket that drops loses nothing: comments still post over the REST route
// and the hello on reconnect resends the list. What it does cost is seeing
// other people's comments as they arrive, which is worth saying -- but only
// once it has lasted longer than a blip, and only while it is true.
let dropped = null;

function setConnected(up) {
  clearTimeout(dropped);
  if (up) {
    connEl.hidden = true;
    return;
  }
  dropped = setTimeout(() => {
    connEl.hidden = false;
  }, 2000);
}

function send(message) {
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(message));
    return;
  }
  // The socket is down; fall back to the REST route so the write is not lost.
  fetch(`/api/documents/${SLUG}/comments`, {
    method: "POST",
    headers: { "content-type": "application/json", ...SHELL_HEADERS },
    body: JSON.stringify(message),
  })
    .then((response) => response.json())
    .then(receive)
    .catch(() => setConnected(false));
}

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
      comments = comments.filter((comment) => comment.temp_id !== event.temp_id);
      comments.forEach((comment) => {
        comment.replies = comment.replies.filter((reply) => reply.temp_id !== event.temp_id);
      });
      render();
      applyHighlights();
    }
    // A refused delete or resolve was applied optimistically before the
    // server had a say; the comment named in the refusal is re-fetched from
    // the REST route and the whole list replaced, the same way the initial
    // hello populates it, so the optimistic change is rolled back.
    if (event.comment_id) {
      fetch(`/api/documents/${SLUG}/comments`)
        .then((response) => response.json())
        .then((data) => receive({ type: "hello", comments: data.comments }))
        .catch(() => {});
    }
    alert(event.message);
    return;
  }
  if (event.type === "comment") {
    const local = comments.find((comment) => comment.temp_id === event.temp_id);
    // Broadcasts carry no `deletable` field, so the caller's own comment,
    // reconciled here from its optimistic placeholder, stays deletable by
    // this browser regardless of what the server sent back.
    if (local) Object.assign(local, event.comment, { temp_id: undefined, pending: false, deletable: true });
    else if (!comments.some((comment) => comment.id === event.comment.id)) {
      comments.push(event.comment);
      // Someone else's comment: anchor just this one against the cached text.
      anchorAll(docText || "", [event.comment], docText === null ? null : docView);
    }
    render();
    applyHighlights();
    return;
  }
  if (event.type === "reply") {
    const comment = comments.find((item) => item.id === event.comment_id);
    if (!comment) return;
    const local = comment.replies.find((reply) => reply.temp_id === event.temp_id);
    if (local) Object.assign(local, event.reply, { temp_id: undefined });
    else if (!comment.replies.some((reply) => reply.id === event.reply.id)) {
      comment.replies.push(event.reply);
    }
    render();
    return;
  }
  if (event.type === "delete") {
    comments = comments.filter((item) => item.id !== event.comment_id);
    render();
    applyHighlights();
    return;
  }
  if (event.type === "resolve") {
    const comment = comments.find((item) => item.id === event.comment_id);
    if (!comment) return;
    comment.resolved = event.resolved;
    comment.resolved_at = event.resolved_at;
    render();
    applyHighlights();
  }
}

function connect() {
  socket = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws/${SLUG}`);
  socket.onopen = () => {
    backoff = 500;
    setConnected(true);
  };
  socket.onmessage = (event) => receive(JSON.parse(event.data));
  socket.onclose = () => {
    setConnected(false);
    // The hello on reconnect resends the full list, so a missed broadcast
    // during the gap heals itself.
    setTimeout(connect, backoff);
    backoff = Math.min(backoff * 2, 15000);
  };
  socket.onerror = () => socket.close();
}

/* ------------------------------------------------------------------ boot */

/* ------------------------------------------------------------------- link */

// The address bar already holds the link; the button spares the reader from
// selecting it. The icon turns into a tick for a moment, since a copy is
// otherwise invisible.
const copyButton = document.getElementById("copyLink");
const linkIcon = copyButton.innerHTML;
let linkTimer;

copyButton.onclick = async () => {
  try {
    await navigator.clipboard.writeText(location.href);
  } catch {
    return; // clipboard blocked; the address bar is still there
  }
  copyButton.classList.add("done");
  copyButton.innerHTML =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';
  clearTimeout(linkTimer);
  linkTimer = setTimeout(() => {
    copyButton.classList.remove("done");
    copyButton.innerHTML = linkIcon;
  }, 1500);
};

// A GET can be forced onto a signed-in reader cross-site, so signing out is a
// POST, carrying the same header every other state change does.
document.getElementById("signOut").onclick = async () => {
  await fetch("/auth/logout", { method: "POST", headers: SHELL_HEADERS }).catch(() => {});
  location.reload();
};

/* ------------------------------------------------------------------ tools */

// The tool decides what a selection becomes. Highlighting is the one that
// needs no words, so it is made on the spot; the rest open the dialog with
// their kind already settled.
let tool = "commenting";
const toolButtons = [...document.querySelectorAll(".tool")];

for (const button of toolButtons) {
  button.onclick = () => {
    tool = button.dataset.tool;
    tell({ type: "tool", tool });
    for (const other of toolButtons) {
      other.setAttribute("aria-pressed", String(other === button));
      other.className = other === button ? "tool" : "tool outline";
    }
    bar.textContent = button.getAttribute("aria-label");
  };
}
// Everything but the pressed one starts outlined.
toolButtons.filter((b) => b.dataset.tool !== tool).forEach((b) => (b.className = "tool outline"));

bar.onclick = () => {
  if (!pending) return;
  bar.style.display = "none";

  if (tool === "highlighting") {
    // No dialog: the passage is the whole annotation.
    submitAnnotation({ motivation: "highlighting", body: "", replacement: "", tags: [] });
    return;
  }

  // If not logged in, ask how they want to identify
  if (!identity) {
    const identityDialog = document.getElementById("identityDialog");
    identityDialog.showModal();
    return;
  }

  showCommentDialog();
};

function showCommentDialog() {
  document.getElementById("selectedQuote").textContent = pending.region
    ? `Figure ${pending.region.image_index + 1}`
    : "“" + pending.exact + "”";
  document.getElementById("replacementField").hidden = tool !== "editing";
  // A region has no passage to replace, so the edit tool falls back to a remark.
  document.getElementById("bodyLabel").textContent =
    tool === "editing" ? "Why" : tool === "questioning" ? "Question" : "Comment";
  if (tool === "editing") document.getElementById("replacement").value = pending.exact;
  dialog.showModal();
  document.getElementById(tool === "editing" ? "replacement" : "body").focus();
}

// One path for every kind, whether it came from the dialog or straight from
// the highlight tool.
function submitAnnotation({ motivation, body, replacement, tags }) {
  if (!pending) return;
  const creator = document.getElementById("author").value;
  if (!identity) localStorage.setItem("komodoc-author", creator);
  const temp_id = crypto.randomUUID();
  const optimistic = {
    id: temp_id,
    temp_id,
    seq: Number.MAX_SAFE_INTEGER,
    ...pending,
    motivation,
    body,
    replacement,
    tags,
    creator: creator || "Anonymous",
    created: new Date().toISOString(),
    resolved: false,
    resolved_at: null,
    replies: [],
    pending: true,
  };
  // Draw it before the round trip; `receive` reconciles it by temp_id.
  anchorAll(docText || "", [optimistic], docText === null ? null : docView);
  comments.push(optimistic);
  render();
  applyHighlights();
  send({ type: "comment", ...pending, motivation, body, replacement, tags, creator, temp_id });
  pending = null;
}

document.getElementById("commentForm").onsubmit = (event) => {
  event.preventDefault();
  submitAnnotation({
    motivation: tool === "region" ? "commenting" : tool,
    body: document.getElementById("body").value,
    replacement: tool === "editing" ? document.getElementById("replacement").value : "",
    tags: parseTags(document.getElementById("tags").value),
  });
  document.getElementById("body").value = "";
  document.getElementById("replacement").value = "";
  document.getElementById("tags").value = "";
  dialog.close();
};

// "methods, typo" becomes ["methods", "typo"]. The server normalises again,
// so this only has to be reasonable.
function parseTags(value) {
  return value
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);
}

document.getElementById("author").value = localStorage.getItem("komodoc-author") || "Anonymous";

// Identity dialog handlers
const identityDialog = document.getElementById("identityDialog");
document.getElementById("signInChoice").onclick = () => {
  identityDialog.close();
  window.location.href = `/auth/login?next=${encodeURIComponent(location.pathname)}`;
};

document.getElementById("nameChoice").onclick = () => {
  identityDialog.close();
  showCommentDialog();
};

// Document bytes and comments are fetched in parallel; whichever lands last
// triggers the single anchoring pass.
connect();
fetch(`/api/documents/${SLUG}`)
  .then((response) => (response.ok ? response.json() : Promise.reject(new Error("not found"))))
  .then((doc) => {
    document.title = `${doc.title} · Komodoc`;
    document.getElementById("docTitle").textContent = doc.title;
    // Documents live on their own origin, which the deployment names.
    docsOrigin = doc.docs_origin || location.origin;
    frame.src = `${docsOrigin}/raw/${SLUG}/${doc.sha}.html`;
    // Whether this caller owns the document, which unlocks deleting anyone's
    // comment on it. This can arrive after comments have already rendered
    // once without it, so redraw to pick it up.
    canModerate = Boolean(doc.can_moderate);
    render();
  })
  .catch(() => {
    document.getElementById("docTitle").textContent = "Document not found";
  });

// Who you are decides how your comments are signed. Signed in, the name is
// your GitHub login and there is nothing to type; the server ignores anything
// sent in its place. Signed out, you type a name, unless this deployment
// requires an account to comment at all.
fetch("/api/me")
  .then((response) => response.json())
  .then((me) => {
    identity = me.login || "";
    // The nav shows who you are on every page, landing or document.
    document.getElementById("who").hidden = !identity;
    document.getElementById("who").textContent = identity ? "@" + identity : "";
    const signIn = document.getElementById("signIn");
    signIn.hidden = Boolean(identity) || !me.can_sign_in;
    signIn.href = `/auth/login?next=${encodeURIComponent(location.pathname)}`;
    document.getElementById("signOut").hidden = !identity;
    const name = document.getElementById("author");
    const label = document.getElementById("nameLabel");

    if (identity) {
      name.value = identity;
      name.hidden = label.hidden = true;
      const note = document.getElementById("signedInAs");
      note.textContent = "commenting as @" + identity;
      note.hidden = false;
      render(); // reply forms lose their name field too
      return;
    }
    if (me.comments_need_login) {
      name.hidden = label.hidden = true;
      document.getElementById("signInToComment").hidden = false;
      document.getElementById("commentForm").querySelector('button[type="submit"]').disabled = true;
    }
  })
  .catch(() => {});

/* ------------------------------------------------------------------- grip */

// The comment pane can be dragged wider or narrower, within limits: never so
// wide that the document is a strip, never so narrow that a comment cannot be
// read. The width is remembered per reader, not per document.
const SIDEBAR_KEY = "komodoc-sidebar";
const SIDEBAR_MIN = 240; // px, about the narrowest a comment card reads at
const SIDEBAR_MAX = 0.6; // of the window, so the document always keeps 40%

const reader = document.querySelector("main.reader");
const grip = document.getElementById("grip");
const gripGuide = document.getElementById("gripGuide");

function clampSidebar(width) {
  return Math.round(Math.max(SIDEBAR_MIN, Math.min(width, innerWidth * SIDEBAR_MAX)));
}

function setSidebar(width) {
  const limit = clampSidebar(width);
  reader.style.setProperty("--komodoc-sidebar", limit + "px");
  return limit;
}

try {
  const remembered = Number(localStorage.getItem(SIDEBAR_KEY));
  if (remembered) setSidebar(remembered);
} catch {
  /* storage disabled; the default width stands */
}

if (grip) {
  // While dragging, only the guide line moves; the real width -- and the
  // iframe reflow that comes with it -- is applied once, on release.
  function guideAt(width) {
    if (gripGuide) gripGuide.style.left = innerWidth - clampSidebar(width) + "px";
  }

  grip.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    grip.classList.add("dragging");
    // The frame swallows pointer events while it has them, so it is deafened
    // for the duration of the drag rather than the drag being lost over it.
    frame.style.pointerEvents = "none";
    if (gripGuide) {
      guideAt(innerWidth - event.clientX);
      gripGuide.hidden = false;
    }
  });

  grip.addEventListener("pointermove", (event) => {
    if (!grip.hasPointerCapture(event.pointerId)) return;
    guideAt(innerWidth - event.clientX);
  });

  const finish = (event) => {
    if (!grip.hasPointerCapture(event.pointerId)) return;
    grip.releasePointerCapture(event.pointerId);
    grip.classList.remove("dragging");
    frame.style.pointerEvents = "";
    if (gripGuide) gripGuide.hidden = true;
    try {
      localStorage.setItem(SIDEBAR_KEY, String(setSidebar(innerWidth - event.clientX)));
    } catch {
      /* the width still applies to this page */
    }
  };
  grip.addEventListener("pointerup", finish);
  grip.addEventListener("pointercancel", finish);

  // Keyboard: the separator is focusable, so it should move without a pointer.
  grip.addEventListener("keydown", (event) => {
    const step = event.key === "ArrowLeft" ? 24 : event.key === "ArrowRight" ? -24 : 0;
    if (!step) return;
    event.preventDefault();
    const width = setSidebar(reader.getBoundingClientRect().width - frame.getBoundingClientRect().width + step);
    try {
      localStorage.setItem(SIDEBAR_KEY, String(width));
    } catch {
      /* nothing to remember it with */
    }
  });
}

// A window narrower than the remembered width would leave no document.
addEventListener("resize", () => {
  const current = parseInt(getComputedStyle(reader).getPropertyValue("--komodoc-sidebar"), 10);
  if (current) setSidebar(current);
});

// When this document was last opened, kept per reader so the landing page can
// sort by it. Nothing about it leaves the browser.
try {
  const seen = JSON.parse(localStorage.getItem("komodoc-viewed") || "{}");
  seen[SLUG] = new Date().toISOString();
  localStorage.setItem("komodoc-viewed", JSON.stringify(seen));
} catch {
  /* storage disabled; the column will read "never" */
}
