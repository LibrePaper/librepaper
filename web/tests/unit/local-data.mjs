// Signing out must clear what this browser kept for the account. The
// databases are fakes here: what matters is which names are deleted, and
// that a delete which never finishes cannot hold the sign-out up.
import assert from "node:assert/strict";
import { OFFLINE_DATABASE as OFFLINE, PREVIEW_DATABASE as PREVIEW, forgetLocalData } from "../../src/lib/local-data.js";

// A store that answers deletes on the next turn, as a browser does when no
// other tab has the database open.
function fakeIndexedDB(databases) {
  return {
    databases,
    deleteDatabase(name) {
      const request = {};
      queueMicrotask(() => {
        databases.delete(name);
        request.onsuccess?.();
      });
      return request;
    },
  };
}

// Both databases hold something, and both are gone once the sign-out is over.
{
  const databases = new Map([
    [OFFLINE, { projects: ["paper"] }],
    [PREVIEW, { pages: ["paper"] }],
    ["unrelated", { keep: true }],
  ]);
  await forgetLocalData(fakeIndexedDB(databases), 1000);
  assert.deepEqual([...databases.keys()], ["unrelated"]);
}

// A browser without IndexedDB has nothing to clear, and the sign-out goes on.
assert.equal(await forgetLocalData(null, 1000), undefined);

// A delete that is blocked by another tab stays pending, so the call gives up
// after the timeout instead of waiting for that tab to close.
{
  const blocked = {
    deleteDatabase() {
      const request = {};
      queueMicrotask(() => request.onblocked?.());
      return request;
    },
  };
  const started = Date.now();
  await forgetLocalData(blocked, 30);
  assert.ok(Date.now() - started >= 25, "resolved before the timeout");
}

// A browser that throws on delete must not stop the sign-out either.
{
  const throwing = {
    deleteDatabase() {
      throw new Error("no storage here");
    },
  };
  await forgetLocalData(throwing, 1000);
}
