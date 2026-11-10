import { latexTree } from '../../../source-trees.js';
import type { Range, Token } from '../../core/tokens';
import { ESCAPABLE, HEADING_LEVELS } from './commands';
import { COMMANDS } from './catalog';
import { accentToken, argumentLimit, commandToken, environmentSpec, environmentToken, ligatureToken, SYMBOL_ACCENTS, tableArguments, WORD_ACCENTS } from './shape';
import type { Arguments } from './shape';

// The Token tree of a LaTeX source: a pure walk over the shared Lezer parse (source-trees.js).
//
// The grammar finds every piece of syntax this layer cares about: comments, math, verbatim text, \verb
// spans, environments, groups, control words and symbols, ligatures, ties and square brackets. The walk
// turns those nodes into tokens. What a command means (how many arguments \frac takes, which control
// words are headings or accents) is catalog data applied to the nodes that follow the command: its
// signature picks up the sibling groups, bracket runs and star, skipping blanks as TeX does. The only
// text it reads is the text of one node, or the first character of the Text node right after a command.
//
// Text tokens are whatever lies between the tokens that were emitted.

const MAX_DEPTH = 12;
const MATH_CLOSE: Record<string, string> = { $$: '$$', '\\[': '\\]', '\\(': '\\)', $: '$' };
const BEGIN_NODES = new Set(['BeginEnv', 'MathBeginEnv', 'VerbBeginEnv', 'ListingBeginEnv']);
const ENVIRONMENT_NODES = new Set(['Environment', 'MathEnvironment', 'VerbatimEnvironment', 'InnerEnvironment']);
const GROUP_NODES = new Set(['Group', 'MathGroup']);

type Node = any;

// A position in a list of sibling nodes: the index of the node holding `pos`, and the source offset.
interface Cursor {
  i: number;
  pos: number;
}

interface Out {
  tokens: Token[];
  textFrom: number;
  limit: number;
}

export function tokenizeLatex(source: string): Token[] {
  return run(source, kids(latexTree(source).topNode), 0, source.length, 0);
}

// The children of a node, with each Command wrapper replaced by its parts (control word, then its attached
// optional argument or URL).
function kids(node: Node): Node[] {
  const list: Node[] = [];
  for (let child = node.firstChild; child; child = child.nextSibling) {
    if (child.name === 'Command') {
      for (let part = child.firstChild; part; part = part.nextSibling) list.push(part);
    } else {
      list.push(child);
    }
  }
  return list;
}

function run(source: string, list: Node[], from: number, to: number, depth: number): Token[] {
  if (depth > MAX_DEPTH || from >= to) return [];
  const out: Out = { tokens: [], textFrom: from, limit: to };
  walk(source, list, out, depth);
  if (to > out.textFrom) out.tokens.push({ kind: 'text', from: out.textFrom, to });
  return out.tokens;
}

// A token that is empty, runs past the range or starts inside an earlier token is dropped.
function emit(out: Out, token: Token | null): boolean {
  if (!token || token.to <= token.from || token.to > out.limit || token.from < out.textFrom) return false;
  if (token.from > out.textFrom) out.tokens.push({ kind: 'text', from: out.textFrom, to: token.from });
  out.tokens.push(token);
  out.textFrom = token.to;
  return true;
}

function walk(source: string, list: Node[], out: Out, depth: number): void {
  for (let i = 0; i < list.length;) i = visit(source, list, i, out, depth);
}

