import { latexTree } from '../../../source-trees.js';
import { RuleContext } from '../../core/language';
import { Token } from '../../core/tokens';
import { accent, command, environmentToken, findMatchingEnd, group, heading, item, ligature, linebreak, escape, tie, verb } from './rules';

// The Token tree of a LaTeX source, built from the shared Lezer parse (source-trees.js).
//
// The grammar is shallow: it knows where comments, math, verbatim text, \verb spans and environments
// are, but not what a command's arguments are. So the tree decides those structural facts, and the
// signature logic of rules.ts (catalog, user macros, ligatures, accents, ties) fills in the rest. The
// scan order, the depth limit and the text tokens between matches are those of core/tokenizer.ts.

const MAX_DEPTH = 12;
const RULES = [heading, item, verb, accent, linebreak, escape, command, group, ligature, tie];
const MATH_CLOSE: Record<string, string> = { $$: '$$', '\\[': '\\]', '\\(': '\\)', $: '$' };

// What the tree says starts at one offset.
type Hit =
  | { type: 'comment'; to: number }
  | { type: 'math'; display: boolean; to: number; closed: boolean }
  | { type: 'environment'; to: number; endFrom: number; endTo: number; name: string | null }
  | { type: 'verb'; to: number };

type Syntax = { name: string; from: number; to: number; node: any };

function lastClosed(node: any): boolean {
  const last = node.lastChild;
  return !!last && last.to > last.from && last.name !== 'ImplicitClose';
}

function indexTree(source: string): Map<number, Hit> {
  const hits = new Map<number, Hit>();
  latexTree(source).iterate({
    enter(ref: Syntax) {
      switch (ref.name) {
        case 'Comment':
          hits.set(ref.from, { type: 'comment', to: ref.to });
          break;
        case 'InlineMath':
        case 'DisplayMath':
          hits.set(ref.from, { type: 'math', display: ref.name === 'DisplayMath', to: ref.to, closed: lastClosed(ref.node) });
          break;
        case 'Verb':
          hits.set(ref.from, { type: 'verb', to: ref.to });
          break;
        case 'Environment':
        case 'MathEnvironment':
        case 'VerbatimEnvironment': {
          const node = ref.node;
          const last = node.lastChild;
          const begin = node.firstChild;
          const name = begin?.getChild('EnvName');
          if (last?.name === 'EndEnv' && last.to > last.from && name) {
            hits.set(ref.from, {
              type: 'environment',
              to: ref.to,
              endFrom: last.from,
              endTo: last.to,
              name: source.slice(name.from, name.to)
            });
          } else {
            hits.set(ref.from, { type: 'environment', to: ref.to, endFrom: -1, endTo: -1, name: null });
          }
          break;
        }
      }
    }
  });
  return hits;
}

export function tokenizeLatex(source: string): Token[] {
  const hits = indexTree(source);
  return run(source, hits, 0, source.length, 0);
}

function run(source: string, hits: Map<number, Hit>, from: number, to: number, depth: number): Token[] {
  const tokens: Token[] = [];
  if (depth > MAX_DEPTH || from >= to) return tokens;

  const ctx: RuleContext = {
    source,
    depth: depth + 1,
    mode: 'markup',
    parse: (childFrom, childTo) => run(source, hits, childFrom, childTo, depth + 1)
  };

  let pos = from;
  let textFrom = from;
  while (pos < to) {
    const token = matchAt(source, hits, pos, to, ctx);
    if (!token) {
      pos++;
      continue;
    }
    if (token.from > textFrom) tokens.push({ kind: 'text', from: textFrom, to: token.from });
    tokens.push(token);
    pos = token.to;
    textFrom = pos;
  }
  if (to > textFrom) tokens.push({ kind: 'text', from: textFrom, to });
  return tokens;
}

// The first candidate that fits in the range, as core/tokenizer.ts does: one that is empty or runs past `limit` is skipped.
function matchAt(source: string, hits: Map<number, Hit>, pos: number, limit: number, ctx: RuleContext): Token | null {
  const fits = (token: Token | null): token is Token => !!token && token.to > token.from && token.to <= limit;
  const hit = hits.get(pos);
  if (hit) {
    const token = fromHit(source, pos, hit, ctx);
    if (fits(token)) return token;
  }
  // Everything the grammar leaves inside Text and control symbols, and the argument grouping of commands.
  for (const rule of RULES) {
    const token = rule(source, pos, ctx);
    if (fits(token)) return token;
  }
  return null;
}

function fromHit(source: string, pos: number, hit: Hit, ctx: RuleContext): Token | null {
  switch (hit.type) {
    case 'comment':
      return { kind: 'comment', from: pos, to: hit.to, body: { from: pos + 1, to: hit.to } };
    case 'math':
      return mathToken(source, pos, hit);
    case 'verb':
      return verbToken(source, pos, hit.to);
    case 'environment':
      return environment(source, pos, hit, ctx);
  }
}

function mathToken(source: string, pos: number, hit: { display: boolean; to: number; closed: boolean }): Token | null {
  if (!hit.closed) return null;
  const open = ['$$', '\\[', '\\(', '$'].find(delimiter => source.startsWith(delimiter, pos));
  if (!open) return null;
  const close = MATH_CLOSE[open];
  if (hit.to - pos < open.length + close.length || !source.startsWith(close, hit.to - close.length)) return null;
  const body = { from: pos + open.length, to: hit.to - close.length };
  if (body.to <= body.from) return null;
  if (/\n[ \t]*\n/.test(source.slice(body.from, body.to))) return null;
  return { kind: 'math', from: pos, to: hit.to, display: hit.display, body, meta: { open, close } };
}

// \verb<delim>...<delim>; the grammar's token also covers an unclosed one, which is not a verb here.
function verbToken(source: string, pos: number, to: number): Token | null {
  const star = source[pos + 5] === '*';
  const delimiterAt = pos + (star ? 6 : 5);
  const delimiter = source[delimiterAt];
  if (!delimiter || to - 1 <= delimiterAt || source[to - 1] !== delimiter) return null;
  const token: Token = { kind: 'verb', name: 'verb', from: pos, to, body: { from: delimiterAt + 1, to: to - 1 } };
  if (star) token.star = true;
  return token;
}

// The tree pairs \begin with \end structurally. It is used when it closed the environment cleanly
// with the same name; a broken or mismatched pair falls back to the name search of the old rules.
function environment(source: string, pos: number, hit: Extract<Hit, { type: 'environment' }>, ctx: RuleContext): Token | null {
  const header = /^\\begin\{([^}\n]+)\}/.exec(source.slice(pos, pos + 200));
  if (!header) return null;
  const name = header[1];
  let end = -1;
  if (hit.name !== null && hit.name === name && source.slice(hit.endFrom, hit.endTo) === `\\end{${name}}`) {
    end = hit.endFrom;
  } else {
    end = findMatchingEnd(source, pos + header[0].length, name);
  }
  if (end === -1) return null;
  return environmentToken(source, pos, name, end, ctx);
}
