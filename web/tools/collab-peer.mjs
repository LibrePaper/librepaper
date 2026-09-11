// The browser's half of the protocol, run outside a browser.
//
// This imports the editor's own `collab.js` -- not a copy of it, not a model
// of it -- opens a real socket to a running librepaper, and answers a line of
// JSON per operation. It exists so the Rust suite can drive the actual client
// module against the actual server, and find out whether the two agree about
// joining, relaying, acknowledgment and reconnection.
//
// Usage: collab-peer.mjs <base-url> <slug> [cookie]

import { join } from "../src/lib/collab.js";

const [base, slug, cookie = ""] = process.argv.slice(2);

let socket = null;
let session = null;
let acknowledged = 0;
let peers = 1;
let state = { pending: 0, local: false, joined: false };
const outbox = [];

function currentText() {
  const value = session?.textOf?.(session.mainId?.());
  if (!value) throw new Error("document has no current main file");
  return value;
}

function send(message) {
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(message));
    return;
  }
  // The socket is down. Nothing is dropped: `collab.js` holds what was not
  // acknowledged and sends it again after the next join.
  outbox.push(message);
}

function connect() {
  const address = base.replace(/^http/, "ws") + `/ws/${slug}`;
  socket = new WebSocket(address, { headers: cookie ? { cookie } : {} });
  socket.onopen = () => session && send(session.open());
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.type === "y-state") session?.start(message).catch(() => {});
    else if (message.type === "y-update") session?.apply(message.update);
    else if (message.type === "y-ack") {
      acknowledged = Math.max(acknowledged, message.seq || 0);
      session?.acknowledge(message.seq || 0);
    } else if (message.type === "y-peers") peers = message.count || 1;
  };
  socket.onclose = () => session?.disconnected();
}

const ops = {
  start: () => {
    session = join({
      send,
      onPeers: (count) => (peers = count),
      onState: (next) => (state = next),
      name: "peer",
      slug,
      mayEdit: true,
    });
    connect();
    return {};
  },
  insert: ({ index, text }) => {
    currentText().insert(index, text);
    return {};
  },
  text: () => ({ text: currentText().toString() }),
  state: () => ({ ...state, acknowledged, peers }),
  // Pretends the socket dropped, without telling the server: what the peer
  // types from here is held rather than sent.
  disconnect: () => {
    socket.onclose = null;
    socket.close();
    socket = null;
    session.disconnected();
    return {};
  },
  reconnect: () => {
    connect();
    return {};
  },
  // Waits until the condition the test named is true, or gives up.
  await_ack: async ({ atLeast }) => {
    for (let tries = 0; tries < 200; tries++) {
      if (acknowledged >= atLeast) return { acknowledged };
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    return { acknowledged };
  },
  await_text: async ({ contains }) => {
    for (let tries = 0; tries < 200; tries++) {
      const value = session?.text;
      if (value?.toString().includes(contains)) return { text: value.toString() };
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    return { text: currentText().toString() };
  },
  stop: () => {
    session?.leave();
    socket?.close();
    return {};
  },
};

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", async (chunk) => {
  buffer += chunk;
  let cut;
  while ((cut = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, cut);
    buffer = buffer.slice(cut + 1);
    if (!line.trim()) continue;
    let answer;
    try {
      const request = JSON.parse(line);
      const run = ops[request.op];
      if (!run) throw new Error(`unknown op ${request.op}`);
      answer = { ok: true, ...(await run(request)) };
    } catch (error) {
      answer = { ok: false, error: String(error?.message ?? error) };
    }
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
