import assert from "node:assert/strict";
import { durableProjectPersistence, forgetProject } from "../../src/lib/offline-projects.js";
import { projectIdentity, projectIdentityKey } from "../../src/lib/project-identity.js";

const blockedIndexedDB = {
  open() {
    const request = {};
    queueMicrotask(() => request.onblocked?.());
    return request;
  },
};

let failure = null;
const persistence = durableProjectPersistence({ origin: "https://example.test", slug: "paper", createdAt: "now" }, blockedIndexedDB);
const opened = persistence.open({}, {
  writing() {},
  hydrated() { throw new Error("blocked storage must not report hydration"); },
  failed(error) { failure = error; },
});
await opened.hydration;
assert.match(failure?.message || "", /blocked by another tab/);
opened.close();

// A small stand-in for the IndexedDB calls forgetProject makes. Requests finish
// on a later turn of the event loop, and a transaction completes once nothing
// is left pending, as in a browser.
function fakeIndexedDB() {
  let database = null;
  return {
    open() {
      const request = {};
      setImmediate(() => {
        if (!database) {
          database = fakeDatabase();
          request.result = database;
          request.onupgradeneeded?.();
        }
        request.result = database;
        request.onsuccess?.();
      });
      return request;
    },
  };
}

function fakeDatabase() {
  const stores = new Map();
  return {
    objectStoreNames: { contains: (name) => stores.has(name) },
    createObjectStore(name, options = {}) {
      const store = { keyPath: options.keyPath, rows: new Map(), indexes: new Map() };
      stores.set(name, store);
      return { createIndex(indexName, keyPath) { store.indexes.set(indexName, keyPath); } };
    },
    transaction() {
      return fakeTransaction((name) => stores.get(name));
    },
    close() {},
  };
}

function fakeTransaction(lookup) {
  const transaction = { oncomplete: null, onabort: null, onerror: null, error: null };
  let pending = 0;
  let done = false;
  const perform = (action) => {
    const request = {};
    pending += 1;
    setImmediate(() => {
      let result;
      try {
        result = action();
      } catch (error) {
        request.error = error;
        request.onerror?.({});
        finish();
        return;
      }
      request.result = result;
      request.onsuccess?.({});
      finish();
    });
    return request;
  };
  const finish = () => {
    pending -= 1;
    if (pending !== 0) return;
    setImmediate(() => {
      if (pending !== 0 || done) return;
      done = true;
      transaction.oncomplete?.({});
    });
  };
  transaction.objectStore = (name) => fakeStore(lookup(name), perform);
  return transaction;
}

function fakeStore(store, perform) {
  const keyOf = (value, key) => {
    if (store.keyPath === undefined) return JSON.stringify(key);
    if (Array.isArray(store.keyPath)) return JSON.stringify(store.keyPath.map((part) => value[part]));
    return JSON.stringify(value[store.keyPath]);
  };
  const rows = () => [...store.rows.values()].map((row) => row.value);
  return {
    get: (key) => perform(() => store.rows.get(JSON.stringify(key))?.value),
    getAll: () => perform(() => rows()),
    put: (value, key) => perform(() => { store.rows.set(keyOf(value, key), { value }); }),
    delete: (key) => perform(() => { store.rows.delete(JSON.stringify(key)); }),
    index: (name) => ({
      getAll: (wanted) => perform(() => rows().filter((value) => value[store.indexes.get(name)] === wanted)),
    }),
  };
}

function openFake(indexedDB_) {
  return new Promise((resolve, reject) => {
    const request = indexedDB_.open("librepaper-offline-v2", 2);
    request.onupgradeneeded = () => {
      request.result.createObjectStore("projects", { keyPath: "key" });
      request.result.createObjectStore("states");
      request.result.createObjectStore("updates", { keyPath: ["project", "id"] }).createIndex("project", "project");
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function readStore(database, store, method, ...args) {
  return new Promise((resolve, reject) => {
    const request = database.transaction([store], "readonly").objectStore(store)[method](...args);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

// forgetProject removes only one project's record, so it no longer opens offline.
// Its saved state and updates stay, since they may hold edits not yet sent, and
// another project's rows are left as they were.
{
  const indexedDB_ = fakeIndexedDB();
  const server = "https://example.test";
  const mine = { server, documentId: "11111111-1111-4111-8111-111111111111", slug: "paper", createdAt: "" };
  const theirs = { server, documentId: "22222222-2222-4222-8222-222222222222", slug: "paper", createdAt: "" };
  const mineKey = projectIdentityKey(projectIdentity(mine));
  const theirsKey = projectIdentityKey(projectIdentity(theirs));

  const database = await openFake(indexedDB_);
  const seeding = database.transaction(["projects", "states", "updates"], "readwrite");
  const seeded = new Promise((resolve) => { seeding.oncomplete = resolve; });
  seeding.objectStore("projects").put({ key: mineKey, prepared: true });
  seeding.objectStore("projects").put({ key: theirsKey, prepared: true });
  seeding.objectStore("states").put({ version: 3 }, mineKey);
  seeding.objectStore("states").put({ version: 3 }, theirsKey);
  seeding.objectStore("updates").put({ project: mineKey, id: "tab:1", update: new Uint8Array([1]) });
  seeding.objectStore("updates").put({ project: mineKey, id: "tab:2", update: new Uint8Array([2]) });
  seeding.objectStore("updates").put({ project: theirsKey, id: "tab:1", update: new Uint8Array([3]) });
  await seeded;

  await forgetProject(mine, indexedDB_);

  assert.equal(await readStore(database, "projects", "get", mineKey), undefined);
  assert.ok(await readStore(database, "states", "get", mineKey));
  assert.deepEqual(
    (await readStore(database, "updates", "getAll")).filter((row) => row.project === mineKey).map((row) => row.id),
    ["tab:1", "tab:2"],
  );
  assert.ok(await readStore(database, "projects", "get", theirsKey));
  assert.ok(await readStore(database, "states", "get", theirsKey));
}
