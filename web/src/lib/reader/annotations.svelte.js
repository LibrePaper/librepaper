import { submissions } from "../submissions.js";
import { applyDecision } from "../suggestions.js";
import { authHeaders } from "../api.js";

// How many pages a refresh re-reads before it gives up and says it is
// holding a prefix. A reader who has pressed "Load more" eight times has
// four hundred comments on screen; re-reading more than that on every
// agent batch would be a drain dressed up as a refresh.
const REFRESH_PAGES_MAX = 8;

// Everything said about the document, and the optimistic commands that add
// to it before the room has confirmed them.
//
// The list is a *prefix* of the document's comments, not the document's
// comments: the server pages them (docs/protocol/comments-v1.md) and
// nothing here drains those pages on its own. `state.page` is what the
// panel shows instead of counting rows -- the authoritative totals, whether
// there is more, and whether the last request failed.
//
// The list is owned here. It used to live in the page, with this controller
// holding a getter and a setter for it, which meant the one thing the module
// is about was the one thing it did not have. Anchoring and repainting are
// still the page's, and genuinely so: both depend on the visible frame, which
// this module cannot see.
export function createAnnotations({ slug, key = "", anchor, repaint, send, renderDigest = () => "" }) {
  const state = $state({
    comments: [],
    // What this browser has said and the server has not yet acknowledged.
    // Offered back from the collaboration panel, so work is never only in a
    // failed request.
    unconfirmed: [],
    // The traversal, as the panel needs to describe it.
    page: {
      /// Nothing has arrived yet, which is not the same as having arrived
      /// and been empty.
      loading: true,
      /// The catalogue's own counts, never `comments.length`.
      total: 0,
      open: 0,
      replies: 0,
      /// Changes whenever the collection does, so a stale continuation is
      /// recognisable rather than silently interleaved.
      revision: "",
      /// Where the next page starts, and whether there is one.
      cursor: null,
      complete: false,
      /// How many pages are held, so a refresh re-reads the same prefix.
      pages: 0,
      /// The last page request's failure, kept beside the comments that are
      /// still on screen rather than replacing them.
      error: "",
      /// A request is in flight.
      busy: false,
    },
  });

  // Every fetch carries the generation it was issued in. A response from an
  // older one -- a "load more" overtaken by a refresh, a refresh overtaken
  // by a reconnect -- is dropped rather than applied over newer state.
  let generation = 0;

  async function readPage(cursor) {
    const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
    const response = await fetch(`/api/documents/${slug}/comments${query}`, {
      headers: authHeaders(key),
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.error || `Comments could not be read (${response.status}).`);
    return body;
  }

  /// Records what a page said about the collection as a whole. `pages` is
  /// how many are held, which a refresh replays.
  function noteState(body, pages) {
    state.page = {
      loading: false,
      total: body.state?.total ?? state.page.total,
      open: body.state?.open ?? state.page.open,
      replies: body.state?.replies ?? state.page.replies,
      revision: body.state?.revision ?? state.page.revision,
      cursor: body.next_cursor ?? null,
      complete: Boolean(body.complete),
      pages,
      error: "",
      busy: false,
    };
  }
  const outbox = submissions({ slug, changed: (items) => (state.unconfirmed = items) });
  const list = () => state.comments;
  const update = (next) => (state.comments = next);
  /// Say that the list changed. A no-op assignment, which is what it has
  /// always been: the items are reactive on their own, and this is the line
  /// that says so out loud at the sites that mutate one in place.
  const publish = () => update(list());
  const stable = (next) => {
    const existing = new Map(list().map((item) => [String(item.id), item]));
    return (next || []).map((item) => {
      const held = existing.get(String(item.id));
      if (held) {
        Object.assign(held, item);
        if (!Object.prototype.hasOwnProperty.call(item, "pending")) delete held.pending;
        return held;
      }
      return { ...item, _uiKey: item._uiKey || item.temp_id || item.id };
    });
  };

  function submit(message) {
    outbox.keep(message);
    send(message);
  }

  function removePending(id) {
    update(list().filter((comment) => comment.temp_id !== id).map((comment) => ({
      ...comment,
      replies: comment.replies.filter((reply) => reply.temp_id !== id),
    })));
    repaint();
  }

  function comment(selection, { motivation, body, proposed, color }, creator) {
    const temp_id = crypto.randomUUID();
    const editingFields = motivation === "editing" ? { proposed: proposed ?? "" } : {};
    const colorFields = motivation !== "editing" && /^#[0-9a-f]{6}$/i.test(color || "")
      ? { color: color.toLowerCase() } : {};
    // The draft is shown the way a stored comment is shown: from what the
    // page had. The server will send back the same thing beside the anchor it
    // works out, so the card and the highlight do not move when it does.
    const optimistic = {
      id: temp_id, temp_id, _uiKey: temp_id, seq: Number.MAX_SAFE_INTEGER,
      ...selection, motivation, body, ...editingFields, ...colorFields, creator,
      presentation: {
        rendered_exact: selection.exact || "",
        rendered_prefix: selection.prefix || "",
        rendered_suffix: selection.suffix || "",
        rendered_position_utf16: Number.isInteger(selection.position) ? selection.position : null,
      },
      created: new Date().toISOString(), resolved: false, resolved_at: null,
      replies: [], pending: true,
    };
    anchor([optimistic]);
    update([...list(), optimistic]);
    repaint();
    submit({ type: "comment", ...selection, render_digest: selection.render_digest ?? renderDigest() ?? "", motivation, body, ...editingFields, ...colorFields, temp_id });
  }

  function reply(parent, body, creator) {
    const temp_id = crypto.randomUUID();
    parent.replies = [...parent.replies, {
      id: temp_id, body, creator: creator || "Anonymous", created: new Date().toISOString(), temp_id,
    }];
    publish();
    // The server determines the author; the optimistic name is never sent.
    submit({ type: "reply", comment_id: parent.id, body, temp_id });
  }

  /// Moves the authoritative counts by what an event says happened. The
  /// totals are the catalogue's; this keeps them current between page
  /// reads without ever recounting the rows on screen.
  function countBy(total, open) {
    state.page = {
      ...state.page,
      total: Math.max(0, state.page.total + total),
      open: Math.max(0, state.page.open + open),
    };
  }

  // Mutation frames carry the catalogue's authoritative shape.  Apply it
  // before touching the visible prefix: optimistic resolve/delete may have
  // already changed or removed the local row, and the mutation may target a
  // row this browser has never loaded.
  function applyEventState(event) {
    if (!event.state) return false;
    state.page = {
      ...state.page,
      total: event.state.total ?? state.page.total,
      open: event.state.open ?? state.page.open,
      replies: event.state.replies ?? state.page.replies,
      revision: event.state.revision ?? state.page.revision,
    };
    return true;
  }

  function resetReplyRequests() {
    for (const item of list()) {
      if (!item.repliesBusy && !item._replyRequest) continue;
      item.repliesBusy = false;
      item.repliesError = "";
      delete item._replyRequest;
    }
  }

  function receive(event) {
    if (event.type === "comment") {
      const hasState = applyEventState(event);
      outbox.acknowledge(event);
      const local = event.temp_id && list().find((item) => item.temp_id === event.temp_id);
      if (local) {
        Object.assign(local, event.comment, { temp_id: undefined, pending: false, deletable: true });
        // The optimistic row was never counted -- the counts are the
        // catalogue's -- so this is where it joins them.
        if (!hasState) countBy(1, event.comment.resolved ? 0 : 1);
      } else if (!list().some((item) => item.id === event.comment.id)) {
        anchor([event.comment]);
        update([...list(), { ...event.comment, _uiKey: event.comment.id }]);
        // A create the page traversal has not reached yet is still one
        // more comment on the document. The counts stay the catalogue's,
        // moved by the events the catalogue sent, never recounted from the
        // rows this browser happens to hold.
        if (!hasState) countBy(1, event.comment.resolved ? 0 : 1);
      }
      publish();
      repaint();
    } else if (event.type === "reply") {
      const hasState = applyEventState(event);
      outbox.acknowledge(event);
      const parent = list().find((item) => item.id === event.comment_id);
      if (!parent) return true;
      const local = event.temp_id && parent.replies.find((item) => item.temp_id === event.temp_id);
      if (local) Object.assign(local, event.reply, { temp_id: undefined });
      else if (!parent.replies.some((item) => item.id === event.reply.id)) {
        parent.replies = [...parent.replies, event.reply];
        parent.reply_total = (parent.reply_total || 0) + 1;
        if (!hasState) state.page = { ...state.page, replies: state.page.replies + 1 };
      }
      publish();
    } else if (event.type === "attachments") {
      // Where every comment's passage got to, after an edit moved it. The
      // server resolves the whole document at once and sends one frame for
      // the pass; without this the cards keep saying where the passages were
      // when the page was opened, and a passage that has since been rewritten
      // or removed says nothing until a reload.
      let changed = false;
      for (const item of event.attachments || []) {
        const comment = list().find((candidate) => candidate.id === item.comment_id);
        if (!comment) continue;
        comment.attachment = item.attachment;
        changed = true;
      }
      if (!changed) return true;
      anchor(list());
      publish();
      repaint();
    } else if (event.type === "delete") {
      const hasState = applyEventState(event);
      const gone = list().find((item) => item.id === event.comment_id);
      update(list().filter((item) => item.id !== event.comment_id));
      if (!hasState && gone) countBy(-1, gone.resolved ? 0 : -1);
      repaint();
    } else if (["resolve", "accept", "reject"].includes(event.type)) {
      const hasState = applyEventState(event);
      const item = list().find((item) => item.id === event.comment_id);
      if (!item) return true;
      if (event.type === "resolve") {
        if (!hasState && item.resolved !== event.resolved) countBy(0, event.resolved ? -1 : 1);
        item.resolved = event.resolved;
        item.resolved_at = event.resolved_at;
        if (!event.resolved) item.outcome = "";
      } else {
        applyDecision(item, event, event.type === "accept" ? "accepted" : "rejected");
      }
      publish();
      repaint();
    } else if (event.type === "comments-changed") {
      // An agent's batch added, edited and deleted in one act, so there is
      // no per-comment event to apply. The frame says the collection moved
      // and how large it now is; it carries no rows, because the
      // collection has no bound. Re-read the prefix this browser holds.
      state.page = {
        ...state.page,
        total: event.state?.total ?? state.page.total,
        open: event.state?.open ?? state.page.open,
        replies: event.state?.replies ?? state.page.replies,
        revision: event.state?.revision ?? state.page.revision,
      };
      void refresh();
    } else if (event.type === "refine") {
      applyEventState(event);
      outbox.acknowledge(event);
      // The response carries `comment_id` nowhere on the wire (`Ok(json!({
      // "type": "refine", "comment": comment}))` in `server/mod.rs`) -- only
      // the full, updated comment, which is where its own id actually is.
      const item = list().find((candidate) => candidate.id === (event.comment_id ?? event.comment?.id));
      if (!item || !event.comment) return true;
      // The authoritative comment may have arrived through a read-only link;
      // preserve the caller's existing delete permission instead of granting
      // it merely because a refinement was broadcast.
      Object.assign(item, event.comment, { pending: false });
      publish();
      repaint();
    } else return false;
    return true;
  }

  /// The first page, as the room states it on joining, plus what it says
  /// about the collection behind it.
  ///
  /// Rows this browser has made and the room has not confirmed are carried
  /// over: the page was read without them, and blinking them out from under
  /// whoever is still typing would lose work that is only in a request.
  function seed(next, collection) {
    // A reconnect delivers a fresh `hello`, and a reader who had pressed
    // "Load more" would otherwise watch their pages vanish. Take the
    // collection's new shape from the frame, then re-read the prefix they
    // were actually holding.
    if (state.page.pages > 1) {
      resetReplyRequests();
      state.page = {
        ...state.page,
        total: collection?.total ?? state.page.total,
        open: collection?.open ?? state.page.open,
        replies: collection?.replies ?? state.page.replies,
        revision: collection?.revision ?? state.page.revision,
      };
      void refresh();
      return;
    }
    ++generation;
    resetReplyRequests();
    const stated = next || [];
    outbox.reconcile(stated);
    const claimed = new Set(stated.map((item) => String(item.id)));
    const pending = list().filter((item) => item.pending && !claimed.has(String(item.id)));
    update([...stable(stated), ...pending]);
    // `hello` folds `complete` and `next_cursor` into its `state` block;
    // an HTTP page carries them beside it. One shape here, either way.
    noteState(
      {
        state: collection || {},
        next_cursor: collection?.next_cursor ?? null,
        complete: collection?.complete ?? true,
      },
      1,
    );
  }

  /// One more page onto the end. Never called on a timer or in a loop: the
  /// reader asks for it.
  async function loadMore() {
    if (state.page.busy || state.page.complete || !state.page.cursor) return;
    const mine = generation;
    const cursor = state.page.cursor;
    state.page = { ...state.page, busy: true, error: "" };
    try {
      const body = await readPage(cursor);
      if (mine !== generation) return;
      const claimed = new Set((body.comments || []).map((item) => String(item.id)));
      // Keyed on id, so a row that also arrived as a live event while this
      // was in flight is one row rather than two.
      const held = list().filter((item) => !claimed.has(String(item.id)));
      update([...held, ...stable(body.comments || [])]);
      noteState(body, state.page.pages + 1);
      anchor(list());
      publish();
      repaint();
    } catch (error) {
      if (mine !== generation) return;
      // The comments already on screen stay exactly as they are.
      state.page = { ...state.page, busy: false, error: error.message || "That page could not be read." };
    }
  }

  /// Re-reads the prefix this browser is holding, from the first page
  /// forward. What a `comments-changed` frame and a reconnect both do.
  ///
  /// Capped: past `REFRESH_PAGES_MAX` it keeps what it read and says it is
  /// incomplete rather than walking the whole collection.
  async function refresh() {
    const mine = ++generation;
    resetReplyRequests();
    const wanted = Math.min(Math.max(state.page.pages, 1), REFRESH_PAGES_MAX);
    state.page = { ...state.page, busy: true, error: "" };
    const collected = [];
    let cursor = null;
    let read = 0;
    let last = null;
    try {
      while (read < wanted) {
        const body = await readPage(cursor);
        if (mine !== generation) return;
        collected.push(...(body.comments || []));
        last = body;
        read += 1;
        cursor = body.next_cursor;
        if (body.complete || !cursor) break;
      }
    } catch (error) {
      if (mine !== generation) return;
      state.page = { ...state.page, busy: false, error: error.message || "Comments could not be reloaded." };
      return;
    }
    if (mine !== generation || !last) return;
    outbox.reconcile(collected);
    const claimed = new Set(collected.map((item) => String(item.id)));
    const pending = list().filter((item) => item.pending && !claimed.has(String(item.id)));
    update([...stable(collected), ...pending]);
    noteState(last, read);
    anchor(list());
    publish();
    repaint();
  }

  /// One more page of one thread's replies, in place on its card.
  async function loadReplies(comment) {
    if (!comment?.reply_cursor || comment.repliesBusy) return;
    const mine = generation;
    // A primitive token survives Svelte's deep proxying; object identity does
    // not, so a proxy-wrapped request would never clear `repliesBusy`.
    const request = Symbol("replies");
    comment._replyRequest = request;
    comment.repliesBusy = true;
    comment.repliesError = "";
    publish();
    try {
      const response = await fetch(
        `/api/documents/${slug}/comments/${comment.id}/replies?cursor=${encodeURIComponent(comment.reply_cursor)}`,
        { headers: authHeaders(key) },
      );
      const body = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(body.error || `Replies could not be read (${response.status}).`);
      if (mine !== generation) return;
      const held = new Set(comment.replies.map((reply) => String(reply.id)));
      comment.replies = [
        ...comment.replies,
        ...(body.replies || []).filter((reply) => !held.has(String(reply.id))),
      ];
      comment.reply_total = body.total ?? comment.reply_total;
      comment.reply_cursor = body.complete ? null : body.next_cursor ?? null;
    } catch (error) {
      if (mine !== generation) return;
      comment.repliesError = error.message || "Those replies could not be read.";
    } finally {
      if (comment._replyRequest === request) {
        comment.repliesBusy = false;
        delete comment._replyRequest;
        publish();
      }
    }
  }

  return {
    state, publish, seed, loadMore, refresh, loadReplies,
    outbox, submit, comment, reply, receive, removePending,
    discard(id) { outbox.discard(id); removePending(id); },
    resolve(item) {
      item.resolved = !item.resolved;
      publish();
      repaint();
      send({ type: "resolve", comment_id: item.id, resolved: item.resolved });
    },
    delete(item) {
      update(list().filter((candidate) => candidate !== item));
      repaint();
      send({ type: "delete", comment_id: item.id });
    },
  };
}
