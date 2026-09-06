// Compilation must not share the editor's event loop. Keep one worker per
// module so warming a large Typst module cannot delay a Markdown preview.
const workers = new Map();
let sequence = 0;

export function rendererRequest(url, operation, args) {
  let client = workers.get(url);
  if (!client) {
    const worker = new Worker(new URL("./renderer-worker.js", import.meta.url), { type: "module" });
    const pending = new Map();
    client = { worker, pending };
    workers.set(url, client);
    worker.onmessage = ({ data }) => {
      const job = pending.get(data.id);
      if (!job) return;
      pending.delete(data.id);
      if (data.error) job.reject(new Error(data.error));
      else job.resolve(data.result);
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
  return new Promise((resolve, reject) => {
    client.pending.set(id, { resolve, reject });
    try {
      client.worker.postMessage({ id, url, operation, args });
    } catch (error) {
      client.pending.delete(id);
      reject(error);
    }
  });
}
