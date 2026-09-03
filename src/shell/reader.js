import { anchorAll } from "./anchor.js";

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
let figureAt = []; // text offset of each figure, by its index in the document
let frameReady = false;
let commentsReady = false;
let identity = ""; // GitHub login, when signed in; comments are signed with it

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
      // Where each figure sits in that text, so a note on a figure can be
      // ordered against the notes on passages.
      figureAt = Array.isArray(message.images) ? message.images.map(Number) : [];
      frameReady = true;
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
function applyHighlights() {
  if (!frameReady) return;
  // Annotations on figures are placed by the agent from the image identifiers,
  // since there is no text for this side to anchor against.
  tell({
    type: "regions",
    regions: comments
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
  });
  tell({
    type: "highlight",
    ranges: comments
      .filter((comment) => !comment.orphaned && comment.start != null)
      .map((comment) => ({
        id: comment.id,
        start: comment.start,
        end: comment.end,
        motivation: comment.motivation,
        resolved: Boolean(comment.resolved),
      })),
  });
}

function reanchor() {
  if (!frameReady || !commentsReady || docText === null) return;
  // A region annotation is placed by the agent, not by text matching, so it is
  // never orphaned for want of a quotation.
  anchorAll(docText, comments.filter((comment) => !comment.region));
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

function quoteOf(exact) {
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
  let open = false;

  const draw = () => {
    text.textContent = open ? "“" + exact + "”" : "“" + short;
    toggle.textContent = open ? " less" : "… ”";
  };
  toggle.onclick = (event) => {
    event.preventDefault();
    event.stopPropagation(); // the card itself scrolls to the highlight
    open = !open;
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

function render() {
  box.innerHTML = "";
  drawTagFilter();
  comments
    .slice()
    .filter((comment) => [...chosenTags].every((tag) => (comment.tags || []).includes(tag)))
    .sort((a, b) => place(a) - place(b) || a.seq - b.seq)
    .forEach((comment) => {
      // Pico styles article as a card and blockquote as a quotation, so a
      // comment needs no classes of its own beyond its resolved state.
      const el = document.createElement("article");
      if (comment.resolved || comment.pending) el.className = "resolved";
      el.id = "comment-" + comment.id;
      if (comment.orphaned) el.appendChild(mark("Needs re-anchoring"));
      // The motivation is the W3C annotation type. Commenting is the default,
      // so only the others are worth showing.
      if (comment.motivation && comment.motivation !== "commenting") {
        el.appendChild(mark(comment.motivation, comment.motivation));
      }
      if (comment.region) {
        const where = element("blockquote", `Figure ${comment.region.image_index + 1}`);
        where.className = "figureref";
        el.appendChild(where);
      } else {
        el.appendChild(quoteOf(comment.exact));
      }
      // A suggested edit reads as what it proposes, not as a remark about it.
      if (comment.replacement) {
        const suggestion = element("p", comment.replacement);
        suggestion.className = "suggestion";
        el.appendChild(suggestion);
      }
      if (comment.body) el.appendChild(element("p", comment.body));
      for (const tag of comment.tags || []) {
        const chip = element("button", tag);
        chip.type = "button";
        chip.className = "tag";
        chip.onclick = (event) => {
          event.stopPropagation();
          toggleTag(tag);
        };
        el.appendChild(chip);
      }
      el.appendChild(element("small", comment.creator + " · " + stamp(comment.created)));

      if (comment.replies.length) {
        const replies = document.createElement("ul");
        comment.replies.forEach((reply) => {
          const item = document.createElement("li");
          item.appendChild(element("span", reply.body));
          item.appendChild(document.createElement("br"));
          item.appendChild(element("small", reply.creator + " · " + stamp(reply.created)));
          replies.appendChild(item);
        });
        el.appendChild(replies);
      }

      // Not role="group": that is Pico's segmented control, which joins its
      // buttons into one shape. These are two separate actions.
      const actions = document.createElement("div");
      actions.className = "actions";
      const resolve = document.createElement("button");
      resolve.textContent = comment.resolved ? "Reopen" : "Resolve";
      resolve.onclick = (event) => {
        event.stopPropagation();
        // Optimistic: flip locally, then tell the room. The broadcast that
        // comes back is idempotent with what we already drew.
        comment.resolved = !comment.resolved;
        render();
        applyHighlights();
        send({ type: "resolve", comment_id: comment.id, resolved: comment.resolved });
      };
      const replyButton = document.createElement("button");
      replyButton.textContent = "Reply";
      const deleteButton = document.createElement("button");
      deleteButton.textContent = "Delete";
      deleteButton.onclick = async (event) => {
        event.stopPropagation();
        if (!(await confirmDelete())) return;
        // Optimistic, like resolve: drop it locally, then tell the room.
        comments = comments.filter((item) => item.id !== comment.id);
        render();
        applyHighlights();
        send({ type: "delete", comment_id: comment.id });
      };
      actions.append(resolve, replyButton, deleteButton);
      el.appendChild(actions);

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
      replyButton.onclick = (event) => {
        event.stopPropagation();
        form.hidden = !form.hidden;
        if (!form.hidden) body.focus();
      };
      form.onclick = (event) => event.stopPropagation();
      form.onsubmit = (event) => {
        event.preventDefault();
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
      el.appendChild(form);

      el.onclick = (event) => {
        if (!comment.orphaned && !event.target.closest("button,input,textarea")) {
          tell({ type: "reveal", id: comment.id });
        }
      };
      box.appendChild(el);
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
    headers: { "content-type": "application/json" },
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
    }
    render();
    applyHighlights();
    alert(event.message);
    return;
  }
  if (event.type === "comment") {
    const local = comments.find((comment) => comment.temp_id === event.temp_id);
    if (local) Object.assign(local, event.comment, { temp_id: undefined, pending: false });
    else if (!comments.some((comment) => comment.id === event.comment.id)) {
      comments.push(event.comment);
      // Someone else's comment: anchor just this one against the cached text.
      anchorAll(docText || "", [event.comment]);
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
};

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
  anchorAll(docText || "", [optimistic]);
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

function setSidebar(width) {
  const limit = Math.max(SIDEBAR_MIN, Math.min(width, innerWidth * SIDEBAR_MAX));
  reader.style.setProperty("--komodoc-sidebar", Math.round(limit) + "px");
  return Math.round(limit);
}

try {
  const remembered = Number(localStorage.getItem(SIDEBAR_KEY));
  if (remembered) setSidebar(remembered);
} catch {
  /* storage disabled; the default width stands */
}

if (grip) {
  grip.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    grip.classList.add("dragging");
    // The frame swallows pointer events while it has them, so it is deafened
    // for the duration of the drag rather than the drag being lost over it.
    frame.style.pointerEvents = "none";
  });

  grip.addEventListener("pointermove", (event) => {
    if (!grip.hasPointerCapture(event.pointerId)) return;
    setSidebar(innerWidth - event.clientX);
  });

  const finish = (event) => {
    if (!grip.hasPointerCapture(event.pointerId)) return;
    grip.releasePointerCapture(event.pointerId);
    grip.classList.remove("dragging");
    frame.style.pointerEvents = "";
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
