// The Loro classes that reader-side modules (anchors, track changes) need at
// run time. Those modules are statically imported by the reader, which must
// not load the CRDT wasm, so they read the constructors from here instead of
// importing "loro-crdt". `loro-web.js` fills this in once the wasm is ready,
// and every function that touches these classes runs on a live document, so
// by then it has been.
//
// Node (tests and check scripts) resolves `loro-crdt` natively and never loads
// loro-web.js, so it fills the registry itself.

/** @type {{ Cursor: typeof import("loro-crdt").Cursor, LoroMap: typeof import("loro-crdt").LoroMap }} */
export const loroClasses = /** @type {any} */ ({});

/** @param {{ Cursor: any, LoroMap: any }} loro */
export function setLoroClasses(loro) {
  loroClasses.Cursor = loro.Cursor;
  loroClasses.LoroMap = loro.LoroMap;
}

if (/** @type {any} */ (globalThis).process?.versions?.node) {
  const specifier = "loro-crdt";
  setLoroClasses(await import(/* @vite-ignore */ specifier));
}
