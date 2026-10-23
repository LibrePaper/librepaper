// This module is imported by the landing page, so it must stay free of loro-crdt.
// Loro-backed persistence lives in offline-persistence.js.
import { projectIdentity, projectIdentityKey } from "./project-identity.js";
import { OFFLINE_DATABASE as DATABASE, localDataForgotten } from "./local-data.js";

// Named for the substrate, not only the schema: the v1 database holds Yjs
// updates, and a Loro document cannot import those -- it rejects them as
// invalid magic bytes, and the project then reads as unsaved on this device.
// The cutover left nothing worth translating, so the old database is
// abandoned rather than migrated. The name itself lives in local-data.js.
const VERSION = 2;
const PROJECTS = "projects";
export const STATES = "states";
export const UPDATES = "updates";

export function requestResult(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("local storage request failed"));
  });
}

export function transactionDone(transaction) {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = resolve;
    transaction.onabort = () => reject(transaction.error || new Error("local storage transaction was aborted"));
    transaction.onerror = () => reject(transaction.error || new Error("local storage transaction failed"));
  });
}

export function openOfflineDatabase(indexedDB_ = globalThis.indexedDB) {
  // After a sign-out this tab must not recreate the database it just deleted.
  if (localDataForgotten()) return Promise.reject(new Error("This browser's copies were cleared at sign-out."));
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

export async function localProjects(indexedDB_ = globalThis.indexedDB) {
  return withDatabase(indexedDB_, async (database) => {
    const records = await requestResult(database.transaction(PROJECTS, "readonly").objectStore(PROJECTS).getAll());
    return records.filter((record) => record?.prepared).sort((a, b) => String(b.preparedAt).localeCompare(String(a.preparedAt)));
  });
}

export async function localProject({ server, slug }, indexedDB_ = globalThis.indexedDB) {
  const origin = new URL(server || globalThis.location?.origin).origin;
  return (await localProjects(indexedDB_)).find((record) => record.identity.origin === origin
    && (record.identity.slug === slug || record.document?.slug === slug)) || null;
}

export async function rememberProject(metadata, indexedDB_ = globalThis.indexedDB) {
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
      slug: metadata.slug || (identity.version === 1 ? identity.slug : ""),
      document_id: identity.version === 2 ? identity.documentId : metadata.document_id || metadata.documentId || "",
      created_at: metadata.created_at || metadata.createdAt || (identity.version === 1 ? identity.createdAt : ""),
      title: metadata.title || metadata.slug || (identity.version === 1 ? identity.slug : ""),
      source_format: metadata.source_format || "",
      role: metadata.role || "editor",
      can_edit: true,
      local_copy: true,
      // A cached open must know where the documents frame lives; a record
      // saved before this was kept has none, and the reader then refuses to
      // show the preview rather than guess an origin.
      ...(metadata.docs_origin ? { docs_origin: metadata.docs_origin } : {}),
    },
  };
  await withDatabase(indexedDB_, async (database) => {
    const transaction = database.transaction(PROJECTS, "readwrite");
    const done = transactionDone(transaction);
    const store = transaction.objectStore(PROJECTS);
    // Remembering a project again must not forget that its shell is cached.
    const existing = await requestResult(store.get(key));
    if (existing?.shellReady) record.shellReady = true;
    store.put(record);
    await done;
  });
  return record;
}

// Records that the app shell is cached for this project, so the next
// rememberProject does not cache it again. A missing record is left alone:
// the project was forgotten while the shell was caching.
export async function markShellReady({ server, documentId, slug, createdAt }, indexedDB_ = globalThis.indexedDB) {
  const identity = projectIdentity({
    server: server || globalThis.location?.origin,
    documentId,
    slug,
    createdAt,
  });
  const key = projectIdentityKey(identity);
  await withDatabase(indexedDB_, async (database) => {
    const transaction = database.transaction(PROJECTS, "readwrite");
    const done = transactionDone(transaction);
    const store = transaction.objectStore(PROJECTS);
    const record = await requestResult(store.get(key));
    if (record) store.put({ ...record, shellReady: true });
    await done;
  });
}

// Removes this browser's offer of one project: its record only. Without the
// record the project no longer opens offline or appears in the offline list.
// The saved state and updates stay, because they may hold edits not yet sent
// to the server, and signing out clears them.
export async function forgetProject({ server, documentId, slug, createdAt }, indexedDB_ = globalThis.indexedDB) {
  const identity = projectIdentity({
    server: server || globalThis.location?.origin,
    documentId,
    slug,
    createdAt,
  });
  const key = projectIdentityKey(identity);
  await withDatabase(indexedDB_, async (database) => {
    const transaction = database.transaction(PROJECTS, "readwrite");
    const done = transactionDone(transaction);
    transaction.objectStore(PROJECTS).delete(key);
    await done;
  });
}
