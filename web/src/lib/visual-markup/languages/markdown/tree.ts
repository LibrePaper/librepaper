import type { SyntaxNode, SyntaxNodeRef } from '@lezer/common';
import { Range, Token } from '../../core/tokens';
import { FIELD, STATE_OF, validateField } from '../../../tasks/syntax.js';
import { markdownTree } from '../../../source-trees.js';
import { lineEndAt, lineStartAt, startsLine } from '../../core/lines';
import {
  aloneOnLine, attributesOf, bareCitation, bareUrlText, bracketCitation, codeMeta,
  optionLine, skipBlanks, trimEnd, unangle
} from './shape';

const HEADING = /^ATXHeading[1-6]$/;
const TABLES = new Set(['Table', 'GridTable']);
const STATES = STATE_OF as Record<string, string>;

function inner(range: { from: number; to: number }): Range {
  return { from: range.from, to: range.to };
}

// Builds the visual Token tree for a Markdown or Quarto source from the shared Pandoc-Markdown
// Lezer parse. Offsets are offsets in `source`. The walk maps each node to a token; the text
// between tokens is text. Nothing here looks for syntax in the source: the grammar has a node
// for every construct, and the mapper reads the nodes and, for one node, its own text.
export function tokenizeMarkdown(source: string): Token[] {
  const { tree, frontMatter } = markdownTree(source);

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

  // The tokens in [from, to] with a text token for every gap between them.
  function region(from: number, to: number): Token[] {
    const tokens: Token[] = [];
    if (from >= to) return tokens;
    let pos = from;
    for (const token of collect(from, to)) {
      if (token.from > pos) tokens.push({ kind: 'text', from: pos, to: token.from });
      tokens.push(token);
      pos = token.to;
    }
    if (pos < to) tokens.push({ kind: 'text', from: pos, to });
    return tokens;
  }

  function build(ref: SyntaxNodeRef, limit: number): Token | null {
    const name = ref.name;
    if (HEADING.test(name)) return headingToken(ref);
    switch (name) {
      case 'SetextHeading1':
      case 'SetextHeading2':
        return setextToken(ref);
      case 'HorizontalRule':
        return startsLine(source, ref.from) ? { kind: 'command', name: 'hr', from: ref.from, to: lineEndAt(source, ref.from) } : null;
      case 'FencedCode':
        return fenceToken(ref);
      case 'CodeBlock':
        return { kind: 'container', name: 'codeblock', from: lineStartAt(source, ref.from), to: ref.to };
      case 'FencedDiv':
        return divToken(ref);
      case 'Table':
      case 'GridTable':
      case 'TableCaption':
        return tableToken(ref);
      case 'Blockquote':
        return quoteToken(ref, limit);
      case 'ListMark':
        return itemToken(ref);
      case 'FootnoteDefinition':
        return footnoteDefinitionToken(ref);
      case 'FootnoteReference':
        return { kind: 'command', name: 'fnref', from: ref.from, to: ref.to, meta: { label: source.slice(ref.from + 2, ref.to - 1) } };
      case 'LinkReference':
        return linkDefinitionToken(ref);
      case 'Link':
        return linkToken(ref);
      case 'Image':
        return imageToken(ref);
      case 'Citation':
        return ref.node.firstChild?.name === 'CitationMark' ? bracketToken(ref) : null;
      case 'CitationKey':
      case 'CrossRef':
        return keyToken(ref);
      case 'Autolink':
        return autolinkToken(ref);
      case 'URL':
        return urlToken(ref);
      case 'Shortcode':
        return shortcodeToken(ref);
      case 'TaskField':
      case 'TaskTag':
        return taskMetaToken(ref);
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
      case 'Highlight':
        return wrapToken(ref, 'mark', 'HighlightMark');
      case 'InlineNote':
        return wrapToken(ref, 'fnref', 'InlineNoteMark');
      case 'Escape':
        return { kind: 'raw', name: 'escape', from: ref.from, to: ref.to, meta: { text: source.slice(ref.from + 1, ref.to) } };
      case 'HardBreak':
        return breakToken(ref);
      case 'Comment':
        return { kind: 'comment', from: ref.from, to: ref.to, body: { from: ref.from + 4, to: ref.to - 3 } };
      case 'CommentBlock':
        return commentToken(ref);
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

  // Display math takes a following `{#eq-x}` when it carries an id: a child of block math, a
  // sibling after math inside a paragraph.
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
    let attributes = ref.node.getChild('Attributes');
    if (!attributes) {
      const next = ref.node.nextSibling;
      if (next?.name === 'Attributes' && source.slice(ref.to, next.from).trim() === '') attributes = next;
    }
    if (attributes) {
      const parsed = attributesOf(source, attributes);
      if (parsed.id !== undefined) {
        meta.id = parsed.id;
        to = attributes.to;
      }
    }
    return { kind: 'math', from: ref.from, to, display: true, body, meta };
  }

  // ATX heading. A trailing `{#id .class}` is hidden by the default concealment (it sits
  // after the body) and read into meta.
  function headingToken(ref: SyntaxNodeRef): Token | null {
    const marks = ref.node.getChildren('HeaderMark');
    if (!marks.length) return null;
    const lineEnd = lineEndAt(source, ref.from);
    const from = skipBlanks(source, marks[0].to, lineEnd);
    let to = marks.length > 1 ? Math.max(from, marks[marks.length - 1].from) : lineEnd;
    to = trimEnd(source, from, to);

    const meta: Record<string, string> = {};
    const attributes = ref.node.getChildren('Attributes').pop();
    if (attributes && attributes.from >= from && attributes.to === to) {
      const parsed = attributesOf(source, attributes);
      if (parsed.id !== undefined) meta.id = parsed.id;
      if (parsed.classes.length) meta.classes = parsed.classes.join(' ');
      to = trimEnd(source, from, attributes.from);
    }
    return {
      kind: 'heading',
      level: Number(ref.name.slice(-1)),
      from: ref.from,
      to: lineEnd,
      body: { from, to },
      children: region(from, to),
      meta: Object.keys(meta).length ? meta : undefined
    };
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

  // Fenced code. Quarto executable cells (```{r}) take their options from `#|` lines at the top of
  // the code, which the header widget replaces together with the opening fence.
  function fenceToken(ref: SyntaxNodeRef): Token | null {
    const marks = ref.node.getChildren('CodeMark');
    if (!marks.length) return null;
    const closed = marks.length > 1;
    const infoNode = ref.node.getChild('CodeInfo');
    const info = infoNode ? source.slice(infoNode.from, infoNode.to).trim() : '';
    const bodyFrom = Math.min(lineEndAt(source, marks[0].from) + 1, source.length);

    let contentFrom = bodyFrom;
    const optionLines: string[] = [];
    const code = ref.node.getChild('CodeText');
    if (info.startsWith('{') && code) {
      let line = bodyFrom;
      while (line < code.to) {
        const lineEnd = lineEndAt(source, line);
        const text = source.slice(line, lineEnd);
        if (!optionLine(text)) break;
        optionLines.push(text);
        line = Math.min(lineEnd + 1, source.length);
        contentFrom = line;
      }
    }

    const closeStart = closed ? lineStartAt(source, marks[marks.length - 1].from) : ref.to;
    const bodyTo = !closed ? ref.to : closeStart - 1 >= contentFrom ? closeStart - 1 : contentFrom;
    return {
      kind: 'container',
      name: 'codeblock',
      from: ref.from,
      to: closed ? lineEndAt(source, marks[marks.length - 1].from) : ref.to,
      body: { from: contentFrom, to: bodyTo },
      meta: codeMeta(info, optionLines)
    };
  }

  function divToken(ref: SyntaxNodeRef): Token | null {
    const open = ref.node.firstChild;
    const attributes = ref.node.getChild('Attributes');
    if (!open || open.name !== 'FencedDivMark' || !attributes) return null;
    const spec = source.slice(attributes.from, attributes.to);
    const attrText = spec.startsWith('{') ? spec.slice(1, -1) : `.${spec.split(/\s+/)[0]}`;
    const parsed = attributesOf(source, attributes);

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

  // Pipe and grid tables, drawn whole. A caption paragraph (`: Caption {#tbl-x}`) directly above the
  // table, or below it after at most one blank line, belongs to the same token.
  function tableToken(ref: SyntaxNodeRef): Token | null {
    if (!startsLine(source, ref.from)) return null;
    const node = ref.node;
    if (ref.name === 'TableCaption') {
      const table = node.nextSibling;
      if (!table || !TABLES.has(table.name) || lineStartAt(source, table.from) !== lineEndAt(source, node.to) + 1) return null;
      return mdtable(ref.from, table.to, node);
    }
    const caption = node.nextSibling;
    if (caption?.name === 'TableCaption') {
      const gap = source.slice(node.to, caption.from);
      if (gap.trim() === '' && gap.split('\n').length <= 3) return mdtable(ref.from, caption.to, caption);
    }
    return mdtable(ref.from, ref.to, null);
  }

  function mdtable(from: number, to: number, caption: SyntaxNode | null): Token {
    const meta: Record<string, string> = {};
    const mark = caption?.getChild('TableCaptionMark');
    if (caption && mark) {
      let end = trimEnd(source, mark.to, caption.to);
      const attributes = caption.getChildren('Attributes').pop();
      if (attributes && trimEnd(source, attributes.from, caption.to) === attributes.to) {
        const parsed = attributesOf(source, attributes);
        if (parsed.id !== undefined) meta.id = parsed.id;
        end = attributes.from;
      }
      const text = source.slice(mark.to, end).trim();
      if (text) meta.caption = text;
    }
    return { kind: 'container', name: 'mdtable', from, to, meta };
  }

  // The markers of a quote's own lines, in order: nested quotes have markers of their own.
  function quoteMarks(node: SyntaxNode, found: SyntaxNode[] = []): SyntaxNode[] {
    for (let child = node.firstChild; child; child = child.nextSibling) {
      if (child.name === 'QuoteMark') found.push(child);
      else if (child.name !== 'Blockquote') quoteMarks(child, found);
    }
    return found;
  }

  // Block quote: the marker of every line is a child, and the rest of the line is parsed after it.
  // A line without a marker (a lazy continuation) ends the token.
  function quoteToken(ref: SyntaxNodeRef, limit: number): Token | null {
    const children: Token[] = [];
    const stop = Math.min(ref.to, limit);
    const marks = quoteMarks(ref.node).filter(mark => mark.from >= ref.from && mark.from < stop).sort((a, b) => a.from - b.from);
    let end = ref.from;
    let lineStart = lineStartAt(source, ref.from);
    for (const mark of marks) {
      if (lineStartAt(source, mark.from) !== lineStart) break;
      const lineEnd = Math.min(lineEndAt(source, mark.from), stop);
      const markerTo = source[mark.to] === ' ' || source[mark.to] === '\t' ? mark.to + 1 : mark.to;
      children.push({ kind: 'raw', name: 'marker', from: mark.from, to: markerTo });
      if (markerTo < lineEnd) children.push(...region(markerTo, lineEnd));
      end = lineEnd;
      lineStart = lineEnd + 1;
    }
    if (end <= ref.from) return null;
    return { kind: 'container', name: 'blockquote', from: ref.from, to: end, children };
  }

  // List item marker, with the task box that may follow. Only the marker (and box) is the token.
  function itemToken(ref: SyntaxNodeRef): Token | null {
    const item = ref.node.parent;
    if (item?.name !== 'ListItem') return null;
    const marker = source.slice(ref.from, ref.to);
    const meta: Record<string, string> = { marker };
    let to = ref.to;
    const task = ref.node.nextSibling;
    const box = task?.name === 'Task' ? task.getChild('TaskMarker') : null;
    if (task && box) {
      const char = source[box.from + 1];
      meta.task = STATES[char];
      meta.box = char;
      const progress = taskProgress(item, task);
      if (progress) meta.progress = progress;
      to = box.to;
    }
    return { kind: 'item', name: /^\d/.test(marker) ? 'number' : 'bullet', from: ref.from, to, meta };
  }

  // "done/total" over the task items nested in the item, or null when it has none. Cancelled tasks are
  // left out of both numbers.
  function taskProgress(item: SyntaxNode, own: SyntaxNode): string | null {
    let done = 0;
    let total = 0;
    tree.iterate({
      from: item.from,
      to: item.to,
      enter(ref: SyntaxNodeRef) {
        if (ref.name !== 'Task' || ref.from <= item.from || ref.to > item.to) return;
        const box = ref.node.getChild('TaskMarker');
        if (ref.from === own.from || !box || ref.node.prevSibling?.name !== 'ListMark') return false;
        const state = STATES[source[box.from + 1]];
        if (state !== 'cancelled') {
          total++;
          if (state === 'done') done++;
        }
        return false;
      }
    });
    return total > 0 ? `${done}/${total}` : null;
  }

  // Task metadata chips. The grammar finds them on a task's first line; the mapper gives a field its
  // key, value and state, and a warning for an invalid value or a key already used on the line.
  function taskMetaToken(ref: SyntaxNodeRef): Token | null {
    const text = source.slice(ref.from, ref.to);
    if (ref.name === 'TaskTag') return { kind: 'command', name: 'tasktag', from: ref.from, to: ref.to, meta: { tag: text.slice(1) } };

    let task: SyntaxNode | null = ref.node.parent;
    while (task && task.name !== 'Task') task = task.parent;
    const box = task?.getChild('TaskMarker');
    if (!task || !box || task.prevSibling?.name !== 'ListMark') return null;
    FIELD.lastIndex = 0;
    const field = FIELD.exec(text);
    if (!field) return null;
    const key = field[1];
    const value = field[3];
    let warning: string | null = validateField(key, value);
    if (warning === null) {
      const found = { duplicate: false };
      tree.iterate({
        from: task.from,
        to: ref.from,
        enter(earlier: SyntaxNodeRef) {
          if (earlier.name === 'TaskField' && earlier.to <= ref.from && source.startsWith(`${key}:`, earlier.from)) found.duplicate = true;
        }
      });
      if (found.duplicate) warning = `Duplicate ${key}.`;
    }
    const meta: Record<string, string> = { key, value, state: STATES[source[box.from + 1]] };
    if (warning) meta.warning = warning;
    return { kind: 'command', name: 'taskfield', from: ref.from, to: ref.to, meta };
  }

  // Footnote definition `[^1]: text`, with indented continuation blocks.
  function footnoteDefinitionToken(ref: SyntaxNodeRef): Token | null {
    const label = ref.node.getChild('FootnoteLabel');
    if (!label) return null;
    const end = lineEndAt(source, ref.to);
    const bodyFrom = Math.min(label.nextSibling?.from ?? end, end);
    return {
      kind: 'container',
      name: 'fndef',
      from: ref.from,
      to: end,
      body: { from: bodyFrom, to: end },
      children: region(bodyFrom, end),
      meta: { label: source.slice(label.from + 2, label.to - 1) }
    };
  }

  // Link reference definition `[ref]: url`. It is source only, hidden from the view.
  function linkDefinitionToken(ref: SyntaxNodeRef): Token | null {
    const label = ref.node.getChild('LinkLabel');
    if (!label || !startsLine(source, ref.from)) return null;
    return { kind: 'raw', name: 'linkdef', from: ref.from, to: lineEndAt(source, ref.to), meta: { label: source.slice(label.from + 1, label.to - 1) } };
  }

  // The address of a link or image: the URL node without angle brackets, or empty.
  function destination(node: SyntaxNode, open: SyntaxNode): Range {
    const url = node.getChild('URL');
    if (!url) return { from: open.to, to: open.to };
    const { skip, text } = unangle(source.slice(url.from, url.to));
    return { from: url.from + skip, to: url.from + skip + text.length };
  }

  // Link [text](url), reference link [text][ref] and span [text]{.class}. A bracket with none of
  // those is plain text.
  function linkToken(ref: SyntaxNodeRef): Token | null {
    const node = ref.node;
    const marks = node.getChildren('LinkMark');
    if (marks.length < 2) return null;
    const text: Range = { from: marks[0].to, to: marks[1].from };
    if (marks.length >= 4 && source[marks[2].from] === '(') {
      const url = destination(node, marks[2]);
      return { kind: 'command', name: 'href', from: ref.from, to: ref.to, body: text, args: [url, text], children: region(text.from, text.to) };
    }
    const label = node.getChild('LinkLabel');
    if (label) {
      // [text][] uses the text as its own label.
      const range: Range = label.to === label.from + 2 ? text : { from: label.from + 1, to: label.to - 1 };
      return { kind: 'command', name: 'href', from: ref.from, to: ref.to, body: text, args: [range, text], meta: { ref: 'true' }, children: region(text.from, text.to) };
    }
    return spanToken(node, text);
  }

  // The bracketed text, with the attribute list that directly follows it.
  function spanToken(node: SyntaxNode, text: Range): Token | null {
    const next = node.nextSibling;
    if (next?.name !== 'Attributes' || next.from !== node.to) return null;
    const attributes = attributesOf(source, next);
    const meta: Record<string, string> = { classes: attributes.classes.join(' ') };
    if (attributes.id !== undefined) meta.id = attributes.id;
    return { kind: 'command', name: 'span', from: node.from, to: next.to, body: text, children: region(text.from, text.to), meta };
  }

  // True when the image is the only content of its paragraph and the paragraph starts its line:
  // then it is a figure, drawn as a block.
  function isFigure(node: SyntaxNode, to: number): boolean {
    const paragraph = node.parent;
    return paragraph?.name === 'Paragraph' && paragraph.from === node.from && startsLine(source, node.from)
      && source.slice(to, paragraph.to).trim() === '';
  }

  // Image `![alt](src){#fig-x width=50%}`: a figure when alone, otherwise inline.
  function imageToken(ref: SyntaxNodeRef): Token | null {
    const node = ref.node;
    const marks = node.getChildren('LinkMark');
    if (marks.length < 4 || source[marks[2].from] !== '(') return null;
    const url = destination(node, marks[2]);
    const next = node.nextSibling;
    const attributesNode = next?.name === 'Attributes' && next.from === ref.to ? next : null;
    const to = attributesNode ? attributesNode.to : ref.to;
    const meta: Record<string, string> = {
      src: source.slice(url.from, url.to),
      alt: source.slice(marks[0].to, marks[1].from),
      attrs: attributesNode ? source.slice(attributesNode.from + 1, attributesNode.to - 1) : ''
    };
    const attributes = attributesOf(source, attributesNode);
    if (attributes.id !== undefined) meta.id = attributes.id;
    if (attributes.classes.length) meta.classes = attributes.classes.join(' ');
    return { kind: 'command', name: isFigure(node, to) ? 'mdfigure' : 'mdimage', from: ref.from, to, meta };
  }

  // Bracketed citation [@a; @b] or cross-reference [-@fig-x]. A directly following attribute list
  // makes the brackets a span.
  function bracketToken(ref: SyntaxNodeRef): Token | null {
    const marks = ref.node.getChildren('CitationMark');
    if (marks.length < 2) return null;
    const text: Range = { from: marks[0].to, to: marks[marks.length - 1].from };
    const span = spanToken(ref.node, text);
    if (span) return span;
    const shaped = bracketCitation(source.slice(text.from, text.to));
    if (shaped.name === 'xref') return { kind: 'command', name: 'xref', from: ref.from, to: ref.to, meta: shaped.meta };
    return { kind: 'command', name: 'mdcite', from: ref.from, to: ref.to, body: text, meta: shaped.meta };
  }

  // @key is a citation, @fig-x a cross-reference.
  function keyToken(ref: SyntaxNodeRef): Token | null {
    const shaped = bareCitation(source.slice(ref.from + 1, ref.to));
    return { kind: 'command', name: shaped.name, from: ref.from, to: ref.to, meta: shaped.meta };
  }

  // Autolink <https://...>: the brackets are concealed, the address is the body. Only web, ftp and
  // mail addresses are links here.
  function autolinkToken(ref: SyntaxNodeRef): Token | null {
    const url = ref.node.getChild('URL');
    if (!url || !/^(?:https?|ftp|mailto):/.test(source.slice(url.from, url.to))) return null;
    return { kind: 'command', name: 'url', from: ref.from, to: ref.to, body: inner(url), args: [inner(url)] };
  }

  // A bare https:// address. Trailing punctuation and an unbalanced parenthesis are the sentence's.
  // The addresses of links and definitions are not bare.
  function urlToken(ref: SyntaxNodeRef): Token | null {
    const parent = ref.node.parent?.name;
    if (parent === 'LinkReference' || parent === 'Autolink') return null;
    const before = ref.node.prevSibling;
    if (before?.name === 'LinkMark' && source[before.from] === '(') return null;
    const text = bareUrlText(source.slice(ref.from, ref.to));
    if (text === null) return null;
    const url: Range = { from: ref.from, to: ref.from + text.length };
    return { kind: 'command', name: 'url', from: ref.from, to: url.to, body: url, args: [url] };
  }

  // Quarto shortcode {{< name args >}}.
  function shortcodeToken(ref: SyntaxNodeRef): Token | null {
    const marks = ref.node.getChildren('ShortcodeMark');
    if (marks.length < 2) return null;
    const content = source.slice(marks[0].to, marks[1].from).trim();
    const name = /^[\w-]+/.exec(content)?.[0] ?? '';
    return {
      kind: 'command',
      name: 'shortcode',
      from: ref.from,
      to: ref.to,
      meta: { name, args: content.slice(name.length).trim(), alone: String(aloneOnLine(source, ref.from, ref.to)) }
    };
  }

  // A line break: a backslash, or the spaces before the newline. The newline itself is not part of the token.
  function breakToken(ref: SyntaxNodeRef): Token | null {
    const to = source[ref.from] === '\\' ? ref.from + 1 : source[ref.to - 1] === '\n' ? ref.to - 1 : ref.to;
    return to > ref.from ? { kind: 'linebreak', from: ref.from, to } : null;
  }

  // HTML comment block. The markers are concealed, the text is drawn as a comment.
  function commentToken(ref: SyntaxNodeRef): Token | null {
    const close = source.indexOf('-->', ref.from + 4);
    if (close === -1 || close + 3 > ref.to) return null;
    return { kind: 'comment', from: ref.from, to: close + 3, body: { from: ref.from + 4, to: close } };
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
