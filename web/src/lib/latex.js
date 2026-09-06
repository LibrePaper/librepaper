// LaTeX, compiled in this browser.
//
// This module plays the part `renderers.js` plays for the engine: it owns the
// compiler, the worker and the promise of the next compile, and it presents
// one interface to the editor while hiding three different Emscripten
// programs behind it. Everything past this file's exports is a distribution's
// private business.
//
// Three words carry the weight, and each means one thing here.
//
// A **distribution** is a set of static files a browser fetches and runs: one
// or more WebAssembly modules, the JavaScript Emscripten generated to host
// them, and the TeX Live files the engine reads. Komodoc carries no TeX and
// no build embeds one. Nothing is fetched until a reader chooses, the choice
// is kept in this browser and asked for once, and every file comes from one
// base URL -- the deployment's mirror -- because the list of packages a
// document asks for is a description of the document, and because a
// third-party endpoint that goes away would take every LaTeX document on
// every deployment with it. (One has: SwiftLaTeX's own package server has
// been answering 522 for some time. That is what a mirror is for.)
//
// A **tree** is the document: `{ main, texts, assets }`, the main path, the
// texts as strings and the assets as bytes, exactly as `renderers.js` takes
// it. Every path is written into the engine's
// in-memory filesystem where the path says, and the engine is run on the main
// file, so `\include`, `\includegraphics`, a `.sty` of the author's own and a
// `.bib` beside the document all work the way they do on a desk.
//
// The **log** is parsed in `latex/log.js`, in JavaScript, because the
// compiler is not our crate and there is nothing on the Rust side to parse
// it. That file says the rest.
//
// A compile takes seconds, so it runs in a Web Worker: at most one running
// and one queued, and the queued one is always the latest tree.

import { cached, persist } from "./cache.js";

/// Where the distributions are served from: this origin, always. `--latex`
/// says where the *server* reads them from -- a bucket, or a directory on the
/// machine it runs on -- and the server proxies them to here, so this is the
/// only origin a compile ever fetches from and it is the one the page came
/// from. The tests override it to point at a mirror served beside them.
export const DEFAULT_BASE = "/latex/";

/// How long the source has to be quiet before a compile starts.
///
/// Twenty-five times the typst delay, and still well under the compile
/// itself: a LaTeX compile takes seconds, and a preview that started one on
/// every pause would spend the whole session behind. A constant rather than a
/// setting, because a setting nobody changes is a lie in the documentation.
/// The reader imports this rather than keeping a number of its own.
export const DEBOUNCE = 1500;

/// Which browser storage remembers the choice. The spec asks for the choice
/// to be kept for this browser rather than for this document, so the key
/// carries no slug.
const CHOSEN = "komodoc-latex";

let base = DEFAULT_BASE;
let manifest = null;
let worker = null;
let loaded = null; // a promise of the distribution being ready
let name = null;

let running = false;
let queued = null; // { tree, resolve, reject } -- at most one, always the latest

/// Points this module at a mirror. Called once, by whatever knows the
/// deployment's `--latex`; the tests call it with a local one.
export function at(url) {
  if (url && url !== base) {
    base = url.endsWith("/") ? url : url + "/";
    manifest = null;
    if (worker) worker.terminate();
    worker = null;
    loaded = null;
    name = null;
  }
  return base;
}

/// The mirror's manifest: every distribution, its files with their digests,
/// and the sizes the card needs. Fetched once and never again, because it is
/// the only file in the mirror whose URL carries no digest.
async function index() {
  if (manifest) return manifest;
  const response = await fetch(base + "manifest.json");
  if (!response.ok) throw new Error(`no LaTeX mirror at ${base} (${response.status})`);
  manifest = await response.json();
  return manifest;
}

/// The distributions this deployment offers, with the measured sizes and the
/// licence names, which is everything the card puts on a row. The sizes are
/// bytes, measured by `latex/tools/mirror.mjs`, not estimated here.
///
/// The list is what the manifest marks `shown`, and nothing here names a
/// distribution. The worker can drive more than the card offers, on purpose:
/// what a mirror carries and what an engine can actually compile are two
/// questions, and `latex/corpus/MEASUREMENTS.md` records where the answers
/// differ. A self-hoster who fixes a bundle turns one on by flipping that
/// flag and rebuilding the mirror, with no build of Komodoc involved -- which
/// only works while the decision lives in the manifest and not in this file.
export async function available() {
  const list = await index();
  return Object.entries(list.distributions)
    .filter(([, one]) => one.shown === true)
    .map(([id, one]) => ({
      name: id,
      label: one.label,
      engines: one.engines,
      bibliography: one.bibliography,
      licence: one.licence,
      trade: one.trade,
      upfront: one.upfront,
      // What a document may pull down later, over and above the up-front
      // cost.
      later: one.bundle_bytes || 0,
      // The two measurements the up-front number alone would mislead about.
      // Every distribution here defers most of its weight -- SwiftLaTeX
      // fetches the LaTeX format and each package from *inside* the first
      // compile, BusyTeX pulls its TeX Live down when a document arrives --
      // so a card that printed `upfront` on its own would be off by an order
      // of magnitude on the number a person actually waits for. `first` is a
      // first document on a cold cache and `next` a second one.
      first: one.measured?.first ?? 0,
      next: one.measured?.next ?? 0,
    }));
}

/// The distribution this browser chose, or null. Cheap and synchronous: the
/// card is drawn from it before anything is fetched.
export function chosen() {
  try {
    return localStorage.getItem(CHOSEN) || null;
  } catch {
    // A browser that refuses storage is a browser that will be asked again,
    // which is a worse experience and not a broken one.
    return null;
  }
}