// Handles list[i] and returns the index of the next node to visit.
function visit(source: string, list: Node[], i: number, out: Out, depth: number): number {
  const node = list[i];
  switch (node.name) {
    case 'Comment':
      emit(out, { kind: 'comment', from: node.from, to: node.to, body: { from: node.from + 1, to: node.to } });
      return i + 1;
    case 'Ligature':
      emit(out, ligatureToken(node.from, node.to, source.slice(node.from, node.to)));
      return i + 1;
    case 'Tie':
      emit(out, { kind: 'tie', from: node.from, to: node.to });
      return i + 1;
    case 'InlineMath':
    case 'DisplayMath':
      if (!emit(out, mathToken(source, node))) descend(source, node, out, depth);
      return i + 1;
    case 'Verb':
      if (!emit(out, verbToken(source, node.from, node.to))) {
        // An unclosed \verb is an ordinary command with no arguments.
        const name = /^\\([a-zA-Z]+)/.exec(source.slice(node.from, node.from + 12))?.[1] ?? '';
        const parsed: Arguments = { args: [], nodes: [], options: [], star: false, to: node.from + 1 + name.length };
        emit(out, commandToken(source, name, node.from, parsed, () => []));
      }
      return i + 1;
    case 'Group':
    case 'MathGroup':
      if (!emit(out, groupToken(source, node, depth))) descend(source, node, out, depth);
      return i + 1;
    case 'ControlSequence':
    case 'SectionName':
      return controlWord(source, list, i, out, depth);
    case 'ControlSymbol':
      return controlSymbol(source, list, i, out);
    default:
      if (ENVIRONMENT_NODES.has(node.name)) {
        if (!emit(out, environment(source, node, depth))) descend(source, node, out, depth);
      } else if (BEGIN_NODES.has(node.name)) {
        return unclosedBegin(source, list, i, out, depth);
      } else if (node.firstChild) {
        // Whatever the walk does not turn into a token itself (an optional argument no command took,
        // an unclosed group) is searched for tokens in place.
        descend(source, node, out, depth);
      }
      return i + 1;
  }
}

function descend(source: string, node: Node, out: Out, depth: number): void {
  walk(source, kids(node), out, depth);
}

function isClosed(node: Node, closer: string): boolean {
  const last = node.lastChild;
  return !!last && last.name === closer && last.to > last.from;
}

function inner(node: Node): Range {
  return { from: node.from + 1, to: Math.max(node.from + 1, node.to - 1) };
}

// --- Math, \verb, groups -------------------------------------------------------------------------------

function mathToken(source: string, node: Node): Token | null {
  const last = node.lastChild;
  if (!last || last.to <= last.from || last.name === 'ImplicitClose') return null;
  const pos = node.from;
  const open = ['$$', '\\[', '\\(', '$'].find(delimiter => source.startsWith(delimiter, pos));
  if (!open) return null;
  const close = MATH_CLOSE[open];
  if (node.to - pos < open.length + close.length || !source.startsWith(close, node.to - close.length)) return null;
  const body = { from: pos + open.length, to: node.to - close.length };
  if (body.to <= body.from) return null;
  if (/\n[ \t]*\n/.test(source.slice(body.from, body.to))) return null;
  return { kind: 'math', from: pos, to: node.to, display: node.name === 'DisplayMath', body, meta: { open, close } };
}

// A Verb node is one opaque token: \verb<d>..<d>, \verb*<d>..<d>, \Verb, \lstinline[opts]<d>..<d> or {..},
// \mintinline[opts]{lang}<d>..<d> or {..}. The node holds the whole span; this reads its parts back from
// the text of that node alone. An unclosed \verb span is a Verb node too, and is not a verb here.
function verbToken(source: string, from: number, to: number): Token | null {
  const name = /^\\([a-zA-Z]+)/.exec(source.slice(from, from + 12))?.[1];
  if (!name) return null;
  const verbName = name === 'verb' || name === 'Verb';
  let cursor = from + 1 + name.length;
  const star = verbName && source[cursor] === '*';
  if (star) cursor++;

  const options: Range[] = [];
  const args: Range[] = [];
  if (!verbName && source[cursor] === '[') {
    const close = source.indexOf(']', cursor);
    if (close < 0 || close >= to) return null;
    options.push({ from: cursor + 1, to: close });
    cursor = close + 1;
  }
  if (name === 'mintinline') {
    if (source[cursor] !== '{') return null;
    const close = source.indexOf('}', cursor);
    if (close < 0 || close >= to) return null;
    args.push({ from: cursor + 1, to: close });
    cursor = close + 1;
  }

  // \verb takes any delimiter, braces included; the other commands also accept a braced argument.
  const open = source[cursor];
  const closer = !verbName && open === '{' ? '}' : open;
  if (!open || to - 1 <= cursor || source[to - 1] !== closer) return null;

  const token: Token = { kind: 'verb', name, from, to, body: { from: cursor + 1, to: to - 1 } };
  if (args.length) token.args = args;
  if (options.length) token.options = options;
  if (star) token.star = true;
  return token;
}

