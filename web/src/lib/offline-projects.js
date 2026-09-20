import { projectIdentity, projectIdentityKey } from "./project-identity.js";
import { LoroDoc, VersionVector } from "loro-crdt";

// Named for the substrate, not only the schema: the v1 database holds Yjs
// updates, and a Loro document cannot import those -- it rejects them as
// invalid magic bytes, and the project then reads as unsaved on this device.
// The cutover left nothing worth translating, so the old database is
// abandoned rather than migrated.
const DATABASE = "librepaper-offline-v2";
const VERSION = 2;
const PROJECTS = "projects";
const STATES = "states";
const UPDATES = "updates";
const COMPACT_AFTER_UPDATES = 64;
const COMPACT_AFTER_BYTES = 4 * 1024 * 1024;

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
    let blocked = false;
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(PROJECTS)) database.createObjectStore(PROJECTS, { keyPath: "key" });
      if (!database.objectStoreNames.contains(STATES)) database.createObjectStore(STATES);
      if (!database.objectStoreNames.contains(UPDATES)) {
        const updates = database.createObjectStore(UPDATES, { keyPath: ["project", "id"] });
        updates.createIndex("project", "project", { unique: false });
      }
    };
    request.onsuccess = () => {
      if (blocked) {
        request.result.close();
        return;
      }
      request.result.onversionchange = () => request.result.close();
      resolve(request.result);
    };
    request.onerror = () => reject(request.error || new Error("could not open offline storage"));
    request.onblocked = () => {
      blocked = true;
      reject(new Error("offline storage is blocked by another tab"));
    };
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
  return (await preparedProjects(indexedDB_)).find((record) => record.identity.origin === origin
    && (record.identity.slug === slug || record.document?.slug === slug)) || null;
}

