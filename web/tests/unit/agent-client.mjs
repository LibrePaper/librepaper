import assert from "node:assert/strict";
import { createAgentClient } from "../../src/lib/agent-client.js";

const requests = [];
let conversationCount = 0;
let failNextMessage = false;
let silenceNextMessage = false;
const storage = { values: new Map(), getItem(key) { return this.values.get(key) || null; }, setItem(key, value) { this.values.set(key, value); }, removeItem(key) { this.values.delete(key); } };
const fetcher = async (url, init) => {
  requests.push({ url, ...init, headers: { ...init.headers } });
  const pathname = new URL(url, "https://docs.example").pathname;
  if (pathname === "/api/documents/paper/agent/candidates/candidate-large") {
    return Response.json({ candidate_id: "candidate-large", base_revision: "base", revision: "candidate", main: "paper.md",
      files: { "paper.md": { kind: "text", id: "paper", sha: "source-sha", size: 20000 } } });
  }
  if (pathname === "/api/documents/paper/agent/candidates/candidate-large/source") {
    assert.equal(new URL(url, "https://docs.example").searchParams.get("path"), "paper.md");
    return new Response("x".repeat(20000), { headers: { "content-type": "text/plain" } });
  }
  if (pathname === "/api/documents/paper/agent/candidates/candidate-overlimit") {
    return Response.json({ candidate_id: "candidate-overlimit", base_revision: "base", revision: "candidate", main: "paper.md",
      files: {
        "paper.md": { kind: "text", id: "paper", sha: "source-sha", size: 20 * 1024 * 1024 },
        "chapter.md": { kind: "text", id: "chapter", sha: "chapter-sha", size: 20 * 1024 * 1024 },
      } });
  }
  if (pathname.endsWith("/assistant/capabilities")) return Response.json({ can_read: true, can_comment: true, can_edit: false });
  const route = pathname.split("/chat")[1];
  if (route === "") return Response.json({ id: `conversation-${++conversationCount}`, token: "chat-secret" });
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
      queueMicrotask(() => this.emit({ type: "ready", agent: true, browser: true }));
    } else if (frame.type === "message") {
      if (silenceNextMessage) {
        silenceNextMessage = false;
        return;
      }
      if (failNextMessage) {
        failNextMessage = false;
        queueMicrotask(() => this.emit({ type: "error", id: frame.id, status: 409, message: "temporary delivery failure" }));
        return;
      }
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
  storage,
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
  const candidate = await client.fetchCandidate("candidate-large", undefined, "candidate-token");
  assert.equal(candidate.texts["paper.md"].length, 20000);
  await assert.rejects(() => client.fetchCandidate("candidate-overlimit"), /aggregate limit/);
  const candidateRequest = requests.find((item) => item.url.endsWith("/agent/candidates/candidate-large"));
  const sourceRequest = requests.find((item) => item.url.includes("/agent/candidates/candidate-large/source"));
  assert.equal(candidateRequest.headers["X-LibrePaper-Key"], "document-secret");
  assert.equal(candidateRequest.headers["X-LibrePaper-Candidate-Token"], "candidate-token");
  assert.equal(sourceRequest.headers["X-LibrePaper-Candidate-Token"], "candidate-token");
  const socket = FakeWebSocket.instances[0];
  assert.equal(new URL(socket.url).searchParams.get("k"), "document-secret");
  assert.equal(socket.url.includes("chat-secret"), false, "chat capability stays out of the URL");
  assert.deepEqual(socket.sent[0], { type: "join", token: "chat-secret", role: "user" });
  assert.equal(client.current.runnerConnected, true);

  socket.emit({ type: "presence", agent: false, browser: true });
  await assert.rejects(() => client.send("too soon"), /Start the LibrePaper runner/);
  socket.emit({ type: "presence", agent: true, browser: true });
  const queued = client.send("Explain this", { path: "paper.md", selection: { exact: "A passage" } });
  const sent = socket.sent.at(-1);
  assert.equal(client.current.tasks[sent.id].status, "queued", "a request is visible while awaiting acceptance");
  await queued;
  assert.equal(sent.type, "message");
  assert.equal(sent.text, "Explain this");
  assert.equal(sent.context.file, "paper.md");
  assert.deepEqual(sent.context.selection, { path: "paper.md", exact: "A passage", prefix: "", suffix: "", position: null });
  assert.match(sent.id, /^[a-z0-9-]+$/);
  socket.emit({ type: "task", id: "event-1", task_id: sent.id, status: "working", text: "Working" });
  assert.equal(client.current.tasks[sent.id].status, "working");
  socket.emit({ type: "task", id: "event-1b", task_id: sent.id, status: "working", text: "Still working" });
  assert.equal(client.current.tasks[sent.id].status, "working", "progress does not imply completion");
  socket.emit({ type: "task", id: "event-finished", task_id: "finished", status: "completed", context: { results: { suggestions: ["suggestion-1"] } } });
  assert.equal(client.current.tasks.finished.status, "completed");
  assert.deepEqual(client.current.tasks.finished.result, { suggestions: ["suggestion-1"] });
  socket.emit({ type: "task", id: "event-interrupted", seq: 1, task_id: "recovery", status: "interrupted", text: "Reconcile receipts" });
  assert.equal(client.current.tasks.recovery.status, "interrupted");
  socket.emit({ type: "task", id: "event-stale", seq: 1, task_id: "recovery", status: "completed" });
  assert.equal(client.current.tasks.recovery.status, "interrupted", "stale task frames do not roll state back");

  // Admission is authoritative when the acknowledgement was lost. The task
  // event resolves the original send promise and removes its retry marker.
  silenceNextMessage = true;
  const admitted = client.send("Confirm admission");
  const admittedFrame = socket.sent.at(-1);
  socket.emit({ type: "task", id: "admission-event", task_id: admittedFrame.id, status: "queued", text: "Queued" });
  await admitted;
  assert.equal(client.current.tasks[admittedFrame.id].delivery, undefined);

  failNextMessage = true;
  await assert.rejects(() => client.send("Retry this request"), /temporary delivery failure/);
  const failed = socket.sent.at(-1);
  assert.equal(client.current.tasks[failed.id].status, "queued");
  await client.retry(failed.id);
  const retries = socket.sent.filter((frame) => frame.type === "message" && frame.id === failed.id);
  assert.equal(retries.length, 2, "retry sends the same event id for server deduplication");
  assert.equal(retries[0].task_id, undefined, "the message ID is the canonical task ID");

  socket.emit({ type: "task", id: "event-input", task_id: sent.id, status: "needs_input", context: { input: { request_id: "approval-1", kind: "approval", message: "Run the command?" } } });
  assert.equal(client.current.tasks[sent.id].status, "needs_input");
  assert.equal(client.current.tasks[sent.id].input.request_id, "approval-1");
  const responded = client.respond(sent.id, "approval-1", { decision: "accept" });
  const input = socket.sent.at(-1);
  assert.deepEqual(input, { type: "input", id: input.id, task_id: sent.id, request_id: "approval-1", response: { decision: "accept" } });
  socket.emit({ type: "ack", id: input.id });
  await responded;

  await client.cancel(sent.id);
  assert.equal(client.current.tasks[sent.id].cancelRequested, true);
  assert.equal(client.current.tasks[sent.id].status, "needs_input", "cancellation is only a request until the runner confirms it");
  socket.emit({ type: "task", id: "event-2", task_id: sent.id, status: "cancelled" });
  assert.equal(client.current.tasks[sent.id].status, "cancelled");
  assert.match(client.current.messages.find((message) => message.context?.task_id === sent.id && message.role === "agent").text, /cancel/i);
  assert.deepEqual(client.current.messages.find((message) => message.id === sent.id), {
    id: sent.id, role: "user", text: "Explain this", task_id: sent.id,
    context: { file: "paper.md", selection: { path: "paper.md", exact: "A passage", prefix: "", suffix: "", position: null }, diagnostics: [], diagnostics_omitted: 0 },
  });
  await client.previewResult({ id: "preview-result", request_id: "preview-request", task_id: sent.id,
    base_revision: "base", revision: "candidate",
    diagnostics: [...Array.from({ length: 20 }, (_, index) => ({ severity: "error", message: `error-${index}` })),
      ...Array.from({ length: 100 }, () => ({ severity: "warning", message: "warning", source: "x".repeat(4000) }))], ok: false });
  const previewResult = socket.sent.at(-1);
  assert.equal(previewResult.type, "preview_result");
  assert.equal(previewResult.diagnostics[0].severity, "error", "errors are retained before warnings when diagnostics are bounded");
  assert.match(previewResult.diagnostics.at(-1).message, /omitted/);
  assert.ok(new TextEncoder().encode(JSON.stringify(previewResult.diagnostics)).byteLength <= 16 * 1024);

  // An unconfirmed request survives a browser socket reconnect and is sent
  // again with the exact event ID, allowing the relay to deduplicate it.
  failNextMessage = true;
  await assert.rejects(() => client.send("Retry after reconnect"), /temporary delivery failure/);
  const uncertain = socket.sent.at(-1);
  socket.close();
  await new Promise((resolve) => setTimeout(resolve, 650));
  const reconnected = FakeWebSocket.instances.at(-1);
  assert.notEqual(reconnected, socket);
  const replay = reconnected.sent.find((frame) => frame.type === "message" && frame.id === uncertain.id);
  assert.ok(replay, "the same request is retried after reconnect");
  assert.equal(replay.task_id, uncertain.task_id);
} finally { client.dispose(); }

const fresh = createAgentClient(options);
try {
  await fresh.resume();
  assert.equal(fresh.current.id, "conversation-1");
  assert.deepEqual(fresh.current.messages, client.current.messages);
  const resumedSocket = FakeWebSocket.instances.at(-1);
  resumedSocket.emit({ type: "error", id: "expired", status: 404, message: "channel not found" });
  assert.equal(fresh.current.id, "", "an expired channel is discarded locally");
  assert.equal(storage.getItem("librepaper.agent.session.https://docs.example:paper"), null);
  await fresh.create();
  assert.equal(fresh.current.id, "conversation-2", "the browser can create a replacement channel after expiry");
} finally { fresh.dispose(); }

console.log("agent-client: live socket, private capability, presence, acknowledgement and no replay passed");
