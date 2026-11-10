import type { SyntaxNodeRef } from '@lezer/common';
import { Rule, RuleContext } from '../../core/language';
import { Range, Token } from '../../core/tokens';
import { STATE_OF } from '../../../tasks/syntax.js';
import { markdownTree } from '../../../source-trees.js';
import {
  fence, footnoteDefinition, heading, hr, linkDefinition, table, TASK, taskProgress
} from './block';
import {
  autolink, bareUrl, bracket, cite, comment, emphasis, escape, image, linebreak, shortcode, taskmeta
} from './inline';
import { lineEndAt, lineStartAt, parseAttributes } from './scan';

const ATTRIBUTE_BLOCK = /\{([^{}\n]*)\}/y;
const HEADING = /^ATXHeading[1-6]$/;

// `==mark==` and the inline footnote `^[note]` are not in the Lezer grammar; the emphasis rule reads them.
const mark: Rule = (source, pos, ctx) => {
  const char = source[pos];
  if (char === '=' || (char === '^' && source[pos + 1] === '[')) return emphasis(source, pos, ctx);
  return null;
};

// Constructs the Lezer tree does not model, found by position in the text between its nodes.
// Line-start blocks first, as in the rule list they replace.
const RESIDUAL: Rule[] = [table, footnoteDefinition, linkDefinition, linebreak, escape, comment, shortcode, bareUrl, taskmeta, mark];

function inner(range: { from: number; to: number }): Range {
  return { from: range.from, to: range.to };
}

