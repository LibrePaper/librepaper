export type TokenKind =
  | 'text'
  | 'break'
  | 'comment'
  | 'math'
  | 'heading'
  | 'container'
  | 'item'
  | 'command'
  | 'table'
  | 'raw'
  | 'tie'
  | 'group'
  | 'verb'
  | 'ligature'
  | 'accent'
  | 'linebreak';

export interface Range {
  from: number;
  to: number;
}

export interface Token extends Range {
  kind: TokenKind;
  name?: string;
  level?: number;
  display?: boolean;
  body?: Range;
  args?: Range[];
  // Named arguments of a call (Typst `name: value`), keyed by name, valued by the trimmed value.
  named?: Record<string, Range>;
  // Inner ranges of `[...]` arguments, including the ones a signature skips.
  options?: Range[];
  // The command or environment was written with a star.
  star?: boolean;
  children?: Token[];
  meta?: Record<string, string>;
}

export function textOf(source: string, range: Range): string {
  return source.slice(range.from, range.to);
}

export function walk(tokens: Token[], visit: (token: Token, depth: number) => void, depth = 0): void {
  for (const token of tokens) {
    visit(token, depth);
    if (token.children) walk(token.children, visit, depth + 1);
  }
}

export function tokenAt(tokens: Token[], pos: number): Token | null {
  for (const token of tokens) {
    if (pos < token.from || pos > token.to) continue;
    return (token.children && tokenAt(token.children, pos)) || token;
  }
  return null;
}
