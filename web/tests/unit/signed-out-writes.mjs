// After a sign-out the tab may still be alive for a moment. Nothing that keeps
// a secret or a transcript in localStorage may write again in that window.
// This file signs out partway through, so it must not share a process with
// tests that expect writes to work.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";
import { forgetLocalData } from "../../src/lib/local-data.js";
import { KEYS, VIEWED, markViewed, takeLinkKey } from "../../src/lib/storage.js";

const { createAgentClient } = await loadRunes(
  new URL("../../src/lib/agent-client.svelte.js", import.meta.url),
);

function fakeStorage() {
  const values = new Map();
  return {
    values,
    get length() { return values.size; },
    key: (i) => [...values.keys()][i] ?? null,
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => { values.set(key, String(value)); },
    removeItem: (key) => { values.delete(key); },
  };
}

class FakeWebSocket {
  static OPEN = 1;
  readyState = 1;
  constructor() { queueMicrotask(() => this.onopen?.()); }
  send() {}
  close() {}
}
const fetcher = async () => Response.json({ id: "conversation-1", token: "chat-secret" });
const make = (storage) => createAgentClient({
  origin: "https://docs.example", slug: "paper",
  link: "https://docs.example/docs/paper#k=document-secret",
  fetcher, WebSocketImpl: FakeWebSocket, storage,
});

const local = fakeStorage();
globalThis.localStorage = local;
globalThis.location = { hash: "#k=link-secret", pathname: "/docs/paper", search: "" };
globalThis.history = { replaceState() {} };

// Before a sign-out every writer stores what it is given.
const before = fakeStorage();
const first = make(before);
await first.create();
assert.equal(before.length, 1, "the transcript is stored while signed in");
first.dispose();
takeLinkKey("paper");
markViewed("paper");
assert.ok(local.getItem(KEYS) && local.getItem(VIEWED));

await forgetLocalData(null, 1000, local);
assert.equal(local.length, 0);

// After it, none of them writes again.
const after = fakeStorage();
const second = make(after);
await second.create();
second.dispose();
assert.equal(after.length, 0, "the agent client writes nothing after sign-out");
takeLinkKey("other");
markViewed("other");
assert.equal(local.length, 0, "key and viewed stores write nothing after sign-out");
