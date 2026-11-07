// LaTeX language support: the Lezer grammar (latex.grammar, built into
// parser.js by tools/latex-grammar.mjs) plus highlighting, folding,
// indentation and a few editing helpers.
import {
  LRLanguage, LanguageSupport, foldNodeProp, indentNodeProp, delimitedIndent, syntaxTree,
} from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { styleTags, tags } from "@lezer/highlight";
import { parser } from "./parser.js";
import { latexFolding } from "../latex-folding.js";

const BEGIN_KEYWORDS = "BeginKw MathBeginKw VerbBeginKw ListingBeginKw EndKw";
const MATH_DELIMITERS = "DollarOpen DollarClose DisplayDollar ParenOpen ParenClose BracketOpen BracketClose StrayDollar";

const ENVIRONMENTS = "Environment MathEnvironment InnerEnvironment VerbatimEnvironment";

/** Fold an environment from the end of its \begin line to just before its \end. */
function foldEnvironment(node, state) {
  const end = node.lastChild;
  const begin = node.firstChild;
  if (!begin || end?.name !== "EndEnv") return null;
  const from = state.doc.lineAt(begin.to).to;
  if (from >= end.from) return null;
  return state.doc.lineAt(from).number < state.doc.lineAt(end.from).number
    ? { from, to: end.from } : null;
}

/** Fold the inside of a multi-line group. */
function foldGroup(node, state) {
  const close = node.lastChild;
  if (close?.name !== "CloseBrace" || close.from === node.from) return null;
  const from = node.from + 1;
  return state.doc.lineAt(from).number < state.doc.lineAt(close.from).number
    ? { from, to: close.from } : null;
}

/** Indent the body of an environment one unit; line up a closing \end with its \begin. */
function indentEnvironment(context) {
  const closing = /^\s*\\end\b/.test(context.textAfter);
  return context.baseIndent + (closing ? 0 : context.unit);
}

export const latexLanguage = LRLanguage.define({
  name: "latex",
  parser: parser.configure({
    props: [
      styleTags({
        Comment: tags.lineComment,
        "ControlSequence ControlSymbol": tags.tagName,
        SectionName: tags.heading,
        [BEGIN_KEYWORDS]: tags.keyword,
        EnvName: tags.className,
        [MATH_DELIMITERS]: tags.processingInstruction,
        // The delimiters and commands inside math override this colour.
        "InlineMath DisplayMath MathEnvironment InnerEnvironment": tags.special(tags.string),
        "OpenBrace CloseBrace": tags.brace,
        "OptOpen OptClose": tags.squareBracket,
        "Verb VerbBody": tags.monospace,
      }),
      foldNodeProp.add({
        [ENVIRONMENTS]: foldEnvironment,
        "Group MathGroup": foldGroup,
      }),
      indentNodeProp.add({
        Environment: indentEnvironment,
        MathEnvironment: indentEnvironment,
        InnerEnvironment: indentEnvironment,
        Group: delimitedIndent({ closing: "}" }),
        MathGroup: delimitedIndent({ closing: "}" }),
      }),
    ],
  }),
  languageData: {
    commentTokens: { line: "%" },
    closeBrackets: { brackets: ["(", "[", "{", "$"] },
  },
});

// Sections are not tree nodes (a \section runs to the next one, not to a
// closing token), so they keep folding through the line scanner.
export const latexSectionFolding = latexFolding;

const MATH_NODES = new Set(["InlineMath", "DisplayMath", "MathEnvironment"]);
const OPAQUE_NODES = new Set(["Comment", "Verb", "VerbBody"]);
const CLOSERS = new Set(["DollarClose", "DisplayDollar", "ParenClose", "BracketClose", "EndEnv"]);

/**
 * Whether pos is inside math: between the delimiters of $...$, \(...\),
 * $$...$$, \[...\] or a math environment, and not in a comment or \verb. The
 * edges of the delimiters count as outside; an unclosed formula runs to the end.
 * @param {import("@codemirror/state").EditorState} state
 * @param {number} pos
 */
export function inMath(state, pos) {
  for (let node = syntaxTree(state).resolveInner(pos, -1); node; node = node.parent) {
    if (OPAQUE_NODES.has(node.name)) return false;
    if (!MATH_NODES.has(node.name)) continue;
    const open = node.firstChild;
    const close = node.lastChild;
    if (!open || pos < open.to) return false;
    return !(close && close.from > open.from && CLOSERS.has(close.name) && pos > close.from);
  }
  return false;
}

