// Loading a `.svelte.js` module in plain Node.
//
// A module that owns its own state writes `$state`, which is Svelte syntax
// rather than JavaScript: the compiler turns it into the signal calls that
// make a read reactive. Node cannot import such a file directly, so this
// compiles it the way vite would and imports the result.
//
// It exists so that owning state costs a module nothing in testability. A
// controller that holds its own `$state` is loaded here in one line and
// driven like any other object; without it, the alternative is the older
// shape where the component keeps the state and hands the module getters for
// it, which is what made Reader.svelte need a VM harness in the first place.

import { compileModule } from "svelte/compiler";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/// Compile and import the `.svelte.js` module at `url`. Its imports are
/// rewritten to absolute URLs, since the compiled copy is written to a
/// temporary directory and its relative specifiers would resolve from there.
export async function loadRunes(url) {
  const dir = mkdtempSync(join(tmpdir(), "runes-module-"));
  try {
    const compiled = compileModule(readFileSync(url, "utf8"), { filename: url.pathname, generate: "client" });
    const code = compiled.js.code.replace(
      /from (["'])([^"']+)\1/g,
      (_, quote, specifier) => `from ${JSON.stringify(specifier.startsWith(".") ? new URL(specifier, url).href : import.meta.resolve(specifier))}`,
    );
    const output = join(dir, "module.mjs");
    writeFileSync(output, code);
    return await import(pathToFileURL(output).href);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
