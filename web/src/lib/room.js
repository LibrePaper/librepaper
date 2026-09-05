// The document's room: one socket carrying every comment on it, and the
// updates of whoever is editing it.
//
// A socket that drops loses nothing. Comments still post over the REST route,
// and the hello frame on reconnect resends the whole list, so a broadcast
// missed during the gap heals itself. What a drop does cost is seeing other
// people's comments as they arrive, which is worth saying -- but only once it
// has lasted longer than a blip, and only while it is true.

import { SHELL_HEADERS, keyHeaders } from "./api.js";

export function openRoom(slug, { onMessage, onConnected, key = "" }) {
  let socket = null;
  let backoff = 500;
  let dropped = null;
  let closed = false;

  function connected(up) {
    clearTimeout(dropped);
    if (up) {
      onConnected(true);
      return;
    }
    dropped = setTimeout(() => onConnected(false), 2000);
  }

  function connect() {
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    // The one request a browser cannot put a header on, so the link key rides
    // in the query string here. Over TLS that is seen by this server and by
    // nobody else, and what the server logs is the digest.
    const query = key ? `?k=${encodeURIComponent(key)}` : "";
    socket = new WebSocket(`${scheme}://${location.host}/ws/${slug}${query}`);
    socket.onopen = () => {
      backoff = 500;
      connected(true);
    };
    socket.onmessage = (event) => onMessage(JSON.parse(event.data));
    socket.onclose = () => {
      if (closed) return;
      connected(false);
      setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 15000);
    };
    socket.onerror = () => socket.close();
  }
  connect();

  return {
    /// Sends over the socket, or over the REST route when it is down, so a
    /// write is never lost to a reconnect.
    send(message) {
      if (socket && socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify(message));
        return;
      }
      fetch(`/api/documents/${slug}/comments`, {
        method: "POST",
        headers: { "content-type": "application/json", ...SHELL_HEADERS, ...keyHeaders(key) },
        body: JSON.stringify(message),
      })
        .then((response) => response.json())
        .then(onMessage)
        .catch(() => connected(false));
    },
    close() {
      closed = true;
      socket?.close();
    },
  };
}