/// Fetches, caches and loads a distribution, and remembers it for this
/// browser. The promise resolves when a compile could start.
///
/// `onProgress({ done, total })` is called with bytes as the up-front files
/// arrive, so the pane can show a bar over the wait rather than a spinner
/// over an unknown. Only the up-front files are counted, and honestly: they
/// are the ones this module fetches, and they are the ones whose sizes the
/// manifest knows. What a compile fetches afterwards is fetched by the engine
/// itself, over synchronous XHR from inside WebAssembly, where there is
/// nothing here to count -- which is exactly the weight the card warns about
/// in words instead.
export function choose(which, onProgress) {
  if (loaded && name === which) return loaded;
  if (worker) worker.terminate();
  name = which;
  try {
    localStorage.setItem(CHOSEN, which);
  } catch {
    /* the choice still holds for this session */
  }
  // Asked once, when a distribution is chosen, and a refusal is tolerated:
  // the distribution still works and may have to be fetched again some day,
  // which the card says.
  persist();

  loaded = index()
    .then(async (list) => {
      if (!list.distributions[which]) throw new Error(`no distribution named ${which}`);
      await warmUpFront(list.distributions[which], onProgress);
      return list;
    })
    .then(
    (list) =>
      new Promise((resolve, reject) => {
        // A module worker, so the glue and the log parser are ordinary
        // imports and the same files run under Vite and, unbundled, under
        // the headless check. Vite discovers this form and bundles the
        // worker on its own, which is why nothing in `vite.config.js`
        // changes for it.
        worker = new Worker(new URL("./latex/worker.js", import.meta.url), { type: "module" });
        worker.onmessage = (event) => {
          const message = event.data;
          if (message.ready) resolve(true);
          else if (message.failed) reject(new Error(message.failed));
        };
        worker.onerror = (event) => reject(new Error(event.message || "the compiler worker failed"));
        worker.postMessage({
          cmd: "choose",
          name: which,
          base: new URL(base, self.location.href).href,
          distribution: list.distributions[which],
        });
      }),
    );
  loaded.catch(() => {
    // A failed load must not poison the module: the card is shown again and
    // the reader may choose the same one or another.
    loaded = null;
    name = null;
  });
  return loaded;
}

/// Pulls the up-front files down before the engine is started, so that the
/// wait has a length a person can see.
///
/// The engine's own loader fetches these again the moment it starts, and gets
/// them for nothing: they are served with a digest in the path and
/// `immutable`, so the second fetch is answered out of the HTTP cache. Going
/// through `cached` as well puts them in Cache Storage, which is what
/// survives an eviction of the HTTP cache and is what `persist` was asked
/// about.
///
/// A file that will not fetch is not raised here. The engine is about to try
/// the same URL and will fail with a message about the thing it was actually
/// doing, which is a better error than this loop could write.
async function warmUpFront(distribution, onProgress) {
  const files = Object.values(distribution.files || {});
  const total = files.reduce((sum, one) => sum + (one.size || 0), 0);
  let done = 0;
  onProgress?.({ done, total });
  for (const file of files) {
    try {
      const response = await cached(new URL(file.url, new URL(base, self.location.href)).href);
      await response.arrayBuffer();
    } catch {
      /* the engine is about to ask for the same file and will say so */
    }
    done += file.size || 0;
    onProgress?.({ done, total });
  }
}

/// Compiles a tree and returns the PDF, the SyncTeX file, the raw log, the
/// diagnostics parsed from the log, and how long it took.
///
/// A document that does not compile is an ordinary state of an editor rather
/// than an error of this module's, so it resolves with `pdf: null` and a list
/// of diagnostics. It rejects only for a distribution that could not be
/// fetched or loaded.
///
/// A compile requested while one is running waits, and a request that arrives
/// while another is waiting replaces it: at most one runs and one is queued,
/// and the queued one is always the latest tree. A replaced request resolves
/// with the result of the one that replaced it, because the caller asked to
/// see the newest text and that is what it gets.
export function compile(tree) {
  if (!name) return Promise.reject(new Error("no LaTeX distribution has been chosen"));
  return new Promise((resolve, reject) => {
    if (queued) {
      // The one already waiting is stale before it ever ran. Its caller is
      // handed this compile's result rather than left holding a promise that
      // will never settle.
      const stale = queued;
      queued = { tree, waiting: [...stale.waiting, resolve], failing: [...stale.failing, reject] };
    } else {
      queued = { tree, waiting: [resolve], failing: [reject] };
    }
    pump();
  });
}

function pump() {
  if (running || !queued) return;
  const { tree, waiting, failing } = queued;
  queued = null;
  running = true;
  const started = performance.now();
  loaded
    .then(
      () =>
        new Promise((resolve, reject) => {
          worker.onmessage = (event) => {
            const message = event.data;
            if (message.compiled) resolve(message.compiled);
            else if (message.failed) reject(new Error(message.failed));
          };
          // The tree is copied into the worker rather than moved. Moving the
          // asset buffers would empty the session's own copy of them, and a
          // figure that vanished on the second compile would be a strange
          // bug to find; a structured clone of a document's assets is
          // bounded by `max_assets` and costs milliseconds.
          worker.postMessage({ cmd: "compile", tree });
        }),
    )
    .then((result) => {
      const done = { ...result, seconds: (performance.now() - started) / 1000 };
      for (const resolve of waiting) resolve(done);
    })
    .catch((error) => {
      for (const reject of failing) reject(error);
    })
    .finally(() => {
      running = false;
      pump();
    });
}
