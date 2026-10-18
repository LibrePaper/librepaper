const FIELD = /#\{\d+:[^{}]*\}/g;

// CodeMirror's snippet parser reads the field markers, then removes one
// backslash before each brace on the line. A LaTeX brace, written \{ or \},
// would lose its backslash, so it gets a second one here. Bare braces are
// left alone: they survive the parser as they are, and escaping them too
// would put a field after a run of escapes one character off, because the
// parser compares positions it has already moved with offsets it has not.
function escapeBraces(text) {
  return text.replace(/\\([{}])/g, (_, brace) => `\\\\${brace}`);
}

// Turn generated text with #{n:name} fields into a template for snippet().
// The markers are copied as they are; only the text between them is escaped.
export function snippetTemplate(text) {
  let out = "";
  let last = 0;
  for (const match of text.matchAll(FIELD)) {
    out += escapeBraces(text.slice(last, match.index)) + match[0];
    last = match.index + match[0].length;
  }
  return out + escapeBraces(text.slice(last));
}
