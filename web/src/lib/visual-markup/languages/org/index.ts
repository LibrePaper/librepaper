import { registerLanguage, Language, LanguageCommands, TokenStyle } from '../../core/language';
import { Token } from '../../core/tokens';
import { tokenizeOrg } from './tree';

const commands: LanguageCommands = {
  wrap: {
    bold: ['*', '*'],
    italic: ['/', '/'],
    underline: ['_', '_'],
    mono: ['~', '~'],
    inlineMath: ['\\(', '\\)'],
    displayMath: ['\\[\n', '\n\\]'],
    quote: ['#+begin_quote\n', '\n#+end_quote']
  },
  heading(level, text) {
    return `${'*'.repeat(Math.max(level, 1))} ${text}`;
  },
  list(kind, items) {
    return items.map((entry, index) => (kind === 'number' ? `${index + 1}. ${entry}` : `- ${entry}`)).join('\n');
  },
  table(rows, cols) {
    const header = `| ${Array.from({ length: cols }, (_, index) => `Cell ${index + 1}`).join(' | ')} |`;
    const rule = `|${Array.from({ length: cols }, () => '---').join('+')}|`;
    const body = Array.from({ length: Math.max(rows - 1, 0) }, () => `| ${Array.from({ length: cols }, () => '   ').join(' | ')} |`);
    return [header, rule, ...body].join('\n');
  },
  color(kind, color, text) {
    return `@@html:<span style="${kind === 'text' ? 'color' : 'background-color'}: ${color}">@@${text}@@html:</span>@@`;
  }
};

function containerStyle(token: Token): TokenStyle | null {
  switch (token.name) {
    case 'codeblock':
      // A block with a language draws a header in place of its #+begin line, as Markdown fences do.
      if (token.meta?.lang) return { class: 'cm-lp-code', block: true, header: 'cellhead' };
      return { class: 'cm-lp-code', block: true };
    case 'blockquote': {
      const variant = token.meta?.classes;
      return { class: variant ? `cm-lp-quote cm-lp-quote-${variant}` : 'cm-lp-quote', block: true };
    }
    case 'mdtable':
      return { widget: 'tablebox', block: true };
    case 'fndef':
      return { class: 'cm-lp-fndef', block: true };
    case 'orgheader':
      return { widget: 'orgheader', block: true };
    case 'orgplanning':
      return { class: 'cm-lp-org-planning', block: true };
    case 'orgdrawer':
      return { widget: 'orgdrawer', block: true };
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
    case 'span':
      return (token.meta?.classes ?? '').split(' ').includes('underline') ? { class: 'cm-lv-underline' } : {};
    case 'code':
      return { widget: 'verb' };
    case 'href':
    case 'url':
      return { widget: 'link' };
    case 'orgref':
      return { widget: 'orgref' };
    case 'mdfigure':
      return { widget: 'mdfigure', block: true };
    case 'mdimage':
      return { widget: 'image' };
    case 'mdcite':
      return { widget: 'mdcite' };
    case 'fnref':
      return { widget: 'mdnote' };
    case 'hr':
      return { widget: 'hr', block: true };
    case 'orgkeyword':
      // Keywords that describe the element after them are drawn by that element.
      return token.meta?.affiliated === 'true' ? { hidden: true } : { widget: 'orgkeyword' };
    case 'orgtodo':
      return { widget: 'orgtodo' };
    case 'orgpriority':
      return { widget: 'orgpriority' };
    case 'orgtags':
      return { widget: 'orgtags' };
    case 'orgstamp':
      return { widget: 'orgstamp' };
    case 'orgtarget':
      return { widget: 'orgtarget' };
    case 'orgmacro':
      return { widget: 'orgmacro' };
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
      return { class: `cm-lv-heading cm-lv-h${Math.min(token.level ?? 1, 6)}`, block: true, before: 'headnum' };
    case 'item':
      if (token.meta?.task) return { widget: 'taskbox', lineClass: `cm-lp-task cm-lp-task-${token.meta.task}` };
      return { replaceWith: token.name === 'number' ? token.meta?.marker ?? '' : '•' };
    case 'linebreak':
      return { replaceWith: '↵' };
    case 'container':
      return containerStyle(token);
    case 'command':
      return commandStyle(token);
    case 'raw':
      if (token.name === 'entity') return { replaceWith: token.meta?.text ?? '' };
      // Fixed-width markers and clock lines are source only.
      if (token.name === 'marker' || token.name === 'clock') return { hidden: true };
      return null;
    default:
      return null;
  }
}

// A run of hidden lines collapses to a chip. Syntax-only lines (block delimiters, drawer ends,
// keywords drawn by the element they describe, table rules) collapse silently.
const SYNTAX_LINE = /^\s*(?:#\+(?:begin|end)_\S*.*|#\+(?:name|caption|header|headers|results|plot|label|attr_\S+)(?:\[[^\]]*\])?:.*|:end:|:\S+:|\|[-+|]*)$/i;

function hiddenSummary(text: string): string | null {
  const lines = text.split('\n').filter(line => line.trim() !== '');
  if (lines.every(line => SYNTAX_LINE.test(line))) return null;
  const first = lines.find(line => !SYNTAX_LINE.test(line)) ?? '';
  const label = first.trim();
  return label.length > 48 ? `${label.slice(0, 47)}…` : label;
}

export const org: Language = {
  id: 'org',
  name: 'Org',
  tokenize: tokenizeOrg,
  style,
  commands,
  imageSrc(_source: string, token: Token) {
    return token.meta?.src ?? null;
  },
  hiddenSummary
};

registerLanguage(org);
