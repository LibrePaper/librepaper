import type { SyntaxNode, SyntaxNodeRef } from '@lezer/common';
import { Range, Token } from '../../core/tokens';
import { orgTree } from '../../../source-trees.js';
import { lineEndAt, lineStartAt, startsLine } from '../../core/lines';
import { AFFILIATED, ENTITY_CHARS, HEADER_KEYS, IMAGE_PATH, MATH_ENVIRONMENTS, QUOTE_BLOCKS, TODO_KEYS } from './catalog';

const SCHEME = /^[A-Za-z][A-Za-z0-9+.-]*:/;
const FILE_PATH = /^(?:\.{0,2}\/|~)/;
const WEB_IMAGE = /^https?:/i;

interface Affiliated {
  name?: string;
  caption?: string;
  attrs: string[];
}

// Builds the visual Token tree for an Org source from the shared Org Lezer parse. Each tree node
// maps to a token; the text between tokens is text. The mapper reads the text of one node at a
// time and never searches across the source.
export function tokenizeOrg(source: string): Token[] {
  const tree = orgTree(source);
  if (!tree) return [];
  const text = (node: { from: number; to: number }): string => source.slice(node.from, node.to);
  const keyOf = (keyword: SyntaxNode): string => {
    const key = keyword.getChild('KeywordKey');
    return key ? text(key).replace(/\[.*$/s, '').toUpperCase() : '';
  };
  const valueOf = (keyword: SyntaxNode): string => {
    const value = keyword.getChild('KeywordValue');
    return value ? text(value) : '';
  };

  // The states that end a sequence: DONE, and the words after `|` in each #+TODO line (the last
  // word when a line has no bar).
  const doneStates = new Set(['DONE']);
  const top = tree.topNode;
  for (let child = top.firstChild; child; child = child.nextSibling) {
    if (child.name !== 'Keyword' || !TODO_KEYS.has(keyOf(child))) continue;
    const words = valueOf(child).split(/\s+/).filter(Boolean);
    const bar = words.indexOf('|');
    const ending = bar < 0 ? words.slice(-1) : words.slice(bar + 1);
    for (const word of ending) doneStates.add(word.replace(/\(.*\)$/, ''));
  }

  // The keyword lines that end the document's leading run belong to the first element when they
  // directly precede it and it is one of those they describe.
  function describes(next: SyntaxNode | null): boolean {
    if (!next) return false;
    switch (next.name) {
      case 'Table':
        return true;
      case 'Block': {
        const name = text(next.getChild('BlockBegin')?.getChild('BlockName') ?? next).toLowerCase();
        return name === 'src' || name === 'example' || name === 'export' || name === 'comment' || QUOTE_BLOCKS.has(name);
      }
      case 'Paragraph':
        return figureLink(next) !== null;
      default:
        return false;
    }
  }

  function adjacent(before: SyntaxNode, after: SyntaxNode): boolean {
    const gap = source.slice(before.to, after.from);
    return gap.trim() === '' && gap.split('\n').length <= 2;
  }

  // The element an affiliated keyword line describes, or null.
  function target(keyword: SyntaxNode): SyntaxNode | null {
    if (!AFFILIATED.test(keyOf(keyword))) return null;
    let last = keyword;
    for (let next = keyword.nextSibling; next; next = next.nextSibling) {
      if (!adjacent(last, next)) return null;
      if (next.name === 'Keyword') {
        if (!AFFILIATED.test(keyOf(next))) return null;
        last = next;
        continue;
      }
      return describes(next) ? next : null;
    }
    return null;
  }

  // The affiliated keywords right before an element.
  function affiliatedOf(node: SyntaxNode): Affiliated {
    const found: Affiliated = { attrs: [] };
    let after = node;
    for (let before = node.prevSibling; before?.name === 'Keyword' && adjacent(before, after); before = before.prevSibling) {
      const key = keyOf(before);
      if (!AFFILIATED.test(key)) break;
      const value = valueOf(before);
      if (key === 'NAME') found.name = value;
      else if (key === 'CAPTION') found.caption = value;
      else if (key.startsWith('ATTR_')) found.attrs.push(value);
      after = before;
    }
    return found;
  }

  // The leading run of keywords, as a document header, or null.
  let header: { from: number; to: number; meta: Record<string, string> } | null = null;
  {
    const run: SyntaxNode[] = [];
    for (let child = top.firstChild; child?.name === 'Keyword'; child = child.nextSibling) run.push(child);
    while (run.length && target(run[run.length - 1])) run.pop();
    // A run that ends in affiliated keywords before an element hands those keywords to it.
    while (run.length && AFFILIATED.test(keyOf(run[run.length - 1])) && describes(run[run.length - 1].nextSibling)) run.pop();
    if (run.some(keyword => HEADER_KEYS.has(keyOf(keyword)))) {
      const meta: Record<string, string> = {};
      for (const keyword of run) {
        const key = keyOf(keyword);
        if (!HEADER_KEYS.has(key)) continue;
        const name = key.toLowerCase();
        meta[name] = meta[name] ? `${meta[name]}, ${valueOf(keyword)}` : valueOf(keyword);
      }
      header = { from: run[0].from, to: run[run.length - 1].to, meta };
    }
  }

  let descend = false;

  // Tokens for the nodes that lie in [from, to], outermost first, in order and without overlap.
  // `inside` is a node that is not built again, for the children of a token made from it.
  function collect(from: number, to: number, inside?: SyntaxNode): Token[] {
    const found: Token[] = [];
    let end = from;
    tree.iterate({
      from,
      to,
      enter(ref: SyntaxNodeRef) {
        if (ref.to <= end) return false;
        if (ref.from < end || ref.to > to) return true;
        if (inside && ref.name === inside.name && ref.from === inside.from) return true;
        descend = false;
        const token = build(ref);
        if (!token || token.to <= token.from || token.to > to) return true;
        found.push(token);
        end = token.to;
        return descend;
      }
    });
    return found;
  }

  function region(from: number, to: number, inside?: SyntaxNode): Token[] {
    const tokens: Token[] = [];
    if (from >= to) return tokens;
    let pos = from;
    for (const token of collect(from, to, inside)) {
      if (token.from > pos) tokens.push({ kind: 'text', from: pos, to: token.from });
      tokens.push(token);
      pos = token.to;
    }
    if (pos < to) tokens.push({ kind: 'text', from: pos, to });
    return tokens;
  }

  function build(ref: SyntaxNodeRef): Token | null {
    switch (ref.name) {
      case 'Headline':
        return headlineToken(ref.node);
      case 'Planning':
        return planningToken(ref.node);
      case 'Clock':
        return startsLine(source, ref.from) ? { kind: 'raw', name: 'clock', from: ref.from, to: ref.to } : null;
      case 'PropertyDrawer':
      case 'Drawer':
        return drawerToken(ref.node);
      case 'Keyword':
        return keywordToken(ref.node);
      case 'Block':
        return blockToken(ref.node);
      case 'LatexEnvironment':
        return latexEnvironmentToken(ref.node);
      case 'Comment':
        return commentToken(ref.node);
      case 'FixedWidth':
        return fixedWidthToken(ref.node);
      case 'HorizontalRule':
        return startsLine(source, ref.from) ? { kind: 'command', name: 'hr', from: ref.from, to: ref.to } : null;
      case 'Table':
        return tableToken(ref.node);
      case 'ListBullet':
        return itemToken(ref.node);
      case 'FootnoteDefinition':
        return footnoteDefinitionToken(ref.node);
      case 'FootnoteReference':
        return footnoteReferenceToken(ref.node);
      case 'Citation':
        return citationToken(ref.node);
      case 'Link':
        return linkToken(ref.node);
      case 'Bold':
        return wrapToken(ref.node, 'strong');
      case 'Italic':
        return wrapToken(ref.node, 'emph');
      case 'StrikeThrough':
        return wrapToken(ref.node, 'strike');
      case 'Underline':
        return wrapToken(ref.node, 'span', { classes: 'underline' });
      case 'Verbatim':
      case 'Code':
        return verbatimToken(ref.node);
      case 'InlineSrc':
        return inlineSrcToken(ref.node);
      case 'InlineMath':
      case 'DisplayMath':
        return mathToken(ref.node);
      case 'Entity':
        return entityToken(ref.node);
      case 'LineBreak':
        return { kind: 'linebreak', from: ref.from, to: ref.to };
      case 'TimestampActive':
      case 'TimestampInactive':
        return stampToken(ref.node);
      case 'Macro':
        return macroToken(ref.node);
      case 'Target':
      case 'RadioTarget':
        return targetToken(ref.node);
      default:
        return null;
    }
  }

  // ---- Inline markup ------------------------------------------------------

  function wrapToken(node: SyntaxNode, name: string, meta?: Record<string, string>): Token | null {
    const marks = node.getChildren('EmphasisMark');
    if (marks.length < 2) return null;
    const body = { from: marks[0].to, to: marks[marks.length - 1].from };
    if (body.to < body.from) return null;
    const token: Token = { kind: 'command', name, from: node.from, to: node.to, body, children: region(body.from, body.to) };
    if (meta) token.meta = meta;
    return token;
  }

  function verbatimToken(node: SyntaxNode): Token | null {
    const marks = node.getChildren('EmphasisMark');
    if (marks.length < 2) return null;
    return { kind: 'command', name: 'code', from: node.from, to: node.to, body: { from: marks[0].to, to: marks[marks.length - 1].from } };
  }

  function inlineSrcToken(node: SyntaxNode): Token | null {
    const body = node.getChild('InlineSrcBody');
    return body ? { kind: 'command', name: 'code', from: node.from, to: node.to, body: { from: body.from, to: body.to } } : null;
  }

  // `$x$`, `$$x$$`, `\(x\)` and `\[x\]`. A display formula that shares its line with text is drawn inline.
  function mathToken(node: SyntaxNode): Token | null {
    const content = text(node);
    const open = content.startsWith('$$') ? '$$' : content.startsWith('$') ? '$' : content.slice(0, 2);
    const close = open === '\\(' ? '\\)' : open === '\\[' ? '\\]' : open;
    if (content.length < open.length + close.length || !content.endsWith(close)) return null;
    const display = node.name === 'DisplayMath' && startsLine(source, node.from);
    return {
      kind: 'math',
      from: node.from,
      to: node.to,
      display,
      body: { from: node.from + open.length, to: node.to - close.length },
      meta: { open, close }
    };
  }

  function entityToken(node: SyntaxNode): Token | null {
    const name = /^\\([A-Za-z]+)/.exec(text(node))?.[1] ?? '';
    const char = Object.prototype.hasOwnProperty.call(ENTITY_CHARS, name) ? ENTITY_CHARS[name] : undefined;
    return char === undefined ? null : { kind: 'raw', name: 'entity', from: node.from, to: node.to, meta: { text: char } };
  }

  function stampToken(node: SyntaxNode): Token {
    const meta: Record<string, string> = { text: source.slice(node.from + 1, node.to - 1), active: String(node.name === 'TimestampActive') };
    if (node.parent?.name === 'Planning') {
      for (let before = node.prevSibling; before; before = before.prevSibling) {
        if (before.name === 'PlanningKeyword') {
          meta.keyword = text(before).replace(/:$/, '');
          break;
        }
      }
    }
    return { kind: 'command', name: 'orgstamp', from: node.from, to: node.to, meta };
  }

  function macroToken(node: SyntaxNode): Token {
    const inner = source.slice(node.from + 3, Math.max(node.from + 3, node.to - 3));
    const open = inner.indexOf('(');
    const name = open < 0 ? inner : inner.slice(0, open);
    const args = open < 0 ? '' : inner.slice(open + 1).replace(/\)$/, '');
    return { kind: 'command', name: 'orgmacro', from: node.from, to: node.to, meta: { name: name.trim(), args } };
  }

  function targetToken(node: SyntaxNode): Token {
    const width = node.name === 'RadioTarget' ? 3 : 2;
    return { kind: 'command', name: 'orgtarget', from: node.from, to: node.to, meta: { key: source.slice(node.from + width, node.to - width) } };
  }

  // ---- Links, citations, footnotes ---------------------------------------

  // The image link that is the whole content of a paragraph, or null.
  function figureLink(paragraph: SyntaxNode): SyntaxNode | null {
    const link = paragraph.firstChild;
    if (link?.name !== 'Link' || link.from !== paragraph.from || link.getChild('LinkDescription')) return null;
    const target = link.getChild('LinkTarget');
    if (!target || !imagePath(text(target)) || source.slice(link.to, paragraph.to).trim() !== '') return null;
    return link;
  }

  // The file path of an image link target, or null when the target is not an image.
  function imagePath(url: string): string | null {
    const scheme = SCHEME.exec(url)?.[0].toLowerCase();
    if (scheme && scheme !== 'file:' && !WEB_IMAGE.test(url)) return null;
    if (!IMAGE_PATH.test(url)) return null;
    return scheme === 'file:' ? url.slice(5).replace(/::.*$/, '') : url.replace(/::.*$/, '');
  }

  function widthAttribute(attrs: string[]): string {
    for (const value of attrs) {
      const width = /:width\s+(\S+)/.exec(value)?.[1];
      if (width) return `width=${width}`;
    }
    return '';
  }

  function linkToken(node: SyntaxNode): Token | null {
    const targetNode = node.getChild('LinkTarget');
    if (!targetNode) return null;
    const desc = node.getChild('LinkDescription');
    const url = text(targetNode);
    const targetRange: Range = { from: targetNode.from, to: targetNode.to };
    const path = desc ? null : imagePath(url);
    if (path !== null) {
      const paragraph = node.parent;
      const figure = paragraph?.name === 'Paragraph' && figureLink(paragraph)?.from === node.from;
      const meta: Record<string, string> = { src: path, alt: '', attrs: '' };
      if (figure && paragraph) {
        const affiliated = affiliatedOf(paragraph);
        if (affiliated.caption) {
          meta.alt = affiliated.caption;
          meta.caption = affiliated.caption;
        }
        if (affiliated.name) meta.id = affiliated.name;
        meta.attrs = widthAttribute(affiliated.attrs);
      }
      return { kind: 'command', name: figure ? 'mdfigure' : 'mdimage', from: node.from, to: node.to, meta };
    }
    if (!SCHEME.test(url) && !FILE_PATH.test(url)) {
      const meta: Record<string, string> = { key: url };
      if (desc) meta.description = text(desc);
      const body: Range = desc ? { from: desc.from, to: desc.to } : targetRange;
      return { kind: 'command', name: 'orgref', from: node.from, to: node.to, body, children: desc ? region(desc.from, desc.to) : undefined, meta };
    }
    if (desc) {
      const body = { from: desc.from, to: desc.to };
      return { kind: 'command', name: 'href', from: node.from, to: node.to, body, args: [targetRange, body], children: region(desc.from, desc.to) };
    }
    return { kind: 'command', name: 'url', from: node.from, to: node.to, body: targetRange, args: [targetRange] };
  }

  // `[cite/style:@a; @b]`: the content after the colon is what the citation widget reads.
  function citationToken(node: SyntaxNode): Token | null {
    const content = text(node);
    const colon = content.indexOf(':');
    if (colon < 0 || !content.endsWith(']')) return null;
    const body = { from: node.from + colon + 1, to: node.to - 1 };
    return { kind: 'command', name: 'mdcite', from: node.from, to: node.to, body, meta: { text: source.slice(body.from, body.to), bracketed: 'true', style: content.slice(5, colon).replace(/^\//, '') } };
  }

  // `[fn:label]`, `[fn:label:definition]` and `[fn::definition]`.
  function footnoteReferenceToken(node: SyntaxNode): Token | null {
    const content = text(node);
    const match = /^\[fn:([^\]\s:]*)(?::([\s\S]*))?\]$/.exec(content);
    if (!match) return null;
    const meta: Record<string, string> = { label: match[1], format: 'org' };
    const token: Token = { kind: 'command', name: 'fnref', from: node.from, to: node.to, meta };
    if (match[2] !== undefined) {
      meta.inline = 'true';
      token.body = { from: node.to - 1 - match[2].length, to: node.to - 1 };
      token.children = region(token.body.from, token.body.to);
    }
    return token;
  }

  // `[fn:label] text`: the token covers the first element of the definition; the rest is walked on.
  function footnoteDefinitionToken(node: SyntaxNode): Token | null {
    const label = node.getChild('FootnoteLabel');
    if (!label || !startsLine(source, node.from)) return null;
    const first = label.nextSibling;
    const bodyFrom = Math.min(first?.from ?? lineEndAt(source, label.to), lineEndAt(source, label.to));
    const end = first ? lineEndAt(source, first.to) : lineEndAt(source, label.to);
    descend = true;
    return {
      kind: 'container',
      name: 'fndef',
      from: node.from,
      to: end,
      body: { from: bodyFrom, to: end },
      children: region(bodyFrom, end),
      meta: { label: text(label) }
    };
  }

  // ---- Headlines and their lines -----------------------------------------

  function headlineToken(node: SyntaxNode): Token | null {
    const stars = node.getChild('Stars');
    if (!stars) return null;
    const title = node.getChild('Title');
    const tags = node.getChild('Tags');
    const children: Token[] = [];
    const todo = node.getChild('TodoKeyword');
    if (todo) {
      const state = text(todo);
      children.push({ kind: 'command', name: 'orgtodo', from: todo.from, to: todo.to, meta: { state, done: String(doneStates.has(state)) } });
    }
    const priority = node.getChild('Priority');
    if (priority) {
      children.push({ kind: 'command', name: 'orgpriority', from: priority.from, to: priority.to, meta: { value: source[priority.from + 2] } });
    }
    const body: Range = title ? { from: title.from, to: title.to } : { from: tags?.from ?? node.to, to: tags?.from ?? node.to };
    if (title) children.push(...region(title.from, title.to));
    if (tags) {
      const names = tags.getChildren('Tag').map(tag => text(tag));
      children.push({ kind: 'command', name: 'orgtags', from: tags.from, to: tags.to, meta: { tags: names.join(',') } });
    }
    return {
      kind: 'heading',
      level: stars.to - stars.from,
      from: node.from,
      to: node.to,
      body,
      children
    };
  }

  function planningToken(node: SyntaxNode): Token | null {
    if (!startsLine(source, node.from)) return null;
    const keyword = node.getChild('PlanningKeyword');
    return {
      kind: 'container',
      name: 'orgplanning',
      from: node.from,
      to: node.to,
      children: region(node.from, node.to, node),
      meta: { keyword: keyword ? text(keyword).replace(/:$/, '') : '' }
    };
  }

  function drawerToken(node: SyntaxNode): Token | null {
    const name = node.getChild('DrawerName');
    if (!name || !startsLine(source, node.from)) return null;
    const meta: Record<string, string> = { name: text(name) };
    if (node.name === 'PropertyDrawer') {
      const items = node.getChildren('Property').map(property => [
        text(property.getChild('PropertyKey') ?? property),
        text(property.getChild('PropertyValue') ?? { from: property.to, to: property.to })
      ]);
      meta.count = String(items.length);
      meta.items = JSON.stringify(items);
    } else {
      const end = node.getChild('DrawerEnd');
      const inner = source.slice(name.to + 1, end ? end.from : node.to);
      meta.count = String(inner.split('\n').filter(line => line.trim() !== '').length);
    }
    return { kind: 'container', name: 'orgdrawer', from: node.from, to: node.to, meta };
  }

  // ---- Keywords ----------------------------------------------------------

  function keywordToken(node: SyntaxNode): Token | null {
    if (header && node.from === header.from) {
      return { kind: 'container', name: 'orgheader', from: header.from, to: header.to, meta: header.meta };
    }
    if (!startsLine(source, node.from)) return null;
    const key = keyOf(node);
    const meta: Record<string, string> = { key, value: valueOf(node) };
    if (target(node)) meta.affiliated = 'true';
    return { kind: 'command', name: 'orgkeyword', from: node.from, to: node.to, meta };
  }

  // ---- Blocks ------------------------------------------------------------

  // The options of a src block header, `:tangle yes :results output`, as [key, value] pairs.
  function headerArguments(parameters: string): [string, string][] {
    const options: [string, string][] = [];
    for (const match of parameters.matchAll(/:(\S+)((?:\s+(?!:)\S+)*)/g)) options.push([match[1], match[2].trim()]);
    return options;
  }

  function blockToken(node: SyntaxNode): Token | null {
    const begin = node.getChild('BlockBegin');
    const nameNode = begin?.getChild('BlockName');
    if (!begin || !nameNode || !startsLine(source, node.from)) return null;
    const name = text(nameNode).toLowerCase();
    const close = node.getChild('BlockEnd');
    const bodyFrom = Math.min(lineEndAt(source, begin.from) + 1, node.to);
    const bodyTo = close ? Math.max(bodyFrom, lineStartAt(source, close.from) - 1) : node.to;
    const to = close ? lineEndAt(source, close.from) : node.to;
    const body = { from: bodyFrom, to: bodyTo };
    if (name === 'comment') return { kind: 'comment', from: node.from, to, body };
    if (name === 'src' || name === 'example' || name === 'export') {
      const meta: Record<string, string> = {};
      const language = begin.getChild('BlockParameters')?.getChild('BlockLanguage');
      if (language) meta.lang = text(language);
      if (name === 'export') meta.export = 'true';
      const affiliated = affiliatedOf(node);
      if (affiliated.name) meta.label = affiliated.name;
      if (affiliated.caption) meta.caption = affiliated.caption;
      const parameters = begin.getChild('BlockParameters');
      if (name === 'src' && parameters) {
        const rest = source.slice(language ? language.to : parameters.from, parameters.to);
        const options = headerArguments(rest);
        if (options.length) meta.options = JSON.stringify(options);
      }
      return { kind: 'container', name: 'codeblock', from: node.from, to, body, meta };
    }
    if (QUOTE_BLOCKS.has(name)) {
      const token: Token = { kind: 'container', name: 'blockquote', from: node.from, to, body, children: region(bodyFrom, bodyTo) };
      if (name !== 'quote') token.meta = { classes: name };
      return token;
    }
    return null;
  }

  // A math environment is a display formula; any other environment is text.
  function latexEnvironmentToken(node: SyntaxNode): Token | null {
    const begin = node.getChild('LatexEnvBegin');
    const end = node.getChild('LatexEnvEnd');
    const name = begin?.getChild('LatexEnvName');
    if (!begin || !end || !name || !MATH_ENVIRONMENTS.has(text(name)) || !startsLine(source, node.from)) return null;
    return {
      kind: 'math',
      name: text(name),
      from: node.from,
      to: node.to,
      display: true,
      body: { from: begin.to, to: end.from },
      meta: { open: text(begin), close: text(end) }
    };
  }

  // `# text`: the hash and the space after it are source only.
  function commentToken(node: SyntaxNode): Token | null {
    const content = text(node);
    const hash = content.indexOf('#');
    if (hash < 0) return null;
    const bodyFrom = node.from + hash + (content[hash + 1] === ' ' ? 2 : 1);
    return { kind: 'comment', from: node.from, to: node.to, body: { from: Math.min(bodyFrom, node.to), to: node.to } };
  }

  // Consecutive `: text` lines are one block; each line's `: ` is source only.
  function fixedWidthToken(node: SyntaxNode): Token | null {
    if (!startsLine(source, node.from)) return null;
    const children: Token[] = [];
    let last = node;
    for (let line: SyntaxNode | null = node; line?.name === 'FixedWidth' && (line === node || adjacent(last, line)); line = line.nextSibling) {
      const content = text(line);
      const colon = content.indexOf(':');
      const markerTo = line.from + colon + (content[colon + 1] === ' ' ? 2 : 1);
      if (line.from < markerTo) children.push({ kind: 'raw', name: 'marker', from: line.from, to: Math.min(markerTo, line.to) });
      if (markerTo < line.to) children.push({ kind: 'text', from: markerTo, to: line.to });
      last = line;
    }
    return { kind: 'container', name: 'codeblock', from: node.from, to: last.to, children };
  }

  function tableToken(node: SyntaxNode): Token | null {
    if (!startsLine(source, node.from)) return null;
    const meta: Record<string, string> = {};
    const affiliated = affiliatedOf(node);
    if (affiliated.caption) meta.caption = affiliated.caption;
    if (affiliated.name) meta.id = affiliated.name;
    return { kind: 'container', name: 'mdtable', from: node.from, to: node.to, meta };
  }

  // ---- Lists -------------------------------------------------------------

  const TASKS: Record<string, string> = { ' ': 'todo', x: 'done', X: 'done', '-': 'doing' };

  // The bullet, with the checkbox that may follow it.
  function itemToken(node: SyntaxNode): Token | null {
    if (node.parent?.name !== 'ListItem') return null;
    const marker = text(node);
    const meta: Record<string, string> = { marker };
    let to = node.to;
    const box = node.nextSibling?.name === 'Checkbox' ? node.nextSibling : null;
    if (box) {
      const char = source[box.from + 1];
      meta.task = TASKS[char];
      meta.box = char;
      to = box.to;
    }
    return { kind: 'item', name: /^\d/.test(marker) ? 'number' : 'bullet', from: node.from, to, meta };
  }

  return region(0, source.length);
}
