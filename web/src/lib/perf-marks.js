// Opt-in timing for the live preview and the reader. Nothing is recorded unless
// globalThis.__LP_PERF__ is true, which the typing bench sets, so a normal
// session leaves no entries in the performance timeline.
export function perfStart() { return globalThis.__LP_PERF__ ? performance.now() : 0; }
export function perfEnd(name, start) { if (start && globalThis.__LP_PERF__) performance.measure(name, { start, end: performance.now() }); }
