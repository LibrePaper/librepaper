// The compile, off the main thread.
//
// A LaTeX compile holds a thread for seconds and the editor must not freeze
// for it, so everything below runs in a Web Worker. This file is the worker's
// whole protocol -- two messages in, two out -- and it owns nothing else: the
// distribution's own way of being driven lives in `swiftlatex.js` and
// `busytex.js`, one file each, and the reading of the log lives in `log.js`.
//
//   in   { cmd: "choose",  name, base, distribution }
//   in   { cmd: "compile", tree }
//   out  { ready: true }
//   out  { compiled: { pdf, synctex, log, diagnostics } }
//   out  { failed: "..." }
//
// This is a module worker, so `log.js` and the glue are ordinary imports and
// the same files run bundled under Vite and unbundled under the headless
// check. The cost is `importScripts`, which a module worker does not have and
// which both distributions were written for; `busytex.js` says how it gets
// round that, and SwiftLaTeX's engine is a worker of its own anyway.

import { create as swiftlatex } from "./swiftlatex.js";
import { create as busytex } from "./busytex.js";
import { parse } from "./log.js";

const GLUE = {
  "swiftlatex-pdftex": swiftlatex,
  "swiftlatex-xetex": swiftlatex,
  busytex: busytex,
};

let engine = null;

self.onmessage = async (event) => {
  const message = event.data;
  try {
    if (message.cmd === "choose") {
      const glue = GLUE[message.name];
      if (!glue) throw new Error(`no glue for ${message.name}`);
      engine = await glue({
        name: message.name,
        base: message.base,
        distribution: message.distribution,
      });
      self.postMessage({ ready: true });
      return;
    }
    if (message.cmd === "compile") {
      if (!engine) throw new Error("no distribution has been loaded");
      const tree = message.tree;
      const { pdf, synctex, log } = await engine.compile(tree);
      // The log is read here rather than on the main thread because the
      // worker already has it and parsing it is the cheap end of a compile
      // that took seconds. The main thread gets the list, not the job.
      const diagnostics = parse(log || "", {
        main: tree.main,
        paths: [...Object.keys(tree.texts || {}), ...Object.keys(tree.assets || {})],
      });
      self.postMessage({ compiled: { pdf, synctex, log, diagnostics } });
      return;
    }
    throw new Error(`unknown command ${message.cmd}`);
  } catch (error) {
    self.postMessage({ failed: String(error?.message || error) });
  }
};
