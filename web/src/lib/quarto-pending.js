// A completed render waiting for publication is an outbox, not an instruction
// to execute code. Keep it across reloads without putting large figure payloads
// in synchronous localStorage. IndexedDB already isolates records by origin;
// the document ID is an additional mandatory scope check.
const DATABASE = "librepaper-quarto-outbox";
const STORE = "pending";
const MAX_DOCUMENTS = 8;
const MAX_CHARACTERS = 96 * 1024 * 1024;
const LIFETIME = 24 * 60 * 60 * 1000;

function valid(slug, payload) {
  return typeof slug === "string" && slug.length > 0 &&
    payload?.manifest?.schema === "librepaper-quarto-bundle/v1" &&
    payload.manifest.document_id === slug &&
    typeof payload.manifest.render_id === "string" &&
    Array.isArray(payload.blobs);
}

function open() {
  return new Promise((resolve, reject) => {
    if (!globalThis.indexedDB) return reject(new Error("Browser storage is unavailable; keep this tab open to retry sharing."));
    const request = indexedDB.open(DATABASE, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE, { keyPath: "slug" });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("Could not open the Quarto outbox."));
    request.onblocked = () => reject(new Error("Another tab is blocking the Quarto outbox."));
  });
}

async function transaction(mode, action) {
  const database = await open();
  try {
    return await new Promise((resolve, reject) => {
      const tx = database.transaction(STORE, mode);
      let result;
      let failure;
      tx.oncomplete = () => resolve(result);
      tx.onerror = tx.onabort = () => reject(failure || tx.error || new Error("Could not save the completed render; keep this tab open to retry sharing."));
      action(tx.objectStore(STORE), (value) => { result = value; }, (error) => { failure = error; tx.abort(); });
    });
  } finally {
    database.close();
  }
}

export async function savePendingQuarto(slug, payload) {
  if (!valid(slug, payload)) throw new Error("Completed render does not belong to this document.");
  // Match the local runner's bounded output budget with room for base64 and
  // metadata. Browser quota failures still propagate to the caller.
  if (JSON.stringify(payload).length > MAX_CHARACTERS) throw new Error("Completed render is too large for the browser outbox; keep this tab open to retry sharing.");
  const now = Date.now();
  return transaction("readwrite", (store, done, fail) => {
    const request = store.getAll();
    request.onsuccess = () => {
      const retained = [];
      for (const record of request.result) {
        if (record.expires <= now || !valid(record.slug, record.payload)) store.delete(record.slug);
        else retained.push(record);
      }
      const existing = retained.find((record) => record.slug === slug);
      if (existing && existing.payload.manifest.render_id !== payload.manifest.render_id) {
        fail(new Error("Another completed render for this document is waiting to share; keep this tab open to retain this result."));
        return;
      }
      if (!retained.some((record) => record.slug === slug) && retained.length >= MAX_DOCUMENTS) {
        fail(new Error("The Quarto outbox has eight documents waiting to share; keep this tab open and share those results first."));
        return;
      }
      store.put({ slug, expires: now + LIFETIME, payload });
      done(true);
    };
  });
}

export async function loadPendingQuarto(slug) {
  return transaction("readwrite", (store, done) => {
    const request = store.get(slug);
    request.onsuccess = () => {
      const record = request.result;
      if (!record) return done(null);
      if (record.expires <= Date.now() || !valid(slug, record.payload)) {
        store.delete(slug);
        return done(null);
      }
      done(record.payload);
    };
  });
}

// An acknowledgement for an older job must not remove a newer pending render
// written by another tab while the upload was in progress.
export async function clearPendingQuarto(slug, renderId) {
  return transaction("readwrite", (store, done) => {
    const request = store.get(slug);
    request.onsuccess = () => {
      if (request.result?.payload?.manifest?.render_id === renderId) store.delete(slug);
      done(true);
    };
  });
}
