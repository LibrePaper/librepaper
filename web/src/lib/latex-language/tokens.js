// External tokenizers for the LaTeX grammar.
//
// What a character means depends on where the parser is, so these ask the
// parser what it could shift (stack.canShift) instead of tracking state of
// their own. That makes them "contextual" in Lezer's terms. Anything read from
// the input through peek or advance is remembered as lookahead, so incremental
// reparsing stays correct.
import { ExternalTokenizer } from "@lezer/lr";
import {
  ControlSequence, ControlSymbol, SectionName, BeginKw, MathBeginKw, VerbBeginKw,
  ListingBeginKw, EndKw, Verb, ParenOpen, ParenClose, BracketOpen, BracketClose,
  DollarOpen, DollarClose, DisplayDollar, StrayDollar, OptOpen, OptClose, Text,
  VerbBody, OpenBrace, CloseBrace, ImplicitClose, UrlArg,
} from "./parser.terms.js";

const BACKSLASH = 92, PERCENT = 37, DOLLAR = 36, LBRACE = 123, RBRACE = 125;
const LBRACKET = 91, RBRACKET = 93, NEWLINE = 10, STAR = 42, SPACE = 32, TAB = 9;

const SECTIONS = new Set([
  "part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph",
]);
const MATH_ENVS = new Set([
  "equation", "align", "gather", "multline", "flalign", "alignat", "displaymath", "math", "eqnarray",
]);
// Environments whose body is opaque. The second set takes options first.
// Not `example`: verbatim in the R Journal style, but an ordinary theorem-like
// environment with math in most documents. An R Journal `$` stops at its \end.
export const VERBATIM_ENVS = new Set([
  "verbatim", "comment", "CodeInput", "CodeOutput", "Sinput", "Soutput", "Scode", "filecontents",
]);
export const LISTING_ENVS = new Set(["lstlisting", "minted", "Verbatim", "BVerbatim", "LVerbatim", "Highlighting"]);

const MATH_DELIMITERS = { 40: ParenOpen, 41: ParenClose, 91: BracketOpen, 93: BracketClose };

/** A letter or an at sign: the characters of a control word, as in \makeatletter names. */
function isLetter(ch) {
  return (ch >= 65 && ch <= 90) || (ch >= 97 && ch <= 122) || ch === 64;
}

/** True when the input is at a newline that starts a blank line (a paragraph break). */
function atParagraphBreak(input) {
  if (input.next !== NEWLINE) return false;
  let i = 1;
  for (let ch = input.peek(i); ch === SPACE || ch === TAB; ch = input.peek(++i));
  return input.peek(i) === NEWLINE;
}

/**
 * Whether the parser is inside $...$, \(...\), $$...$$ or \[...\] (not a
 * math environment or a group within one). Only there can a formula close.
 */
function inDelimitedMath(stack) {
  return stack.canShift(DollarClose) || stack.canShift(ParenClose) || stack.canShift(BracketClose) ||
    (stack.canShift(DisplayDollar) && stack.canShift(StrayDollar));
}

/** The environment name in `{name}` starting at offset, or null. */
function braceName(input, offset) {
  if (input.peek(offset) !== LBRACE) return null;
  let name = "";
  for (let i = offset + 1; i < offset + 60; i++) {
    const ch = input.peek(i);
    if (ch === RBRACE) return name;
    if (ch < 0 || ch === NEWLINE || ch === LBRACE || ch === BACKSLASH) return null;
    name += String.fromCharCode(ch);
  }
  return null;
}

/** The number of spaces or tabs at offset when a `{` follows them, else 0. */
function spacesToBrace(input, offset) {
  let gap = 0;
  while (input.peek(offset + gap) === SPACE || input.peek(offset + gap) === TAB) gap++;
  return gap && input.peek(offset + gap) === LBRACE ? gap : 0;
}

/** Pick the first token the parser can shift, or fall back. */
function firstShiftable(stack, candidates, fallback) {
  for (const term of candidates) if (stack.canShift(term)) return term;
  return fallback;
}