// Builds the visual Token tree for a Markdown or Quarto source from the shared Pandoc-Markdown
// Lezer parse. Offsets are offsets in `source`.
export function tokenizeMarkdown(source: string): Token[] {
  const { tree, frontMatter } = markdownTree(source);

  const context: RuleContext = {
    source,
    depth: 1,
    mode: 'markup',
    parse: (from, to) => region(from, to)
  };

  // Tokens for the nodes that lie in [from, to], outermost first, in order and without overlap.
  // A node that reaches outside the range is entered but makes no token, except a block quote,
  // which is cut at the end of the range (a quote's lines are parsed one at a time).
  function collect(from: number, to: number): Token[] {
    const found: Token[] = [];
    let end = from;
    tree.iterate({
      from,
      to,
      enter(ref: SyntaxNodeRef) {
        if (ref.to <= end) return false;
        const fits = ref.from >= end && ref.to <= to;
        if (!fits && !(ref.name === 'Blockquote' && ref.from >= end)) return true;
        const token = build(ref, to);
        if (!token || token.to <= token.from || token.to > to) return true;
        found.push(token);
        end = token.to;
        return false;
      }
    });
    return found;
  }

  function region(from: number, to: number): Token[] {
    const tokens: Token[] = [];
    if (from >= to) return tokens;
    const known = collect(from, to);
    let next = 0;
    let pos = from;
    let textFrom = from;

    const flush = (end: number) => {
      if (end > textFrom) tokens.push({ kind: 'text', from: textFrom, to: end });
    };

    while (pos < to) {
      while (next < known.length && known[next].from < pos) next++;
      let token: Token | null = null;
      if (next < known.length && known[next].from === pos) {
        token = known[next++];
      } else {
        for (const rule of RESIDUAL) {
          const found = rule(source, pos, context);
          if (found && found.to > found.from && found.to <= to) {
            token = found;
            break;
          }
        }
      }
      if (!token) {
        pos++;
        continue;
      }
      flush(token.from);
      tokens.push(token);
      pos = token.to;
      textFrom = pos;
    }
    flush(to);
    return tokens;
  }

  // A rule applied at the start of a node: the Lezer parse says a construct is there, the rule
  // shapes the token.
  function viaRule(rule: Rule, ref: SyntaxNodeRef): Token | null {
    return rule(source, ref.from, context);
  }

  function build(ref: SyntaxNodeRef, limit: number): Token | null {
    const name = ref.name;
    if (HEADING.test(name)) return viaRule(heading, ref);
    switch (name) {
      case 'SetextHeading1':
      case 'SetextHeading2':
        return setextToken(ref);
      case 'HorizontalRule':
        return viaRule(hr, ref);
      case 'FencedCode':
        return viaRule(fence, ref);
      case 'CodeBlock':
        return { kind: 'container', name: 'codeblock', from: lineStartAt(source, ref.from), to: ref.to };
      case 'FencedDiv':
        return divToken(ref);
      case 'Table':
        return viaRule(table, ref);
      case 'Blockquote':
        return quoteToken(ref, limit);
      case 'ListMark':
        return ref.node.parent?.name === 'ListItem' ? itemToken(ref) : null;
      case 'LinkReference':
      case 'Link':
        return viaRule(footnoteDefinition, ref) ?? viaRule(linkDefinition, ref) ?? (name === 'Link' ? viaRule(bracket, ref) : null);
      case 'Image':
        return viaRule(image, ref);
      case 'Citation': {
        const first = ref.node.firstChild;
        return first?.name === 'CitationMark' ? viaRule(bracket, ref) : viaRule(cite, ref);
      }
      case 'CitationKey':
      case 'CrossRef':
        return viaRule(cite, ref);
      case 'Autolink':
        return viaRule(autolink, ref);
      case 'InlineCode':
        return codeToken(ref);
      case 'InlineMath':
        return mathToken(ref, false);
      case 'DisplayMath':
        return mathToken(ref, true);
      case 'Emphasis':
        return wrapToken(ref, 'emph', 'EmphasisMark');
      case 'StrongEmphasis':
        return wrapToken(ref, 'strong', 'EmphasisMark');
      case 'Strikethrough':
        return wrapToken(ref, 'strike', 'StrikethroughMark');
      case 'Subscript':
        return wrapToken(ref, 'sub', 'SubscriptMark');
      case 'Superscript':
        return wrapToken(ref, 'sup', 'SuperscriptMark');
      default:
        return null;
    }
  }

  // Inline token around children parsed from the text between the first and last marker.
  function wrapToken(ref: SyntaxNodeRef, name: string, markName: string): Token | null {
    const marks = ref.node.getChildren(markName);
    if (marks.length < 2) return null;
    const body = { from: marks[0].to, to: marks[marks.length - 1].from };
    if (body.to < body.from) return null;
    return { kind: 'command', name, from: ref.from, to: ref.to, body, children: region(body.from, body.to) };
  }

  function codeToken(ref: SyntaxNodeRef): Token | null {
    const marks = ref.node.getChildren('CodeMark');
    if (marks.length < 2) return null;
    let from = marks[0].to;
    let to = marks[marks.length - 1].from;
    const content = source.slice(from, to);
    // One space is stripped from each side when both are present, so `` ` x ` `` shows x.
    if (content.length >= 2 && content[0] === ' ' && content[content.length - 1] === ' ' && content.trim() !== '') {
      from++;
      to--;
    }
    return { kind: 'command', name: 'code', from: ref.from, to: ref.to, body: { from, to } };
  }

  function mathToken(ref: SyntaxNodeRef, display: boolean): Token | null {
    const marks = ref.node.getChildren(display ? 'DisplayMathMark' : 'InlineMathMark');
    if (marks.length < 2) return null;
    const first = marks[0];
    const last = marks[marks.length - 1];
    const body = { from: first.to, to: last.from };
    if (!display) {
      return { kind: 'math', from: ref.from, to: ref.to, display: false, body, meta: { open: '$', close: '$' } };
    }
    let to = last.to;
    const meta: Record<string, string> = { open: '$$', close: '$$' };
    // A trailing `{#eq-x}` belongs to the equation only when it carries an id.
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
    return { kind: 'math', from: ref.from, to, display: true, body, meta };
  }

  function setextToken(ref: SyntaxNodeRef): Token | null {
    const underline = ref.node.getChild('HeaderMark');
    if (!underline) return null;
    const body = { from: ref.from, to: Math.max(ref.from, lineStartAt(source, underline.from) - 1) };
    return {
      kind: 'heading',
      level: ref.name === 'SetextHeading1' ? 1 : 2,
      from: ref.from,
      to: lineEndAt(source, underline.from),
      body,
      children: region(body.from, body.to)
    };
  }

  function divToken(ref: SyntaxNodeRef): Token | null {
    const open = ref.node.firstChild;
    const attributes = ref.node.getChild('Attributes');
    if (!open || open.name !== 'FencedDivMark' || !attributes) return null;
    const spec = source.slice(attributes.from, attributes.to);
    const attrText = spec.startsWith('{') ? spec.slice(1, -1) : `.${spec.split(/\s+/)[0]}`;
    const parsed = parseAttributes(attrText);

    const headEnd = lineEndAt(source, open.from);
    const last = ref.node.lastChild;
    const closed = !!last && last.from !== open.from && last.name === 'FencedDivMark' && last.from > headEnd;
    const bodyFrom = Math.min(headEnd + 1, source.length);
    let bodyTo = ref.to;
    let to = ref.to;
    if (closed && last) {
      const closeStart = lineStartAt(source, last.from);
      bodyTo = closeStart - 1 >= bodyFrom ? closeStart - 1 : bodyFrom;
      to = lineEndAt(source, last.to);
    }
    const meta: Record<string, string> = { attrs: attrText, classes: parsed.classes.join(' ') };
    if (parsed.id !== undefined) meta.id = parsed.id;
    return {
      kind: 'container',
      name: 'div',
      from: ref.from,
      to,
      body: inner({ from: bodyFrom, to: bodyTo }),
      children: region(bodyFrom, bodyTo),
      meta
    };
  }

  // Block quote: the marker of every line is a child, and the rest of the line is parsed after it.
  function quoteToken(ref: SyntaxNodeRef, limit: number): Token | null {
    const children: Token[] = [];
    const stop = Math.min(ref.to, limit);
    let end = ref.from;
    let cursor = ref.from;
    while (cursor < stop) {
      const lineEnd = Math.min(lineEndAt(source, cursor), stop);
      const match = /^[ \t]*>[ \t]?/.exec(source.slice(cursor, lineEnd));
      if (!match) break;
      const markerFrom = cursor + match[0].indexOf('>');
      const markerTo = cursor + match[0].length;
      children.push({ kind: 'raw', name: 'marker', from: markerFrom, to: markerTo });
      if (markerTo < lineEnd) children.push(...region(markerTo, lineEnd));
      end = lineEnd;
      cursor = lineEnd + 1;
    }
    if (end <= ref.from) return null;
    return { kind: 'container', name: 'blockquote', from: ref.from, to: end, children };
  }

  // List item marker, with the task box that may follow. Only the marker (and box) is the token.
  function itemToken(ref: SyntaxNodeRef): Token | null {
    const marker = source.slice(ref.from, ref.to);
    const meta: Record<string, string> = { marker };
    let to = ref.to;
    TASK.lastIndex = ref.to;
    const task = TASK.exec(source);
    if (task) {
      meta.task = (STATE_OF as Record<string, string>)[task[1]];
      meta.box = task[1];
      const progress = taskProgress(source, ref.from);
      if (progress) meta.progress = progress;
      to = ref.to + task[0].length;
    }
    return { kind: 'item', name: /^\d/.test(marker) ? 'number' : 'bullet', from: ref.from, to, meta };
  }

  const tokens: Token[] = [];
  let start = 0;
  if (frontMatter) {
    const [, end] = frontMatter;
    const bodyFrom = Math.min(lineEndAt(source, 0) + 1, source.length);
    tokens.push({
      kind: 'container',
      name: 'frontmatter',
      from: 0,
      to: end,
      body: { from: bodyFrom, to: Math.max(bodyFrom, lineStartAt(source, end)) }
    });
    start = end;
  }
  tokens.push(...region(start, source.length));
  return tokens;
}
