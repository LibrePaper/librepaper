// On a shared computer the next person must not be able to open what the
// last one edited. Projects and rendered pages are kept in this browser, so
// signing out deletes both databases.
export const OFFLINE_DATABASE = "librepaper-offline-v2";
export const PREVIEW_DATABASE = "librepaper-preview-v1";

const DATABASES = [OFFLINE_DATABASE, PREVIEW_DATABASE];

// Resolves once both deletes have finished, failed, or run out the timeout.
// A delete is blocked while another tab holds the database open, and the
// browser completes it when that tab closes, so waiting longer than the
// timeout gains nothing. It never rejects: a sign-out must still finish.
export function forgetLocalData(indexedDB_ = globalThis.indexedDB, timeoutMs = 2000) {
  if (!indexedDB_) return Promise.resolve();
  const deletes = DATABASES.map((name) => new Promise((resolve) => {
    try {
      const request = indexedDB_.deleteDatabase(name);
      request.onsuccess = () => resolve();
      request.onerror = () => resolve();
    } catch {
      resolve();
    }
  }));
  let timer;
  const timeout = new Promise((resolve) => {
    timer = setTimeout(resolve, timeoutMs);
  });
  return Promise.race([Promise.all(deletes), timeout]).finally(() => clearTimeout(timer));
}
