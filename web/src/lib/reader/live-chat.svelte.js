import { authHeaders } from "../api.js";

// This tab holds every message it has been sent, and no more. The server
// bounds the chat by the owner's storage quota, and older pages are fetched
// on request.
const PAGE = 50;

// The project chat as this tab holds it: the newest messages the hello
// carried, whatever has arrived since, and the older pages the reader has
// asked for. Ids are UUIDv7 strings, so ordering by id is ordering by time.
//
// The list is a suffix of the stored chat, the same way the comments are a
// prefix of theirs: `more` says whether older messages exist on the server.
// A viewer's hello carries no chat at all, which is a room with no chat
// access rather than a room with an empty one.
export function createLiveChat({ slug, key = "" }) {
  const state = $state({
    /// Oldest first.
    messages: [],
    /// Older messages exist beyond the first one held.
    more: false,
    /// Only the project owner may clear the chat.
    canClear: false,
    loadingOlder: false,
    loadError: "",
  });

  // A clear or a gap replacement bumps this, so an older page that arrives
  // afterwards is dropped rather than put back over newer state.
  let generation = 0;

  const byId = (a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

  // The first list wins for an id both hold, so pass the fresher one first.
  function union(first, second) {
    const seen = new Map();
    for (const message of first) seen.set(message.id, message);
    for (const message of second) if (!seen.has(message.id)) seen.set(message.id, message);
    return [...seen.values()].sort(byId);
  }

  function seed(hello) {
    generation++;
    state.loadingOlder = false;
    state.loadError = "";
    if (!Array.isArray(hello?.chat)) {
      state.messages = [];
      state.more = false;
      state.canClear = false;
      return;
    }
    const incoming = hello.chat;
    const more = Boolean(hello.chat_more);
    state.canClear = Boolean(hello.chat_can_clear);
    const held = state.messages;
    if (!incoming.length) {
      state.messages = [];
      state.more = false;
      return;
    }
    // A gap: everything held is older than everything the hello carries, so
    // the two cannot be joined without the messages between them. The hello
    // wins, and the reader can page back from there.
    if (held.length && held.at(-1).id < incoming[0].id) {
      state.messages = union([], incoming);
      state.more = more;
      return;
    }
    // The hello wins for the messages it carries: an erased author comes
    // back as "Deleted user" and must replace the name held from before.
    const merged = union(incoming, held);
    // The hello's answer about older messages holds when it reaches back at
    // least as far as what was already held.
    if (!held.length || merged[0].id < held[0].id || incoming[0].id === held[0].id) state.more = more;
    state.messages = merged;
  }

  function receive(event) {
    if (event.type === "chat-cleared") {
      generation++;
      state.loadingOlder = false;
      state.messages = [];
      state.more = false;
      state.loadError = "";
      return true;
    }
    if (event.type !== "chat") return false;
    if (state.messages.some((message) => message.id === event.id)) return true;
    const last = state.messages.at(-1);
    state.messages = !last || last.id < event.id
      ? [...state.messages, event]
      : union(state.messages, [event]);
    return true;
  }

  async function loadOlder() {
    const oldest = state.messages[0];
    if (state.loadingOlder || !state.more || !oldest) return;
    const issued = generation;
    state.loadingOlder = true;
    state.loadError = "";
    try {
      const query = `before=${encodeURIComponent(oldest.id)}&limit=${PAGE}`;
      const response = await fetch(`/api/documents/${slug}/chat-history?${query}`, {
        headers: authHeaders(key),
      });
      const body = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(body.error || `Older messages could not be read (${response.status}).`);
      if (issued !== generation) return;
      const page = Array.isArray(body.messages) ? body.messages : [];
      state.messages = union(page, state.messages);
      state.more = Boolean(body.more);
    } catch (cause) {
      if (issued === generation) state.loadError = cause?.message || "Older messages could not be read.";
    } finally {
      if (issued === generation) state.loadingOlder = false;
    }
  }

  return {
    state,
    has: (id) => state.messages.some((message) => message.id === id),
    seed,
    receive,
    loadOlder,
  };
}
