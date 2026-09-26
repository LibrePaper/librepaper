// Shared bits for the browser tests that drive a Loro document.
//
// ## One wasm module, or none of it means anything
//
// A test that needs its own `LoroDoc` -- to play the part of a peer -- has to
// get it from the same module the components got theirs from. Several of
// these tests reached into the package for `bundler/index.js` by absolute
// path, which is a different file from the one the export map hands the
// components: two wasm modules, with two memories.
//
// Nothing about that shows until a handle crosses between them. A
// `VersionVector` or a `Frontiers` is a pointer into one module's memory, so
// `peer.export({from: session.doc.oplogVersion()})` reads whatever happens to
// sit at that address in the other -- "memory access out of bounds" when the
// address is past the heap, and a plausible wrong answer when it is not. The
// insert test failed the first way, which is the lucky one.
//
// The test entries are written to a temporary directory, so a bare
// `"loro-crdt"` in one of them does not resolve at all -- there is no
// `node_modules` above `/tmp`. So the two sides are brought together by an
// alias instead: the test entry imports `"loro-crdt"` like everything else,
// and this points every such import, the components' included, at one file.
//
// Why that file and not `src/lib/loro-web.js`, which is what `vite.config.js`
// aliases to in the real build: under `build.lib` -- which is how every one of
// these tests builds -- assets are inlined, so the three-megabyte wasm becomes
// a `data:` URL and the `web` build cannot start from it. These tests take the
// package's browser build, which carries its own module. That is a difference
// from production, and it is the narrow kind: the wasm is loaded another way,
// and the API above it is the same.
import { fileURLToPath } from "node:url";

const web = fileURLToPath(new URL("../../", import.meta.url));

/// Pass as `resolve: { alias: loroAlias }` beside `configFile: false`, in
/// every browser test that builds a component touching the document --
/// whether or not the test names Loro itself, because the components do.
///
/// Anchored, so `loro-crdt/web` and `loro-crdt/base64` still resolve normally.
export const loroAlias = [
  { find: /^loro-crdt$/, replacement: `${web}node_modules/loro-crdt/browser/index.js` },
];

/// The MIME type a test's static server should answer with, given the path it
/// is serving. `.wasm` is here because a build that does fetch its module
/// over HTTP wants `application/wasm`: without it wasm-bindgen warns and
/// falls back from `instantiateStreaming` to buffering the whole module.
export const contentType = (path) => {
  if (path.endsWith(".wasm")) return "application/wasm";
  if (path.endsWith(".css")) return "text/css";
  if (path.endsWith(".html")) return "text/html";
  return "text/javascript";
};
