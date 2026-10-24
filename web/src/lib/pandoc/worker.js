// Pandoc runs in its own worker so a conversion never blocks the editor, and
// so its 61 MB of memory goes away with the worker when a conversion traps.
//
// pandoc.wasm is the official Pandoc build, GPL-2.0-or-later. It is fetched as
// a separate program from the asset mirror, unmodified, and is not part of
// this bundle. The glue below follows Tweag's and John MacFarlane's
// pandoc.js (MIT): initialise the module once, then write the options as JSON
// into its memory for each conversion and read the results back out of files.
import { WASI, OpenFile, File, Directory, ConsoleStdout, PreopenDirectory } from "@bjorn3/browser_wasi_shim";
import { verifiedModuleBytes } from "../renderer-wasm.js";
import { OUTPUT_NAME, fillTree } from "./files.js";

const ARGS = ["pandoc.wasm", "+RTS", "-H64m", "-RTS"];

/// A ready Pandoc from the module's bytes (or a Response holding them).
/// `convert(options, files)` resolves { output, warnings, stderr } and throws
/// an Error with Pandoc's message when it wrote no output. A trap inside the
/// module surfaces as the engine's own error; the instance is not usable after.
export async function createPandoc(source) {
  const bytes = typeof source?.arrayBuffer === "function" ? await source.arrayBuffer() : source;
  const root = new Map();
  const wasi = new WASI(ARGS, [], [
    new OpenFile(new File(new Uint8Array(), { readonly: true })),
    ConsoleStdout.lineBuffered((message) => console.log(`[pandoc] ${message}`)),
    ConsoleStdout.lineBuffered((message) => console.warn(`[pandoc] ${message}`)),
    new PreopenDirectory("/", root),
  ], { debug: false });
  const { instance } = await WebAssembly.instantiate(bytes, { wasi_snapshot_preview1: wasi.wasiImport });
  const exports = instance.exports;
  wasi.initialize(instance);
  exports.__wasm_call_ctors();

  // Haskell's runtime reads argc and argv through pointers into the module's
  // memory. The memory can be replaced when it grows, so each view is taken
  // fresh.
  const view = () => new DataView(exports.memory.buffer);
  const argc = exports.malloc(4);
  view().setUint32(argc, ARGS.length, true);
  const argv = exports.malloc(4 * (ARGS.length + 1));
  const encoder = new TextEncoder();
  ARGS.forEach((arg, i) => {
    const bytes = encoder.encode(arg);
    const pointer = exports.malloc(bytes.length + 1);
    const target = new Uint8Array(exports.memory.buffer, pointer, bytes.length + 1);
    target.set(bytes);
    target[bytes.length] = 0;
    view().setUint32(argv + 4 * i, pointer, true);
  });
  view().setUint32(argv + 4 * ARGS.length, 0, true);
  const argvPointer = exports.malloc(4);
  view().setUint32(argvPointer, argv, true);
  exports.hs_init_with_rtsopts(argc, argvPointer);

  const decoder = new TextDecoder();
  return {
    async convert(options, files) {
      const output = new File(new Uint8Array(), { readonly: false });
      const stdout = new File(new Uint8Array(), { readonly: false });
      const stderr = new File(new Uint8Array(), { readonly: false });
      const warnings = new File(new Uint8Array(), { readonly: false });
      fillTree(root, files, (data) => new File(data, { readonly: true }), (contents) => new Directory(contents));
      root.set("stdin", new File(new Uint8Array(), { readonly: true }));
      root.set("stdout", stdout);
      root.set("stderr", stderr);
      root.set("warnings", warnings);
      root.set(OUTPUT_NAME, output);
      const request = encoder.encode(JSON.stringify({ ...options, "output-file": OUTPUT_NAME }));
      const pointer = exports.malloc(request.length);
      new Uint8Array(exports.memory.buffer, pointer, request.length).set(request);
      exports.convert(pointer, request.length);
      const message = decoder.decode(stderr.data);
      if (!output.data.length) throw new PandocFailure(message.trim() || "Pandoc failed");
      const raw = decoder.decode(warnings.data);
      let list = [];
      try {
        list = raw ? JSON.parse(raw) : [];
      } catch {
        list = [];
      }
      return { output: output.data.slice(), warnings: list, stderr: message };
    },
  };
}

/// Pandoc ran and reported an error, as opposed to the module failing. The
/// worker stays usable after this one.
export class PandocFailure extends Error {}

let ready = null;

async function convertOnce(url, options, files) {
  if (!ready) {
    ready = verifiedModuleBytes(url).then(createPandoc).catch((error) => {
      ready = null;
      throw error;
    });
  }
  return (await ready).convert(options, files);
}

if (typeof WorkerGlobalScope !== "undefined" && self instanceof WorkerGlobalScope) {
  self.onmessage = async ({ data: { id, url, options, files } }) => {
    try {
      const result = await convertOnce(url, options, files);
      self.postMessage({ id, result }, [result.output.buffer]);
    } catch (error) {
      // After a trap the module's memory is in an unknown state, so the client
      // is told to discard this worker.
      self.postMessage({ id, error: error?.message || String(error), fatal: !(error instanceof PandocFailure) });
    }
  };
}