export async function prepareOfflineProject(metadata, indexedDB_ = globalThis.indexedDB) {
  const identity = projectIdentity({
    server: metadata.server || globalThis.location?.origin,
    documentId: metadata.document_id || metadata.documentId,
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
      slug: metadata.slug || identity.slug,
      document_id: identity.documentId || metadata.document_id || "",
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

export function durableProjectPersistence(identity, indexedDB_ = globalThis.indexedDB, legacyIdentity = null) {
  const key = projectIdentityKey(identity);
  const legacyKey = legacyIdentity ? projectIdentityKey(legacyIdentity) : null;
  return {
    open(doc, events) {
      let database;
      let closed = false;
      let dirty = false;
      let writing = false;
      let stored = false;
      let record = null;
      let persistedVector = null;
      let pendingFlush = Promise.resolve();
      let unsubscribe;
      let hydrating = true;
      const tab = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random()}`;
      let batch = 0;

      const updateId = () => `${tab}:${String(batch += 1).padStart(12, "0")}`;

      function updatesFor(transaction, project = key) {
        return requestResult(transaction.objectStore(UPDATES).index("project").getAll(project));
      }

      async function putState() {
        if (closed || writing || !dirty) return pendingFlush;
        dirty = false;
        writing = true;
        events.writing(1);
        pendingFlush = (async () => {
          try {
            database ||= await openOfflineDatabase(indexedDB_);
            const version = doc.oplogVersion().encode();
            const update = doc.export({
              mode: "update",
              ...(persistedVector ? { from: VersionVector.decode(persistedVector) } : {}),
            });
            const transaction = database.transaction([STATES, UPDATES], "readwrite");
            const done = transactionDone(transaction);
            const states = transaction.objectStore(STATES);
            const updates = transaction.objectStore(UPDATES);
            const [current, pending] = await Promise.all([
              requestResult(states.get(key)),
              updatesFor(transaction),
            ]);
            const entry = { project: key, id: updateId(), update, vector: version, bytes: update.byteLength };
            updates.add(entry);
            const count = pending.length + 1;
            const bytes = pending.reduce((sum, item) => sum + Number(item.bytes || item.update?.byteLength || 0), 0)
              + update.byteLength;
            let next = current?.version === 3 ? current : {
              version: 3,
              snapshot: current?.snapshot || (current instanceof Uint8Array ? current : null),
            };
            if (!next.snapshot || count >= COMPACT_AFTER_UPDATES || bytes >= COMPACT_AFTER_BYTES) {
              const merging = new LoroDoc();
              if (next.snapshot) merging.import(new Uint8Array(next.snapshot));
              for (const item of pending) merging.import(new Uint8Array(item.update));
              merging.import(update);
              const snapshot = merging.export({ mode: "update" });
              next = { ...next, version: 3, snapshot, vector: merging.oplogVersion().encode() };
              for (const item of pending) updates.delete([key, item.id]);
              updates.delete([key, entry.id]);
            }
            states.put(next, key);
            await done;
            record = next;
            persistedVector = version;
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

      // The offline copy is a recovery point, not merely an outbox. Persist
      // imports from the room as well as local edits so an explicitly flushed
      // document is exactly the document that will open offline. The saved
      // state itself is imported before this subscription is installed, which
      // is the sole update that must not dirty the record again.
      function changed() {
        if (closed || hydrating) return;
        dirty = true;
        void putState();
      }

      const hydration = (async () => {
        try {
          database = await openOfflineDatabase(indexedDB_);
          let saved = await requestResult(database.transaction(STATES, "readonly").objectStore(STATES).get(key));
          if (!saved && legacyKey && legacyKey !== key) {
            saved = await requestResult(database.transaction(STATES, "readonly").objectStore(STATES).get(legacyKey));
            if (saved) {
              const migration = database.transaction(STATES, "readwrite");
              migration.objectStore(STATES).put(saved, key);
              migration.objectStore(STATES).delete(legacyKey);
              await transactionDone(migration);
            }
          }
          if (closed) return;
          if (saved) {
            stored = true;
            if (saved.version === 2 || saved.version === 3) {
              record = saved;
              if (saved.snapshot) doc.import(new Uint8Array(saved.snapshot));
              for (const update of saved.updates || []) doc.import(new Uint8Array(update));
              persistedVector = saved.vector ? new Uint8Array(saved.vector) : doc.oplogVersion().encode();
            } else {
              // v1 records were a single full-history update. Read them once;
              // the next transaction rewrites them in the incremental form.
              doc.import(new Uint8Array(saved));
              persistedVector = doc.oplogVersion().encode();
              record = { version: 2, snapshot: new Uint8Array(saved), updates: [], bytes: saved.byteLength, vector: persistedVector };
            }
            const transaction = database.transaction(UPDATES, "readonly");
            const pending = await updatesFor(transaction);
            for (const item of pending) doc.import(new Uint8Array(item.update));
            if (pending.length) persistedVector = doc.oplogVersion().encode();
          }
          hydrating = false;
          unsubscribe = doc.subscribe(changed);
          if (closed) {
            unsubscribe();
            unsubscribe = undefined;
            return;
          }
          events.hydrated();
        } catch (error) {
          hydrating = false;
          if (closed) return;
          events.failed(error);
        }
      })();

      return {
        hydration,
        async flush() {
          await hydration;
          const current = doc.oplogVersion().encode();
          if (!stored || !persistedVector || current.byteLength !== persistedVector.byteLength
              || current.some((byte, index) => byte !== persistedVector[index])) dirty = true;
          if (dirty) await putState();
          await pendingFlush;
        },
        // There is no `confirm(coverage)` any more, and there is no stored
        // coverage to confirm. The server used to send the Loro vector it
        // had durably covered on every acknowledgement, and this kept it so
        // a restart could tell which of its own updates were already safe.
        // SPEC-server-is-a-log §6.3 removes it: nothing is persisted per
        // batch and no replay cursor exists, so there is no ordering between
        // cursor persistence and update persistence to get wrong. What is
        // here is the whole document, and reconnecting exports whatever the
        // server's vector says it is missing.
        close() {
          closed = true;
          unsubscribe?.();
          void pendingFlush.catch(() => {}).finally(() => database?.close());
        },
      };
    },
  };
}
