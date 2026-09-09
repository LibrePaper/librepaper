import { read, write } from "./storage.js";

// Keep the complete submission until the server acknowledges it. A reload or
// a reconnect never treats a missing acknowledgment as a successful write.
export function submissions({ slug, changed = () => {} }) {
  const key = `librepaper-submissions-${slug}`;
  const saved = read(key, []);
  const items = new Map(
    (Array.isArray(saved) ? saved : [])
      .filter((item) => item?.message?.temp_id && ["comment", "reply"].includes(item.message.type))
      .map((item) => [item.message.temp_id, {
        message: item.message,
        error: "Submission not confirmed. Retry to check or send it.",
      }]),
  );
  function publish() {
    const list = [...items.values()];
    write(key, list);
    changed(list);
  }
  publish();
  return {
    keep(message) {
      // The wire payload is JSON; serializing also unwraps Svelte's nested
      // state proxies, which structuredClone cannot copy.
      items.set(message.temp_id, { message: JSON.parse(JSON.stringify(message)), error: "" });
      publish();
    },
    failed(id, error) {
      const item = items.get(id);
      if (!item) return;
      items.set(id, { ...item, error: error || "Submission not confirmed. You can retry." });
      publish();
    },
    disconnected() {
      for (const [id, item] of items) {
        items.set(id, { ...item, error: "Connection lost before confirmation. You can retry." });
      }
      publish();
    },
    acknowledge(event) {
      if (!["comment", "reply"].includes(event.type) || !event.temp_id) return;
      if (items.delete(event.temp_id)) publish();
    },
    reconcile(comments) {
      // New submissions use their UUID as the durable record ID, so even a
      // lost acknowledgment is reconciled by the next authoritative snapshot.
      const ids = new Set(comments.flatMap((comment) => [comment.id, ...(comment.replies || []).map((reply) => reply.id)]));
      for (const id of ids) items.delete(id);
      publish();
    },
    retry(id, send) {
      const item = items.get(id);
      if (!item) return;
      items.set(id, { ...item, error: "" });
      publish();
      send(item.message);
    },
    discard(id) {
      items.delete(id);
      publish();
    },
  };
}
