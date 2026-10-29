import { registerLanguage, Language, LanguageCommands, TokenStyle } from '../../core/language';
import { Token } from '../../core/tokens';
import { rules } from './rules';

const commands: LanguageCommands = {
  wrap: {
    bold: ['**', '**'],
    italic: ['*', '*'],
    underline: ['[', ']{.underline}'],
    mono: ['`', '`'],
    smallcaps: ['[', ']{.smallcaps}'],
    inlineMath: ['$', '$'],
    displayMath: ['$$\n', '\n$$'],
    quote: ['> ', '']
  },
  heading(level, text) {
    return `${'#'.repeat(Math.min(Math.max(level, 1), 6))} ${text}`;
  },
  list(kind, items) {
    return items.map((entry, index) => (kind === 'number' ? `${index + 1}. ${entry}` : `- ${entry}`)).join('\n');
  },
  table(rows, cols) {
    const header = `| ${Array.from({ length: cols }, (_, index) => `Cell ${index + 1}`).join(' | ')} |`;
    const separator = `|${Array.from({ length: cols }, () => ' --- ').join('|')}|`;
    const body = Array.from({ length: Math.max(rows - 1, 0) }, () => `| ${Array.from({ length: cols }, () => '   ').join(' | ')} |`);
    return [header, separator, ...body].join('\n');
  },
  color(kind, color, text) {
    return `<span style="${kind === 'text' ? 'color' : 'background-color'}: ${color}">${text}</span>`;
  }
};

const CALLOUT = /^callout-(note|tip|warning|important|caution)$/;
const THEOREM = /^(thm|lem|cor|prp|cnj|def|exm|exr|sol|rem)-/;
const SPAN_CLASSES = new Map([
  ['smallcaps', 'cm-lv-smallcaps'],
  ['underline', 'cm-lv-underline'],
  ['mark', 'cm-lp-highlight']
]);

function divStyle(token: Token): TokenStyle {
  const callout = (token.meta?.classes ?? '').split(' ').find(name => CALLOUT.test(name));
  if (callout) {
    const type = callout.slice('callout-'.length);
    return { class: `cm-lp-callout cm-lp-callout-${type}`, block: true, header: 'divhead' };
  }
  if (THEOREM.test(token.meta?.id ?? '')) {
    return { class: 'cm-lp-env cm-lp-env-theorem', block: true, header: 'divhead' };
  }
  return { block: true, header: 'divhead' };
}

function containerStyle(token: Token): TokenStyle | null {
  switch (token.name) {
    case 'frontmatter':
      return { widget: 'frontmatter', block: true };
    case 'codeblock':
      // Cells draw their header in place of the opening fence; the closing fence collapses
      // through the default concealment. Plain code keeps its fences as source.
      if (token.meta?.cell === 'true') return { class: 'cm-lp-code', block: true, header: 'cellhead' };
      return { class: 'cm-lp-code', block: true, keepSyntax: true };
    case 'blockquote':
      return { class: 'cm-lp-quote', block: true };
    case 'mdtable':
      return { widget: 'mdtable', block: true };
    case 'fndef':
      return { class: 'cm-lp-fndef', block: true };
    case 'div':
      return divStyle(token);
    default:
      return null;
  }
}

function commandStyle(token: Token): TokenStyle | null {
  switch (token.name) {
    case 'strong':
      return { class: 'cm-lv-bold' };
    case 'emph':
      return { class: 'cm-lv-italic' };
    case 'strike':
      return { class: 'cm-lp-strike' };
    case 'sub':
      return { class: 'cm-lp-sub' };
    case 'sup':
      return { class: 'cm-lp-sup' };
    case 'mark':
      return { class: 'cm-lp-highlight' };
    case 'code':
      return { widget: 'verb' };
    case 'href':
    case 'url':
      return { widget: 'link' };
    case 'span': {
      const classes = (token.meta?.classes ?? '').split(' ');
      const found = classes.map(name => SPAN_CLASSES.get(name)).find(Boolean);
      return found ? { class: found } : {};
    }
    case 'mdfigure':
      return { widget: 'mdfigure', block: true };
    case 'mdimage':
      return { widget: 'image' };
    case 'mdcite':
      return { widget: 'mdcite' };
    case 'xref':
      return { widget: 'xref' };
    case 'fnref':
      return { widget: 'mdnote' };
    case 'shortcode':
      return { widget: 'shortcode' };
    case 'hr':
      return { widget: 'hr', block: true };
    default:
      return null;
  }
}

function style(token: Token): TokenStyle | null {
  switch (token.kind) {
    case 'comment':
      return { class: 'cm-lv-comment' };
    case 'math':
      return { widget: 'math', block: token.display };
    case 'heading':
      return { class: `cm-lv-heading cm-lv-h${token.level}`, block: true, before: 'headnum' };
    case 'item':
      if (token.meta?.task) return { replaceWith: token.meta.task === 'done' ? '☑' : '☐' };
      // Numbers keep the marker as written, `1)` included.
      return { replaceWith: token.name === 'number' ? token.meta?.marker ?? '' : '•' };
    case 'linebreak':
      return { replaceWith: '↵' };
    case 'container':
      return containerStyle(token);
    case 'command':
      return commandStyle(token);
    case 'raw':
      if (token.name === 'escape') return { replaceWith: token.meta?.text ?? '' };
      // Quote markers and link definitions are source only.
      if (token.name === 'marker' || token.name === 'linkdef') return { hidden: true };
      return null;
    default:
      return null;
  }
}

// A run of hidden lines collapses to a chip. Syntax-only lines (fences, attribute lines,
// quote markers, link definitions, rules) collapse silently; anything else is named.
const SYNTAX_LINE = /^\s*(?:(?:>\s*)+|`{3,}.*|~{3,}.*|:{3,}\s*|\{[^}]*\}\s*|=+\s*|-+\s*|\[[^\]]+\]:.*)$/;

function hiddenSummary(text: string): string | null {
  const lines = text.split('\n').filter(line => line.trim() !== '');
  if (lines.every(line => SYNTAX_LINE.test(line))) return null;
  const first = lines.find(line => !SYNTAX_LINE.test(line)) ?? '';
  const label = first.trim();
  return label.length > 48 ? `${label.slice(0, 47)}…` : label;
}

export const markdown: Language = {
  id: 'markdown',
  name: 'Markdown',
  rules,
  style,
  commands,
  // A widget reads the source through the image resolver, which takes the path from meta.
  imageSrc(_source: string, token: Token) {
    return token.meta?.src ?? null;
  },
  hiddenSummary
};

export { rules } from './rules';

registerLanguage(markdown);
