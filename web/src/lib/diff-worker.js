import { projectionHunks, validateProjection } from "./diff-display.js";

self.onmessage = ({ data }) => {
  const { id, baseline, target } = data || {};
  if (!Number.isSafeInteger(id)) return;
  try {
    if (!validateProjection(baseline) || !validateProjection(target)) throw new Error("Invalid comparison projection");
    const hunks = projectionHunks(baseline, target);
    self.postMessage({ id, hunks });
  } catch (error) { self.postMessage({ id, error: error.message || "Comparison failed" }); }
};
