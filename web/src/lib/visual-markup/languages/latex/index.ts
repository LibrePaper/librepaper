import { cssColor } from '../../core/color';
import { registerLanguage, Language, LanguageCommands, TokenStyle } from '../../core/language';
import { Token } from '../../core/tokens';
import { COMMANDS, ENVIRONMENTS, type EnvironmentRender, type Render } from './catalog';
import { ARGUMENT_COUNT, COMMAND_CLASSES, EDITABLE_COMMANDS, INCLUDE_COMMANDS, LIST_ENVIRONMENTS, REF_COMMANDS, VERBATIM_ENVIRONMENTS } from './commands';
import { rules } from './rules';
import { table } from './table';
import { figure, latexImageStyle } from './figure';


// `label` is not here: a standalone \label shows as a tag (see style()).
const HIDDEN_LAYOUT_COMMANDS = new Set([
  'centering', 'raggedleft', 'raggedright', 'hfill', 'hfil',
  'hspace', 'hspace*', 'vspace', 'vspace*', 'smallskip', 'medskip', 'bigskip',
  'bibliographystyle', 'bibliography', 'nocite', 'pagestyle', 'thispagestyle',
  'newpage', 'clearpage', 'noindent'
]);

// Matches a layout command with its optional and mandatory arguments, for example \vspace*{1em}.
const LAYOUT_PATTERN = new RegExp(
  `\\\\(?:${[...HIDDEN_LAYOUT_COMMANDS].map(name => name.replace('*', '\\*')).join('|')})(?![a-zA-Z])(?:\\[[^\\]]*\\])?(?:\\{[^}]*\\})?`,
  'g'
);
// Matches \begin{...} with its optional arguments, and \end{...}.
const ENVIRONMENT_PATTERN = /\\begin\{[^}]*\}(?:\[[^\]]*\])?(?:\{[^}]*\})?|\\end\{[^}]*\}/g;

const commands: LanguageCommands = {
  wrap: {
    bold: ['\\textbf{', '}'],
    italic: ['\\textit{', '}'],
    underline: ['\\underline{', '}'],
    mono: ['\\texttt{', '}'],
    smallcaps: ['\\textsc{', '}'],
    inlineMath: ['$', '$'],
    displayMath: ['$$', '$$'],
    quote: ['\\begin{quote}\n', '\n\\end{quote}']
  },
  heading(level, text) {
    const name = level <= 1 ? 'section' : level === 2 ? 'subsection' : 'subsubsection';
    return `\\${name}{${text}}`;
  },
  list(kind, items) {
    const name = kind === 'number' ? 'enumerate' : 'itemize';
    const body = items.map(entry => `  \\item ${entry}`).join('\n');
    return `\\begin{${name}}\n${body}\n\\end{${name}}`;
  },
  table(rows, cols) {
    const alignment = 'l'.repeat(cols);
    const line = Array.from({ length: cols }, (_, index) => `Cell ${index + 1}`).join(' & ');
    const body = Array.from({ length: rows }, () => `  ${line} \\\\`).join('\n');
    return `\\begin{tabular}{${alignment}}\n${body}\n\\end{tabular}`;
  },
  color(kind, color, text) {
    const hex = /^#([0-9a-f]{6})$/i.exec(color);
    const value = hex ? `[HTML]{${hex[1].toUpperCase()}}` : `{${color}}`;
    return `\\${kind === 'text' ? 'textcolor' : 'colorbox'}${value}{${text}}`;
  }
};


// Tokens store names without the star. The tables below still key starred forms
// (caption*, hspace*), so they are looked up with the star restored.
function fullName(token: Token): string {
  return token.star ? `${token.name}*` : token.name ?? '';
}

function commandStyle(render: Render): TokenStyle {
  switch (render.kind) {
    case 'mark':
      return { class: render.class };
    case 'declaration':
      return { hidden: true, declaration: render.class };
    case 'hide':
      return { hidden: true };
    case 'symbol':
      return { replaceWith: render.text };
    case 'chip':
      return { widget: 'chip', block: render.block };
    case 'widget':
      return { widget: render.widget, block: render.block };
    case 'source':
      return { class: 'cm-lv-cmd-unknown', keepSyntax: true };
  }
}

function environmentStyle(render: EnvironmentRender): TokenStyle {
  switch (render.kind) {
    case 'theorem':
    case 'abstract':
    case 'keywords':
      return { class: `cm-lp-env cm-lp-env-${render.kind}`, block: true, header: 'envhead' };
    case 'proof':
      return { class: 'cm-lp-env cm-lp-env-proof', block: true, header: 'envhead', footer: '∎' };
    case 'quote':
      return { class: 'cm-lp-quote', block: true };
    case 'align':
      return { class: `cm-lp-align-${render.align}`, block: true };
    case 'columns':
      return { class: 'cm-lp-columns', block: true };
    case 'box':
      return { class: 'cm-lp-box', block: true };
    case 'code':
      return { class: 'cm-lp-code', block: true, keepSyntax: true };
    case 'opaque':
      return { widget: 'chip', block: true };
    case 'bibliography':
      return { class: 'cm-lp-thebibliography', block: true };
    case 'plain':
      return { block: true };
  }
}