// A bare group is a `{` that no command claimed as an argument.
function groupToken(source: string, node: Node, depth: number): Token | null {
  if (!isClosed(node, 'CloseBrace')) return null;
  const body = inner(node);
  return { kind: 'group', from: node.from, to: node.to, body, children: run(source, kids(node), body.from, body.to, depth + 1) };
}

// --- Argument reading ------------------------------------------------------------------------------------

function isBlank(char: string, anyNewline: boolean): boolean {
  return /\s/.test(char) && (anyNewline || char !== '\n');
}

// Skips the blanks at the cursor: spaces and tabs (and newlines with `anyNewline`), as the old signature
// parser did. A newline ends a command's arguments, since it may be a paragraph break.
function blank(source: string, list: Node[], cursor: Cursor, anyNewline: boolean): Cursor {
  let { i, pos } = cursor;
  while (i < list.length && list[i].name === 'Text') {
    const node = list[i];
    let k = Math.max(pos, node.from);
    while (k < node.to && isBlank(source[k], anyNewline)) k++;
    pos = k;
    if (k < node.to) break;
    i++;
  }
  return { i, pos };
}

// A closed brace group (or URL argument) that starts at the cursor.
function takeGroup(list: Node[], cursor: Cursor, urls = true): Node | null {
  const node = list[cursor.i];
  if (!node || node.from !== cursor.pos) return null;
  if (node.name === 'UrlArg') return urls ? node : null;
  return GROUP_NODES.has(node.name) && isClosed(node, 'CloseBrace') ? node : null;
}

function takeStar(source: string, list: Node[], cursor: Cursor): Cursor | null {
  const node = list[cursor.i];
  if (!node || node.name !== 'Text' || node.from > cursor.pos || cursor.pos >= node.to || source[cursor.pos] !== '*') return null;
  const pos = cursor.pos + 1;
  return pos >= node.to ? { i: cursor.i + 1, pos } : { i: cursor.i, pos };
}

interface Taken {
  body: Range;
  to: number;
  cursor: Cursor;
}

// An optional argument at the cursor: the optional argument node the grammar attached to the command, or
// a run of `[` ... `]` nodes (nested pairs count). Whether the command takes it is the caller's decision.
// A run does not cross a blank line.
function takeOptional(source: string, list: Node[], cursor: Cursor): Taken | null {
  const node = list[cursor.i];
  if (!node || node.from !== cursor.pos) return null;
  if (node.name === 'OptionalArgument') {
    return isClosed(node, 'OptClose') ? { body: inner(node), to: node.to, cursor: { i: cursor.i + 1, pos: node.to } } : null;
  }
  if (node.name !== 'OpenBracket') return null;
  let depth = 1;
  for (let j = cursor.i + 1; j < list.length; j++) {
    const next = list[j];
    if (next.name === 'OpenBracket') depth++;
    else if (next.name === 'CloseBracket' && --depth === 0) {
      return { body: { from: node.to, to: next.from }, to: next.to, cursor: { i: j + 1, pos: next.to } };
    } else if (next.name === 'Text' && /\n[ \t]*\n/.test(source.slice(next.from, next.to))) return null;
  }
  return null;
}

// Reads an xparse-style signature: `s` an immediate star, `o` an optional [..], `m` a mandatory {..}.
// Stops at the first missing mandatory argument, as LaTeX does.
function readSignature(source: string, list: Node[], start: Cursor, signature: string, star: boolean): Arguments {
  const parsed: Arguments = { args: [], nodes: [], options: [], star, to: start.pos };
  let cursor = start;
  for (const letter of signature) {
    if (letter === 's') {
      const next = star ? null : takeStar(source, list, cursor);
      if (next) {
        parsed.star = true;
        parsed.to = next.pos;
        cursor = next;
      }
    } else if (letter === 'o') {
      const option = takeOptional(source, list, blank(source, list, cursor, false));
      if (option) {
        parsed.options.push(option.body);
        parsed.to = option.to;
        cursor = option.cursor;
      }
    } else if (letter === 'm') {
      const at = blank(source, list, cursor, false);
      const group = takeGroup(list, at);
      if (!group) break;
      parsed.args.push(inner(group));
      parsed.nodes.push(group);
      parsed.to = group.to;
      cursor = { i: at.i + 1, pos: group.to };
    }
  }
  return parsed;
}

