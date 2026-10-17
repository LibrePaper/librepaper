// `completion/typst.js` with an injected request in place of the renderer
// worker. The offset helpers, the snippet conversion and the kind mapping are
// pure; the source is run against a real EditorState and CompletionContext.
import assert from "node:assert/strict";
import { CompletionContext } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { convertSnippet, typstCompletion, utf16ToUtf8, utf8ToUtf16 } from "../../src/lib/completion/typst.js";

// --- Offsets ---------------------------------------------------------------
{
  const ascii = "abc";
  assert.equal(utf16ToUtf8(ascii, 2), 2);
  assert.equal(utf8ToUtf16(ascii, 2), 2);

  // "e" with an acute accent is two bytes in UTF-8.
  const accented = "café x";
  assert.equal(utf16ToUtf8(accented, 4), 5);
  assert.equal(utf8ToUtf16(accented, 5), 4);
  assert.equal(utf8ToUtf16(accented, 6), 5);

  // A CJK character is one UTF-16 unit and three UTF-8 bytes.
  const cjk = "中文 y";
  assert.equal(utf16ToUtf8(cjk, 2), 6);
  assert.equal(utf8ToUtf16(cjk, 6), 2);
  assert.equal(utf8ToUtf16(cjk, 7), 3);

  // An emoji is a surrogate pair: two UTF-16 units, four UTF-8 bytes.
  const emoji = "a\u{1F600}b";
  assert.equal(utf16ToUtf8(emoji, 3), 5);
  assert.equal(utf16ToUtf8(emoji, 1), 1);
  assert.equal(utf8ToUtf16(emoji, 5), 3);
  assert.equal(utf8ToUtf16(emoji, 1), 1);
  // Mid-character offsets round down to the character start.
  assert.equal(utf8ToUtf16(emoji, 3), 1);
  assert.equal(utf8ToUtf16(emoji, 99), emoji.length);

  for (const text of [ascii, accented, cjk, emoji]) {
    for (let index = 0; index <= text.length; index++) {
      if (text.charCodeAt(index - 1) >= 0xd800 && text.charCodeAt(index - 1) < 0xdc00) continue;
      assert.equal(utf8ToUtf16(text, utf16ToUtf8(text, index)), index);
    }
  }
}

// --- Snippets --------------------------------------------------------------
{
  assert.equal(convertSnippet("#box(${})"), "#box(${})");
  assert.equal(convertSnippet("#let ${name} = ${}"), "#let ${name} = ${}");
  // Braces that are not placeholders are escaped, so CodeMirror does not
  // read `#{` as a field opener.
  assert.equal(convertSnippet("#{ ${x} }"), "#\\{ ${x} \\}");
  assert.equal(convertSnippet("#let f(x) = { ${} }"), "#let f(x) = \\{ ${} \\}");
  assert.equal(convertSnippet("plain text"), "plain text");
}

// --- Source ----------------------------------------------------------------
const project = (overrides = {}) => ({
  path: "chapters/main.typ",
  format: "typst",
  texts: { "chapters/main.typ": "stale", "notes.typ": "= Notes" },
  files: ["chapters/main.typ", "notes.typ", "logo.png"],
  ...overrides,
});

function contextFor(doc, pos, explicit = false) {
  const state = EditorState.create({ doc });
  return new CompletionContext(state, pos, explicit);
}

// A fake engine reply: the `from` is a byte offset that a UTF-16 index must
// be recovered from.
function replyWith(reply) {
  const calls = [];
  const request = async (args) => {
    calls.push(args);
    return typeof reply === "function" ? reply(args) : reply;
  };
  return { request, calls };
}

{
  // Cursor after "#" in a document with accented and CJK text before it.
  const doc = "café 中 #";
  const { request, calls } = replyWith({
    from: 10,
    options: [
      { label: "box", kind: "func", detail: "Box", apply: "#box(${})" },
      { label: "text", kind: "func", detail: null, apply: null },
    ],
  });
  const source = typstCompletion(() => project(), request);
  const result = await source(contextFor(doc, doc.length));

  assert.equal(calls.length, 1);
  const [args] = calls;
  assert.equal(args.path, "chapters/main.typ");
  assert.equal(args.source, doc);
  assert.equal(args.explicit, false);
  assert.equal(args.offset, utf16ToUtf8(doc, doc.length));
  assert.equal(args.tree.main, "chapters/main.typ");
  assert.equal(args.tree.texts["chapters/main.typ"], doc);
  assert.equal(args.tree.texts["notes.typ"], "= Notes");
  assert.deepEqual(Object.keys(args.tree.files), ["chapters/main.typ", "notes.typ", "logo.png"]);

  // The "#" starts at byte 10, which is UTF-16 index 7.
  assert.equal(result.from, 7);
  assert.equal(result.filter, true);
  assert.deepEqual(result.validFor, /^[\w-]*$/);
  assert.equal(result.options[0].label, "box");
  assert.equal(result.options[0].type, "function");
  assert.equal(result.options[0].detail, "Box");
  assert.equal(typeof result.options[0].apply, "function");
  assert.equal(result.options[1].apply, undefined);
}

