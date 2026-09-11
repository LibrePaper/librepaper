import { projectionHunks, validateProjection } from "./diff-display.js";

const MAX_PENDING = 4;
const WATCHDOG_MS = 15_000;
let worker = null;
let sequence = 0;
const pending = new Map();

export function retireComparisons(message = "Comparison cancelled") {
  worker?.terminate();
  worker = null;
  for (const job of pending.values()) { clearTimeout(job.timer); job.reject(new Error(message)); }
  pending.clear();
}

function getWorker() {
  if (worker) return worker;
  worker = new Worker(new URL("./diff-worker.js", import.meta.url), { type: "module" });
  worker.onmessage = ({ data }) => {
    const job = pending.get(data?.id);
    if (!job) return;
    pending.delete(data.id);
    clearTimeout(job.timer);
    if (data.error) job.reject(new Error(data.error));
    else if (!Array.isArray(data.hunks) || data.hunks.length > 1000) job.reject(new Error("Invalid comparison result"));
    else job.resolve(data.hunks);
  };
  worker.onerror = () => retireComparisons("Comparison worker failed; retry or use source comparison.");
  return worker;
}

export async function compareProjections(baseline, target) {
  if (!validateProjection(baseline) || !validateProjection(target)) throw new Error("The document is too large or its projection is unavailable.");
  // Node runs the same pure bounded algorithm for central verification.
  if (typeof window === "undefined" && typeof Worker === "undefined") return projectionHunks(baseline, target);
  if (typeof Worker === "undefined") throw new Error("This browser cannot run rendered comparisons. Use source comparison.");
  if (pending.size >= MAX_PENDING) throw new Error("Comparison is busy. Retry when the current comparison finishes.");
  const active = getWorker();
  const id = ++sequence;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => retireComparisons("Comparison took too long; retry or use source comparison."), WATCHDOG_MS);
    pending.set(id, { resolve, reject, timer });
    try { active.postMessage({ id, baseline, target }); }
    catch (error) { clearTimeout(timer); pending.delete(id); reject(error); }
  });
}
