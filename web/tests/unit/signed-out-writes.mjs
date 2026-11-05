// After a sign-out the tab may still be alive for a moment. Nothing that keeps
// a secret in localStorage may write again in that window.
// This file signs out partway through, so it must not share a process with
// tests that expect writes to work.
import assert from "node:assert/strict";
import { forgetLocalData } from "../../src/lib/local-data.js";
import { KEYS, VIEWED, markViewed, takeLinkKey } from "../../src/lib/storage.js";

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

const local = fakeStorage();
globalThis.localStorage = local;
globalThis.location = { hash: "#k=link-secret", pathname: "/docs/paper", search: "" };
globalThis.history = { replaceState() {} };

// Before a sign-out every writer stores what it is given.
takeLinkKey("paper");
markViewed("paper");
assert.ok(local.getItem(KEYS) && local.getItem(VIEWED));

await forgetLocalData(null, 1000, local);
assert.equal(local.length, 0);

// After it, none of them writes again.
takeLinkKey("other");
markViewed("other");
assert.equal(local.length, 0, "key and viewed stores write nothing after sign-out");