{
  // Plain string apply stays a string; the kind maps to its CodeMirror type.
  const { request } = replyWith({
    from: 1,
    options: [{ label: "red", kind: "constant", detail: "Color", apply: "red" }],
  });
  const source = typstCompletion(() => project(), request);
  const doc = "#r";
  const result = await source(contextFor(doc, doc.length));
  assert.equal(result.from, 1);
  assert.equal(result.options[0].apply, "red");
  assert.equal(result.options[0].type, "constant");
}

{
  // The engine's reply is an offset into the live document, not the stale
  // project text: `from` is converted against the document the request sent.
  const doc = "\u{1F600} #p";
  const { request } = replyWith({ from: 6, options: [{ label: "pagebreak", kind: "func", apply: null }] });
  const source = typstCompletion(() => project(), request);
  const result = await source(contextFor(doc, doc.length));
  assert.equal(result.from, 4);
}

{
  // Kinds map to CodeMirror's types.
  const kinds = {
    func: "function", type: "type", param: "property", constant: "constant", path: "file",
    package: "namespace", label: "variable", font: "text", symbol: "text", syntax: "keyword",
  };
  const options = Object.keys(kinds).map((kind) => ({ label: kind, kind, apply: null }));
  const { request } = replyWith({ from: 0, options });
  const source = typstCompletion(() => project(), request);
  const result = await source(contextFor("#", 1, true));
  for (const option of result.options) assert.equal(option.type, kinds[option.label]);
}

{
  // Explicit completion after a letter reaches the engine.
  const { request, calls } = replyWith({ from: 0, options: [{ label: "x", kind: "func", apply: null }] });
  const source = typstCompletion(() => project(), request);
  await source(contextFor("ab", 2, true));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].explicit, true);
}

{
  // Format other than typst: no request, and the accessor's texts are not read.
  let texts = 0;
  const { request, calls } = replyWith({ from: 0, options: [{ label: "x", kind: "func", apply: null }] });
  const source = typstCompletion(() => ({
    path: "main.tex",
    format: "latex",
    get texts() { texts++; return {}; },
    get files() { texts++; return []; },
  }), request);
  assert.equal(await source(contextFor("\\", 1, true)), null);
  assert.equal(calls.length, 0);
  assert.equal(texts, 0);
}

{
  // Typing a space never reaches the engine, even when the request is explicit.
  const { request, calls } = replyWith({ from: 0, options: [{ label: "x", kind: "func", apply: null }] });
  const source = typstCompletion(() => project(), request);
  assert.equal(await source(contextFor("#let x ", 7)), null);
  assert.equal(await source(contextFor("#let x ", 7, true)), null);
  assert.equal(calls.length, 0);
}

{
  // Plain prose typing does not call the engine.
  const { request, calls } = replyWith({ from: 0, options: [{ label: "x", kind: "func", apply: null }] });
  const source = typstCompletion(() => project(), request);
  assert.equal(await source(contextFor("!", 1)), null);
  assert.equal(calls.length, 0);
}

{
  // Request errors and empty replies yield no completions.
  const failing = typstCompletion(() => project(), async () => { throw new Error("worker failed"); });
  assert.equal(await failing(contextFor("#b", 2)), null);

  const empty = typstCompletion(() => project(), async () => ({ from: 0, options: [] }));
  assert.equal(await empty(contextFor("#b", 2)), null);

  const none = typstCompletion(() => project(), async () => null);
  assert.equal(await none(contextFor("#b", 2)), null);
}

{
  // A reply that arrives after the completion was aborted is dropped.
  const context = contextFor("#b", 2);
  const source = typstCompletion(() => project(), async () => {
    // `aborted` is a getter on CompletionContext; an own property shadows it.
    Object.defineProperty(context, "aborted", { get: () => true });
    return { from: 0, options: [{ label: "box", kind: "func", apply: null }] };
  });
  assert.equal(await source(context), null);
}
