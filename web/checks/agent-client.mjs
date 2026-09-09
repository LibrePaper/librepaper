import assert from "node:assert/strict";
import { createAgentClient } from "../src/lib/agent-client.js";

const requests = [];
const fetcher = async (url, init) => {
  requests.push({ url, ...init, headers: { ...init.headers } });
  const pathname = new URL(url, "https://docs.example").pathname;
  if (pathname.endsWith("/assistant/capabilities")) return Response.json({ can_read: true, can_comment: true, can_edit: false });
  const route = pathname.split("/chat")[1];
  if (route === "") return Response.json({ id: "conversation-1", token: "chat-secret" });
  if (route === "/conversation-1" && init.method === "DELETE") return Response.json({ deleted: true });
  throw new Error(`unexpected route ${route} ${init.method}`);
};

class FakeWebSocket {
  static OPEN = 1;
  static instances = [];
  readyState = FakeWebSocket.OPEN;
  sent = [];
  constructor(url) {
    this.url = url;
    FakeWebSocket.instances.push(this);
    queueMicrotask(() => this.onopen?.());
  }
  send(raw) {
    const frame = JSON.parse(raw);
    this.sent.push(frame);
    if (frame.type === "join") {
      queueMicrotask(() => this.emit({ type: "ready", listening: false, browser: true }));
    } else if (frame.type === "message") {
      queueMicrotask(() => {
        this.emit({ type: "message", message: { id: frame.id, role: "user", text: frame.text, context: frame.context } });
        this.emit({ type: "ack", id: frame.id });
      });
    }
  }
  emit(frame) { this.onmessage?.({ data: JSON.stringify(frame) }); }
  close() { this.readyState = 3; this.onclose?.(); }
}

const options = {
  origin: "https://docs.example", slug: "paper",
  link: "https://docs.example/docs/paper#k=document-secret",
  fetcher, WebSocketImpl: FakeWebSocket,
};
const client = createAgentClient(options);
try {
  await client.create();
  await new Promise((resolve) => setTimeout(resolve, 0));
  const created = requests[0];
  assert.equal(created.url, "/api/documents/paper/chat");
  assert.equal(created.headers["X-LibrePaper-Client"], "shell");
  assert.equal(created.headers["X-LibrePaper-Key"], "document-secret");
  assert.equal(created.headers["X-LibrePaper-Chat-Token"], undefined);
  const capabilities = await client.capabilities();
  assert.deepEqual(capabilities, { can_read: true, can_comment: true, can_edit: false });
  assert.equal(requests[1].url, "/api/documents/paper/assistant/capabilities");
  assert.equal(requests[1].headers["X-LibrePaper-Key"], "document-secret");
  const socket = FakeWebSocket.instances[0];
  assert.equal(new URL(socket.url).searchParams.get("k"), "document-secret");
  assert.equal(socket.url.includes("chat-secret"), false, "chat capability stays out of the URL");
  assert.deepEqual(socket.sent[0], { type: "join", token: "chat-secret", role: "user" });
  assert.equal(client.current.listening, false);

  await assert.rejects(() => client.send("too soon"), /Connect your agent/);
  socket.emit({ type: "presence", listening: true, browser: true });
  await client.send("Explain this", { path: "paper.md", selection: { exact: "A passage" } });
  const sent = socket.sent.at(-1);
  assert.equal(sent.type, "message");
  assert.equal(sent.text, "Explain this");
  assert.equal(sent.context.file, "paper.md");
  assert.deepEqual(sent.context.selection, { path: "paper.md", exact: "A passage", prefix: "", suffix: "", position: null });
  assert.match(sent.id, /^[a-z0-9-]+$/);
  assert.deepEqual(client.current.messages.at(-1), {
    id: sent.id, role: "user", text: "Explain this",
    context: { file: "paper.md", selection: { path: "paper.md", exact: "A passage", prefix: "", suffix: "", position: null } },
  });
} finally { client.dispose(); }

const fresh = createAgentClient(options);
try {
  await fresh.resume();
  assert.equal(fresh.current.id, "");
  assert.deepEqual(fresh.current.messages, []);
} finally { fresh.dispose(); }

console.log("agent-client: live socket, private capability, presence, acknowledgement and no replay passed");
