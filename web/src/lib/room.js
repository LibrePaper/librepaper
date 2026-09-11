// The document's room: one socket carrying every comment on it, and the
// updates of whoever is editing it.
//
// A socket that drops loses nothing. Comments still post over the REST route,
// and the hello frame on reconnect resends the whole list, so a broadcast
// missed during the gap heals itself. What a drop does cost is seeing other
// people's comments as they arrive, which is worth saying -- but only once it
// has lasted longer than a blip, and only while it is true.

import { authHeaders } from "./api.js";

export function openRoom(slug, { onMessage, onConnected, key = "" }) {
  let socket = null;
  let backoff = 500;
  let dropped = null;
  let reconnect = null;
  let generation = 0;
  let closed = false;

  function connected(up) {
    if (closed) return;
    clearTimeout(dropped);
    if (up) {
      onConnected(true);
      return;
    }
    dropped = setTimeout(() => onConnected(false), 2000);
  }

  function connect() {
    if (closed) return;
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    // The one request a browser cannot put a header on, so the link key rides
    // in the query string here. Over TLS that is seen by this server and by
    // nobody else, and what the server logs is the digest.
    const query = key ? `?k=${encodeURIComponent(key)}` : "";
    const current = ++generation;
    const next = new WebSocket(`${scheme}://${location.host}/ws/${slug}${query}`);
    socket = next;
    next.onopen = () => {
      if (closed || current !== generation || socket !== next) return;
      backoff = 500;
      connected(true);
    };
    next.onmessage = (event) => {
      if (closed || current !== generation || socket !== next) return;
      onMessage(JSON.parse(event.data));
    };
    next.onclose = () => {
      if (closed || current !== generation || socket !== next) return;
      connected(false);
      reconnect = setTimeout(() => {
        reconnect = null;
        connect();
      }, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
    next.onerror = () => {
      if (current === generation && socket === next) next.close();
    };
  }
  connect();

  return {
    // Ephemeral events have no HTTP fallback: if nobody has a live room
    // socket, there is deliberately nowhere to leave them.
    sendLive(message) {
      if (socket && socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify(message));
        return { ok: true, via: "socket" };
      }
      return { ok: false, error: new Error("room is disconnected") };
    },
    /// Sends over the socket, or over the REST route when it is down, so a
    /// write is never lost to a reconnect.
    send(message) {
      if (socket && socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify(message));
        return { ok: true, via: "socket" };
      }
      // Yjs messages must be delivered by the room socket. Sending a Yjs
      // update through the comments endpoint would return an error and, more
      // dangerously, make callers believe that the document was persisted.
      if (message.type?.startsWith("y-")) {
        return { ok: false, error: new Error("room is disconnected") };
      }
      return fetch(`/api/documents/${slug}/comments`, {
        method: "POST",
        headers: authHeaders(key, "application/json"),
        body: JSON.stringify(message),
      })
        .then(async (response) => {
          if (!response.ok) {
            const body = await response.json().catch(() => ({}));
            throw new Error(body.error || `comment submission failed (${response.status})`);
          }
          const event = await response.json();
          onMessage(event);
          return { ok: true, via: "http", event };
        })
        .catch((error) => {
          connected(false);
          // The temporary id lets the caller retain precisely this optimistic
          // draft. Existing callers may ignore the event; recovery-aware
          // callers can display it and offer retry.
          if (message.temp_id) {
            onMessage({
              type: "submission-failed",
              temp_id: message.temp_id,
              message: error?.message || "comment submission failed",
            });
          }
          return { ok: false, error };
        });
    },
    close() {
      closed = true;
      clearTimeout(reconnect);
      clearTimeout(dropped);
      reconnect = null;
      dropped = null;
      generation++;
      const old = socket;
      socket = null;
      old?.close();
    },
  };
}
