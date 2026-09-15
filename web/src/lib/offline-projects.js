import { projectIdentity, projectIdentityKey } from "./project-identity.js";

// Named for the substrate, not only the schema: the v1 database holds Yjs
// updates, and a Loro document cannot import those -- it rejects them as
// invalid magic bytes, and the project then reads as unsaved on this device.
// The cutover left nothing worth translating, so the old database is
// abandoned rather than migrated.
const DATABASE = "librepaper-offline-v2";
const VERSION = 1;
const PROJECTS = "projects";
const STATES = "states";

function requestResult(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("local storage request failed"));
  });
}

function transactionDone(transaction) {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = resolve;
    transaction.onabort = () => reject(transaction.error || new Error("local storage transaction was aborted"));
    transaction.onerror = () => reject(transaction.error || new Error("local storage transaction failed"));
  });
}

function openOfflineDatabase(indexedDB_ = globalThis.indexedDB) {
  if (!indexedDB_) return Promise.reject(new Error("This browser does not provide offline storage."));
  return new Promise((resolve, reject) => {
    const request = indexedDB_.open(DATABASE, VERSION);
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(PROJECTS)) database.createObjectStore(PROJECTS, { keyPath: "key" });
      if (!database.objectStoreNames.contains(STATES)) database.createObjectStore(STATES);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("could not open offline storage"));
  });
}

async function withDatabase(indexedDB_, action) {
  const database = await openOfflineDatabase(indexedDB_);
  try {
    return await action(database);
  } finally {
    database.close();
  }
}

async function readStore(store, key, indexedDB_) {
  return withDatabase(indexedDB_, (database) =>
    requestResult(database.transaction(store, "readonly").objectStore(store).get(key)));
}

export async function preparedProjects(indexedDB_ = globalThis.indexedDB) {
  return withDatabase(indexedDB_, async (database) => {
    const records = await requestResult(database.transaction(PROJECTS, "readonly").objectStore(PROJECTS).getAll());
    return records.filter((record) => record?.prepared).sort((a, b) => String(b.preparedAt).localeCompare(String(a.preparedAt)));
  });
}

export async function preparedProject({ server, slug }, indexedDB_ = globalThis.indexedDB) {
  const origin = new URL(server || globalThis.location?.origin).origin;
  return (await preparedProjects(indexedDB_)).find((record) => record.identity.origin === origin && record.identity.slug === slug) || null;
}

export async function prepareOfflineProject(metadata, indexedDB_ = globalThis.indexedDB) {
  const identity = projectIdentity({
    server: metadata.server || globalThis.location?.origin,
    slug: metadata.slug,
    createdAt: metadata.created_at || metadata.createdAt,
  });
  const key = projectIdentityKey(identity);
  const state = await readStore(STATES, key, indexedDB_);
  if (!state) throw new Error("Wait for the project to finish saving on this device.");
  const record = {
    key,
    version: VERSION,
    prepared: true,
    preparedAt: new Date().toISOString(),
    identity,
    document: {
      slug: identity.slug,
      created_at: identity.createdAt,
      title: metadata.title || identity.slug,
      source_format: metadata.source_format || "",
      role: metadata.role || "editor",
      can_edit: true,
      offline_prepared: true,
    },
  };
  await withDatabase(indexedDB_, async (database) => {
    const transaction = database.transaction(PROJECTS, "readwrite");
    transaction.objectStore(PROJECTS).put(record);
    await transactionDone(transaction);
  });
  return record;
}

export function durableProjectPersistence(identity, indexedDB_ = globalThis.indexedDB) {
  const key = projectIdentityKey(identity);
  return {
    open(doc, events) {
      let database;
      let closed = false;
      let dirty = false;
      let writing = false;
      let stored = false;
      let pendingFlush = Promise.resolve();
      let unsubscribe;

      async function putState() {
        if (closed || writing || !dirty) return pendingFlush;
        dirty = false;
        writing = true;
        events.writing(1);
        pendingFlush = (async () => {
          try {
            database ||= await openOfflineDatabase(indexedDB_);
            const update = doc.export({ mode: "update" });
            const transaction = database.transaction(STATES, "readwrite");
            transaction.objectStore(STATES).put(update, key);
            await transactionDone(transaction);
            stored = true;
            writing = false;
            if (dirty) return putState();
            events.persisted();
          } catch (error) {
            writing = false;
            events.failed(error);
            throw error;
          }
        })();
        pendingFlush.catch(() => {});
        return pendingFlush;
      }

      // Only local edits need persisting, and `subscribeLocalUpdates` reports
      // only those -- so hydrating the document below cannot re-trigger a write
      // of what we just read.
      function changed() {
        dirty = true;
        void putState();
      }

      const hydration = (async () => {
        try {
          database = await openOfflineDatabase(indexedDB_);
          const saved = await requestResult(database.transaction(STATES, "readonly").objectStore(STATES).get(key));
          if (saved) {
            stored = true;
            doc.import(new Uint8Array(saved));
          }
          unsubscribe = doc.subscribeLocalUpdates(changed);
          events.hydrated();
        } catch (error) {
          events.failed(error);
        }
      })();

      return {
        hydration,
        async flush() {
          await hydration;
          if (!stored) dirty = true;
          if (dirty) await putState();
          await pendingFlush;
        },
        close() {
          closed = true;
          unsubscribe?.();
          database?.close();
        },
      };
    },
  };
}
