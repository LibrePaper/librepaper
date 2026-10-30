import { Rule } from '../../core/language';
import { Range, Token } from '../../core/tokens';
import { inner, isEscaped, matchDelimited, matchFenced, matchLine, skipSpace } from '../../core/scanner';
import { ARGUMENT_COUNT, ESCAPABLE, HEADING_LEVELS, LITERAL_ARGUMENT_COMMANDS, MATH_ENVIRONMENTS, TABLE_ENVIRONMENT_ARGS, TABLE_ENVIRONMENTS, VERBATIM_ENVIRONMENTS } from './commands';
import { COMMANDS, ENVIRONMENTS } from './catalog';
import { FIGURE_ENVIRONMENTS, isCompositeFigure, latexDimension } from './figure';

const MATH_DELIMITERS: Array<[string, string, boolean]> = [
  ['$$', '$$', true],
  ['\\[', '\\]', true],
  ['\\(', '\\)', false],
  ['$', '$', false]
];

// Sticky patterns match at `lastIndex` only, so a rule never copies the rest of the source.
const COMMAND_NAME = /\\([a-zA-Z]+)/y;
const HEADING_NAME = /\\([a-zA-Z]+)(\*?)/y;
const BEGIN_HEADER = /\\begin\{([^}\n]+)\}/y;
const VERB_COMMAND = /\\(verb|Verb|lstinline|mintinline)(?![a-zA-Z])/y;
const OPTIONAL_ARGUMENT = /\[[^\]\n]*\]/y;
// Accents written with a symbol (\'e) or with a letter followed by a space or braces (\v{s}, \c c).
const ACCENT_COMMAND = /\\(?:(['`^"~=.])|([cvHubdrkt])(?![a-zA-Z]))/y;
const ACCENT_BASE = /\\[ij](?![a-zA-Z])|[a-zA-Z]/y;

const ACCENT_MARKS: Record<string, string> = {
  "'": '́', '`': '̀', '^': '̂', '"': '̈', '~': '̃', '=': '̄', '.': '̇',
  c: '̧', v: '̌', H: '̋', u: '̆', b: '̱', d: '̣', r: '̊', k: '̨', t: '͡'
};

// Longer inputs come first so that `---` wins over `--`, and `''` over `'`.
const LIGATURES: Array<[string, string]> = [
  ['---', '—'], ['--', '–'], ['``', '“'], ["''", '”'], ['`', '‘'], ["'", '’']
];

interface Span {
  body: Range;
  to: number;
}

interface Arguments {
  args: Range[];
  options: Range[];
  star: boolean;
  to: number;
}

export const comment: Rule = (source, pos) => {
  if (source[pos] !== '%' || isEscaped(source, pos)) return null;
  const line = matchLine(source, pos);
  return { kind: 'comment', from: pos, to: line.to, body: { from: pos + 1, to: line.to } };
};

export const escape: Rule = (source, pos) => {
  if (source[pos] !== '\\') return null;
  const next = source[pos + 1];
  if (!next || !ESCAPABLE.has(next)) return null;
  return { kind: 'raw', from: pos, to: pos + 2 };
};

export const math: Rule = (source, pos, ctx) => {
  for (const [open, close, display] of MATH_DELIMITERS) {
    if (!source.startsWith(open, pos)) continue;
    if (open === '$' && source.startsWith('$$', pos)) continue;
    const range = matchFenced(source, pos, open, close);
    if (!range) continue;
    const body = inner(range, open.length, close.length);
    if (body.to <= body.from) continue;
    if (/\n[ \t]*\n/.test(source.slice(body.from, body.to))) continue;
    return { kind: 'math', from: range.from, to: range.to, display, body, meta: { open, close } };
  }
  return null;
};

export const environment: Rule = (source, pos, ctx) => {
  if (!source.startsWith('\\begin{', pos)) return null;

  BEGIN_HEADER.lastIndex = pos;
  const header = BEGIN_HEADER.exec(source);
  if (!header) return null;

  const name = header[1];
  const argsFrom = pos + header[0].length;
  const end = findMatchingEnd(source, argsFrom, name);
  if (end === -1) return null;

  const spec = ENVIRONMENTS.get(name) ?? ENVIRONMENTS.get(name.replace(/\*$/, ''));
  const tableArgs = TABLE_ENVIRONMENT_ARGS.get(name);
  // The star is part of the environment name here, so the signature has no `s`.
  const parsed = spec
    ? parseSignature(source, argsFrom, spec.args.replace('s', ''))
    : consumeArguments(source, argsFrom, tableArgs, tableArgs !== undefined);

  const token: Token = {
    kind: TABLE_ENVIRONMENTS.has(name) ? 'table' : 'container',
    name,
    from: pos,
    to: end + `\\end{${name}}`.length,
    body: { from: parsed.to, to: end },
    args: parsed.args
  };
  if (parsed.options.length) token.options = parsed.options;
  if (name.endsWith('*')) token.star = true;

  // Code and opaque environments keep their text out of the markup tree.
  const verbatim = VERBATIM_ENVIRONMENTS.has(name) || spec?.render.kind === 'code' || spec?.render.kind === 'opaque';
  if (!verbatim) {
    token.children = ctx.parse(token.body!.from, token.body!.to);
  }
  if (MATH_ENVIRONMENTS.has(name)) {
    token.kind = 'math';
    token.display = true;
    token.children = undefined;
  } else if (FIGURE_ENVIRONMENTS.has(name)) {
    token.meta = { ...token.meta, figure: 'true' };
    if (name === 'figure*' || name === 'SCfigure*' || name === 'sidewaysfigure' || name === 'sidewaysfigure*') token.meta.figureWide = 'true';
    if ((name === 'wrapfigure' || name === 'wrapfigure*') && token.args?.[0]) {
      const side = source.slice(token.args[0].from, token.args[0].to).trim().toLowerCase();
      if (/l/.test(side)) token.meta.figureAlign = 'left';
      else if (/r/.test(side)) token.meta.figureAlign = 'right';
      if (token.args[1]) {
        const width = latexDimension(source.slice(token.args[1].from, token.args[1].to).trim());
        if (width) token.meta.figureWidth = width;
      }
    }
    if (isCompositeFigure(source, token)) token.meta.figureComposite = 'true';
  }

  return token;
};

export const heading: Rule = (source, pos, ctx) => {
  if (source[pos] !== '\\') return null;

  HEADING_NAME.lastIndex = pos;
  const match = HEADING_NAME.exec(source);
  if (!match) return null;

  const level = HEADING_LEVELS.get(match[1]);
  if (level === undefined) return null;

  const braces = matchDelimited(source, skipSpace(source, pos + match[0].length), '{');
  if (!braces) return null;

  const token: Token = {
    kind: 'heading',
    name: match[1],
    level,
    from: pos,
    to: braces.to,
    body: inner(braces, 1, 1)
  };
  // The title is markup: a tie, a dash or \emph inside it is drawn like body text.
  token.children = ctx.parse(token.body!.from, token.body!.to);
  if (match[2]) token.star = true;
  return token;
};

export const item: Rule = (source, pos) => {
  if (!source.startsWith('\\item', pos)) return null;
  if (/[a-zA-Z]/.test(source[pos + 5] || '')) return null;

  let to = pos + 5;
  const label = matchDelimited(source, to, '[');
  if (label) to = label.to;

  return { kind: 'item', from: pos, to, body: label ? inner(label, 1, 1) : undefined };
};

// Runs before the generic command rule so that the argument of \verb and friends is never parsed as markup.
export const verb: Rule = (source, pos) => {
  if (source[pos] !== '\\') return null;

  VERB_COMMAND.lastIndex = pos;
  const match = VERB_COMMAND.exec(source);
  if (!match) return null;

  const name = match[1];
  const verbName = name === 'verb' || name === 'Verb';
  let cursor = pos + match[0].length;
  const star = verbName && source[cursor] === '*';
  if (star) cursor++;

  const options: Range[] = [];
  const args: Range[] = [];
  if (!verbName) {
    const option = optionalAt(source, cursor);
    if (option) {
      options.push(option.body);
      cursor = option.to;
    }
  }
  if (name === 'mintinline') {
    const language = matchDelimited(source, cursor, '{');
    if (!language) return null;
    args.push(inner(language, 1, 1));
    cursor = language.to;
  }

  // \verb takes any delimiter, braces included; the other commands also accept a braced argument.
  const braced = !verbName && source[cursor] === '{';
  let body: Range;
  let to: number;
  if (braced) {
    const braces = matchDelimited(source, cursor, '{');
    if (!braces) return null;
    body = inner(braces, 1, 1);
    to = braces.to;
  } else {
    const delimited = delimitedAt(source, cursor);
    if (!delimited) return null;
    body = delimited.body;
    to = delimited.to;
  }

  const token: Token = { kind: 'verb', name, from: pos, to, body };
  if (args.length) token.args = args;
  if (options.length) token.options = options;
  if (star) token.star = true;
  return token;
};

export const accent: Rule = (source, pos) => {
  if (source[pos] !== '\\') return null;

  ACCENT_COMMAND.lastIndex = pos;
  const match = ACCENT_COMMAND.exec(source);
  if (!match) return null;

  const name = match[1] ?? match[2];
  // A letter accent may be followed by spaces before its base, so \c c works as well as \c{c}.
  let cursor = pos + match[0].length;
  if (match[2] !== undefined) cursor = skipSpace(source, cursor, true);

  let base: Range;
  let content: string;
  if (source[cursor] === '{') {
    const braces = matchDelimited(source, cursor, '{');
    if (!braces) return null;
    base = inner(braces, 1, 1);
    content = source.slice(base.from, base.to).trim();
    cursor = braces.to;
  } else {
    ACCENT_BASE.lastIndex = cursor;
    const letter = ACCENT_BASE.exec(source);
    if (!letter) return null;
    base = { from: cursor, to: cursor + letter[0].length };
    content = letter[0];
    cursor = base.to;
  }

  const glyph = baseGlyph(content);
  if (!glyph) return null;

  // The mark combines with the first letter; NFC merges the pair into one character where Unicode has one.
  const text = (glyph.charAt(0) + ACCENT_MARKS[name] + glyph.slice(1)).normalize('NFC');
  return { kind: 'accent', name, from: pos, to: cursor, body: base, meta: { text } };
};

// The table code reads the source for its rows, so these tokens do not change how tables are parsed.
export const linebreak: Rule = (source, pos) => {
  if (source[pos] !== '\\') return null;

  if (source[pos + 1] === '\\') {
    let to = pos + 2;
    if (source[to] === '*') to++;
    const length = optionalAt(source, to);
    if (length) to = length.to;
    return { kind: 'linebreak', from: pos, to };
  }

  if (source.startsWith('\\newline', pos) && !/[a-zA-Z]/.test(source[pos + 8] || '')) {
    return { kind: 'linebreak', from: pos, to: pos + 8 };
  }
  return null;
};

export const command: Rule = (source, pos, ctx) => {
  if (source[pos] !== '\\') return null;

  COMMAND_NAME.lastIndex = pos;
  const match = COMMAND_NAME.exec(source);
  if (!match) return null;

  const name = match[1];
  const afterName = pos + match[0].length;
  const spec = COMMANDS.get(name);

  let parsed: Arguments;
  if (spec) {
    parsed = parseSignature(source, afterName, spec.args);
  } else {
    // Without a catalog entry the old argument count applies; the star is read here, not folded into the name.
    const starred = source[afterName] === '*';
    const limit = ARGUMENT_COUNT.get(name) ?? (starred ? ARGUMENT_COUNT.get(`${name}*`) : undefined);
    parsed = { ...consumeArguments(source, starred ? afterName + 1 : afterName, limit), star: starred };
  }

  const body = parsed.args[parsed.args.length - 1];
  const token: Token = { kind: 'command', name, from: pos, to: parsed.to, body, args: parsed.args };
  if (parsed.options.length) token.options = parsed.options;
  if (parsed.star) token.star = true;

  if ((name === 'textcolor' || name === 'colorbox') && parsed.args[0]) {
    token.meta = { color: source.slice(parsed.args[0].from, parsed.args[0].to).trim() };
  } else if (name === 'fcolorbox' && parsed.args[1]) {
    token.meta = {
      borderColor: source.slice(parsed.args[0].from, parsed.args[0].to).trim(),
      color: source.slice(parsed.args[1].from, parsed.args[1].to).trim()
    };
  }
  // Kept beside the star flag for consumers that still compare names such as `caption*`.
  if (parsed.star) token.meta = { ...token.meta, starred: 'true' };

  if (body && !LITERAL_ARGUMENT_COMMANDS.has(name)) token.children = ctx.parse(body.from, body.to);
  return token;
};

// A bare group is a `{` that no command claimed as an argument. `\{` never gets here: escape runs first.
export const group: Rule = (source, pos, ctx) => {
  if (source[pos] !== '{') return null;
  const braces = matchDelimited(source, pos, '{');
  if (!braces) return null;
  const body = inner(braces, 1, 1);
  return { kind: 'group', from: pos, to: braces.to, body, children: ctx.parse(body.from, body.to) };
};

// Ligatures apply to text only. Math, comments and verbatim are consumed by earlier rules; `\-` and `\'` are escapes, so they are skipped here.
export const ligature: Rule = (source, pos) => {
  const char = source[pos];
  if (char !== '-' && char !== '`' && char !== "'") return null;
  if (isEscaped(source, pos)) return null;

  for (const [input, glyph] of LIGATURES) {
    if (!source.startsWith(input, pos)) continue;
    return { kind: 'ligature', from: pos, to: pos + input.length, meta: { text: glyph } };
  }
  return null;
};

// `tie` comes last, with `ligature`, so that math, comments and verbatim environments, which consume their whole range, claim their characters first.
export const tie: Rule = (source, pos) => {
  if (source[pos] !== '~' || isEscaped(source, pos)) return null;
  return { kind: 'tie', from: pos, to: pos + 1 };
};

// Order differs from the plan in one place: linebreak precedes escape, because escape would otherwise take `\\` as a raw token.
export const rules: Rule[] = [comment, math, environment, heading, item, verb, accent, linebreak, escape, command, group, ligature, tie];

// Reads an xparse-style signature: `s` an immediate star, `o` an optional [..], `m` a mandatory {..}.
// Stops at the first missing mandatory argument, as LaTeX does.
function parseSignature(source: string, pos: number, signature: string): Arguments {
  const args: Range[] = [];
  const options: Range[] = [];
  let star = false;
  let cursor = pos;

  for (const letter of signature) {
    if (letter === 's') {
      if (source[cursor] === '*') {
        star = true;
        cursor++;
      }
    } else if (letter === 'o') {
      const option = optionalAt(source, skipSpace(source, cursor, true));
      if (option) {
        options.push(option.body);
        cursor = option.to;
      }
    } else if (letter === 'm') {
      const braces = matchDelimited(source, skipSpace(source, cursor, true), '{');
      if (!braces) break;
      args.push(inner(braces, 1, 1));
      cursor = braces.to;
    }
  }

  return { args, options, star, to: cursor };
}

function consumeArguments(
  source: string,
  pos: number,
  limit?: number,
  trailingOptional = false
): Arguments {
  const args: Range[] = [];
  const options: Range[] = [];
  let cursor = pos;

  while (limit === undefined || args.length < limit) {
    const next = skipSpace(source, cursor, true);
    const optional = matchDelimited(source, next, '[');
    if (optional) {
      options.push(inner(optional, 1, 1));
      cursor = optional.to;
      continue;
    }
    const braces = matchDelimited(source, next, '{');
    if (!braces) break;
    args.push(inner(braces, 1, 1));
    cursor = braces.to;
  }

  if (trailingOptional) {
    while (true) {
      const optional = matchDelimited(source, skipSpace(source, cursor, true), '[');
      if (!optional) break;
      options.push(inner(optional, 1, 1));
      cursor = optional.to;
    }
  }

  return { args, options, star: false, to: cursor };
}

// A `[...]` that starts at pos and closes on the same line, so an unclosed bracket costs one line, not the file.
function optionalAt(source: string, pos: number): Span | null {
  if (source[pos] !== '[') return null;
  OPTIONAL_ARGUMENT.lastIndex = pos;
  const match = OPTIONAL_ARGUMENT.exec(source);
  if (!match) return null;
  const to = pos + match[0].length;
  return { body: { from: pos + 1, to: to - 1 }, to };
}

// A \verb-style delimiter at pos: any character except letters and spaces, closed on the same line.
function delimitedAt(source: string, pos: number): Span | null {
  const delimiter = source[pos];
  if (!delimiter || /[\sa-zA-Z]/.test(delimiter)) return null;
  const newline = source.indexOf('\n', pos + 1);
  const limit = newline === -1 ? source.length : newline;
  const close = source.slice(pos + 1, limit).indexOf(delimiter);
  if (close === -1) return null;
  const end = pos + 1 + close;
  return { body: { from: pos + 1, to: end }, to: end + 1 };
}

// The glyph an accent combines with: a run of letters, or the dotless i and j.
function baseGlyph(content: string): string | null {
  const dotless = /^\\([ij])$/.exec(content);
  if (dotless) return dotless[1] === 'i' ? 'ı' : 'ȷ';
  return /^[a-zA-Z]+$/.test(content) ? content : null;
}

function findMatchingEnd(source: string, from: number, name: string): number {
  const open = `\\begin{${name}}`;
  const close = `\\end{${name}}`;
  let depth = 1;
  let cursor = from;

  while (cursor < source.length && depth > 0) {
    const nextOpen = source.indexOf(open, cursor);
    const nextClose = source.indexOf(close, cursor);

    if (nextClose === -1) return -1;

    if (nextOpen !== -1 && nextOpen < nextClose) {
      depth++;
      cursor = nextOpen + open.length;
    } else {
      depth--;
      if (depth === 0) return nextClose;
      cursor = nextClose + close.length;
    }
  }

  return -1;
}
