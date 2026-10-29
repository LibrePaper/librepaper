// Lookahead for a closing delimiter. A span never crosses a blank line, and the search
// stops after this many characters, so each opener costs a constant amount of work and
// the whole document is tokenized in time linear in its length.
export const LOOKAHEAD = 2000;

// The tokenizer hands every rule the same source string, so the blank-line index is
// computed once per source and reused for every position.
let indexedSource = '';
let blankBreaks: number[] = [];

// Newlines that end a line followed by a blank line: the boundaries of paragraphs.
function paragraphBreaks(source: string): number[] {
  if (source !== indexedSource) {
    indexedSource = source;
    blankBreaks = [];
    const pattern = /\n(?=[ \t]*\n)/g;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(source))) blankBreaks.push(match.index);
  }
  return blankBreaks;
}

// End of the paragraph that holds pos. Capped by LOOKAHEAD unless `capped` is false
// (display math may run longer than an inline span, still within one paragraph).
export function paragraphEnd(source: string, pos: number, capped = true): number {
  const breaks = paragraphBreaks(source);
  let low = 0;
  let high = breaks.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (breaks[middle] < pos) low = middle + 1;
    else high = middle;
  }
  const end = low < breaks.length ? breaks[low] : source.length;
  return capped ? Math.min(end, pos + LOOKAHEAD) : end;
}

export function lineStartAt(source: string, pos: number): number {
  return pos === 0 ? 0 : source.lastIndexOf('\n', pos - 1) + 1;
}

export function lineEndAt(source: string, pos: number): number {
  const end = source.indexOf('\n', pos);
  return end === -1 ? source.length : end;
}

// True when only spaces, tabs and block quote markers precede pos on its line. Block
// constructs start there, and inside a quote their content is parsed after the markers.
export function atBlockStart(source: string, pos: number): boolean {
  for (let index = pos - 1; index >= 0; index--) {
    const char = source[index];
    if (char === '\n') return true;
    if (char !== ' ' && char !== '\t' && char !== '>') return false;
  }
  return true;
}

// True when only spaces and tabs precede pos on its line. Block widgets need this:
// a block replacement cannot start in the middle of a line.
export function atLineHead(source: string, pos: number): boolean {
  for (let index = pos - 1; index >= 0; index--) {
    const char = source[index];
    if (char === '\n') return true;
    if (char !== ' ' && char !== '\t') return false;
  }
  return true;
}

// Text of the nearest non-blank line before `start` (a line start), or null.
export function previousNonBlank(source: string, start: number): string | null {
  let end = start - 1;
  while (end > 0) {
    const from = source.lastIndexOf('\n', end - 1) + 1;
    const text = source.slice(from, end);
    if (text.trim() !== '') return text;
    end = from - 1;
  }
  return null;
}

export function runLength(source: string, pos: number, char: string, end = source.length): number {
  let index = pos;
  while (index < end && source[index] === char) index++;
  return index - pos;
}

const PUNCTUATION = /[\p{P}\p{S}]/u;
const SPACE = /\s/;

// Flanking of a delimiter run from start to end (CommonMark). `open` and `close` say
// whether the run may open or close emphasis. Underscores follow the stricter rule
// that keeps snake_case_name from emphasis.
export function delimiterAt(source: string, start: number, end: number, underscore: boolean): { open: boolean; close: boolean } {
  const before = start > 0 ? source[start - 1] : '\n';
  const after = end < source.length ? source[end] : '\n';
  const beforeSpace = SPACE.test(before);
  const afterSpace = SPACE.test(after);
  const beforePunct = PUNCTUATION.test(before);
  const afterPunct = PUNCTUATION.test(after);
  const left = !afterSpace && (!afterPunct || beforeSpace || beforePunct);
  const right = !beforeSpace && (!beforePunct || afterSpace || afterPunct);
  if (underscore) {
    return { open: left && (!right || beforePunct), close: right && (!left || afterPunct) };
  }
  return { open: left, close: right };
}

// Start of the next backtick run of exactly `count` characters, or -1.
export function findBacktickRun(source: string, from: number, count: number, end: number): number {
  let index = from;
  while (index < end) {
    if (source[index] === '`') {
      const run = runLength(source, index, '`', end);
      if (run === count) return index;
      index += run;
    } else {
      index++;
    }
  }
  return -1;
}

// Start of the run that closes an opener of `count` markers, searched from `from`
// (just after the opener), or -1. Nested openers of the same kind are counted, so
// `*a *b* c*` closes at the last star. Code spans are skipped, so markers inside
// them never close emphasis.
export function closingRun(source: string, from: number, marker: string, count: number, end: number, underscore: boolean): number {
  let depth = 0;
  let index = from;
  while (index < end) {
    const char = source[index];
    if (char === '\\') {
      index += 2;
      continue;
    }
    if (char === '`') {
      const run = runLength(source, index, '`', end);
      const close = findBacktickRun(source, index + run, run, end);
      index = close === -1 ? index + run : close + run;
      continue;
    }
    if (char !== marker) {
      index++;
      continue;
    }
    const run = runLength(source, index, marker, end);
    if (run === count) {
      const flags = delimiterAt(source, index, index + run, underscore);
      if (flags.close && depth === 0) return index;
      if (flags.close) depth--;
      else if (flags.open) depth++;
    }
    index += run;
  }
  return -1;
}

// Index of the bracket that closes the one at pos, or -1. The search stops at `end`.
export function matchClose(source: string, pos: number, open: string, close: string, end: number): number {
  let depth = 0;
  for (let index = pos; index < end; index++) {
    const char = source[index];
    if (char === '\\') {
      index++;
      continue;
    }
    if (char === open) depth++;
    else if (char === close && --depth === 0) return index;
  }
  return -1;
}

export interface Attributes {
  id?: string;
  classes: string[];
}

// Reads a Pandoc attribute list such as `#fig-x .unnumbered width=50%`. Key values are
// consumed first, so a dot inside a title does not read as a class.
export function parseAttributes(text: string): Attributes {
  const attributes: Attributes = { classes: [] };
  const pattern = /([\w-]+)=("[^"]*"|[^\s"}]*)|#([\w:.-]+)|\.([\w-]+)/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(text))) {
    if (match[3] !== undefined) {
      if (attributes.id === undefined) attributes.id = match[3];
    } else if (match[4] !== undefined) {
      attributes.classes.push(match[4]);
    }
  }
  return attributes;
}