// Reads the arguments of a command the catalog does not know: options and groups, up to `limit` groups
// (all adjacent ones when undefined), then with `trailingOptional` any options after them.
function readAdjacent(source: string, list: Node[], start: Cursor, limit?: number, trailingOptional = false): Arguments {
  const parsed: Arguments = { args: [], nodes: [], options: [], star: false, to: start.pos };
  let cursor = start;
  while (limit === undefined || parsed.args.length < limit) {
    const at = blank(source, list, cursor, false);
    const option = takeOptional(source, list, at);
    if (option) {
      parsed.options.push(option.body);
      parsed.to = option.to;
      cursor = option.cursor;
      continue;
    }
    const group = takeGroup(list, at);
    if (!group) break;
    parsed.args.push(inner(group));
    parsed.nodes.push(group);
    parsed.to = group.to;
    cursor = { i: at.i + 1, pos: group.to };
  }
  if (trailingOptional) {
    for (;;) {
      const option = takeOptional(source, list, blank(source, list, cursor, false));
      if (!option) break;
      parsed.options.push(option.body);
      parsed.to = option.to;
      cursor = option.cursor;
    }
  }
  return parsed;
}

// The index of the first node that starts at or after `to`: where the walk resumes after a token.
function resume(list: Node[], from: number, to: number): number {
  let i = from;
  while (i < list.length && list[i].from < to) i++;
  return i;
}

// The children of the argument node (a group's contents), or just its text.
function argumentChildren(source: string, node: Node | null | undefined, range: Range, depth: number): Token[] {
  return run(source, node && GROUP_NODES.has(node.name) ? kids(node) : [], range.from, range.to, depth + 1);
}

// --- Control words ----------------------------------------------------------------------------------------

function controlWord(source: string, list: Node[], i: number, out: Out, depth: number): number {
  const head = list[i];
  const raw = source.slice(head.from + 1, head.to);
  const name = raw.replace(/\*$/, '');
  const headStar = raw.endsWith('*');
  const start: Cursor = { i: i + 1, pos: head.to };

  // Headings: the title is the braced group after the name; an optional short title makes it a plain command.
  const level = HEADING_LEVELS.get(name);
  if (level !== undefined) {
    const group = takeGroup(list, blank(source, list, start, true), false);
    if (group) {
      const body = inner(group);
      const token: Token = {
        kind: 'heading', name, level, from: head.from, to: group.to, body,
        children: argumentChildren(source, group, body, depth)
      };
      if (headStar) token.star = true;
      if (emit(out, token)) return resume(list, i + 1, group.to);
    }
  }

  if (name === 'item') {
    const label = list[i + 1];
    const taken = label && label.name === 'OptionalArgument' ? takeOptional(source, list, start) : null;
    const to = taken ? taken.to : head.to;
    if (emit(out, { kind: 'item', from: head.from, to, body: taken?.body })) return resume(list, i + 1, to);
  }

  if (WORD_ACCENTS.has(name) && !headStar) {
    const token = wordAccent(source, list, name, head, start);
    if (token && emit(out, token)) return resume(list, i + 1, token.to);
  }

  if (name === 'newline' && !headStar) {
    if (emit(out, { kind: 'linebreak', from: head.from, to: head.to })) return i + 1;
  }

  const spec = COMMANDS.get(name);
  let parsed: Arguments;
  if (spec) {
    parsed = readSignature(source, list, start, spec.args, headStar);
  } else {
    // Without a catalog entry the old argument count applies; the star is read here, not folded into the name.
    const star = takeStar(source, list, start);
    const starred = headStar || !!star;
    parsed = { ...readAdjacent(source, list, star ?? start, argumentLimit(name, starred)), star: starred };
  }
  const token = commandToken(source, name, head.from, parsed, (shown, node) => argumentChildren(source, node, shown, depth));
  if (!emit(out, token)) return i + 1;
  return resume(list, i + 1, token.to);
}

// \v{s}, \c c: a letter accent over a braced base or the next letter, blanks allowed before it.
function wordAccent(source: string, list: Node[], name: string, head: Node, start: Cursor): Token | null {
  const at = blank(source, list, start, false);
  const group = takeGroup(list, at, false);
  const found = group
    ? { base: inner(group), content: source.slice(group.from + 1, group.to - 1).trim(), to: group.to }
    : baseAt(source, list, at);
  return found ? accentToken(name, head.from, found.to, found.base, found.content) : null;
}

