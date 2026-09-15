// Loro, initialised, for the browser.
//
// The package ships four builds and the one a bundler picks by default is the
// wrong one. `loro-crdt`'s `browser` export condition is wasm-bindgen's
// no-modules glue, and that glue loads three megabytes of wasm with
// `XMLHttpRequest.open(url, false)` -- a blocking main-thread fetch, followed
// by a charCodeAt copy of every byte and a synchronous `WebAssembly.Module`
// -- all at module-evaluation time, before anything is painted. Loro's own
// build says so: "Use the nodejs, web, base64, or bundler entry for this
// runtime."
//
// So the browser takes the `web` build, which fetches and instantiates the
// module the streaming way, and this file is where the one initialisation
// happens. `vite.config.js` points every `loro-crdt` import here, including
// the one inside `loro-codemirror`, so there is a single wasm instance and a
// single init behind all of them.
//
// Node keeps resolving `loro-crdt` itself -- the alias is the bundler's, not
// the package's -- so the check scripts still get the native build.
//
// The top-level await is the point: every importer of this module is reached
// through a dynamic `import()`, so awaiting here holds only the chunk that
// actually needs a CRDT, and a reader who never edits never loads it at all.
import init from "loro-crdt/web";

await init();

export * from "loro-crdt/web";
