import { Range, Token } from '../../core/tokens';
import { inner } from '../../core/scanner';
import { prepareFigureToken } from './figure';

// Bound for every balanced construct, so unclosed input cannot scan the whole document.
const MAX_SPAN = 20000;
// Named argument values at least this long are not copied into meta.
const MAX_META_VALUE = 80;

// Sticky patterns: they match exactly at lastIndex, so no source slice is made per token.
const NAME = /[a-zA-Z][\w-]*(?:\.[a-zA-Z][\w-]*)*/y;
const ELSE = /else(?![\w-])/y;
const IDENT = /[A-Za-z_][\w-]*/y;
const TARGET = /[a-zA-Z][\w.-]*/y;
const TEMPLATE = /([a-zA-Z][\w-]*(?:\.[a-zA-Z][\w-]*)*)\.with\s*\(/y;
const STRING_PATH = /"([^"\n\\]*)"/y;
function sticky(pattern: RegExp, source: string, pos: number): RegExpExecArray | null {
  pattern.lastIndex = pos;
  return pattern.exec(source);
}

export function isWord(char: string | undefined): boolean {
  return char !== undefined && /\w/.test(char);
}

export function isSpace(char: string | undefined): boolean {
  return char !== undefined && /\s/.test(char);
}

export function lineEnd(source: string, pos: number, limit: number): number {
  const end = source.indexOf('\n', pos);
  return Math.min(end === -1 ? source.length : end, limit);
}

function skipWs(source: string, pos: number, limit: number): number {
  let i = pos;
  while (i < limit && isSpace(source[i])) i++;
  return i;
}

export function trimRange(source: string, from: number, to: number): Range {
  let a = from;
  let b = to;
  while (a < b && isSpace(source[a])) a++;
  while (b > a && isSpace(source[b - 1])) b--;
  return { from: a, to: b };
}

// Returns the index just past the closing quote, or -1 when the string is unclosed.
function skipString(source: string, pos: number, limit: number): number {
  for (let i = pos + 1; i < limit; i++) {
    if (source[i] === '\\') i++;
    else if (source[i] === '"') return i + 1;
  }
  return -1;
}

// Nested block comment starting at pos. Returns the index past `*/`, or -1.
function skipBlockComment(source: string, pos: number, limit: number): number {
  let depth = 0;
  for (let i = pos; i < limit - 1; i++) {
    if (source[i] === '/' && source[i + 1] === '*') {
      depth++;
      i++;
    } else if (source[i] === '*' && source[i + 1] === '/') {
      depth--;
      i++;
      if (depth === 0) return i + 1;
    }
  }
  return -1;
}

// Index just past the backtick fence that closes a raw span, or -1.
function findRun(source: string, from: number, limit: number, fence: string): number {
  const index = source.slice(from, limit).indexOf(fence);
  return index < 0 ? -1 : from + index + fence.length;
}

interface Frame {
  close: string;
  markup: boolean;
}

const FRAMES: Record<string, Frame> = {
  '(': { close: ')', markup: false },
  '{': { close: '}', markup: false },
  '[': { close: ']', markup: true }
};

// Index just past the bracket matching source[start] ('(', '[' or '{'), or -1 when it is unclosed.
// Content blocks are markup, where parentheses and braces are text; strings, comments and
// embedded `#` code are skipped so their brackets do not count.
function skipGroup(source: string, start: number, limit: number): number {
  const stop = Math.min(limit, source.length, start + MAX_SPAN);
  const stack: Frame[] = [FRAMES[source[start]]];
  let i = start + 1;

  while (i < stop) {
    const char = source[i];
    if (char === '\\') {
      i += 2;
      continue;
    }

    const top = stack[stack.length - 1];
    if (top.markup) {
      if (char === '/' && source[i + 1] === '/' && (i === 0 || isSpace(source[i - 1]))) {
        i = lineEnd(source, i, stop);
        continue;
      }
      if (char === '/' && source[i + 1] === '*') {
        const end = skipBlockComment(source, i, stop);
        if (end < 0) return -1;
        i = end;
        continue;
      }
      if (char === '`') {
        let n = 0;
        while (source[i + n] === '`') n++;
        const end = findRun(source, i + n, stop, '`'.repeat(n));
        i = end < 0 ? i + n : end;
        continue;
      }
      if (char === '#') {
        const end = skipHash(source, i, stop);
        if (end < 0) return -1;
        i = end;
        continue;
      }
      if (char === top.close) {
        stack.pop();
        if (stack.length === 0) return i + 1;
      } else if (char === '[') {
        stack.push(FRAMES['[']);
      }
      i++;
      continue;
    }

    if (char === '"') {
      const end = skipString(source, i, stop);
      if (end < 0) return -1;
      i = end;
      continue;
    }
    if (char === '/' && source[i + 1] === '/') {
      i = lineEnd(source, i, stop);
      continue;
    }
    if (char === '/' && source[i + 1] === '*') {
      const end = skipBlockComment(source, i, stop);
      if (end < 0) return -1;
      i = end;
      continue;
    }
    if (char === '(' || char === '[' || char === '{') {
      stack.push(FRAMES[char]);
    } else if (char === top.close) {
      stack.pop();
      if (stack.length === 0) return i + 1;
    }
    i++;
  }
  return -1;
}

// `#` inside content: skips an embedded `#{...}`, `#(...)`, `#"..."` or `#name(...)` so its
// brackets are not read as content brackets. Returns -1 when that code is unclosed.
function skipHash(source: string, pos: number, limit: number): number {
  const next = source[pos + 1];
  if (next === '(' || next === '{') return skipGroup(source, pos + 1, limit);
  if (next === '"') return skipString(source, pos + 1, limit);
  const name = sticky(NAME, source, pos + 1);
  if (!name) return pos + 1;
  const after = pos + 1 + name[0].length;
  return source[after] === '(' ? skipGroup(source, after, limit) : after;
}

