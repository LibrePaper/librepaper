// Typst's own parser, compiled alone to a small wasm module (typst-syntax.wasm),
// as a Lezer tree. The module speaks plain exports: strings go in as UTF-8
// written into its memory, and a tree comes out as a Lezer buffer of u32s in
// postorder, four per node: [kind, from, to, size]. Offsets are UTF-16.
//
// Usable from the main thread and from Node. Never throws: a module that
// cannot load is null, and the caller keeps what it had.
import { NodeSet, NodeType, Tree } from "@lezer/common";
import { nodeProps } from "./props.js";

const SOURCES = 16;
const MEMO = 16;

let loading = null;
let api = null;

function resolveUrl(url) {
  const base = globalThis.location?.href;
  try { return base ? new URL(url, base).href : String(url); } catch { return String(url); }
}

async function instantiate(source) {
  if (typeof source === "string") {
    const url = resolveUrl(source);
    if (typeof WebAssembly.instantiateStreaming === "function") {
      try { return (await WebAssembly.instantiateStreaming(fetch(url), {})).instance; } catch { /* wrong MIME type or no streaming: read the bytes */ }
    }
    const response = await fetch(url);
    if (!response.ok) throw new Error(`typst-syntax.wasm: ${response.status}`);
    return (await WebAssembly.instantiate(await response.arrayBuffer(), {})).instance;
  }
  const bytes = source instanceof Uint8Array ? source : new Uint8Array(source);
  return WebAssembly.instantiate(await WebAssembly.compile(bytes), {});
}

function readString(exports, ptr, len) {
  return new TextDecoder().decode(new Uint8Array(exports.memory.buffer, ptr, len));
}

/** The kind names, one per id, from the module. */
function kindNames(exports) {
  return readString(exports, exports.kinds(), exports.kinds_len()).split("\n");
}

/** The node types: one per kind, then the synthetic top `Document`. */
function nodeSetOf(names) {
  const types = names.map((name, id) => NodeType.define({ id, name, error: name === "Error", props: nodeProps }));
  types.push(NodeType.define({ id: names.length, name: "Document", top: true, props: nodeProps }));
  return new NodeSet(types);
}

/** The index where the second string stops matching the first, from the front and from the back. */
export function changedRange(before, after) {
  const shortest = Math.min(before.length, after.length);
  let prefix = 0;
  while (prefix < shortest && before.charCodeAt(prefix) === after.charCodeAt(prefix)) prefix++;
  // Never cut a surrogate pair in two.
  if (prefix > 0 && (before.charCodeAt(prefix - 1) & 0xfc00) === 0xd800) prefix--;
  let suffix = 0;
  while (suffix < shortest - prefix && before.charCodeAt(before.length - 1 - suffix) === after.charCodeAt(after.length - 1 - suffix)) suffix++;
  if (suffix > 0 && (before.charCodeAt(before.length - suffix) & 0xfc00) === 0xdc00) suffix--;
  return { from: prefix, to: before.length - suffix, insert: after.slice(prefix, after.length - suffix) };
}

function build(exports) {
  const names = kindNames(exports);
  const nodeSet = nodeSetOf(names);
  const topID = names.length;
  const encoder = new TextEncoder();

  /** Write a string into the module's memory, run `use(ptr, len)`, free it. */
  const withString = (text, use) => {
    const bytes = encoder.encode(text);
    const ptr = exports.alloc(bytes.length);
    try {
      new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
      return use(ptr, bytes.length);
    } finally {
      exports.dealloc(ptr, bytes.length);
    }
  };

  /** The tree of a source handle, copied out of the module's memory. */
  const read = (handle, length) => {
    const ptr = exports.source_tree(handle);
    // tree_len counts u32s; the root comes last and its size covers them all.
    const words = exports.tree_len();
    const memory = exports.memory.buffer;
    if (words < 4 || ptr + words * 4 > memory.byteLength || new Uint32Array(memory, ptr, words)[words - 1] !== words) {
      throw new Error("typst-syntax: bad tree");
    }
    // Tree.build wants a plain array (a typed array would be taken for a cursor).
    const buffer = Array.from(new Uint32Array(memory, ptr, words));
    return Tree.build({ buffer, nodeSet, topID, length });
  };

  /** Live sources, oldest first: key -> { handle, text }. */
  const sources = new Map();
  let keyless = null;
  const memo = new Map();
  let anonymous = 0;

  const free = (entry) => { try { exports.source_free(entry.handle); } catch { /* already gone */ } };

  const parse = (text, key) => {
    const slot = key === undefined ? keyless : key;
    let entry = slot === null ? null : sources.get(slot);
    if (entry) {
      sources.delete(slot);
      const { from, to, insert } = changedRange(entry.text, text);
      const ok = withString(insert, (ptr, len) => exports.source_edit(entry.handle, from, to, ptr, len));
      if (ok !== 1) { free(entry); entry = null; }
    }
    if (!entry) entry = { handle: withString(text, (ptr, len) => exports.source_new(ptr, len)), text: "" };
    entry.text = text;
    const name = slot ?? `\0keyless${++anonymous}`;
    if (key === undefined) keyless = name;
    sources.set(name, entry);
    while (sources.size > SOURCES) {
      const [oldest, evicted] = sources.entries().next().value;
      sources.delete(oldest);
      if (oldest === keyless) keyless = null;
      free(evicted);
    }
    return read(entry.handle, text.length);
  };

  return {
    nodeSet,
    /**
     * The syntax tree of `text`. The same text gives the same Tree. With a
     * `key`, the module keeps that document's parse and applies only what
     * changed since the last call for the key; without one, it reuses the
     * most recent keyless parse.
     * @param {string} text
     * @param {string} [key]
     * @returns {import("@lezer/common").Tree}
     */
    tree(text, key) {
      const known = memo.get(text);
      if (known) { memo.delete(text); memo.set(text, known); return known; }
      const tree = parse(text, key);
      memo.set(text, tree);
      if (memo.size > MEMO) memo.delete(memo.keys().next().value);
      return tree;
    },
  };
}

/**
 * Load the module once. `source` is a URL, bytes, or omitted for the page's
 * `LIBREPAPER_MODULES["typst-syntax"]`. Resolves to the API or null.
 * @param {string | ArrayBuffer | Uint8Array} [source]
 */
export function loadTypstSyntax(source) {
  if (api) return Promise.resolve(api);
  if (loading) return loading;
  const where = source ?? globalThis.LIBREPAPER_MODULES?.["typst-syntax"];
  if (!where) return Promise.resolve(null);
  const attempt = (async () => {
    try {
      const instance = await instantiate(where);
      api = build(instance.exports);
      return api;
    } catch {
      return null;
    }
  })();
  loading = attempt;
  attempt.then((result) => { if (!result && loading === attempt) loading = null; });
  return attempt;
}

/** The loaded API, or null while it is not. */
export function typstSyntax() {
  return api;
}
