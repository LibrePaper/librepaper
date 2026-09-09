import { submissions } from "../submissions.js";
import { applyDecision } from "../suggestions.js";

// The annotation list stays reactive in the reader. This controller owns its
// optimistic commands and their reconciliation with the authoritative room.
// Anchoring is supplied by the reader because it depends on the visible frame.
export function createAnnotations({ slug, list, update, anchor, repaint, send, changed }) {
  const outbox = submissions({ slug, changed });
  const publish = () => update(list());

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

  function comment(selection, { motivation, body, proposed }, creator) {
    const temp_id = crypto.randomUUID();
    const editingFields = motivation === "editing" ? { proposed: proposed ?? "" } : {};
    const optimistic = {
      id: temp_id, temp_id, seq: Number.MAX_SAFE_INTEGER,
      ...selection, motivation, body, ...editingFields, creator,
      created: new Date().toISOString(), resolved: false, resolved_at: null,
      replies: [], pending: true,
    };
    anchor([optimistic]);
    update([...list(), optimistic]);
    repaint();
    submit({ type: "comment", ...selection, motivation, body, ...editingFields, temp_id });
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
        update([...list(), event.comment]);
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
    } else return false;
    return true;
  }

  return {
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
