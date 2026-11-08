// What a Typst syntax tree needs to highlight, fold and indent: node props
// attached by node name, so they cover whatever kinds the wasm module lists
// and ignore the ones it lacks.
import { defineLanguageFacet, foldNodeProp, indentNodeProp, delimitedIndent, languageDataProp } from "@codemirror/language";
import { styleTags, tags } from "@lezer/highlight";

export const typstData = defineLanguageFacet({
  commentTokens: { line: "//", block: { open: "/*", close: "*/" } },
});

// Operators are only operators inside an expression: `*` and `_` in markup
// delimit strong and emphasis, and must keep that colour.
const OPERATORS = "Plus Minus Star Slash Hat Eq EqEq ExclEq Lt LtEq Gt GtEq PlusEq HyphEq StarEq SlashEq In Root";
const EXPRESSIONS = ["Binary", "Unary", "LetBinding", "DestructAssignment", "Named", "Keyed"];
const operatorSelectors = EXPRESSIONS.flatMap((parent) => OPERATORS.split(" ").map((kind) => `${parent}/${kind}`)).join(" ");

const highlighting = styleTags({
  "Heading HeadingMarker": tags.heading,
  Strong: tags.strong,
  Emph: tags.emphasis,
  "Raw RawLang RawDelim RawTrimmed": tags.monospace,
  Link: tags.url,
  Label: tags.labelName,
  "Ref RefMarker": tags.link,
  "Equation Math": tags.special(tags.string),
  Dollar: tags.processingInstruction,
  MathIdent: tags.variableName,
  "Escape Shorthand": tags.escape,
  Str: tags.string,
  "Int Float Numeric": tags.number,
  Bool: tags.bool,
  "None Auto": tags.null,
  Let: tags.definitionKeyword,
  "Set Show Context": tags.keyword,
  "If Else For While Break Continue Return": tags.controlKeyword,
  "In Not And Or As": tags.operatorKeyword,
  "Import Include": tags.moduleKeyword,
  Ident: tags.variableName,
  "FuncCall/Ident": tags.function(tags.variableName),
  [operatorSelectors]: tags.operator,
  "Dots Arrow": tags.operator,
  "Comma Semicolon Colon": tags.punctuation,
  "LeftBrace RightBrace": tags.brace,
  "LeftBracket RightBracket": tags.squareBracket,
  "LeftParen RightParen": tags.paren,
  "LineComment": tags.lineComment,
  "BlockComment": tags.blockComment,
  Hash: tags.meta,
  "ListMarker EnumMarker TermMarker": tags.list,
  Error: tags.invalid,
});

const multiline = (state, from, to) => from < to && state.doc.lineAt(from).number < state.doc.lineAt(to).number;

/** Fold what lies between the first child and the closing child named `close`. */
function foldBetween(close) {
  return (node, state) => {
    const open = node.firstChild;
    let end = node.lastChild;
    if (close) for (let child = node.firstChild; child; child = child.nextSibling) if (child.name === close) { end = child; break; }
    if (!open || !end || open.from === end.from) return null;
    return multiline(state, open.to, end.from) ? { from: open.to, to: end.from } : null;
  };
}

/** Fold a fenced block from the end of its first line to the closing fence. */
function foldRaw(node, state) {
  const last = node.lastChild;
  if (!last || last.name !== "RawDelim" || last.from === node.firstChild?.from) return null;
  const from = state.doc.lineAt(node.from).to;
  return multiline(state, from, last.from) ? { from, to: last.from } : null;
}

const folding = foldNodeProp.add({
  "CodeBlock ContentBlock Array Dict Params Equation": foldBetween(),
  Args: foldBetween("RightParen"),
  Raw: foldRaw,
});

const indentation = indentNodeProp.add({
  CodeBlock: delimitedIndent({ closing: "}" }),
  ContentBlock: delimitedIndent({ closing: "]" }),
  "Args Array Dict Params": delimitedIndent({ closing: ")" }),
});

/** The node props every Typst node type is defined with. */
export const nodeProps = [highlighting, folding, indentation, languageDataProp.add({ Document: typstData })];
