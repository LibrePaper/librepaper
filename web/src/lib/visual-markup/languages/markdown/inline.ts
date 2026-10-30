import { Rule, RuleContext } from '../../core/language';
import { Range, Token } from '../../core/tokens';
import { FIELD, STATE_OF, TAG, TASK_LINE, validateField } from '../../../tasks/syntax.js';
import {
  closingRun, delimiterAt, findBacktickRun, lineEndAt, lineStartAt, matchClose,
  paragraphEnd, parseAttributes, runLength
} from './scan';

// ASCII punctuation, the characters a backslash can escape.
const ESCAPABLE = /[!-\/:-@\[-`{-~]/;
const XREF_PREFIX = /^(fig|tbl|sec|eq|thm|lem|cor|prp|cnj|def|exm|exr|sol|rem|lst)-/i;
const CITE_KEY = /[A-Za-z0-9_](?:[A-Za-z0-9_:.\-]*[A-Za-z0-9_])?/y;
const CITE_IN_BRACKETS = /(?:^|[\s;[])-?@[A-Za-z0-9_]/;
const BRACKET_CITE = /^(-?)@([A-Za-z0-9_](?:[A-Za-z0-9_:.\-]*[A-Za-z0-9_])?)$/;
const FOOTNOTE_REF = /\[\^([^\]\s^]+)\]/y;
const ATTRIBUTE_BLOCK = /\{([^{}\n]*)\}/y;
const AUTOLINK = /<((?:https?|ftp|mailto):[^\s<>]*)>/y;
const BARE_URL = /https?:\/\/[^\s<]+/y;

// Inline token wrapping children parsed from the body, with the markers concealed by default.
function wrap(ctx: RuleContext, name: string, from: number, to: number, bodyFrom: number, bodyTo: number): Token {
  return {
    kind: 'command',
    name,
    from,
    to,
    body: { from: bodyFrom, to: bodyTo },
    children: ctx.parse(bodyFrom, bodyTo)
  };
}

export const linebreak: Rule = (source, pos) => {
  if (source[pos] === '\\' && source[pos + 1] === '\n') return { kind: 'linebreak', from: pos, to: pos + 1 };
  // Two or more trailing spaces break the line. Only the first space of a run is tested.
  if (source[pos] !== ' ' || source[pos - 1] === ' ') return null;
  let end = pos;
  while (source[end] === ' ') end++;
  if (end - pos < 2 || source[end] !== '\n') return null;
  // A blank next line ends the paragraph, so there is no break to draw.
  if (source.slice(end + 1, lineEndAt(source, end + 1)).trim() === '') return null;
  return { kind: 'linebreak', from: pos, to: end };
};

// A backslash before ASCII punctuation shows the character alone.
export const escape: Rule = (source, pos) => {
  if (source[pos] !== '\\') return null;
  const next = source[pos + 1];
  if (!next || !ESCAPABLE.test(next)) return null;
  return { kind: 'raw', name: 'escape', from: pos, to: pos + 2, meta: { text: next } };
};

// Code span. It is matched before anything inside it, so its text is never parsed.
// A backtick run is claimed at its first character only.
export const code: Rule = (source, pos) => {
  if (source[pos] !== '`' || source[pos - 1] === '`') return null;
  const end = paragraphEnd(source, pos);
  const count = runLength(source, pos, '`', end);
  const close = findBacktickRun(source, pos + count, count, end);
  if (close === -1) return null;

  let from = pos + count;
  let to = close;
  const content = source.slice(from, to);
  // One space is stripped from each side when both are present, so `` ` x ` `` shows x.
  if (content.length >= 2 && content[0] === ' ' && content[content.length - 1] === ' ' && content.trim() !== '') {
    from++;
    to--;
  }
  return { kind: 'command', name: 'code', from: pos, to: close + count, body: { from, to } };
};

// Display math `$$ ... $$` (may span lines within the paragraph) with an optional {#eq-x},
// and inline math `$...$` following Pandoc: no space inside the delimiters, and the
// closing dollar is not followed by a digit, so prices stay text.
export const math: Rule = (source, pos) => {
  if (source[pos] !== '$') return null;
  return source[pos + 1] === '$' ? displayMath(source, pos) : inlineMath(source, pos);
};

function displayMath(source: string, pos: number): Token | null {
  const end = paragraphEnd(source, pos, false);
  const close = source.slice(pos + 2, end).indexOf('$$');
  if (close === -1 || close === 0) return null;
  const bodyTo = pos + 2 + close;
  let to = bodyTo + 2;
  const meta: Record<string, string> = { open: '$$', close: '$$' };

  let cursor = to;
  while (source[cursor] === ' ' || source[cursor] === '\t') cursor++;
  ATTRIBUTE_BLOCK.lastIndex = cursor;
  const attr = ATTRIBUTE_BLOCK.exec(source);
  if (attr) {
    const attributes = parseAttributes(attr[1]);
    if (attributes.id !== undefined) {
      meta.id = attributes.id;
      to = ATTRIBUTE_BLOCK.lastIndex;
    }
  }
  return { kind: 'math', from: pos, to, display: true, body: { from: pos + 2, to: bodyTo }, meta };
}

function inlineMath(source: string, pos: number): Token | null {
  const next = source[pos + 1];
  if (next === undefined || /\s/.test(next)) return null;
  const end = paragraphEnd(source, pos);
  for (let index = pos + 1; index < end; index++) {
    const char = source[index];
    if (char === '\\') {
      index++;
      continue;
    }
    if (char !== '$') continue;
    if (/\s/.test(source[index - 1])) continue;
    if (/[0-9]/.test(source[index + 1] ?? '')) continue;
    return {
      kind: 'math',
      from: pos,
      to: index + 1,
      display: false,
      body: { from: pos + 1, to: index },
      meta: { open: '$', close: '$' }
    };
  }
  return null;
}

// HTML comment. The markers are concealed, the text is drawn as a comment.
export const comment: Rule = (source, pos) => {
  if (!source.startsWith('<!--', pos)) return null;
  const close = source.indexOf('-->', pos + 4);
  if (close === -1) return null;
  return { kind: 'comment', from: pos, to: close + 3, body: { from: pos + 4, to: close } };
};

// True when nothing but whitespace shares the line with the range.
function isAlone(source: string, from: number, to: number): boolean {
  return source.slice(lineStartAt(source, from), from).trim() === '' && source.slice(to, lineEndAt(source, to)).trim() === '';
}

// Quarto shortcode {{< name args >}}, within the paragraph.
export const shortcode: Rule = (source, pos) => {
  if (!source.startsWith('{{<', pos)) return null;
  const end = paragraphEnd(source, pos);
  const close = source.slice(pos + 3, end).indexOf('>}}');
  if (close === -1) return null;
  const content = source.slice(pos + 3, pos + 3 + close).trim();
  const name = /^[\w-]+/.exec(content)?.[0] ?? '';
  return {
    kind: 'command',
    name: 'shortcode',
    from: pos,
    to: pos + 3 + close + 3,
    meta: { name, args: content.slice(name.length).trim(), alone: String(isAlone(source, pos, pos + 3 + close + 3)) }
  };
};

// Destination of an inline link at `open` (the parenthesis): the URL and the end of the
// closing parenthesis. An optional title is ignored.
function linkTarget(source: string, open: number, end: number): { url: Range; to: number } | null {
  const close = matchClose(source, open, '(', ')', end);
  if (close === -1) return null;
  let from = open + 1;
  while (from < close && /\s/.test(source[from])) from++;
  if (source[from] === '<') {
    const angle = source.indexOf('>', from);
    if (angle === -1 || angle > close) return null;
    return { url: { from: from + 1, to: angle }, to: close + 1 };
  }
  let to = from;
  while (to < close && !/\s/.test(source[to])) to++;
  return { url: { from, to }, to: close + 1 };
}

// True when the image is the only content of its line and the lines around it are blank
// (or a block boundary): then it is a figure, drawn as a block.
function isFigure(source: string, pos: number, to: number): boolean {
  const start = lineStartAt(source, pos);
  if (source.slice(start, pos).trim() !== '') return false;
  const lineEnd = lineEndAt(source, to);
  if (source.slice(to, lineEnd).trim() !== '') return false;

  const previous = start > 0 ? source.slice(lineStartAt(source, start - 1), start - 1) : '';
  const next = lineEnd < source.length ? source.slice(lineEnd + 1, lineEndAt(source, lineEnd + 1)) : '';
  const blockBefore = previous.trim() === '' || /^\s*(#{1,6}[ \t]|`{3,}|~{3,}|:{3,})/.test(previous);
  const blockAfter = next.trim() === '' || /^\s*(`{3,}|~{3,}|:{3,})/.test(next);
  return blockBefore && blockAfter;
}

// Image `![alt](src){#fig-x width=50%}`: a figure when alone on its lines, otherwise inline.
export const image: Rule = (source, pos) => {
  if (source[pos] !== '!' || source[pos + 1] !== '[') return null;
  const end = paragraphEnd(source, pos);
  const close = matchClose(source, pos + 1, '[', ']', end);
  if (close === -1 || source[close + 1] !== '(') return null;
  const target = linkTarget(source, close + 1, end);
  if (!target) return null;

  let to = target.to;
  let attrs = '';
  ATTRIBUTE_BLOCK.lastIndex = to;
  const attr = ATTRIBUTE_BLOCK.exec(source);
  if (attr) {
    attrs = attr[1];
    to = ATTRIBUTE_BLOCK.lastIndex;
  }

  const meta: Record<string, string> = {
    src: source.slice(target.url.from, target.url.to),
    alt: source.slice(pos + 2, close),
    attrs
  };
  const attributes = parseAttributes(attrs);
  if (attributes.id !== undefined) meta.id = attributes.id;
  if (attributes.classes.length) meta.classes = attributes.classes.join(' ');

  return { kind: 'command', name: isFigure(source, pos, to) ? 'mdfigure' : 'mdimage', from: pos, to, meta };
};

// Bracketed text: footnote reference [^1], citation [@a; @b], cross-reference [-@fig-x],
// link [text](url), reference link [text][ref] and span [text]{.class}.
export const bracket: Rule = (source, pos, ctx) => {
  if (source[pos] !== '[') return null;

  if (source[pos + 1] === '^') {
    FOOTNOTE_REF.lastIndex = pos;
    const ref = FOOTNOTE_REF.exec(source);
    return ref ? { kind: 'command', name: 'fnref', from: pos, to: pos + ref[0].length, meta: { label: ref[1] } } : null;
  }

  const end = paragraphEnd(source, pos);
  const close = matchClose(source, pos, '[', ']', end);
  if (close === -1) return null;
  const after = source[close + 1];
  const text: Range = { from: pos + 1, to: close };

  if (after !== '(' && after !== '[' && after !== '{') {
    const content = source.slice(text.from, text.to);
    if (CITE_IN_BRACKETS.test(content)) {
      const single = BRACKET_CITE.exec(content.trim());
      const prefix = single ? XREF_PREFIX.exec(single[2]) : null;
      if (single && prefix) {
        const kind = prefix[1].toLowerCase();
        const meta: Record<string, string> = { key: kind + single[2].slice(prefix[1].length), prefix: kind };
        if (single[1] === '-') meta.bare = 'true';
        if (single[2][0] !== single[2][0].toLowerCase()) meta.capital = 'true';
        return { kind: 'command', name: 'xref', from: pos, to: close + 1, meta };
      }
      return {
        kind: 'command',
        name: 'mdcite',
        from: pos,
        to: close + 1,
        body: text,
        meta: { text: content, bracketed: 'true' }
      };
    }
  }

  if (after === '(') {
    const target = linkTarget(source, close + 1, end);
    if (!target) return null;
    return {
      kind: 'command',
      name: 'href',
      from: pos,
      to: target.to,
      body: text,
      args: [target.url, text],
      children: ctx.parse(text.from, text.to)
    };
  }

  if (after === '[') {
    const labelClose = matchClose(source, close + 1, '[', ']', end);
    if (labelClose === -1) return null;
    // [text][] uses the text as its own label.
    const label: Range = labelClose === close + 2 ? text : { from: close + 2, to: labelClose };
    return {
      kind: 'command',
      name: 'href',
      from: pos,
      to: labelClose + 1,
      body: text,
      args: [label, text],
      meta: { ref: 'true' },
      children: ctx.parse(text.from, text.to)
    };
  }

  if (after === '{') {
    ATTRIBUTE_BLOCK.lastIndex = close + 1;
    const attr = ATTRIBUTE_BLOCK.exec(source);
    if (!attr) return null;
    const attributes = parseAttributes(attr[1]);
    const meta: Record<string, string> = { classes: attributes.classes.join(' ') };
    if (attributes.id !== undefined) meta.id = attributes.id;
    return {
      kind: 'command',
      name: 'span',
      from: pos,
      to: ATTRIBUTE_BLOCK.lastIndex,
      body: text,
      children: ctx.parse(text.from, text.to),
      meta
    };
  }

  return null;
};

// @key is a citation, @fig-x a cross-reference. An @ inside a word (an email address) is text.
export const cite: Rule = (source, pos) => {
  if (source[pos] !== '@') return null;
  if (pos > 0 && /[A-Za-z0-9_]/.test(source[pos - 1])) return null;
  CITE_KEY.lastIndex = pos + 1;
  const key = CITE_KEY.exec(source);
  if (!key) return null;

  const word = key[0];
  const to = pos + 1 + word.length;
  const prefix = XREF_PREFIX.exec(word);
  if (prefix) {
    const kind = prefix[1].toLowerCase();
    const meta: Record<string, string> = { key: kind + word.slice(prefix[1].length), prefix: kind };
    if (word[0] !== word[0].toLowerCase()) meta.capital = 'true';
    return { kind: 'command', name: 'xref', from: pos, to, meta };
  }
  return {
    kind: 'command',
    name: 'mdcite',
    from: pos,
    to,
    meta: { text: source.slice(pos, to), bracketed: 'false' }
  };
};

// Autolink <https://...>: the brackets are concealed, the address is the body.
export const autolink: Rule = (source, pos) => {
  if (source[pos] !== '<') return null;
  AUTOLINK.lastIndex = pos;
  const match = AUTOLINK.exec(source);
  if (!match) return null;
  const url: Range = { from: pos + 1, to: pos + 1 + match[1].length };
  return { kind: 'command', name: 'url', from: pos, to: pos + match[0].length, body: url, args: [url] };
};

// Bare https:// address. Trailing punctuation and an unbalanced parenthesis are the sentence's.
export const bareUrl: Rule = (source, pos) => {
  if (source[pos] !== 'h' || (pos > 0 && /[A-Za-z0-9]/.test(source[pos - 1]))) return null;
  BARE_URL.lastIndex = pos;
  const match = BARE_URL.exec(source);
  if (!match) return null;

  let text = match[0].replace(/[.,;:!?'"*_~]+$/, '');
  while (text.endsWith(')') && text.split('(').length < text.split(')').length) text = text.slice(0, -1);
  if (text.length <= 'https://'.length) return null;
  const url: Range = { from: pos, to: pos + text.length };
  return { kind: 'command', name: 'url', from: pos, to: url.to, body: url, args: [url] };
};

// Task metadata on the first line of a task item: `key:value` fields and `#tags` after the box.
// Both need a space or tab before them. Code, links, URLs and escapes are matched by their own
// rules before this one reaches their text, so their contents are never metadata.
export const taskmeta: Rule = (source, pos) => {
  if (pos === 0 || (source[pos - 1] !== ' ' && source[pos - 1] !== '\t')) return null;
  FIELD.lastIndex = pos;
  const field = FIELD.exec(source);
  TAG.lastIndex = pos;
  const tag = field ? null : TAG.exec(source);
  if (!field && !tag) return null;
  // A tag ends where its name ends, so `#paper.` is the tag `paper` and `#paper#x` is not a tag.
  if (tag && !taskTagEnd(source[pos + tag[0].length])) return null;

  const lineStart = lineStartAt(source, pos);
  const head = TASK_LINE.exec(source.slice(lineStart, lineEndAt(source, pos)));
  if (!head || pos <= lineStart + head[0].length) return null;
  const state = (STATE_OF as Record<string, string>)[head[2]];

  if (field) {
    const key = field[1];
    const gap = field[2];
    const value = field[3];
    const invalid = validateField(key, value);
    // A gap after the colon makes the text a field only when the value is valid, so prose such as
    // "done: the tables" stays text. Without a gap an invalid field is still a field, with a warning.
    if (gap !== '' && invalid !== null) return null;
    const boxEnd = lineStart + head[0].length;
    const warning = invalid ?? (taskKeyBefore(source, boxEnd, pos, key) ? `Duplicate ${key}.` : null);
    const meta: Record<string, string> = { key, value, state };
    if (warning) meta.warning = warning;
    return { kind: 'command', name: 'taskfield', from: pos, to: pos + field[0].length, meta };
  }
  return { kind: 'command', name: 'tasktag', from: pos, to: pos + tag[0].length, meta: { tag: tag[0].slice(1) } };
};

// A tag name ends before whitespace, the end of the input or closing punctuation.
function taskTagEnd(char: string | undefined): boolean {
  return char === undefined || /[\s.,;:!?)]/.test(char);
}

// True when `key:` starts a field between from and to, after a space or tab. An earlier field
// written with a gap counts only when its value is valid, the same rule that makes it a field.
function taskKeyBefore(source: string, from: number, to: number, key: string): boolean {
  for (let index = from + 1; index < to; index++) {
    if (source[index - 1] !== ' ' && source[index - 1] !== '\t') continue;
    FIELD.lastIndex = index;
    const earlier = FIELD.exec(source);
    if (!earlier || earlier[1] !== key) continue;
    if (earlier[2] === '' || validateField(key, earlier[3]) === null) return true;
  }
  return false;
}

// Emphasis (* and _), strong, strikethrough (~~), subscript (~), superscript (^),
// mark (==) and inline footnotes (^[note]). Only the first character of a run is tested.
export const emphasis: Rule = (source, pos, ctx) => {
  const marker = source[pos];
  if (marker !== '*' && marker !== '_' && marker !== '~' && marker !== '=' && marker !== '^') return null;
  if (source[pos - 1] === marker) return null;

  if (marker === '^' && source[pos + 1] === '[') {
    const end = paragraphEnd(source, pos);
    const close = matchClose(source, pos + 1, '[', ']', end);
    if (close === -1) return null;
    return wrap(ctx, 'fnref', pos, close + 1, pos + 2, close);
  }

  const end = paragraphEnd(source, pos);
  const count = runLength(source, pos, marker, end);

  // Sub and superscript: one marker, no space inside, closed by the next marker.
  if (marker === '^' || (marker === '~' && count === 1)) {
    if (count !== 1 || /\s/.test(source[pos + 1] ?? ' ')) return null;
    for (let index = pos + 2; index < end; index++) {
      const char = source[index];
      if (char === '\\') {
        index++;
        continue;
      }
      if (/\s/.test(char)) return null;
      if (char === marker) return wrap(ctx, marker === '^' ? 'sup' : 'sub', pos, index + 1, pos + 1, index);
    }
    return null;
  }

  const underscore = marker === '_';
  if (!delimiterAt(source, pos, pos + count, underscore).open) return null;

  if ((marker === '~' || marker === '=') && count === 2) {
    const close = closingRun(source, pos + count, marker, count, end, false);
    if (close === -1) return null;
    return wrap(ctx, marker === '~' ? 'strike' : 'mark', pos, close + 2, pos + 2, close);
  }

  if (marker !== '*' && marker !== '_') return null;

  if (count === 1) {
    const close = closingRun(source, pos + 1, marker, 1, end, underscore);
    if (close === -1) return null;
    return wrap(ctx, 'emph', pos, close + 1, pos + 1, close);
  }

  if (count === 2) {
    const close = closingRun(source, pos + 2, marker, 2, end, underscore);
    if (close === -1) return null;
    return wrap(ctx, 'strong', pos, close + 2, pos + 2, close);
  }

  // ***x***: emphasis around strong, with the markers of each concealed by their own tokens.
  if (count === 3) {
    const close = closingRun(source, pos + 3, marker, 3, end, underscore);
    if (close === -1) return null;
    const strong = wrap(ctx, 'strong', pos + 1, close + 2, pos + 3, close);
    return {
      kind: 'command',
      name: 'emph',
      from: pos,
      to: close + 3,
      body: { from: pos + 1, to: close + 2 },
      children: [strong]
    };
  }
  return null;
};
