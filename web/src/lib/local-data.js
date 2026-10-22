// On a shared computer the next person must not be able to open what the
// last one edited. Projects and rendered pages are kept in this browser, so
// signing out deletes all of them. The forge database holds the GitHub tokens.
export const OFFLINE_DATABASE = "librepaper-offline-v2";
export const PREVIEW_DATABASE = "librepaper-preview-v1";
export const FORGE_DATABASE = "librepaper-forge-v1";

const DATABASES = [OFFLINE_DATABASE, PREVIEW_DATABASE, FORGE_DATABASE];
const CHANNEL = "librepaper-local-data";

// Set once this tab has signed out, or heard another tab do so. Writers check
// it before opening a database, so a pending write cannot recreate the
// previous account's data after the delete.
let forgotten = false;
export const localDataForgotten = () => forgotten;

// Resolves once the deletes have finished, failed, or run out the timeout.
// A delete is blocked while another tab holds the database open, and the
// browser completes it when that tab closes, so waiting longer than the
// timeout gains nothing. It never rejects: a sign-out must still finish.
export function forgetLocalData(indexedDB_ = globalThis.indexedDB, timeoutMs = 2000) {
  forgotten = true;
  if (globalThis.BroadcastChannel) {
    const channel = new BroadcastChannel(CHANNEL);
    channel.postMessage({ type: "signed-out" });
    channel.close();
  }
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

// Another tab of this browser signed out, so this one must stop writing. The
// databases it had open are already being deleted; what remains is the page,
// which would otherwise keep the previous account's work on screen.
// Returns a function that stops listening.
export function listenForSignOut(onSignedOut) {
  if (!globalThis.BroadcastChannel) return () => {};
  const channel = new BroadcastChannel(CHANNEL);
  // The tab that signed out hears its own message too. It is already
  // forgotten and navigates on its own terms, so only other tabs react.
  channel.onmessage = (event) => {
    if (event.data?.type !== "signed-out" || forgotten) return;
    forgotten = true;
    onSignedOut();
  };
  return () => channel.close();
}
