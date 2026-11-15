// Typst completion. The engine parses the edited file and answers what can be
// typed at the cursor; this module moves the request across the renderer
// worker and maps the answer onto CodeMirror.
//
// Positions cross the boundary as UTF-8 byte offsets (the engine's native
// unit) and come back the same way. CodeMirror counts UTF-16 units, so every
// offset is converted at this edge and nowhere else.

import { snippetCompletion } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import { rendererRequest } from "../renderer-client.js";
import { explicitlyAsked } from "./asked.js";

// The character before the cursor that starts a typst completion without an
// explicit request: markup and code sigils, a path or a label.
const TRIGGER = /[#.\w@<"/(,:]/;

/**
 * Where the Typst source can ask the engine. It reads the character before the
 * cursor as the source does. Past that only plain markup text is ruled out:
 * a word, or punctuation inside a word, whose node is a Text directly in a
 * Markup node. Code, math, arguments, strings, labels, references and raw text
 * all keep the wake. Without the Typst parse (the stream-mode fallback while
 * typst-syntax.wasm loads) no node is a Text in a Markup, so every trigger
 * character wakes, as the source itself would.
 * @param {string} before the line's text up to the cursor
 * @param {import("@codemirror/state").EditorState} state
 * @param {number} pos
 */
export function typstTrigger(before, state, pos) {
  const last = before.slice(-1);
  if (!last || /\s/.test(last) || !TRIGGER.test(last)) return false;
  if ("#@<\"".includes(last)) return true;
  const node = syntaxTree(state).resolveInner(pos, -1);
  return !(node.name === "Text" && node.parent?.name === "Markup");
}

// The engine's kind to CodeMirror's completion type.
const TYPES = {
  func: "function",
  type: "type",
  param: "property",
  constant: "constant",
  path: "file",
  package: "namespace",
  label: "variable",
  font: "text",
  symbol: "text",
  syntax: "keyword",
};

/** Number of UTF-8 bytes in the first `index` UTF-16 units of `text`. */
export function utf16ToUtf8(text, index) {
  let bytes = 0;
  for (let i = 0; i < index && i < text.length; ) {
    const code = text.codePointAt(i);
    bytes += utf8Length(code);
    i += code > 0xffff ? 2 : 1;
  }
  return bytes;
}

/**
 * The UTF-16 index of a UTF-8 byte offset. An offset inside a character
 * rounds down to that character's start.
 */
export function utf8ToUtf16(text, byteOffset) {
  let bytes = 0;
  for (let i = 0; i < text.length; ) {
    const code = text.codePointAt(i);
    const size = utf8Length(code);
    if (bytes + size > byteOffset) return i;
    bytes += size;
    i += code > 0xffff ? 2 : 1;
  }
  return text.length;
}

function utf8Length(code) {
  if (code < 0x80) return 1;
  if (code < 0x800) return 2;
  if (code < 0x10000) return 3;
  return 4;
}

/**
 * Converts a typst-ide snippet (`${}` and `${name}` placeholders) to
 * CodeMirror's snippet syntax. Both accept the same placeholders, but
 * CodeMirror also reads a bare `#{` or `{` in the text as a placeholder
 * opener, so every brace outside a placeholder is escaped.
 */
export function convertSnippet(apply) {
  let out = "";
  let last = 0;
  for (const match of apply.matchAll(/\$\{([^{}]*)\}/g)) {
    out += escapeBraces(apply.slice(last, match.index));
    out += match[0];
    last = match.index + match[0].length;
  }
  return out + escapeBraces(apply.slice(last));
}

function escapeBraces(text) {
  return text.replace(/[{}]/g, (brace) => `\\${brace}`);
}

/**
 * The default request: the typst module the renderer worker loads, asked for
 * completions. Resolves to null when the build has no typst module.
 */
export function defaultRequest(args) {
  const modules = globalThis.LIBREPAPER_MODULES || {};
  const url = modules.typst;
  if (!url) return Promise.resolve(null);
  return rendererRequest(new URL(url, globalThis.location.href).href, "complete", args);
}

function option(item) {
  const completion = { label: item.label, detail: item.detail, type: TYPES[item.kind] };
  if (typeof item.apply !== "string") return completion;
  if (item.apply.includes("${")) return snippetCompletion(convertSnippet(item.apply), completion);
  return { ...completion, apply: item.apply };
}

/**
 * A completion source for typst files. `project` is the shared accessor (see
 * completion/index.js); `request` sends the engine a request and resolves to
 * its reply, or null.
 */
export function typstCompletion(project, request = defaultRequest) {
  return async (context) => {
    // The cursor's neighbour is read from the document, which costs nothing,
    // before the project is touched.
    const before = context.state.sliceDoc(Math.max(0, context.pos - 1), context.pos);
    if (/\s/.test(before)) return null;
    if (!explicitlyAsked(context) && !TRIGGER.test(before)) return null;
    const current = project();
    if (current.format !== "typst") return null;

    // The edited file's text comes from the live document, not the project's
    // copy, which may be stale. Its binary assets cannot be sent to the
    // worker; they are listed in `files` so path completion can see them.
    const doc = context.state.doc.toString();
    const tree = {
      main: current.path,
      texts: { ...current.texts, [current.path]: doc },
      files: Object.fromEntries(current.files.map((path) => [path, {}])),
    };
    const args = {
      tree,
      path: current.path,
      source: doc,
      offset: utf16ToUtf8(doc, context.pos),
      explicit: explicitlyAsked(context),
    };

    let reply;
    try {
      reply = await request(args);
    } catch {
      return null;
    }
    // The document may have moved on while the engine worked.
    if (context.aborted || !reply || !Array.isArray(reply.options) || !reply.options.length) return null;
    return {
      from: utf8ToUtf16(doc, reply.from),
      options: reply.options.map(option),
      filter: true,
      validFor: /^[\w-]*$/,
    };
  };
}
