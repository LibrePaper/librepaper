// Org's syntax tree from org.wasm, the module that also renders the Org preview
// (it exports a Lezer tree beside the renderer). Strings go in as UTF-8 written
// into its memory; a tree comes out as a Lezer buffer of u32s in postorder, four
// per node: [kind, from, to, size]. Offsets are UTF-16, relative to the parsed
// chunk. The vocabulary is the one documented at the top of parser.js.
//
// Like the JS parser it replaces, a parse splits the text at top-level
// headlines, takes unchanged chunks from the previous tree and has the module
// parse only the chunks that changed.
//
// Usable from the main thread and from Node. Never throws: a module that cannot
// load is null, and a parse that traps gives an empty tree.
import { NodeSet, NodeType, Tree } from "@lezer/common";
import { nodeProps } from "./props.js";

const MEMO = 16;
const TODO_LINE = /^#\+(?:SEQ_|TYP_)?TODO:(.*)$/i;
const HEAD_AT = /\*+(?=[ \t])/y;

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
    if (!response.ok) throw new Error(`org.wasm: ${response.status}`);
    return (await WebAssembly.instantiate(await response.arrayBuffer(), {})).instance;
  }
  const bytes = source instanceof Uint8Array ? source : new Uint8Array(source);
  return WebAssembly.instantiate(await WebAssembly.compile(bytes), {});
}

