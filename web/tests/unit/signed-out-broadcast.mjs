// A tab that hears another tab sign out clears the secrets it may have written
// since that tab cleared storage. This file marks the process as signed out,
// so it runs alone.
import assert from "node:assert/strict";
import { listenForSignOut, localDataForgotten } from "../../src/lib/local-data.js";
import { AGENT_SESSION_PREFIX, KEYS, VIEWED } from "../../src/lib/storage.js";

const values = new Map([
  [KEYS, "{}"],
  [VIEWED, "{}"],
  [`${AGENT_SESSION_PREFIX}https://app.example:paper`, "{}"],
]);
globalThis.localStorage = {
  get length() { return values.size; },
  key: (i) => [...values.keys()][i] ?? null,
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => { values.set(key, String(value)); },
  removeItem: (key) => { values.delete(key); },
};

const heard = new Promise((resolve) => {
  globalThis.stopListening = listenForSignOut(resolve);
});
const other = new BroadcastChannel("librepaper-local-data");
other.postMessage({ type: "signed-out" });
await heard;

assert.equal(values.size, 0);
assert.equal(localDataForgotten(), true);
other.close();
globalThis.stopListening();
