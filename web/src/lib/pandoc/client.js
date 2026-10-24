// One Pandoc worker, started on first use and shared by every conversion.
// The module is large, so nothing loads until a download asks for it, and
// conversions run one at a time because the module holds a single file system.
let client = null;
let sequence = 0;
let queue = Promise.resolve();

function moduleUrl() {
  const url = globalThis.LIBREPAPER_MODULES?.pandoc;
  return url ? new URL(url, globalThis.location.href).href : null;
}

/** Whether this deployment offers Pandoc: globalThis.LIBREPAPER_MODULES?.pandoc is a URL. */
export function pandocAvailable() {
  return Boolean(globalThis.LIBREPAPER_MODULES?.pandoc);
}

function start() {
  const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
  const pending = new Map();
  const current = { worker, pending };
  const discard = (message) => {
    if (client === current) client = null;
    worker.terminate();
    for (const job of pending.values()) job.reject(new Error(message));
    pending.clear();
  };
  worker.onmessage = ({ data }) => {
    const job = pending.get(data.id);
    if (!job) return;
    pending.delete(data.id);
    if (!data.error) return job.resolve(data.result);
    job.reject(new Error(data.error));
    if (data.fatal) discard(data.error);
  };
  const failed = (event) => discard(event.message || "Pandoc worker failed");
  worker.onerror = failed;
  worker.onmessageerror = failed;
  return current;
}

function run(options, files) {
  const url = moduleUrl();
  if (!url) return Promise.reject(new Error("Pandoc is not available"));
  client ||= start();
  const current = client;
  const id = ++sequence;
  return new Promise((resolve, reject) => {
    current.pending.set(id, { resolve, reject });
    try {
      current.worker.postMessage({ id, url, options, files });
    } catch (error) {
      current.pending.delete(id);
      reject(error);
    }
  });
}

/**
 * Run Pandoc once, in its worker.
 * options: a Pandoc defaults-file object; the output file is chosen here.
 * files: { [projectPath]: string | Uint8Array }, written at those relative paths.
 * Resolves { output: Uint8Array, warnings: object[], stderr: string }.
 */
export function pandoc(options, files) {
  const result = queue.then(() => run(options, files));
  queue = result.catch(() => {});
  return result;
}
