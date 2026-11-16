import type { Range, Token } from '../../core/tokens';
import { ARGUMENT_COUNT, FIRST_ARGUMENT_COMMANDS, LITERAL_ARGUMENT_COMMANDS, MATH_ENVIRONMENTS, TABLE_ENVIRONMENT_ARGS, TABLE_ENVIRONMENTS, VERBATIM_ENVIRONMENTS } from './commands';
import { ENVIRONMENTS } from './catalog';
import { FIGURE_ENVIRONMENTS, isCompositeFigure, latexDimension } from './figure';

// Token shaping for the tree walker (tree.ts): the catalog data applied to what the walker found.
// Nothing here reads syntax from the source; it only slices the text of ranges the tree gave.

/** The arguments a command or environment took. `nodes` holds the tree node of each `args` entry. */
export interface Arguments {
  args: Range[];
  nodes: any[];
  options: Range[];
  star: boolean;
  to: number;
}

const ACCENT_MARKS: Record<string, string> = {
  "'": '́', '`': '̀', '^': '̂', '"': '̈', '~': '̃', '=': '̄', '.': '̇',
  c: '̧', v: '̌', H: '̋', u: '̆', b: '̱', d: '̣', r: '̊', k: '̨', t: '͡'
};

/** Accents written as a control symbol (\'e) and as a control word (\v{s}, \c c). */
export const SYMBOL_ACCENTS = new Set(["'", '`', '^', '"', '~', '=', '.']);
export const WORD_ACCENTS = new Set(['c', 'v', 'H', 'u', 'b', 'd', 'r', 'k', 't']);

const LIGATURE_GLYPHS: Record<string, string> = {
  '---': '—', '--': '–', '``': '“', "''": '”', '`': '‘', "'": '’'
};

export function ligatureToken(from: number, to: number, text: string): Token | null {
  const glyph = LIGATURE_GLYPHS[text];
  return glyph === undefined ? null : { kind: 'ligature', from, to, meta: { text: glyph } };
}

// The glyph an accent combines with: a run of letters, or the dotless i and j.
function baseGlyph(content: string): string | null {
  const dotless = /^\\([ij])$/.exec(content);
  if (dotless) return dotless[1] === 'i' ? 'ı' : 'ȷ';
  return /^[a-zA-Z]+$/.test(content) ? content : null;
}

/** An accent token over [from, to) whose base, written `content`, sits at `body`; null when the base is not a letter. */
export function accentToken(name: string, from: number, to: number, body: Range, content: string): Token | null {
  const glyph = baseGlyph(content);
  if (!glyph) return null;
  // The mark combines with the first letter; NFC merges the pair into one character where Unicode has one.
  const text = (glyph.charAt(0) + ACCENT_MARKS[name] + glyph.slice(1)).normalize('NFC');
  return { kind: 'accent', name, from, to, body, meta: { text } };
}

/** The signature letters of a command without a catalog entry: how many mandatory arguments it takes, or undefined for all adjacent ones. */
export function argumentLimit(name: string, starred: boolean): number | undefined {
  return ARGUMENT_COUNT.get(name) ?? (starred ? ARGUMENT_COUNT.get(`${name}*`) : undefined);
}

export function commandToken(source: string, name: string, from: number, parsed: Arguments, children: (shown: Range, node: any) => Token[]): Token {
  const body = parsed.args[parsed.args.length - 1];
  const token: Token = { kind: 'command', name, from, to: parsed.to, body, args: parsed.args };
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

  // A command drawn as its first argument parses that one, not the last.
  const index = FIRST_ARGUMENT_COMMANDS.has(name) ? 0 : parsed.args.length - 1;
  const shown = parsed.args[index];
  if (shown && !LITERAL_ARGUMENT_COMMANDS.has(name)) token.children = children(shown, parsed.nodes[index]);
  return token;
}

/** The catalog spec of an environment, by its name with or without the star. */
export function environmentSpec(name: string) {
  return ENVIRONMENTS.get(name) ?? ENVIRONMENTS.get(name.replace(/\*$/, ''));
}

/** How many mandatory arguments a table environment takes, if it is one. */
export function tableArguments(name: string): number | undefined {
  return TABLE_ENVIRONMENT_ARGS.get(name);
}

/**
 * The token of `\begin{name}` ... `\end{name}`: `from` to `to` whole, `endFrom` where the \end starts,
 * `parsed` the arguments taken after the \begin tag, `children` the markup of the body.
 */
export function environmentToken(source: string, name: string, from: number, endFrom: number, to: number, parsed: Arguments, children: () => Token[]): Token {
  const spec = environmentSpec(name);
  const token: Token = {
    kind: TABLE_ENVIRONMENTS.has(name) ? 'table' : 'container',
    name,
    from,
    to,
    body: { from: parsed.to, to: endFrom },
    args: parsed.args
  };
  if (parsed.options.length) token.options = parsed.options;
  if (name.endsWith('*')) token.star = true;

  // Code and opaque environments keep their text out of the markup tree.
  const verbatim = VERBATIM_ENVIRONMENTS.has(name) || spec?.render.kind === 'code' || spec?.render.kind === 'opaque';
  if (!verbatim && !MATH_ENVIRONMENTS.has(name)) {
    token.children = children();
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
}
