import { read, write } from "./storage.js";
import { m } from "./paraglide/messages.js";

function stableRequest(message) {
  // Creation uses request_id as the durable annotation ID. Matching it to the
  // outbox key makes retries idempotent and lets snapshots reconcile drafts.
  if (["comment", "reply"].includes(message?.type) && message.temp_id) {
    return { ...message, request_id: message.temp_id };
  }
  return message;
}

// Keep the complete submission until the server acknowledges it. A reload or
// a reconnect never treats a missing acknowledgment as a successful write.
/** @param {{slug: string, changed?: (items: {message: object, error: string}[]) => void}} options */
export function submissions({ slug, changed = () => {} }) {
  const key = `librepaper-submissions-${slug}`;
  const saved = read(key, []);
  const items = new Map(
    (Array.isArray(saved) ? saved : [])
      .filter((item) => item?.message?.temp_id && ["comment", "reply"].includes(item.message.type))
      .map((item) => {
        const message = stableRequest(item.message);
        return [message.temp_id, {
          message,
          error: String(m.submissions_not_confirmed_retry()),
        }];
      }),
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
      const stable = JSON.parse(JSON.stringify(stableRequest(message)));
      items.set(stable.temp_id, { message: stable, error: "" });
      publish();
      return stable;
    },
    failed(id, error) {
      const item = items.get(id);
      if (!item) return;
      items.set(id, { ...item, error: error || m.submissions_not_confirmed() });
      publish();
    },
    disconnected() {
      for (const [id, item] of items) {
        items.set(id, { ...item, error: m.submissions_connection_lost() });
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
