import assert from "node:assert/strict";

// A Map behind the Storage interface: the grant store reads nothing else.
const items = new Map();
globalThis.localStorage = {
  get length() { return items.size; },
  key: (index) => [...items.keys()][index] ?? null,
  getItem: (key) => items.get(key) ?? null,
  setItem: (key, value) => items.set(key, String(value)),
  removeItem: (key) => items.delete(key),
};
const { granted, grant, revoke, revokeAll, isGrantKey } = await import("../../src/lib/local-execution.js");

const alice = { origin: "https://app.example", user: "github:alice", slug: "paper" };

// Off by default, for every document.
assert.equal(granted(alice), false);

// A grant belongs to one document, one person and one origin.
grant(alice);
assert.equal(granted(alice), true);
assert.equal(granted({ ...alice, slug: "other" }), false, "another document is not granted");
assert.equal(granted({ ...alice, user: "github:bob" }), false, "another person is not granted");
assert.equal(granted({ ...alice, origin: "https://elsewhere.example" }), false, "another origin is not granted");

// No document, no grant.
grant({ ...alice, slug: "" });
assert.equal(granted({ ...alice, slug: "" }), false);

// Turning it off forgets it.
revoke(alice);
assert.equal(granted(alice), false);

// Signing out forgets every grant, and only grants.
items.set("librepaper-build-v3:unrelated", "kept");
grant(alice);
grant({ ...alice, user: "github:bob", slug: "notes" });
assert.ok([...items.keys()].filter(isGrantKey).length === 2);
revokeAll();
assert.equal([...items.keys()].filter(isGrantKey).length, 0);
assert.equal(items.get("librepaper-build-v3:unrelated"), "kept");
assert.equal(isGrantKey(null), false);

// Storage that throws (private mode, blocked site data) means not granted.
globalThis.localStorage.getItem = () => { throw new Error("blocked"); };
assert.equal(granted(alice), false);

console.log("local-execution: per document, per person, off by default, forgotten on sign-out");
