// The worker every TeX engine and biber starts in. It is a classic script,
// not a module, so that it can call `importScripts`.
//
// A worker loaded from this origin takes its content security policy from its
// own response, and the server sends none for `/assets/*.js`. The engine may
// therefore `eval` (LaTeXML's `emscripten_run_script_string` does) and load
// its verified blob scripts here, while the app page itself allows neither:
// a worker built from a blob URL would inherit the page's policy instead.
//
// The page posts one message first, `{ librepaperEngineHost: { source,
// imports, assets } }`. `source` is a blob URL of the verified engine worker
// script. `imports` maps a script name to the blob URL of its verified bytes,
// and `assets` does the same for everything the engine locates by name.
// Anything outside those maps is refused. The engine script is loaded last,
// into this global scope, where it installs its own message handler; this
// host never posts a message of its own.

/** @typedef {{importScripts: (...urls: string[]) => void, addEventListener: typeof self.addEventListener, __librepaperLocateFile?: (name: string) => string, Module?: Record<string, unknown> & {locateFile?: (name: string) => string}}} EngineHostScope */
/** @type {EngineHostScope} */
const engineScope = /** @type {unknown} */ (self);
const nativeImportScripts = engineScope.importScripts.bind(engineScope);

function lookup(map, value, what) {
  const key = String(value).split("/").pop();
  const resolved = map[value] || map[key];
  if (!resolved) throw new Error("Unverified LaTeX engine " + what + ": " + value);
  return resolved;
}

function setup({ data }) {
  const host = data && data.librepaperEngineHost;
  if (!host || typeof host.source !== "string") {
    throw new Error("LaTeX engine host needs a librepaperEngineHost message with a source");
  }
  const { source, imports, assets } = host;
  if (imports) {
    engineScope.importScripts = (...urls) => nativeImportScripts(...urls.map((value) => lookup(imports, value, "import")));
  }
  if (assets) {
    engineScope.__librepaperLocateFile = (name) => lookup(assets, name, "asset");
    engineScope.Module = { ...(engineScope.Module || {}), locateFile: engineScope.__librepaperLocateFile };
  }
  nativeImportScripts(source);
}

// Listeners added while an event is being dispatched are not called for it, so
// the engine's own handler only ever sees the messages that come after this one.
engineScope.addEventListener("message", setup, { once: true });