function style(token: Token): TokenStyle | null {
  switch (token.kind) {
    case 'comment':
      return { class: 'cm-lv-comment' };
    case 'math':
      return { widget: 'math', block: token.display };
    case 'table':
      return { class: 'cm-lv-table', block: true };
    case 'heading':
      return { class: `cm-lv-heading cm-lv-h${token.level}`, block: true };
    case 'item':
      return { replaceWith: '•' };
    case 'container': {
      if (token.meta?.figureComposite === 'true') return { widget: 'figure', block: true, keepSyntax: true };
      if (token.meta?.figure === 'true') {
        const classes = ['cm-lv-figure', 'cm-lv-figure-simple'];
        if (token.meta.figureWide === 'true') classes.push('cm-lv-figure-wide');
        if (token.meta.figureAlign) classes.push(`cm-lv-figure-${token.meta.figureAlign}`);
        return {
          class: classes.join(' '),
          block: true,
          attributes: token.meta.figureWidth ? { style: `width:${token.meta.figureWidth};max-width:100%` } : undefined
        };
      }
      const name = token.name ?? '';
      const full = fullName(token);
      if (LIST_ENVIRONMENTS.has(full)) return { class: 'cm-lv-list', block: true };
      if (VERBATIM_ENVIRONMENTS.has(full)) return { class: 'cm-lv-env', block: true, keepSyntax: true };
      const spec = ENVIRONMENTS.get(name);
      if (spec) return environmentStyle(spec.render);
      return { class: 'cm-lv-env', block: true };
    }
    case 'tie':
      return { replaceWith: ' ' };
    case 'ligature':
    case 'accent': {
      const text = token.meta?.text;
      return text === undefined ? null : { replaceWith: text };
    }
    case 'linebreak':
      return { replaceWith: '↵' };
    case 'verb':
      return { widget: 'verb' };
    // Braces are hidden by the default concealment of a body; the children stay visible.
    case 'group':
      return { class: '' };
    case 'command': {
      const name = token.name ?? '';
      const full = fullName(token);
      if (REF_COMMANDS.has(full)) return { widget: 'ref' };
      if (INCLUDE_COMMANDS.has(full)) return { widget: 'include' };
      if (name === 'maketitle') return { widget: 'title', block: true };
      if (name === 'includegraphics') return { widget: 'image' };
      // Shown as a tag rather than hidden, so a standalone label stays visible in the document.
      if (name === 'label') return { widget: 'label' };
      // \replaced{new}{old} shows only the new text.
      if (name === 'replaced' && token.args?.length) return { argument: 0 };
      if (HIDDEN_LAYOUT_COMMANDS.has(full)) return { hidden: true };
      const known = COMMAND_CLASSES.get(full);
      if (known) {
        const color = cssColor(token.meta?.color);
        const border = cssColor(token.meta?.borderColor);
        const properties: string[] = [];
        if (color) properties.push(name === 'textcolor' ? `color:${color}` : `background-color:${color}`);
        if (border) properties.push(`box-shadow:inset 0 0 0 1px ${border}`);
        return {
          class: known,
          attributes: properties.length ? { style: properties.join(';') } : undefined
        };
      }
      const spec = COMMANDS.get(name);
      if (spec) return commandStyle(spec.render);
      if (EDITABLE_COMMANDS.has(full)) return { class: 'cm-lv-cmd' };
      return { class: 'cm-lv-cmd-unknown', keepSyntax: true };
    }
    default:
      return null;
  }
}

// Names a fully hidden run of source lines for its chip. Returns null when the run is
// only environment delimiters and layout, which belong to the block around them.
function hiddenSummary(text: string): string | null {
  // A run can hold several things worth naming, such as the page break, the
  // bibliography and the end of a paper; each is named, in source order.
  const parts: string[] = [];
  const style = /\\bibliographystyle\{([^}]*)\}/.exec(text);
  for (const match of text.matchAll(/\\(newpage|clearpage|pagebreak)(?![a-zA-Z])|\\bibliography\{([^}]*)\}|\\end\{document\}/g)) {
    if (match[1]) parts.push('Page break');
    else if (match[2] !== undefined) {
      const database = match[2].trim();
      parts.push(style ? `Bibliography: ${database} (${style[1].trim()})` : `Bibliography: ${database}`);
    } else parts.push('End of document');
  }
  if (parts.length) return [...new Set(parts)].join(' · ');

  const rest = text.replace(ENVIRONMENT_PATTERN, '').replace(LAYOUT_PATTERN, '');
  if (!rest.trim()) return null;

  // Environment names are not listed: the chip names what the run does, not the block around it.
  const names = new Set<string>();
  for (const match of text.matchAll(/\\([a-zA-Z]+\*?)/g)) {
    if (match[1] !== 'begin' && match[1] !== 'end') names.add(match[1]);
  }
  return names.size ? [...names].map(name => `\\${name}`).join(', ') : null;
}

export const latex: Language = {
  id: 'latex',
  name: 'LaTeX',
  rules,
  style,
  commands,
  table,
  figure,
  colorCommands: ['textcolor', 'colorbox', 'color', 'fcolorbox'],
  imageSrc(source, token) {
    return token.body ? source.slice(token.body.from, token.body.to).trim() : null;
  },
  imageStyle: latexImageStyle,
  hiddenSummary
};

export function setMacroSignatures(signatures: Record<string, { signature?: string } | number>): void {
  for (const name of Object.keys(signatures)) {
    const entry = signatures[name];
    const count = typeof entry === 'number' ? entry : countMandatory(entry.signature);
    if (count > 0) ARGUMENT_COUNT.set(name, count);
  }
}

function countMandatory(signature = ''): number {
  return (signature.match(/m/g) ?? []).length;
}

export { rules } from './rules';
export * from './commands';

registerLanguage(latex);