/** The words #+TODO / #+SEQ_TODO / #+TYP_TODO lines declare, plus TODO and DONE. */
function todoWords(text) {
  const words = new Set(["TODO", "DONE"]);
  const see = (from) => {
    let to = text.indexOf("\n", from);
    if (to < 0) to = text.length;
    if (to > from && text.charCodeAt(to - 1) === 13) to--;
    const m = TODO_LINE.exec(text.slice(from, to));
    if (!m) return;
    for (const word of m[1].split(/\s+/)) {
      const name = word.replace(/\(.*$/, "");
      if (name && name !== "|") words.add(name);
    }
  };
  if (text.charCodeAt(0) === 35) see(0);
  for (let i = text.indexOf("\n#"); i >= 0; i = text.indexOf("\n#", i + 1)) see(i + 1);
  return words;
}

/** Where the document's top-level sections start: headlines no deeper than the first one of the section they would nest in. */
function sectionStarts(text) {
  const starts = [];
  let base = 0;
  const see = (at) => {
    HEAD_AT.lastIndex = at;
    const m = HEAD_AT.exec(text);
    if (m && (!starts.length || m[0].length <= base)) {
      starts.push(at);
      base = m[0].length;
    }
  };
  if (text.charCodeAt(0) === 42) see(0);
  for (let i = text.indexOf("\n*"); i >= 0; i = text.indexOf("\n*", i + 1)) see(i + 1);
  return starts;
}

const sameSet = (a, b) => a.size === b.size && [...a].every((w) => b.has(w));

/**
 * What a tree remembers so a later parse can reuse its pieces: the TODO words it
 * was parsed with and its chunks (the text before the first headline, then each
 * top-level section), each with the span it was parsed from and the children it
 * contributed to the Document, positioned relative to the span start.
 * @typedef {{ from: number, to: number, children: readonly (Tree | import("@lezer/common").TreeBuffer)[], positions: readonly number[] }} Chunk
 */

function build(exports) {
  // lezer_names_len fills the name table; the pointer is read after it.
  const namesLen = exports.lezer_names_len();
  const names = new TextDecoder().decode(new Uint8Array(exports.memory.buffer, exports.lezer_names_ptr(), namesLen)).split("\n");
  const types = names.map((name, id) => NodeType.define({ id, name, error: name === "Error", props: nodeProps }));
  const topID = names.length;
  types.push(NodeType.define({ id: topID, name: "Document", top: true, props: nodeProps }));
  const nodeSet = new NodeSet(types);
  const encoder = new TextEncoder();

  /** @type {WeakMap<Tree, { todo: Set<string>, chunks: Chunk[] }>} */
  const memory = new WeakMap();
  const memo = new Map();

  /** Write a string into the module's memory, run `use(ptr, len)`, free it. */
  const withString = (text, use) => {
    const bytes = encoder.encode(text);
    // A zero-length string still needs a valid pointer.
    const size = Math.max(bytes.length, 1);
    const ptr = exports.alloc(size);
    try {
      new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
      return use(ptr, bytes.length);
    } finally {
      exports.dealloc(ptr, size);
    }
  };

  /** The tree of one chunk, copied out of the module's memory. */
  const parseChunk = (chunk, todo) => withString(chunk, (cp, cl) => withString(todo, (tp, tl) => {
    const words = exports.lezer_parse_chunk(cp, cl, tp, tl);
    const ptr = exports.lezer_tree_ptr();
    const memoryBuffer = exports.memory.buffer;
    if (ptr + words * 4 > memoryBuffer.byteLength) throw new Error("org: bad tree");
    // Tree.build wants a plain array (a typed array would be taken for a cursor).
    const buffer = Array.from(new Uint32Array(memoryBuffer, ptr, words));
    return Tree.build({ buffer, nodeSet, topID, length: chunk.length });
  }));

  const parse = (text, fragments) => {
    const todo = todoWords(text);
    const todoText = [...todo].join(" ");
    const starts = sectionStarts(text);
    /** @type {Array<[number, number]>} */
    const spans = [];
    if (!starts.length) spans.push([0, text.length]);
    else {
      if (starts[0] > 0) spans.push([0, starts[0]]);
      starts.forEach((from, k) => spans.push([from, k + 1 < starts.length ? starts[k + 1] : text.length]));
    }
    const usable = new Map();
    const old = (fragment) => {
      let meta = usable.get(fragment.tree);
      if (meta === undefined) {
        meta = memory.get(fragment.tree);
        if (meta && !sameSet(meta.todo, todo)) meta = null;
        usable.set(fragment.tree, meta ?? null);
      }
      return meta;
    };
    const children = [];
    const positions = [];
    /** @type {Chunk[]} */
    const chunks = [];
    let fi = 0;
    for (const [from, to] of spans) {
      let chunk = null;
      while (fi < fragments.length && fragments[fi].to < to) fi++;
      const fragment = fragments[fi];
      const meta = fragment && fragment.from <= from ? old(fragment) : null;
      if (meta) {
        const start = from + fragment.offset;
        let lo = 0;
        let hi = meta.chunks.length - 1;
        while (lo < hi) {
          const mid = (lo + hi) >> 1;
          if (meta.chunks[mid].from < start) lo = mid + 1;
          else hi = mid;
        }
        const hit = meta.chunks[lo];
        if (hit && hit.from === start && hit.to === to + fragment.offset) {
          chunk = { from, to, children: hit.children, positions: hit.positions };
        }
      }
      if (!chunk) {
        // A chunk ends just before a headline line, so its last character is a line break.
        const part = parseChunk(to < text.length ? text.slice(from, to - 1) : text.slice(from, to), todoText);
        chunk = { from, to, children: part.children, positions: part.positions };
      }
      chunks.push(chunk);
      chunk.children.forEach((child, i) => {
        children.push(child);
        positions.push(from + chunk.positions[i]);
      });
    }
    const tree = new Tree(nodeSet.types[topID], children, positions, text.length);
    memory.set(tree, { todo, chunks });
    return tree;
  };

  /** A tree with no children, for a text the module could not parse. */
  const emptyTree = (length) => new Tree(nodeSet.types[topID], [], [], length);

  return {
    nodeSet,
    /**
     * The syntax tree of `text`. Sections inside `fragments` (from a previous
     * tree of this module) are reused instead of parsed again. The same text
     * without fragments gives the same Tree.
     * @param {string} text
     * @param {readonly import("@lezer/common").TreeFragment[]} [fragments]
     * @returns {Tree}
     */
    parse(text, fragments = []) {
      if (!fragments.length) {
        const known = memo.get(text);
        if (known) { memo.delete(text); memo.set(text, known); return known; }
      }
      let tree;
      try {
        tree = parse(text, fragments);
      } catch {
        // Not remembered, so a later call tries the module again.
        return emptyTree(text.length);
      }
      memo.set(text, tree);
      if (memo.size > MEMO) memo.delete(memo.keys().next().value);
      return tree;
    },
  };
}

/**
 * Load the module once. `source` is a URL, bytes, or omitted for the page's
 * `LIBREPAPER_MODULES.org`. Resolves to the API or null.
 * @param {string | ArrayBuffer | Uint8Array} [source]
 */
export function loadOrgSyntax(source) {
  if (api) return Promise.resolve(api);
  if (loading) return loading;
  const where = source ?? globalThis.LIBREPAPER_MODULES?.org;
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
export function orgSyntax() {
  return api;
}
