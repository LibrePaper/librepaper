// One Harper worker, started on first use and shared by every editor. The
// module is large, so nothing loads until a document asks for a check, and
// requests run one at a time because the worker holds a single linter.
import { read, write, HARPER_WORDS } from "../storage.js";

let client = null;
let sequence = 0;
let queue = Promise.resolve();

function moduleUrl() {
  const url = globalThis.LIBREPAPER_MODULES?.harper;
  return url ? new URL(url, globalThis.location.href).href : null;
}

/** Whether this deployment offers Harper: globalThis.LIBREPAPER_MODULES?.harper is a URL. */
export function harperAvailable() {
  return Boolean(globalThis.LIBREPAPER_MODULES?.harper);
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
  const failed = (event) => discard(event.message || "Harper worker failed");
  worker.onerror = failed;
  worker.onmessageerror = failed;
  return current;
}

function run(message) {
  const url = moduleUrl();
  if (!url) return Promise.reject(new Error("Harper is not available"));
  client ||= start();
  const current = client;
  const id = ++sequence;
  return new Promise((resolve, reject) => {
    current.pending.set(id, { resolve, reject });
    try {
      current.worker.postMessage({ id, url, words: harperWords(), ...message });
    } catch (error) {
      current.pending.delete(id);
      reject(error);
    }
  });
}

/** The words the writer has added to Harper's dictionary. */
export function harperWords() {
  const words = read(HARPER_WORDS, []);
  return Array.isArray(words) ? words.filter((word) => typeof word === "string") : [];
}

/** Add a word to the stored list. The next lint sends it. */
export function addWord(word) {
  const trimmed = String(word).trim();
  if (!trimmed) return;
  const words = harperWords();
  if (words.includes(trimmed)) return;
  write(HARPER_WORDS, [...words, trimmed]);
}

/**
 * Lint prose in the worker.
 * dialect: "american" | "british" | "australian" | "canadian" | "indian".
 * Resolves Array<{ from, to, kind, message, problem, suggestions: Array<{ kind: "replace" | "remove" | "insert", text }> }>.
 */
export function lintProse(text, dialect) {
  const result = queue.then(() => run({ op: "lint", dialect, text }));
  queue = result.catch(() => {});
  return result;
}
