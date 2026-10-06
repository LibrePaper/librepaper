import assert from "node:assert/strict";
import { submissions } from "../../src/lib/submissions.js";

const stored = new Map();
globalThis.localStorage = {
  getItem: (key) => stored.get(key) ?? null,
  setItem: (key, value) => stored.set(key, value),
};
let shown;
const make = () => submissions({ slug: "test", changed: (list) => (shown = list) });
let box = make();
const message = {
  type: "comment", temp_id: "one", body: "Do not lose this paragraph", exact: "selected passage",
};
box.keep(message);
assert.equal(shown[0].message.request_id, "one", "the first send has a stable server idempotency key");
message.body = "mutated elsewhere";
box.failed("one", "network failed");
box.reconcile([]);
assert.equal(shown[0].message.body, "Do not lose this paragraph");

box = make();
assert.equal(shown.length, 1);
assert(shown[0].error);
const sends = [];
box.retry("one", (message) => sends.push(message));
assert.equal(sends[0].temp_id, "one");
assert.equal(sends[0].request_id, "one", "the persisted retry reuses the original durable ID");
box.acknowledge({ type: "error", temp_id: "one" });
assert.equal(shown.length, 1);
box.reconcile([{ id: "one", replies: [] }]);
assert.equal(shown.length, 0);

box.keep({ type: "reply", temp_id: "two", body: "reply", comment_id: "parent" });
box.disconnected();
box = make();
assert.equal(shown.length, 1);
box.reconcile([{ id: "parent", replies: [{ id: "two" }] }]);
assert.equal(shown.length, 0);

stored.set("librepaper-submissions-legacy", JSON.stringify([{
  message: { type: "comment", temp_id: "legacy", body: "saved before request IDs" },
  error: "Connection lost",
}]));
let legacyShown;
const legacy = submissions({ slug: "legacy", changed: (items) => (legacyShown = items) });
const legacySent = [];
legacy.retry("legacy", (message) => legacySent.push(message));
assert.equal(legacySent[0].request_id, "legacy", "an older saved outbox item gains a stable request ID before retry");
assert.equal(legacyShown[0].message.request_id, "legacy", "migration is persisted with the outbox item");
box.keep({ type: "comment", temp_id: "three" });
box.acknowledge({ type: "comment", temp_id: "three" });
assert.equal(shown.length, 0);
console.log("submissions: failed drafts survive snapshots and reload; acknowledged records reconcile");
