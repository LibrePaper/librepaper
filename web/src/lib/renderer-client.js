// Compilation must not share the editor's event loop. Keep one worker per
// module so warming a large Typst module cannot delay a Markdown preview.
const workers = new Map();
let sequence = 0;

/// What the page sends a worker instead of a whole tree: the entries that
/// differ from `sent`, the tree that worker already holds (null for a worker
/// that holds nothing). Returns the delta and the new `sent`, copied so it
/// keeps no reference to the caller's objects other than the values.
/** @param {{texts: Map<string, string>, assets: Map<string, Uint8Array>, urls: Map<string, string>} | null} sent @param {{main?: string, settings?: object, texts?: Record<string, string>, assets?: Record<string, Uint8Array>, urls?: Record<string, string>}} tree */
export function treeDelta(sent, tree) {
  const texts = tree.texts || {};
  const assets = tree.assets || {};
  const urls = tree.urls || {};
  const delta = {
    main: tree.main,
    settings: tree.settings,
    full: !sent,
    texts: {},
    assets: {},
    urls: {},
    removed: { texts: [], assets: [], urls: [] },
  };
  for (const [path, body] of Object.entries(texts)) {
    if (!sent?.texts.has(path) || sent.texts.get(path) !== body) delta.texts[path] = body;
  }
  for (const [path, bytes] of Object.entries(assets)) {
    if (!sent?.assets.has(path) || sent.assets.get(path) !== bytes) delta.assets[path] = bytes;
  }
  for (const [path, url] of Object.entries(urls)) {
    if (!sent?.urls.has(path) || sent.urls.get(path) !== url) delta.urls[path] = url;
  }
  if (sent) {
    for (const path of sent.texts.keys()) if (!Object.hasOwn(texts, path)) delta.removed.texts.push(path);
    for (const path of sent.assets.keys()) if (!Object.hasOwn(assets, path)) delta.removed.assets.push(path);
    for (const path of sent.urls.keys()) if (!Object.hasOwn(urls, path)) delta.removed.urls.push(path);
  }
  return {
    delta,
    sent: {
      texts: new Map(Object.entries(texts)),
      assets: new Map(Object.entries(assets)),
      urls: new Map(Object.entries(urls)),
    },
  };
}

export function rendererRequest(url, operation, args) {
  let client = workers.get(url);
  if (!client) {
    const worker = new Worker(new URL("./renderer-worker.js", import.meta.url), { type: "module" });
    const pending = new Map();
    client = { worker, pending, sent: null };
    workers.set(url, client);
    worker.onmessage = ({ data }) => {
      const job = pending.get(data.id);
      if (!job) return;
      pending.delete(data.id);
      if (data.error) {
        // A worker that failed mid-apply may hold a partial mirror: the next
        // request sends the whole tree again.
        client.sent = null;
        job.reject(new Error(data.error));
      } else job.resolve(data.result);
    };
    const failed = (event) => {
      workers.delete(url);
      worker.terminate();
      for (const job of pending.values()) job.reject(new Error(event.message || "Renderer worker failed"));
      pending.clear();
    };
    worker.onerror = failed;
    worker.onmessageerror = failed;
  }
  const id = ++sequence;
  let payload = args;
  if (args?.tree) {
    const { delta, sent } = treeDelta(client.sent, args.tree);
    client.sent = sent;
    payload = { ...args, tree: delta };
  }
  const current = client;
  return new Promise((resolve, reject) => {
    current.pending.set(id, { resolve, reject });
    try {
      current.worker.postMessage({ id, url, operation, args: payload });
    } catch (error) {
      current.pending.delete(id);
      current.sent = null;
      reject(error);
    }
  });
}
