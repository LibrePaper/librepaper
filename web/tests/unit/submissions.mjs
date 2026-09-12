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
  region: new Proxy({ image_digest: "abc", x: 0.1 }, {}),
};
box.keep(message);
message.body = "mutated elsewhere";
box.failed("one", "network failed");
box.reconcile([]);
assert.equal(shown[0].message.body, "Do not lose this paragraph");
assert.equal(shown[0].message.region.x, 0.1);

box = make();
assert.equal(shown.length, 1);
assert(shown[0].error);
const sends = [];
box.retry("one", (message) => sends.push(message));
assert.equal(sends[0].temp_id, "one");
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
box.keep({ type: "comment", temp_id: "three" });
box.acknowledge({ type: "comment", temp_id: "three" });
assert.equal(shown.length, 0);
console.log("submissions: failed drafts survive snapshots and reload; acknowledged records reconcile");
