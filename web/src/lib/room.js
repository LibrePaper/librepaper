// The document's room: one socket carrying every comment on it, and the
// updates of whoever is editing it.
//
// A socket that drops loses nothing. Comments still post over the REST route,
// and the hello frame on reconnect resends the whole list, so a broadcast
// missed during the gap heals itself. What a drop does cost is seeing other
// people's comments as they arrive, which is worth saying -- but only once it
// has lasted longer than a blip, and only while it is true.
//
// The failure that is worth more care is the one that never reports itself. A
// NAT table drops its mapping, or a laptop sleeps, and the socket is gone
// without a close frame ever arriving: `readyState` stays OPEN, nothing
// reconnects, and the page goes on looking connected while it silently stops
// receiving anything. So the socket is asked, periodically, whether it is
// still there, and a missing answer is treated as the close that never came.

import { authHeaders } from "./api.js";

export function openRoom(slug, { onMessage, onConnected, key = "" }) {
  let socket = null;
  let backoff = 500;
  let dropped = null;
  let reconnect = null;
  let generation = 0;
  let closed = false;
  let heartbeat = null;
  let awaitingPong = null;

  // How often the socket is asked whether it is alive, and how long the
  // answer may take. Twenty-five seconds sits under the sixty a typical NAT
  // and a typical proxy idle out at, so the traffic also keeps the path open
  // rather than only measuring it.
  const PING_EVERY = 25_000;
  const PONG_WITHIN = 10_000;

  function stopHeartbeat() {
    clearInterval(heartbeat);
    clearTimeout(awaitingPong);
    heartbeat = null;
    awaitingPong = null;
  }

  // Closing is what starts the normal reconnect: `onclose` runs, the backoff
  // applies, and there is one path back rather than two.
  function startHeartbeat(next) {
    stopHeartbeat();
    heartbeat = setInterval(() => {
      if (closed || socket !== next || next.readyState !== WebSocket.OPEN) return;
      if (awaitingPong) return;
      try {
        next.send(JSON.stringify({ type: "ping" }));
      } catch {
        next.close();
        return;
      }
      awaitingPong = setTimeout(() => {
        awaitingPong = null;
        if (!closed && socket === next) next.close();
      }, PONG_WITHIN);
    }, PING_EVERY);
  }

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
      startHeartbeat(next);
      connected(true);
    };
    next.onmessage = (event) => {
      if (closed || current !== generation || socket !== next) return;
      // Any frame at all proves the socket is delivering, so the outstanding
      // liveness check is satisfied by whatever arrives first.
      clearTimeout(awaitingPong);
      awaitingPong = null;
      let message;
      try {
        message = JSON.parse(event.data);
      } catch {
        // A frame this page cannot parse is the server's business, not a
        // reason to throw inside an event handler where nothing catches it.
        return;
      }
      // The answer to the liveness check carries nothing and is not an event.
      if (message?.type === "pong") return;
      onMessage(message);
    };
    next.onclose = () => {
      if (closed || current !== generation || socket !== next) return;
      stopHeartbeat();
      connected(false);
      // Jittered, because every client of a server that restarted is holding
      // the same backoff and would otherwise come back in lockstep, in waves,
      // at exactly the moment it has least to spare. Half the delay is fixed
      // so the wait still grows; half is spread.
      const wait = backoff / 2 + Math.random() * backoff;
      reconnect = setTimeout(() => {
        reconnect = null;
        connect();
      }, wait);
      backoff = Math.min(backoff * 2, 15000);
    };
    next.onerror = () => {
      if (current === generation && socket === next) next.close();
    };
  }
  connect();

  // A laptop that wakes, or a network that comes back, does not wait out a
  // backoff scheduled while it was away: a wait of up to fifteen seconds
  // after the reader is already looking at the page is the difference between
  // "it reconnected" and "it is broken". Both events are advisory -- they can
  // be wrong in either direction -- so this only ever shortens a wait that is
  // already pending, and never opens a second socket.
  function wake() {
    if (closed || reconnect === null) return;
    if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
    clearTimeout(reconnect);
    reconnect = null;
    backoff = 500;
    connect();
  }

  const watching = typeof globalThis.addEventListener === "function";
  if (watching) {
    globalThis.addEventListener("online", wake);
    document?.addEventListener?.("visibilitychange", wake);
  }

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
      if (message.type?.startsWith("doc-")) {
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
      if (watching) {
        globalThis.removeEventListener("online", wake);
        document?.removeEventListener?.("visibilitychange", wake);
      }
      stopHeartbeat();
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
