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
  VerbBody, OpenBrace, CloseBrace,
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
const VERBATIM_ENVS = new Set(["verbatim", "comment"]);
const LISTING_ENVS = new Set(["lstlisting", "minted", "Verbatim", "BVerbatim", "LVerbatim"]);

const MATH_DELIMITERS = { 40: ParenOpen, 41: ParenClose, 91: BracketOpen, 93: BracketClose };

/** A letter or @, the characters of a control word (\makeatletter style names). */
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

  if (word === "begin") {
    const name = braceName(input, len);
    let candidates = [BeginKw];
    if (name !== null) {
      const bare = name.trim().replace(/\*$/, "");
      if (MATH_ENVS.has(bare)) candidates = [MathBeginKw, BeginKw];
      else if (VERBATIM_ENVS.has(bare)) candidates = [VerbBeginKw, BeginKw];
      else if (LISTING_ENVS.has(bare)) candidates = [ListingBeginKw, BeginKw];
    }
    return input.acceptToken(firstShiftable(stack, candidates, ControlSequence), len);
  }
  if (word === "end") {
    return input.acceptToken(stack.canShift(EndKw) ? EndKw : ControlSequence, len);
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

function textToken(input, stack) {
  if (!stack.canShift(Text)) return;
  const closeBracket = stack.canShift(OptClose);
  // Inline math ends at a blank line, as it does in TeX, so one stray `$`
  // does not turn the rest of the document into math.
  const inlineClose = stack.canShift(DollarClose) ? DollarClose
    : stack.canShift(ParenClose) ? ParenClose : 0;
  if (inlineClose && atParagraphBreak(input)) return input.acceptToken(inlineClose, 0);

  let length = 0;
  for (let ch = input.next; ch >= 0; ch = input.next) {
    if (ch === BACKSLASH || ch === PERCENT || ch === DOLLAR || ch === LBRACE || ch === RBRACE) break;
    if (ch === RBRACKET && closeBracket) break;
    if (inlineClose && ch === NEWLINE && atParagraphBreak(input)) break;
    input.advance();
    length++;
  }
  if (length) input.acceptToken(Text);
}

/** Everything except verbatim bodies, comments and braces. */
export const latexTokens = new ExternalTokenizer((input, stack) => {
  const ch = input.next;
  if (ch < 0 || ch === PERCENT || ch === LBRACE) return; // Comment and OpenBrace are grammar tokens
  if (ch === BACKSLASH) return controlToken(input, stack);
  if (ch === DOLLAR) return dollarToken(input, stack);
  if (ch === RBRACE) {
    // An unbalanced } is plain text.
    if (!stack.canShift(CloseBrace) && stack.canShift(Text)) {
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
  const at = text.lastIndexOf("\\begin{");
  if (at < 0) return null;
  const close = text.indexOf("}", at);
  return close < 0 ? null : text.slice(at + 7, close);
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