function controlToken(input, stack) {
  const next = input.peek(1);
  // A lone backslash at the end of the input.
  if (next < 0) return input.acceptToken(ControlSymbol, 1);

  if (!isLetter(next)) {
    // \( and \[ open math anywhere text is allowed; \) and \] only close.
    // Where they do not fit (a stray \) in text), they are plain control symbols.
    const delimiter = MATH_DELIMITERS[next];
    if (delimiter && stack.canShift(delimiter)) return input.acceptToken(delimiter, 2);
    // Keep a surrogate pair together.
    const width = next >= 0xd800 && next <= 0xdbff ? 3 : 2;
    return input.acceptToken(ControlSymbol, width);
  }

  let len = 2;
  while (isLetter(input.peek(len))) len++;
  const word = input.read(input.pos + 1, input.pos + len);

  // `\begin {name}` is valid too: the keyword token takes the spaces before the brace.
  const keyword = word === "begin" || word === "end" ? len + spacesToBrace(input, len) : len;
  if (word === "begin") {
    const name = braceName(input, keyword);
    let candidates = [BeginKw];
    if (name !== null) {
      const bare = name.trim().replace(/\*$/, "");
      if (MATH_ENVS.has(bare)) candidates = [MathBeginKw, BeginKw];
      else if (VERBATIM_ENVS.has(bare)) candidates = [VerbBeginKw, BeginKw];
      else if (LISTING_ENVS.has(bare)) candidates = [ListingBeginKw, BeginKw];
    }
    const term = firstShiftable(stack, candidates, ControlSequence);
    return input.acceptToken(term, term === ControlSequence ? len : keyword);
  }
  if (word === "end") {
    if (stack.canShift(EndKw)) return input.acceptToken(EndKw, keyword);
    // An \end that belongs to an outer environment closes whatever is open
    // here (unclosed inline math, say) without consuming it.
    if (stack.canShift(ImplicitClose)) return input.acceptToken(ImplicitClose, 0);
    return input.acceptToken(ControlSequence, len);
  }
  if (word === "verb") {
    // \verb<delim>...<delim>, or \verb*<delim>...<delim>, on one line.
    let i = len;
    if (input.peek(i) === STAR) i++;
    const delim = input.peek(i);
    if (delim >= 0 && delim !== NEWLINE && delim !== SPACE && delim !== TAB && delim !== STAR) {
      i++;
      while (input.peek(i) >= 0 && input.peek(i) !== delim && input.peek(i) !== NEWLINE) i++;
      if (input.peek(i) === delim) i++;
      return input.acceptToken(Verb, i);
    }
  }
  if (SECTIONS.has(word)) {
    // A heading cannot sit inside a formula: an unclosed one ends before it.
    if (inDelimitedMath(stack) && stack.canShift(ImplicitClose)) return input.acceptToken(ImplicitClose, 0);
    return input.acceptToken(SectionName, input.peek(len) === STAR ? len + 1 : len);
  }
  return input.acceptToken(ControlSequence, len);
}

function dollarToken(input, stack) {
  if (input.peek(1) === DOLLAR && stack.canShift(DisplayDollar)) {
    return input.acceptToken(DisplayDollar, 2);
  }
  // Inside $...$ a dollar closes; elsewhere in text it opens. Inside other
  // math (\[ ... \], $$ ... $$, a group in math) it is just a character.
  const term = firstShiftable(stack, [DollarClose, DollarOpen, StrayDollar], 0);
  if (term) input.acceptToken(term, 1);
}

/**
 * The braced argument of \url or \href, where % and # are literal. It is one
 * opaque token, so a % does not start a comment. The control word is read back
 * from the input because the tokenizer keeps no state.
 */
function urlToken(input, stack) {
  if (!stack.canShift(UrlArg)) return;
  const before = input.read(Math.max(0, input.pos - 5), input.pos);
  if (!before.endsWith("\\url") && !before.endsWith("\\href")) return;
  let depth = 0;
  for (let i = 0; ; i++) {
    const ch = input.peek(i);
    if (ch < 0 || ch === NEWLINE) return;
    if (ch === LBRACE) depth++;
    else if (ch === RBRACE && --depth === 0) return input.acceptToken(UrlArg, i + 1);
  }
}

