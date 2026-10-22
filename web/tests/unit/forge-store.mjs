// The forge store against a small fake IndexedDB: tokens and pending
// operations round-trip, and nothing opens once local data is forgotten.
import assert from "node:assert/strict";
import { forgetLocalData } from "../../src/lib/local-data.js";

// Only what store.js uses: open, object stores, get/put/delete, transactions.
function fakeIndexedDB() {
  const data = new Map();
  const names = new Set();
  const settle = (request, value) => queueMicrotask(() => {
    request.result = value;
    request.onsuccess?.();
  });
  const database = {
    objectStoreNames: { contains: (name) => names.has(name) },
    createObjectStore(name) {
      names.add(name);
      data.set(name, new Map());
    },
    close() {},
    transaction(store) {
      const transaction = {};
      setTimeout(() => transaction.oncomplete?.(), 0);
      transaction.objectStore = () => ({
        get(key) {
          const request = {};
          settle(request, data.get(store).get(key));
          return request;
        },
        put(value, key) {
          const request = {};
          data.get(store).set(key, structuredClone(value));
          settle(request, key);
          return request;
        },
        delete(key) {
          const request = {};
          data.get(store).delete(key);
          settle(request, undefined);
          return request;
        },
      });
      return transaction;
    },
  };
  return {
    open() {
      const request = { result: database };
      queueMicrotask(() => {
        request.onupgradeneeded?.();
        request.onsuccess?.();
      });
      return request;
    },
  };
}

const { readToken, writeToken, forgetToken, pendingOperation, rememberOperation, settleOperation } =
  await import("../../src/lib/forge/store.js");
const idb = fakeIndexedDB();

assert.equal(await readToken("github", idb), null);
await writeToken("github", { access_token: "t", expires_at: 5 }, idb);
assert.deepEqual(await readToken("github", idb), { access_token: "t", expires_at: 5 });
await forgetToken("github", idb);
assert.equal(await readToken("github", idb), null);

assert.equal(await pendingOperation("paper", idb), null);
await rememberOperation("paper", { operation: "v2.1.aa", body: { operation: "v2.1.aa" } }, idb);
assert.equal((await pendingOperation("paper", idb)).operation, "v2.1.aa");
assert.equal(await pendingOperation("other", idb), null);
await settleOperation("paper", idb);
assert.equal(await pendingOperation("paper", idb), null);

// After a sign-out the store refuses to open.
await forgetLocalData(null, 10);
await assert.rejects(() => readToken("github", idb), /cleared at sign-out/);

console.log("forge-store ok");
