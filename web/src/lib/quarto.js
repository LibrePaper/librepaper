// Compatibility facade for the browser Quarto adapter.
// New code imports the engine explicitly; this path remains for extensions and
// persisted-render checks shipped before the shared-results extraction.
export * from "./engines/quarto.js";
export { sha256, sha256Bytes, stableStringify } from "./results-hash.js";
