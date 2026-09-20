import { submissions } from "../submissions.js";
import { applyDecision } from "../suggestions.js";

// Everything said about the document, and the optimistic commands that add
// to it before the room has confirmed them.
//
// The list is owned here. It used to live in the page, with this controller
// holding a getter and a setter for it, which meant the one thing the module
// is about was the one thing it did not have. Anchoring and repainting are
// still the page's, and genuinely so: both depend on the visible frame, which
// this module cannot see.
export function createAnnotations({ slug, anchor, repaint, send, renderDigest = () => "" }) {
  const state = $state({
    comments: [],
    // What this browser has said and the server has not yet acknowledged.
    // Offered back from the collaboration panel, so work is never only in a
    // failed request.
    unconfirmed: [],
  });
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

  function receive(event) {
    if (event.type === "comment") {
      outbox.acknowledge(event);
      const local = event.temp_id && list().find((item) => item.temp_id === event.temp_id);
      if (local) Object.assign(local, event.comment, { temp_id: undefined, pending: false, deletable: true });
      else if (!list().some((item) => item.id === event.comment.id)) {
        anchor([event.comment]);
        update([...list(), { ...event.comment, _uiKey: event.comment.id }]);
      }
      publish();
      repaint();
    } else if (event.type === "reply") {
      outbox.acknowledge(event);
      const parent = list().find((item) => item.id === event.comment_id);
      if (!parent) return true;
      const local = event.temp_id && parent.replies.find((item) => item.temp_id === event.temp_id);
      if (local) Object.assign(local, event.reply, { temp_id: undefined });
      else if (!parent.replies.some((item) => item.id === event.reply.id)) {
        parent.replies = [...parent.replies, event.reply];
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
      update(list().filter((item) => item.id !== event.comment_id));
      repaint();
    } else if (["resolve", "accept", "reject"].includes(event.type)) {
      const item = list().find((item) => item.id === event.comment_id);
      if (!item) return true;
      if (event.type === "resolve") {
        item.resolved = event.resolved;
        item.resolved_at = event.resolved_at;
        if (!event.resolved) item.outcome = "";
      } else {
        applyDecision(item, event, event.type === "accept" ? "accepted" : "rejected");
      }
      publish();
      repaint();
    } else if (event.type === "comments") {
      // The whole list, as an agent's batch left it. A batch adds, edits and
      // deletes in one act, so there is no per-comment event to apply and
      // the room states where the list ended up instead.
      //
      // Rows this browser has made and the room has not confirmed are not in
      // it -- the batch was prepared without them -- so they are carried
      // over rather than blinking out from under whoever is still typing.
      const stated = event.comments || [];
      outbox.reconcile(stated);
      const claimed = new Set(stated.map((item) => item.id));
      const unconfirmed = list().filter((item) => item.pending && !claimed.has(item.id));
      update([...stable(stated), ...unconfirmed]);
      anchor(list());
      publish();
      repaint();
    } else if (event.type === "refine") {
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

  return {
    state, publish,
    /// The authoritative list, as the room states it on joining.
    replace(next) { update(stable(next)); },
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
