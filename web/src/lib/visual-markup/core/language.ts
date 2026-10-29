import { EditorScope } from './scope';
import { ToolbarEntry } from './toolbar';
import { Range, Token } from './tokens';

export type ParseMode = 'markup' | 'code';

export interface RuleContext {
  source: string;
  depth: number;
  mode: ParseMode;
  parse(from: number, to: number, mode?: ParseMode): Token[];
}

export type Rule = (source: string, pos: number, ctx: RuleContext) => Token | null;

// 'label' shows a standalone \label as a small tag instead of hiding it.
export type WidgetKind =
  | 'math' | 'image' | 'figure' | 'ref' | 'include' | 'title' | 'label'
  | 'cite' | 'link' | 'footnote' | 'verb' | 'chip' | 'unit' | 'gloss' | 'quote'
  | 'envhead' | 'macro' | 'bibitem' | 'symbol'
  // Typst: reference, label tag, included file, title block, colour swatch, and a catalog chip
  // (the Typst catalog's chip text, not the LaTeX one).
  | 'typref' | 'typlabel' | 'typinclude' | 'typtitle' | 'swatch' | 'typchip';

export interface TokenStyle {
  class?: string;
  attributes?: Record<string, string>;
  hidden?: boolean;
  block?: boolean;
  widget?: WidgetKind;
  granular?: boolean;
  keepSyntax?: boolean;
  replaceWith?: string;
  // Widget for an environment's header, from token.from to the start of its body.
  header?: WidgetKind;
  // Text drawn at the end of an environment's body, such as the proof mark.
  footer?: string;
  // Class for the text from a declaration (\bfseries) to the end of its group or environment.
  declaration?: string;
  // Index of the mandatory argument shown in place of the last one (\replaced{new}{old} shows new).
  argument?: number;
}

export interface LanguageCommands {
  wrap: Record<string, [string, string]>;
  heading(level: number, text: string): string;
  list(kind: 'bullet' | 'number', items: string[]): string;
  table(rows: number, cols: number): string;
  color(kind: 'text' | 'background', color: string, text: string): string;
}

export interface CellPosition {
  row: number;
  col: number;
}

export interface TableCellRange extends Range {
  colspan?: number;
  rowspan?: number;
  header?: boolean;
  column?: number;
  row?: number;
  visualFrom?: number;
  visualTo?: number;
}

export interface TableAdapter {
  parse(source: string, token: Token): string[][];
  serialize(cells: string[][], token: Token, source: string): string;
  ranges?(source: string, token: Token): TableCellRange[][];
  locate?(source: string, token: Token, pos: number): CellPosition | null;
  editable?(source: string, token: Token): boolean;
}


export interface FigureImage {
  src: string;
  alt?: string;
  style?: string;
}

export interface FigurePanel {
  images: FigureImage[];
  caption?: string;
  captionRange?: Range;
  captionEditable?: boolean;
  width?: string;
}

export interface FigureModel {
  panels: FigurePanel[];
  caption?: string;
  captionRange?: Range;
  captionEditable?: boolean;
  columns?: number;
  tracks?: string[];
  wide?: boolean;
  align?: 'left' | 'center' | 'right';
  width?: string;
  captionPosition?: 'top' | 'bottom' | 'side';
}

export interface FigureAdapter {
  parse(source: string, token: Token): FigureModel | null;
}

export interface Language {
  id: string;
  name: string;
  rules: Rule[];
  style(token: Token): TokenStyle | null;
  commands: LanguageCommands;
  table?: TableAdapter;
  figure?: FigureAdapter;
  colorCommands?: string[];
  imageSrc?(source: string, token: Token): string | null;
  imageStyle?(source: string, token: Token): string | null;
  // Summarises a run of fully hidden source lines. Returns a short chip label, or null
  // to remove the run without a chip (it is then layout-only source).
  hiddenSummary?(text: string): string | null;
  toolbar?(scope: EditorScope): ToolbarEntry[];
}

const registry = new Map<string, Language>();

export function registerLanguage(language: Language): void {
  registry.set(language.id, language);
}

export function getLanguage(id: string): Language {
  const language = registry.get(id);
  if (!language) throw new Error(`Unknown visual editor language: ${id}`);
  return language;
}

export function listLanguages(): Language[] {
  return [...registry.values()];
}
