import { FORGE_DATABASE as DATABASE, localDataForgotten } from "../local-data.js";

// What sync keeps in this browser: the GitHub tokens of the signed-in account
// and, per document, a source-change request the server has not answered yet.
// Sign-out deletes the whole database with the other local data.
const VERSION = 1;
const TOKENS = "tokens";
const OPERATIONS = "operations";

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

function openForgeDatabase(indexedDB_ = globalThis.indexedDB) {
  // After a sign-out this tab must not recreate the database it just deleted.
  if (localDataForgotten()) return Promise.reject(new Error("This browser's copies were cleared at sign-out."));
  if (!indexedDB_) return Promise.reject(new Error("This browser does not provide local storage."));
  return new Promise((resolve, reject) => {
    const request = indexedDB_.open(DATABASE, VERSION);
    let blocked = false;
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(TOKENS)) database.createObjectStore(TOKENS);
      if (!database.objectStoreNames.contains(OPERATIONS)) database.createObjectStore(OPERATIONS);
    };
    request.onsuccess = () => {
      if (blocked) {
        request.result.close();
        return;
      }
      request.result.onversionchange = () => request.result.close();
      resolve(request.result);
    };
    request.onerror = () => reject(request.error || new Error("could not open local storage"));
    request.onblocked = () => {
      blocked = true;
      reject(new Error("local storage is blocked by another tab"));
    };
  });
}

async function withDatabase(indexedDB_, action) {
  const database = await openForgeDatabase(indexedDB_);
  try {
    return await action(database);
  } finally {
    database.close();
  }
}

const read = (store, key, indexedDB_) =>
  withDatabase(indexedDB_, async (database) =>
    (await requestResult(database.transaction(store, "readonly").objectStore(store).get(key))) ?? null);

const write = (store, key, value, indexedDB_) =>
  withDatabase(indexedDB_, async (database) => {
    const transaction = database.transaction(store, "readwrite");
    const done = transactionDone(transaction);
    const objects = transaction.objectStore(store);
    if (value === undefined) objects.delete(key);
    else objects.put(value, key);
    await done;
  });

/// The saved token record for a provider, or null.
export const readToken = (provider, indexedDB_ = globalThis.indexedDB) => read(TOKENS, provider, indexedDB_);

export const writeToken = (provider, token, indexedDB_ = globalThis.indexedDB) =>
  write(TOKENS, provider, token, indexedDB_);

export const forgetToken = (provider, indexedDB_ = globalThis.indexedDB) =>
  write(TOKENS, provider, undefined, indexedDB_);

/// The `{ operation, body }` of a source-change request sent for this
/// document and never answered, or null.
export const pendingOperation = (slug, indexedDB_ = globalThis.indexedDB) => read(OPERATIONS, slug, indexedDB_);

/// Keeps a request before it is sent, so a retry can reuse its operation id.
export const rememberOperation = (slug, value, indexedDB_ = globalThis.indexedDB) =>
  write(OPERATIONS, slug, value, indexedDB_);

/// Drops the pending request once the server has answered it.
export const settleOperation = (slug, indexedDB_ = globalThis.indexedDB) =>
  write(OPERATIONS, slug, undefined, indexedDB_);
