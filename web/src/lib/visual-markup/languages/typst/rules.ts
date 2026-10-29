import { ParseMode, Rule, RuleContext } from '../../core/language';
import { Range, Token } from '../../core/tokens';
import { atLineStart, inner, matchFenced } from '../../core/scanner';
import { prepareFigureToken } from './figure';

// Bound for every balanced construct, so unclosed input cannot scan the whole document.
const MAX_SPAN = 20000;
// Named argument values at least this long are not copied into meta.
const MAX_META_VALUE = 80;

// Sticky patterns: they match exactly at lastIndex, so no source slice is made per token.
const NAME = /[a-zA-Z][\w-]*(?:\.[a-zA-Z][\w-]*)*/y;
const DOT_NAME = /\.[a-zA-Z][\w-]*/y;
const NAMED_ARG = /([a-zA-Z][\w-]*)\s*:(?!:)/y;
const NAMED_RAW = /[a-zA-Z][\w-]*[ \t]*:(?!:)/y;
const SEPARATOR = /[\s,]+/y;
const STATEMENT = /(let|set|show|import|include|if|for|while|context|return|break|continue)(?![\w.-])/y;
const ELSE = /else(?![\w-])/y;
const IDENT = /[A-Za-z_][\w-]*/y;
const TARGET = /[a-zA-Z][\w.-]*/y;
const TEMPLATE = /([a-zA-Z][\w-]*(?:\.[a-zA-Z][\w-]*)*)\.with\s*\(/y;
const STRING_PATH = /"([^"\n\\]*)"/y;
const BACKTICKS = /`+/y;
const LANG = /[^\s`]*/y;
const LABEL = /<([\w:.-]+)>/y;
const REF = /@([\w:-]+(?:\.[\w:-]+)*)/y;
const URL = /(?:https?|ftp):\/\/[^\s<>"'\[\]()]+/y;
const HEADING = /(=+)[ \t]+/y;
const ENUM = /\d+\.[ \t]+/y;
const BULLET = /[-+][ \t]+/y;
const TERM = /\/[ \t]+/y;

const LIGATURES: [string, string][] = [
  ['---', '\u2014'],
  ['--', '\u2013'],
  ['...', '\u2026']
];

function sticky(pattern: RegExp, source: string, pos: number): RegExpExecArray | null {
  pattern.lastIndex = pos;
  return pattern.exec(source);
}

function isWord(char: string | undefined): boolean {
  return char !== undefined && /\w/.test(char);
}

function isSpace(char: string | undefined): boolean {
  return char !== undefined && /\s/.test(char);
}

function lineEnd(source: string, pos: number, limit: number): number {
  const end = source.indexOf('\n', pos);
  return Math.min(end === -1 ? source.length : end, limit);
}

function skipWs(source: string, pos: number, limit: number): number {
  let i = pos;
  while (i < limit && isSpace(source[i])) i++;
  return i;
}

function trimRange(source: string, from: number, to: number): Range {
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

// Splits the arguments of a call into trimmed ranges at top-level commas.
function splitArguments(source: string, from: number, to: number): Range[] {
  const parts: Range[] = [];
  const push = (start: number, end: number) => {
    const range = trimRange(source, start, end);
    if (range.to > range.from) parts.push(range);
  };

  let start = from;
  let i = from;
  while (i < to) {
    const char = source[i];
    if (char === '"') {
      const end = skipString(source, i, to);
      if (end < 0) break;
      i = end;
    } else if (char === '/' && source[i + 1] === '/') {
      i = lineEnd(source, i, to);
    } else if (char === '(' || char === '[' || char === '{') {
      const end = skipGroup(source, i, to);
      if (end < 0) break;
      i = end;
    } else if (char === ',') {
      push(start, i);
      start = i + 1;
      i++;
    } else {
      i++;
    }
  }
  push(start, to);
  return parts;
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

function statementToken(source: string, pos: number, keyword: string): Token {
  const start = pos + 1 + keyword.length;
  const to = trimmedEnd(source, start, statementEnd(source, start));
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

// Balanced `[...]` at pos as a group token whose children are markup.
function groupToken(source: string, pos: number, ctx: RuleContext): Token | null {
  const end = skipGroup(source, pos, source.length);
  if (end < 0) return null;
  const body = inner({ from: pos, to: end }, 1, 1);
  return {
    kind: 'group',
    name: 'content',
    from: pos,
    to: end,
    body,
    children: ctx.parse(body.from, body.to, 'markup')
  };
}

function callToken(source: string, pos: number, ctx: RuleContext, prefix: number): Token | null {
  const start = pos + prefix;
  const nameMatch = sticky(NAME, source, start);
  if (!nameMatch) return null;

  let name = nameMatch[0];
  let cursor = start + name.length;
  // A bare identifier in code is a call only with arguments; `fill: red` stays text.
  if (prefix === 0 && source[cursor] !== '(' && source[cursor] !== '[') return null;

  if (prefix === 1 && (name === 'sym' || name === 'emoji' || name.startsWith('sym.') || name.startsWith('emoji.'))) {
    const [head, ...rest] = name.split('.');
    return { kind: 'command', name: head, from: pos, to: cursor, meta: { path: rest.join('.') } };
  }

  const parens: Range[] = [];
  const trailing: Range[] = [];
  for (;;) {
    const char = source[cursor];
    if (char === '(' || char === '[') {
      const end = skipGroup(source, cursor, source.length);
      if (end < 0) return null;
      (char === '(' ? parens : trailing).push({ from: cursor, to: end });
      cursor = end;
      continue;
    }
    // Method access after a call, as in `counter(page).display()`.
    if (char === '.' && (parens.length || trailing.length)) {
      const segment = sticky(DOT_NAME, source, cursor);
      if (segment) {
        name += segment[0];
        cursor += segment[0].length;
        continue;
      }
    }
    break;
  }

  const args: Range[] = [];
  const named: Record<string, Range> = {};
  for (const group of parens) {
    const body = inner(group, 1, 1);
    for (const part of splitArguments(source, body.from, body.to)) {
      const match = sticky(NAMED_ARG, source, part.from);
      if (match && part.from + match[0].length <= part.to) {
        named[match[1]] = trimRange(source, part.from + match[0].length, part.to);
      } else {
        args.push(part);
      }
    }
  }

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
    from: pos,
    to: cursor,
    args,
    body,
    meta
  };
  if (Object.keys(named).length) token.named = named;

  // Arguments are parsed as code. With a trailing content block only the content-bearing
  // arguments are kept, so strings and argument names do not inherit the text styling.
  const argTokens = parens.flatMap(group => {
    const body = inner(group, 1, 1);
    return ctx.parse(body.from, body.to, 'code');
  });
  const children = trailing.length
    ? [
        ...argTokens.filter(child => child.kind === 'group' || child.kind === 'command' || child.kind === 'table'),
        ...trailing.flatMap(block => {
          const content = inner(block, 1, 1);
          return ctx.parse(content.from, content.to, 'markup');
        })
      ]
    : argTokens;
  if (children.length) token.children = children;

  if (name === 'figure' || name === 'subpar.grid') prepareFigureToken(source, token);
  return token;
}

export const comment: Rule = (source, pos) => {
  if (source[pos] !== '/') return null;
  if (source[pos + 1] === '/') {
    const end = lineEnd(source, pos, source.length);
    return { kind: 'comment', from: pos, to: end, body: { from: pos + 2, to: end } };
  }
  if (source[pos + 1] !== '*') return null;
  const end = skipBlockComment(source, pos, Math.min(source.length, pos + MAX_SPAN));
  return end < 0 ? null : { kind: 'comment', from: pos, to: end, body: { from: pos + 2, to: end - 2 } };
};

// A backslash before a line end is a Typst line break.
export const linebreak: Rule = (source, pos) => {
  if (source[pos] !== '\\') return null;
  let i = pos + 1;
  while (source[i] === ' ' || source[i] === '\t') i++;
  return i >= source.length || source[i] === '\n' ? { kind: 'linebreak', from: pos, to: pos + 1 } : null;
};

// Any other backslash escapes the next character, which is displayed without the backslash.
export const escape: Rule = (source, pos) => {
  if (source[pos] !== '\\' || pos + 1 >= source.length) return null;
  return { kind: 'raw', name: 'escape', from: pos, to: pos + 2, body: { from: pos + 1, to: pos + 2 } };
};

export const heading: Rule = (source, pos, ctx) => {
  if (source[pos] !== '=' || !atLineStart(source, pos)) return null;
  const match = sticky(HEADING, source, pos);
  if (!match) return null;

  const to = lineEnd(source, pos, source.length);
  const body = { from: pos + match[0].length, to };
  return {
    kind: 'heading',
    level: match[1].length,
    from: pos,
    to,
    body,
    children: ctx.parse(body.from, body.to, 'markup')
  };
};

// Inline raw spans have no escapes. A fence of three or more backticks is a block with an optional language.
export const raw: Rule = (source, pos) => {
  if (source[pos] !== '`') return null;
  const run = sticky(BACKTICKS, source, pos);
  const n = run ? run[0].length : 1;
  const end = findRun(source, pos + n, Math.min(source.length, pos + MAX_SPAN), '`'.repeat(n));
  if (end < 0) return null;

  const body = { from: pos + n, to: end - n };
  if (n < 3) return { kind: 'raw', name: 'inline', from: pos, to: end, body };

  const lang = sticky(LANG, source, pos + n)?.[0] ?? '';
  return {
    kind: 'raw',
    name: 'block',
    from: pos,
    to: end,
    body,
    meta: lang ? { lang } : undefined
  };
};

export const math: Rule = (source, pos) => {
  if (source[pos] !== '$') return null;
  const range = matchFenced(source, pos, '$', '$');
  if (!range) return null;

  const body = inner(range, 1, 1);
  if (body.to <= body.from) return null;
  if (/\n[ \t]*\n/.test(source.slice(body.from, body.to))) return null;

  const display = /^\s/.test(source.slice(body.from, body.from + 1)) && /\s$/.test(source.slice(body.to - 1, body.to));
  return { kind: 'math', from: range.from, to: range.to, display, body, meta: { open: '$', close: '$' } };
};

// Index just past the closing marker of a strong or emphasis span, or -1. Spans do not cross a paragraph.
function closeInline(source: string, pos: number, marker: string): number {
  const limit = Math.min(source.length, pos + MAX_SPAN);
  for (let i = pos + 1; i < limit; i++) {
    if (source[i] === '\\') {
      i++;
    } else if (source[i] === '\n' && /^\n[ \t]*\n/.test(source.slice(i, i + 64))) {
      return -1;
    } else if (source[i] === marker) {
      return i + 1;
    }
  }
  return -1;
}

export const emphasis: Rule = (source, pos, ctx) => {
  const marker = source[pos];
  if (marker !== '*' && marker !== '_') return null;
  if (isWord(source[pos - 1])) return null;

  const end = closeInline(source, pos, marker);
  if (end < 0) return null;

  const body = { from: pos + 1, to: end - 1 };
  if (body.to <= body.from || isSpace(source[body.from]) || isSpace(source[body.to - 1])) return null;

  return {
    kind: 'command',
    name: marker === '*' ? 'strong' : 'emph',
    from: pos,
    to: end,
    body,
    children: ctx.parse(body.from, body.to)
  };
};

// Line-start markers: bullets, numbered and `+` items, and term lists `/ Term: description`.
// A term list marker only covers `/ `; the term and the rest of the line stay as markup.
export const item: Rule = (source, pos) => {
  if (!atLineStart(source, pos)) return null;

  const enumMatch = sticky(ENUM, source, pos);
  if (enumMatch) return { kind: 'item', name: 'number', from: pos, to: pos + enumMatch[0].length };

  const bullet = sticky(BULLET, source, pos);
  if (bullet) {
    return { kind: 'item', name: source[pos] === '+' ? 'number' : 'bullet', from: pos, to: pos + bullet[0].length };
  }

  const term = sticky(TERM, source, pos);
  if (term) {
    const to = pos + term[0].length;
    const end = lineEnd(source, to, source.length);
    const colon = source.slice(to, end).indexOf(':');
    const body = { from: to, to: colon < 0 ? end : to + colon };
    return { kind: 'item', name: 'term', from: pos, to, body };
  }
  return null;
};

// Code after `#`: statements, `#{...}` and `#(...)` blocks, `#[...]`, `#sym.*` and calls.
export const code: Rule = (source, pos, ctx) => {
  if (source[pos] !== '#') return null;

  const next = source[pos + 1];
  if (next === '{' || next === '(') {
    const end = skipGroup(source, pos + 1, source.length);
    if (end < 0) return null;
    const body = inner({ from: pos + 1, to: end }, 1, 1);
    return {
      kind: 'command',
      name: next === '{' ? 'code' : 'expr',
      from: pos,
      to: end,
      body,
      meta: { statement: 'true' }
    };
  }
  if (next === '[') {
    const block = groupToken(source, pos + 1, ctx);
    return block ? { ...block, from: pos } : null;
  }

  const keyword = sticky(STATEMENT, source, pos + 1);
  if (keyword) return statementToken(source, pos, keyword[1]);

  return callToken(source, pos, ctx, 1);
};

// Calls in code mode, such as `f(x)` or `rgb("#fff")`, without a leading `#`.
export const call: Rule = (source, pos, ctx) => {
  if (!/[a-zA-Z]/.test(source[pos]) || /[\w.-]/.test(source[pos - 1] ?? '')) return null;
  return callToken(source, pos, ctx, 0);
};

export const stringLiteral: Rule = (source, pos) => {
  if (source[pos] !== '"') return null;
  const end = skipString(source, pos, Math.min(source.length, pos + MAX_SPAN));
  return end < 0 ? null : { kind: 'raw', name: 'string', from: pos, to: end, body: { from: pos + 1, to: end - 1 } };
};

export const namedArgument: Rule = (source, pos) => {
  const match = sticky(NAMED_RAW, source, pos);
  return match ? { kind: 'raw', name: 'argument', from: pos, to: pos + match[0].length } : null;
};

export const separator: Rule = (source, pos) => {
  const match = sticky(SEPARATOR, source, pos);
  return match ? { kind: 'raw', name: 'separator', from: pos, to: pos + match[0].length } : null;
};

// A `[...]` argument or content block in code. Its body is markup.
export const contentBlock: Rule = (source, pos, ctx) => (source[pos] === '[' ? groupToken(source, pos, ctx) : null);

// `@key` and `@key[supplement]`. The supplement range is options[0].
export const reference: Rule = (source, pos) => {
  if (source[pos] !== '@' || isWord(source[pos - 1])) return null;
  const match = sticky(REF, source, pos);
  if (!match) return null;

  const token: Token = {
    kind: 'command',
    name: 'ref',
    from: pos,
    to: pos + match[0].length,
    meta: { key: match[1] }
  };
  if (source[token.to] === '[') {
    const end = skipGroup(source, token.to, source.length);
    if (end > 0) {
      token.options = [{ from: token.to + 1, to: end - 1 }];
      token.to = end;
    }
  }
  return token;
};

export const label: Rule = (source, pos) => {
  if (source[pos] !== '<') return null;
  const match = sticky(LABEL, source, pos);
  return match
    ? { kind: 'command', name: 'label', from: pos, to: pos + match[0].length, meta: { key: match[1] } }
    : null;
};

// Bare URLs. Trailing sentence punctuation is not part of the link.
export const url: Rule = (source, pos) => {
  if ((source[pos] !== 'h' && source[pos] !== 'f') || isWord(source[pos - 1])) return null;
  const match = sticky(URL, source, pos);
  if (!match) return null;

  const text = match[0].replace(/[.,;:!?]+$/, '');
  if (text.length <= match[0].indexOf('://') + 3) return null;
  return { kind: 'command', name: 'url', from: pos, to: pos + text.length, args: [{ from: pos, to: pos + text.length }] };
};

// `--`, `---` and `...` are typeset as dash and ellipsis glyphs.
export const ligature: Rule = (source, pos) => {
  for (const [input, glyph] of LIGATURES) {
    if (source.startsWith(input, pos)) {
      return { kind: 'ligature', from: pos, to: pos + input.length, meta: { text: glyph } };
    }
  }
  return null;
};

// `~` is a non-breaking space.
export const tie: Rule = (source, pos) => (source[pos] === '~' ? { kind: 'tie', from: pos, to: pos + 1 } : null);

// Smart quotes: an opening quote follows whitespace, an opening bracket or a dash.
export const quote: Rule = (source, pos) => {
  const char = source[pos];
  if (char !== '"' && char !== "'") return null;
  const before = source[pos - 1];
  const opening = before === undefined || isSpace(before) || /[([{\-\u2013\u2014]/.test(before);
  const glyph = char === '"'
    ? (opening ? '\u201C' : '\u201D')
    : (opening ? '\u2018' : '\u2019');
  return { kind: 'ligature', name: 'quote', from: pos, to: pos + 1, meta: { text: glyph } };
};

function inMode(mode: ParseMode, rule: Rule): Rule {
  return (source, pos, ctx) => (ctx.mode === mode ? rule(source, pos, ctx) : null);
}

export const rules: Rule[] = [
  comment,
  inMode('markup', linebreak),
  escape,
  inMode('code', stringLiteral),
  inMode('code', namedArgument),
  inMode('code', separator),
  inMode('code', contentBlock),
  inMode('code', math),
  inMode('code', call),
  inMode('markup', raw),
  inMode('markup', heading),
  inMode('markup', math),
  inMode('markup', code),
  inMode('markup', emphasis),
  inMode('markup', item),
  inMode('markup', ligature),
  inMode('markup', tie),
  inMode('markup', quote),
  inMode('markup', url),
  inMode('markup', reference),
  inMode('markup', label)
];
