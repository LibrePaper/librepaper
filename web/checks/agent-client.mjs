import assert from "node:assert/strict";
import { createAgentClient } from "../src/lib/agent-client.js";

const held = new Map();
const storage = {
  getItem: (key) => held.get(key),
  setItem: (key, value) => held.set(key, value),
  removeItem: (key) => held.delete(key),
};
const requests = [];
let reply = false;
const fetcher = async (url, init) => {
  requests.push({ url, ...init, headers: { ...init.headers } });
  assert.equal(new URL(url, "https://docs.example").pathname.includes("chat-token"), false);
  assert.equal(init.credentials, "same-origin");
  assert.equal(init.cache, "no-store");
  const route = new URL(url, "https://docs.example").pathname.split("/chat")[1];
  if (route === "") return Response.json({ id: "conversation-1", token: "chat-secret" });
  if (route === "/conversation-1" && init.method === "GET") {
    return Response.json({ messages: [{ id: "agent-1", cursor: 1, role: "agent", text: reply ? "<img src=x>" : "hello" }], next_cursor: 1, listening: true });
  }
  if (route === "/conversation-1" && init.method === "POST") {
    const body = JSON.parse(init.body);
    return Response.json({ message: { ...body, cursor: 2 }, next_cursor: 2 });
  }
  if (route === "/conversation-1" && init.method === "DELETE") return Response.json({ deleted: true });
  throw new Error(`unexpected route ${route} ${init.method}`);
};

const options = {
  origin: "https://docs.example", slug: "paper",
  link: "https://docs.example/docs/paper#k=document-secret",
  storage, fetcher,
};
const client = createAgentClient(options);
try {
  await client.create();
  const created = requests[0];
  assert.equal(created.url, "/api/documents/paper/chat");
  assert.equal(created.headers["X-Komodoc-Client"], "shell");
  assert.equal(created.headers["X-Komodoc-Key"], "document-secret");
  assert.equal(created.headers["X-Komodoc-Chat-Token"], undefined);
  assert.equal(created.url.includes("chat-secret"), false);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(client.current.listening, true);
  assert.equal(client.current.messages[0].text, "hello");
  reply = true;
  await client.send("Explain this", { path: "paper.md", selection: { exact: "A passage" } });
  const posted = JSON.parse(requests.find((request) => request.method === "POST" && request.url.endsWith("conversation-1")).body);
  assert.equal(posted.role, "user");
  assert.equal(posted.text, "Explain this");
  assert.equal(posted.context.file, "paper.md");
  assert.equal(posted.context.selection, "A passage");
  assert.match(posted.id, /^[a-z0-9-]+$/);
} finally { client.dispose(); }

// A remount uses the tab capability and replays from the beginning. It never
// puts the capability in a URL or sends a second copy of the user message.
const resumed = createAgentClient(options);
try {
  await resumed.resume();
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(resumed.current.messages[0].text, "<img src=x>");
  const get = requests.at(-1);
  assert.match(get.url, /conversation-1\?after=0$/);
  assert.equal(get.headers["X-Komodoc-Chat-Token"], "chat-secret");
  await resumed.end();
  assert.equal(resumed.current.id, "");
  assert.equal(held.size, 0);
} finally { resumed.dispose(); }

// A failed send remains an error for the caller, so the panel keeps its draft
// and can retry; it is never represented as a silently lost local transcript.
let offline = true;
const unavailable = createAgentClient({ ...options, storage: null, fetcher: async () => {
  if (offline) throw new TypeError("offline");
  return Response.json({ message: { id: "later", cursor: 1, role: "user", text: "later" }, next_cursor: 1 });
} });
await assert.rejects(() => unavailable.send("offline draft"), /unavailable/);
offline = false;
unavailable.dispose();

let lost = true;
const retriedBodies = [];
const retryClient = createAgentClient({ ...options, storage: null, fetcher: async (url, init) => {
  const suffix = new URL(url, "https://docs.example").pathname.split("/chat")[1];
  if (suffix === "") return Response.json({ id: "retry-conversation", token: "retry-token" });
  if (init.method === "GET") return Response.json({ messages: [], next_cursor: 0, listening: false });
  if (init.method === "POST") {
    retriedBodies.push(JSON.parse(init.body));
    if (lost) { lost = false; throw new TypeError("response lost after commit"); }
    return Response.json({ message: { ...retriedBodies.at(-1), cursor: 1 }, next_cursor: 1 });
  }
  return Response.json({ deleted: true });
} });
await assert.rejects(() => retryClient.send("retry me"), /unavailable/);
await retryClient.send("retry me", { path: "changed.md" });
assert.equal(retriedBodies[0].id, retriedBodies[1].id, "a lost POST is retried idempotently");
assert.equal(retriedBodies[1].context.file, undefined, "the retry keeps the original context");
retryClient.dispose();

console.log("agent-client: mailbox capability, document key headers, private replay, context, deletion and offline retry behavior passed");
