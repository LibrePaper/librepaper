// The quiet status in the preview corner while a local run goes on. A run
// that finishes quickly never shows it.
export const RUN_STATUS_DELAY_MS = 2000;

/** Whether a run that has gone on for `elapsedMs` earns the corner status. @param {number} elapsedMs */
export const runStatusVisible = (elapsedMs) => elapsedMs >= RUN_STATUS_DELAY_MS;

/** Whole seconds elapsed, never negative. @param {number} elapsedMs */
export const elapsedSeconds = (elapsedMs) => Math.max(0, Math.floor(elapsedMs / 1000));