function textToken(input, stack) {
  if (!stack.canShift(Text)) return;
  const closeBracket = stack.canShift(OptClose);
  // Inline math ends at a blank line, as it does in TeX, so one stray `$`
  // does not turn the rest of the document into math.
  // Display math ($$ and \[) ends there too, with a zero-length ImplicitClose.
  const inlineClose = stack.canShift(DollarClose) ? DollarClose
    : stack.canShift(ParenClose) ? ParenClose
      : inDelimitedMath(stack) && stack.canShift(ImplicitClose) ? ImplicitClose : 0;
  if (inlineClose && atParagraphBreak(input)) return input.acceptToken(inlineClose, 0);
  // An optional argument never spans a blank line either.
  if (closeBracket && atParagraphBreak(input)) return input.acceptToken(ImplicitClose, 0);

  let length = 0;
  for (let ch = input.next; ch >= 0; ch = input.next) {
    if (ch === BACKSLASH || ch === PERCENT || ch === DOLLAR || ch === LBRACE || ch === RBRACE) break;
    if (ch === RBRACKET && closeBracket) break;
    if ((inlineClose || closeBracket) && ch === NEWLINE && atParagraphBreak(input)) break;
    input.advance();
    length++;
  }
  if (length) input.acceptToken(Text);
}

/** Everything except verbatim bodies, comments and braces. */
export const latexTokens = new ExternalTokenizer((input, stack) => {
  const ch = input.next;
  // At the end of the input, close whatever is still open.
  if (ch < 0) return stack.canShift(ImplicitClose) ? input.acceptToken(ImplicitClose, 0) : undefined;
  if (ch === PERCENT) return; // Comment is a grammar token
  if (ch === LBRACE) return urlToken(input, stack); // otherwise OpenBrace is a grammar token
  if (ch === BACKSLASH) return controlToken(input, stack);
  if (ch === DOLLAR) return dollarToken(input, stack);
  if (ch === RBRACE) {
    if (stack.canShift(CloseBrace)) return;
    // This } belongs to an enclosing group, so whatever is open here (an
    // environment, math, an optional argument) was never closed: end it.
    if (stack.canShift(ImplicitClose)) return input.acceptToken(ImplicitClose, 0);
    // An unbalanced } is plain text.
    if (stack.canShift(Text)) {
      input.advance();
      input.acceptToken(Text);
    }
    return;
  }
  // An optional argument opens only directly after a command or \begin{...}.
  if (ch === LBRACKET && stack.canShift(OptOpen)) {
    input.advance();
    return input.acceptToken(OptOpen);
  }
  if (ch === RBRACKET && stack.canShift(OptClose)) {
    input.advance();
    return input.acceptToken(OptClose);
  }
  textToken(input, stack);
}, { contextual: true });

/** The name of the verbatim environment whose \begin{...} precedes the input position. */
function openEnvironment(input) {
  const text = input.read(Math.max(0, input.pos - 4000), input.pos);
  let found = null;
  for (const match of text.matchAll(/\\begin[ \t]*\{([^{}\n]*)\}/g)) found = match[1];
  return found;
}

/** True when the input continues with `marker`. */
function matches(input, marker) {
  for (let i = 0; i < marker.length; i++) if (input.peek(i) !== marker.charCodeAt(i)) return false;
  return true;
}

/** The body of a verbatim-like environment: everything up to its own \end{name}. */
export const verbatimBody = new ExternalTokenizer((input, stack) => {
  if (!stack.canShift(VerbBody)) return;
  // The options of lstlisting and the language group of minted come first.
  if (input.next === LBRACKET && stack.canShift(OptOpen)) return;
  if (input.next === LBRACE && stack.canShift(OpenBrace)) return;
  const name = openEnvironment(input);
  if (name === null) return;
  const marker = `\\end{${name}}`;
  let length = 0;
  while (input.next >= 0) {
    if (input.next === BACKSLASH && matches(input, marker)) break;
    input.advance();
    length++;
  }
  if (length) input.acceptToken(VerbBody);
}, { contextual: true });