// The unbraced base of an accent at the cursor: the first letter of a Text node, or a \i or \j.
function baseAt(source: string, list: Node[], cursor: Cursor): { base: Range; content: string; to: number } | null {
  const node = list[cursor.i];
  if (!node) return null;
  if (node.name === 'Text' && node.from <= cursor.pos && cursor.pos < node.to && /[a-zA-Z]/.test(source[cursor.pos])) {
    return { base: { from: cursor.pos, to: cursor.pos + 1 }, content: source[cursor.pos], to: cursor.pos + 1 };
  }
  if (node.name === 'ControlSequence' && node.from === cursor.pos) {
    const word = source.slice(node.from, node.to);
    if (word === '\\i' || word === '\\j') return { base: { from: node.from, to: node.to }, content: word, to: node.to };
  }
  return null;
}

// --- Control symbols ----------------------------------------------------------------------------------------

function controlSymbol(source: string, list: Node[], i: number, out: Out): number {
  const head = list[i];
  const char = source[head.from + 1];
  if (!char) return i + 1;
  const start: Cursor = { i: i + 1, pos: head.to };

  if (SYMBOL_ACCENTS.has(char)) {
    // \'e, \"{o}, \'\i: the base follows directly.
    const group = takeGroup(list, start, false);
    const found = group
      ? { base: inner(group), content: source.slice(group.from + 1, group.to - 1).trim(), to: group.to }
      : baseAt(source, list, start);
    const token = found ? accentToken(char, head.from, found.to, found.base, found.content) : null;
    if (token && emit(out, token)) return resume(list, i + 1, token.to);
  }

  // \\, \\*, \\[2pt]; the table code reads the source for its rows, so these tokens do not change how tables are parsed.
  if (char === '\\') {
    let cursor = start;
    let to = head.to;
    const star = takeStar(source, list, cursor);
    if (star) {
      cursor = star;
      to = star.pos;
    }
    const length = takeOptional(source, list, cursor);
    if (length) to = length.to;
    emit(out, { kind: 'linebreak', from: head.from, to });
    return resume(list, i + 1, to);
  }

  if (ESCAPABLE.has(char)) emit(out, { kind: 'raw', from: head.from, to: head.from + 2 });
  return i + 1;
}

// --- Environments ---------------------------------------------------------------------------------------------

// The token of a closed environment; null when the tree has no matching \end for it (the walk then looks
// inside in place). A \begin and \end of different names are whatever the tree paired.
function environment(source: string, node: Node, depth: number): Token | null {
  const begin = node.firstChild;
  const end = node.lastChild;
  if (!begin || end?.name !== 'EndEnv' || end.to <= end.from) return null;
  const nameNode = begin.getChild('EnvName');
  if (!nameNode) return null;
  const name = source.slice(nameNode.from, nameNode.to);

  const list = kids(node).slice(1, -1);
  const start: Cursor = { i: 0, pos: begin.to };
  const spec = environmentSpec(name);
  const tableArgs = tableArguments(name);
  // The star is part of the environment name here, so the signature has no `s`.
  const parsed = spec
    ? readSignature(source, list, start, spec.args.replace('s', ''), false)
    : readAdjacent(source, list, start, tableArgs, tableArgs !== undefined);

  const body = list.slice(resume(list, 0, parsed.to));
  return environmentToken(source, name, node.from, end.from, end.to, parsed, () => run(source, body, parsed.to, end.from, depth + 1));
}

// A \begin whose environment the tree never closed is the command \begin{name} (and any groups after it).
function unclosedBegin(source: string, list: Node[], i: number, out: Out, depth: number): number {
  const node = list[i];
  const nameNode = node.getChild('EnvName');
  if (!nameNode) return i + 1;
  const first: Range = { from: nameNode.from, to: nameNode.to };
  const rest = readAdjacent(source, list, { i: i + 1, pos: node.to });
  const parsed: Arguments = {
    args: [first, ...rest.args], nodes: [null, ...rest.nodes], options: rest.options, star: false, to: rest.to
  };
  const token = commandToken(source, 'begin', node.from, parsed, (shown, group) => argumentChildren(source, group, shown, depth));
  if (!emit(out, token)) return i + 1;
  return resume(list, i + 1, token.to);
}