/** Offset of the first unescaped % in a line, or -1. */
function commentStart(line) {
  let slashes = 0;
  for (let i = 0; i < line.length; i++) {
    const char = line[i];
    if (char === "%" && slashes % 2 === 0) return i;
    slashes = char === "\\" ? slashes + 1 : 0;
  }
  return -1;
}

/** Whether index in text falls after a comment marker on its line. */
function commented(text, index) {
  return commentStart(text.slice(text.lastIndexOf("\n", index - 1) + 1, index)) >= 0;
}

/** Visit the \begin{name} and \end{name} in text, outside comments. */
function eachEnvironmentTag(text, name, visit) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(`\\\\(begin|end)\\{${escaped}\\}`, "g");
  for (let match; (match = pattern.exec(text));) {
    if (!commented(text, match.index)) visit(match[1] === "begin");
  }
}

/** Whether the \begin{name} just typed already has a \end{name} for it. */
function hasEnd(before, after, name) {
  // Begins still open before the new one, from an enclosing environment of the same name.
  let open = 0;
  eachEnvironmentTag(before, name, (isBegin) => { open = isBegin ? open + 1 : Math.max(0, open - 1); });
  // Ends after the cursor that no begin after the cursor claims.
  let depth = 0;
  let spare = 0;
  eachEnvironmentTag(after, name, (isBegin) => {
    if (isBegin) depth++;
    else if (depth > 0) depth--;
    else spare++;
  });
  return spare > open;
}

// Typing the } of \begin{name} adds \end{name} on the next line, with the
// cursor staying after the }. This is a transaction filter so the \end goes in
// the same transaction as the typed character: one undo step, and the
// tracked-changes layer that wraps user input sees a single edit. It reacts
// only to transactions marked as typing (input.type), so remote, programmatic
// and pasted changes never trigger it. A } that closeBrackets steps over is a
// selection-only input.type transaction, so that case counts as typing too.
const autoCloseFilter = EditorState.transactionFilter.of((tr) => {
  if (!tr.isUserEvent("input.type")) return tr;
  const selection = tr.newSelection.main;
  const head = selection.head;
  if (!selection.empty || tr.newDoc.sliceString(head - 1, head) !== "}") return tr;

  let typed = false;
  if (tr.docChanged) {
    let count = 0;
    let match = false;
    tr.changes.iterChanges((fromA, toA, fromB, toB, inserted) => {
      count++;
      match = fromA === toA && toB === head && inserted.toString() === "}";
    });
    typed = count === 1 && match;
  } else {
    const before = tr.startState.selection.main;
    typed = before.empty && before.head === head - 1;
  }
  if (!typed) return tr;

  const doc = tr.newDoc;
  const line = doc.lineAt(head);
  // Only at the end of the line; text after the cursor means this is an edit, not a new environment.
  if (/\S/.test(doc.sliceString(head, line.to))) return tr;
  const begin = /\\begin\{([^{}\\%\s]+)\}$/.exec(doc.sliceString(line.from, head));
  if (!begin || begin.index > 0 && doc.sliceString(line.from + begin.index - 1, line.from + begin.index) === "\\") return tr;
  const prefix = doc.sliceString(line.from, line.from + begin.index);
  if (commentStart(prefix) >= 0) return tr;

  // The tree before the keystroke tells whether the cursor was in a comment or verbatim text.
  for (let node = syntaxTree(tr.startState).resolveInner(head - 1, -1); node; node = node.parent) {
    if (OPAQUE_NODES.has(node.name)) return tr;
  }

  const name = begin[1];
  if (hasEnd(doc.sliceString(0, line.from + begin.index), doc.sliceString(head), name)) return tr;
  const indent = /^\s*/.exec(line.text)[0];
  return [tr, {
    changes: { from: head, insert: `\n${indent}\\end{${name}}` },
    selection: { anchor: head },
    sequential: true,
  }];
});

export const autoCloseEnvironment = autoCloseFilter;

/** LaTeX language support: parser, highlighting, folding and \end completion. */
export function latex() {
  return new LanguageSupport(latexLanguage, [latexSectionFolding, autoCloseEnvironment]);
}
