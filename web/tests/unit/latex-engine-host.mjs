// The engine host is the only thing between a verified blob URL and the
// engine's `importScripts`: an import it was not told about must fail, a
// mapped one must reach the native loader as its blob URL, and the engine
// script itself must load last, once the maps are in place.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const code = readFileSync(new URL("../../src/lib/latex/engine-host.js", import.meta.url), "utf8");

function host() {
  const loaded = [];
  const listeners = [];
  const self = {
    importScripts: (...urls) => { loaded.push({ urls }); },
    addEventListener: (type, listener, options) => listeners.push({ type, listener, options }),
  };
  self.self = self;
  vm.runInNewContext(code, self);
  return { self, loaded, listeners };
}

{
  const { self, listeners } = host();
  assert.equal(listeners.length, 1);
  assert.equal(listeners[0].type, "message");
  assert.equal(listeners[0].options.once, true);
  for (const data of [undefined, {}, { librepaperEngineHost: {} }, { librepaperEngineHost: { source: 5 } }]) {
    assert.throws(() => listeners[0].listener({ data }), /engine host/);
  }
  assert.equal(typeof self.importScripts, "function");
}

{
  const { self, loaded, listeners } = host();
  const native = self.importScripts;
  listeners[0].listener({ data: { librepaperEngineHost: {
    source: "blob:app/engine",
    imports: { "dep.js": "blob:app/dep" },
    assets: { "engine.wasm": "blob:app/wasm" },
  } } });
  assert.equal(loaded.length, 1, "only the source loads during setup");
  assert.deepEqual([...loaded[0].urls], ["blob:app/engine"]);
  assert.notEqual(self.importScripts, native, "importScripts is wrapped");
  self.importScripts("https://mirror.test/x/dep.js");
  assert.deepEqual([...loaded[1].urls], ["blob:app/dep"], "a mapped import reaches the native loader as its blob URL");
  assert.throws(() => self.importScripts("evil.js"), /Unverified LaTeX engine import: evil\.js/);
  assert.equal(loaded.length, 2, "an unmapped import never reaches the native loader");
  assert.equal(self.Module.locateFile("dir/engine.wasm"), "blob:app/wasm");
  assert.equal(self.__librepaperLocateFile("engine.wasm"), "blob:app/wasm");
  assert.throws(() => self.__librepaperLocateFile("other.bin"), /Unverified LaTeX engine asset: other\.bin/);
}

{
  const { self, loaded, listeners } = host();
  const native = self.importScripts;
  listeners[0].listener({ data: { librepaperEngineHost: { source: "blob:app/biber" } } });
  assert.equal(self.importScripts, native, "no imports map leaves importScripts alone");
  assert.equal(self.Module, undefined, "no assets map leaves Module alone");
  assert.deepEqual(loaded.map((call) => [...call.urls]), [["blob:app/biber"]]);
}

console.log("latex-engine-host: unmapped imports fail, mapped ones load verified blobs, the source loads last");