// Index of the first top-level `char` in [from, to), skipping groups and strings.
function topLevelIndex(source: string, from: number, to: number, char: string): number {
  let i = from;
  while (i < to) {
    const current = source[i];
    if (current === char) return i;
    if (current === '"') {
      const end = skipString(source, i, to);
      if (end < 0) return -1;
      i = end;
    } else if (current === '(' || current === '[' || current === '{') {
      const end = skipGroup(source, i, to);
      if (end < 0) return -1;
      i = end;
    } else {
      i++;
    }
  }
  return -1;
}

// End of a statement that starts at `start`: the line, extended while brackets are open
// and across a line break before `else`.
function statementEnd(source: string, start: number): number {
  const limit = Math.min(source.length, start + MAX_SPAN);
  let i = start;
  while (i < limit) {
    const char = source[i];
    if (char === '\n') {
      const next = skipWs(source, i + 1, limit);
      if (sticky(ELSE, source, next)) {
        i = next + 4;
        continue;
      }
      return i;
    }
    if (char === '/' && source[i + 1] === '/') return i;
    if (char === '"') {
      const end = skipString(source, i, limit);
      if (end < 0) return lineEnd(source, i, limit);
      i = end;
    } else if (char === '(' || char === '[' || char === '{') {
      const end = skipGroup(source, i, limit);
      if (end < 0) return lineEnd(source, i, limit);
      i = end;
    } else {
      i++;
    }
  }
  return limit;
}

function trimmedEnd(source: string, start: number, end: number): number {
  let to = end;
  while (to > start && isSpace(source[to - 1])) to--;
  return to;
}

// `end` is the end of the statement when a parse tree gives it; otherwise the line scan finds it.
export function statementToken(source: string, pos: number, keyword: string, end?: number): Token {
  const start = pos + 1 + keyword.length;
  const to = end ?? trimmedEnd(source, start, statementEnd(source, start));
  const meta: Record<string, string> = { statement: 'true' };
  const token: Token = { kind: 'command', name: keyword, from: pos, to, meta };
  const at = skipWs(source, start, to);

  if (keyword === 'let') {
    let cursor = at;
    let name = '';
    let params = '';
    const ident = sticky(IDENT, source, at);
    if (ident) {
      name = ident[0];
      cursor = at + name.length;
    }
    let after = skipWs(source, cursor, to);
    if (!ident && source[after] === '(') {
      // Destructuring pattern: no name, no parameters.
      const end = skipGroup(source, after, to);
      after = end < 0 ? after : skipWs(source, end, to);
    } else if (source[after] === '(') {
      const end = skipGroup(source, after, to);
      if (end > 0) {
        params = source.slice(after + 1, end - 1).trim();
        after = skipWs(source, end, to);
      }
    }
    meta.name = name;
    meta.params = params;
    if (source[after] === '=' && source[after + 1] !== '=') {
      token.body = trimRange(source, after + 1, to);
    }
  } else if (keyword === 'set') {
    meta.target = sticky(TARGET, source, at)?.[0] ?? '';
  } else if (keyword === 'show') {
    const colon = topLevelIndex(source, at, to, ':');
    meta.target = colon < 0 ? '' : source.slice(at, colon).trim();
    if (!meta.target && colon >= 0) {
      const template = sticky(TEMPLATE, source, skipWs(source, colon + 1, to));
      if (template) meta.template = template[1];
    }
  } else if (keyword === 'import' || keyword === 'include') {
    const path = sticky(STRING_PATH, source, at);
    if (path) {
      meta.path = path[1];
      token.args = [{ from: at + 1, to: at + 1 + path[1].length }];
    }
  }
  return token;
}

export interface CallParts {
  name: string;
  from: number;
  to: number;
  // `( ... )` groups and trailing `[ ... ]` blocks, brackets included.
  parens: Range[];
  trailing: Range[];
  // Positional and named arguments of the paren groups.
  args: Range[];
  named: Record<string, Range>;
}

// The token of a call from its parts and the tokens of its paren interiors (parsed as code) and
// trailing blocks (parsed as markup).
export function shapeCall(source: string, parts: CallParts, argTokens: Token[], trailingTokens: Token[]): Token {
  const { name, parens, trailing, args, named } = parts;
  const meta: Record<string, string> = {};
  for (const [key, value] of Object.entries(named)) {
    const text = source.slice(value.from, value.to);
    if (text.length < MAX_META_VALUE) meta[`arg:${key}`] = text;
  }
  if ((name === 'text' || name === 'highlight') && named.fill) {
    meta.color = source.slice(named.fill.from, named.fill.to);
  }
  if (!trailing.length) meta.raw = 'true';
  if (!trailing.length && parens.length) meta.args = 'code';

  const body = trailing.length
    ? inner(trailing[trailing.length - 1], 1, 1)
    : parens.length
      ? inner(parens[0], 1, 1)
      : undefined;

  const token: Token = {
    kind: name === 'table' || name === 'grid' ? 'table' : 'command',
    name,
    from: parts.from,
    to: parts.to,
    args,
    body,
    meta
  };
  if (Object.keys(named).length) token.named = named;

  // Arguments are parsed as code. With a trailing content block only the content-bearing
  // arguments are kept, so strings and argument names do not inherit the text styling.
  const children = trailing.length
    ? [
        ...argTokens.filter(child => child.kind === 'group' || child.kind === 'command' || child.kind === 'table'),
        ...trailingTokens
      ]
    : argTokens;
  if (children.length) token.children = children;

  if (name === 'figure' || name === 'subpar.grid') prepareFigureToken(source, token);
  return token;
}
