import type { SyntaxNode } from '@lezer/common';
import { typstTree } from '../../../source-trees.js';
import { atLineStart, inner } from '../../core/scanner';
import type { Range, Token } from '../../core/tokens';
import { isSpace, isWord, lineEnd, shapeCall, statementToken, type CallParts } from './rules';

// The Token tree of a Typst source from Typst's own parse (typst-syntax), shaped like the tokens
// the regex rules in rules.ts produce, so every consumer of the tokens is unchanged. The tree
// replaces the scanning only: markup is walked like the 'markup' rules, and the arguments of a
// call like the 'code' rules.

// Same bound as the Tokenizer: a run deeper than this has no tokens.
const MAX_DEPTH = 12;

const SEGMENTS = /^[a-zA-Z][\w-]*(?:\.[a-zA-Z][\w-]*)*$/;

const STATEMENTS = new Set([
  'LetBinding', 'SetRule', 'ShowRule', 'ModuleImport', 'ModuleInclude', 'Conditional',
  'ForLoop', 'WhileLoop', 'Contextual', 'FuncReturn', 'LoopBreak', 'LoopContinue'
]);

const NOT_ARGUMENT = new Set(['LeftParen', 'RightParen', 'Space', 'Comma', 'LineComment', 'BlockComment', 'Error']);

const LIGATURES: Record<string, string> = { '---': '—', '--': '–', '...': '…' };

const MARKERS: Record<string, string> = { ListItem: 'bullet', EnumItem: 'number', TermItem: 'term' };

interface Chain {
  // The whole dotted name, method segments after calls included.
  name: string;
  // The dotted name before the first argument list, and where it ends.
  head: string;
  headEnd: number;
  args: SyntaxNode[];
}

export function tokenizeTypst(source: string): Token[] | null {
  const tree = typstTree(source);
  if (!tree) return null;

  const text = (node: Range): string => source.slice(node.from, node.to);
  const child = (node: SyntaxNode, name: string): SyntaxNode | null => {
    for (let c = node.firstChild; c; c = c.nextSibling) if (c.name === name) return c;
    return null;
  };
  const fill = (tokens: Token[], from: number, to: number): Token[] => {
    const out: Token[] = [];
    let pos = from;
    for (const token of tokens) {
      if (token.from < pos) continue;
      if (token.from > pos) out.push({ kind: 'text', from: pos, to: token.from });
      out.push(token);
      pos = token.to;
    }
    if (pos < to) out.push({ kind: 'text', from: pos, to });
    return out;
  };

  // Tokens over [from, to) of one run at `depth`, from the children of `node` (markup mode).
  const markup = (node: SyntaxNode | null, from: number, to: number, depth: number): Token[] => {
    if (depth > MAX_DEPTH || from >= to) return [];
    if (!node) return [{ kind: 'text', from, to }];
    const out: Token[] = [];
    emitMarkup(node, out, depth);
    return fill(out, from, to);
  };

  const codeRun = (nodes: SyntaxNode[], from: number, to: number, depth: number): Token[] => {
    if (depth > MAX_DEPTH || from >= to) return [];
    const out: Token[] = [];
    for (const node of nodes) emitCode(node, out, depth);
    return fill(out, from, to);
  };

  const commentToken = (node: SyntaxNode): Token | null => {
    if (node.name === 'LineComment') return { kind: 'comment', from: node.from, to: node.to, body: { from: node.from + 2, to: node.to } };
    if (node.to - node.from < 4 || !text(node).endsWith('*/')) return null;
    return { kind: 'comment', from: node.from, to: node.to, body: { from: node.from + 2, to: node.to - 2 } };
  };

  const escapeToken = (node: SyntaxNode): Token => ({
    kind: 'raw', name: 'escape', from: node.from, to: node.to, body: { from: node.from + 1, to: node.to }
  });

  const mathToken = (node: SyntaxNode): Token | null => {
    const open = node.firstChild;
    const close = node.lastChild;
    if (!open || !close || open === close || open.name !== 'Dollar' || close.name !== 'Dollar') return null;
    const body = inner(node, 1, 1);
    if (body.to <= body.from) return null;
    const content = text(body);
    if (/\n[ \t]*\n/.test(content)) return null;
    const display = /^\s/.test(content.slice(0, 1)) && /\s$/.test(content.slice(-1));
    return { kind: 'math', from: node.from, to: node.to, display, body, meta: { open: '$', close: '$' } };
  };

  const groupToken = (node: SyntaxNode, from: number, depth: number): Token | null => {
    const last = node.lastChild;
    if (!last || last.name !== 'RightBracket' || last === node.firstChild) return null;
    const body = inner(node, 1, 1);
    return {
      kind: 'group',
      name: 'content',
      from,
      to: node.to,
      body,
      children: markup(child(node, 'Markup'), body.from, body.to, depth + 1)
    };
  };

  // The dotted name of a call target and the argument lists applied to it, or null when the
  // target is not a plain name.
  const chainOf = (node: SyntaxNode): Chain | null => {
    if (node.name === 'Ident') return { name: text(node), head: text(node), headEnd: node.to, args: [] };
    if (node.name === 'FieldAccess') {
      const target = node.firstChild;
      const field = node.lastChild;
      if (!target || !field || field.name !== 'Ident') return null;
      const base = chainOf(target);
      if (!base) return null;
      const name = `${base.name}.${text(field)}`;
      return base.args.length
        ? { ...base, name }
        : { name, head: name, headEnd: node.to, args: [] };
    }
    if (node.name === 'FuncCall') {
      const target = node.firstChild;
      const args = node.lastChild;
      if (!target || !args || args.name !== 'Args') return null;
      const base = chainOf(target);
      return base ? { ...base, args: [...base.args, args] } : null;
    }
    return null;
  };

  // A call token for a call, a bare name or a field access; `hash` is the `#` that starts it in markup.
  const callToken = (node: SyntaxNode, hash: SyntaxNode | null, depth: number): Token | null => {
    const chain = chainOf(node);
    if (!chain || !SEGMENTS.test(chain.name) || !SEGMENTS.test(chain.head)) return null;
    // A bare name in code is not a call.
    if (!hash && !chain.args.length) return null;
    const from = hash ? hash.from : node.from;

    if (hash && (chain.head === 'sym' || chain.head === 'emoji' || chain.head.startsWith('sym.') || chain.head.startsWith('emoji.'))) {
      const [name, ...rest] = chain.head.split('.');
      return { kind: 'command', name, from, to: chain.headEnd, meta: { path: rest.join('.') } };
    }

    const parts: CallParts = { name: chain.name, from, to: node.to, parens: [], trailing: [], args: [], named: {} };
    const interiors: SyntaxNode[][] = [];
    const blocks: SyntaxNode[] = [];
    for (const list of chain.args) {
      let nodes: SyntaxNode[] | null = null;
      for (let c = list.firstChild; c; c = c.nextSibling) {
        if (c.name === 'LeftParen') {
          nodes = [];
          parts.parens.push({ from: c.from, to: c.to });
          continue;
        }
        if (c.name === 'RightParen') {
          if (!nodes) continue;
          parts.parens[parts.parens.length - 1].to = c.to;
          interiors.push(nodes);
          nodes = null;
          continue;
        }
        if (nodes) {
          nodes.push(c);
          if (NOT_ARGUMENT.has(c.name)) continue;
          const key = c.name === 'Named' ? c.firstChild : null;
          if (c.name === 'Named' && key?.name === 'Ident') {
            const colon = child(c, 'Colon');
            let value = colon?.nextSibling ?? null;
            while (value && (value.name === 'Space' || value.name === 'LineComment' || value.name === 'BlockComment')) value = value.nextSibling;
            parts.named[text(key)] = { from: value ? value.from : c.to, to: c.to };
          } else {
            parts.args.push({ from: c.from, to: c.to });
          }
        } else if (c.name === 'ContentBlock') {
          if (c.lastChild?.name !== 'RightBracket' || c.lastChild === c.firstChild) return null;
          parts.trailing.push({ from: c.from, to: c.to });
          blocks.push(c);
        }
      }
      // An argument list that never closes is not a call.
      if (nodes) return null;
    }

    const argTokens = interiors.flatMap((nodes, index) => {
      const body = inner(parts.parens[index], 1, 1);
      return codeRun(nodes, body.from, body.to, depth + 1);
    });
    const trailingTokens = blocks.flatMap(block => {
      const body = inner(block, 1, 1);
      return markup(child(block, 'Markup'), body.from, body.to, depth + 1);
    });
    return shapeCall(source, parts, argTokens, trailingTokens);
  };

  const hashToken = (hash: SyntaxNode, target: SyntaxNode, depth: number): Token | null => {
    const pos = hash.from;
    switch (target.name) {
      case 'CodeBlock':
        return { kind: 'command', name: 'code', from: pos, to: target.to, body: inner(target, 1, 1), meta: { statement: 'true' } };
      case 'Parenthesized':
      case 'Array':
      case 'Dict':
        return { kind: 'command', name: 'expr', from: pos, to: target.to, body: inner(target, 1, 1), meta: { statement: 'true' } };
      case 'ContentBlock':
        return groupToken(target, pos, depth);
      case 'Ident':
      case 'FieldAccess':
      case 'FuncCall':
        return callToken(target, hash, depth);
      default:
        if (STATEMENTS.has(target.name) && target.firstChild) {
          return statementToken(source, pos, text(target.firstChild), target.to);
        }
        return null;
    }
  };

  const itemTokens = (node: SyntaxNode, out: Token[]): void => {
    const marker = node.firstChild;
    if (!marker || !atLineStart(source, node.from)) return;
    let to = marker.to;
    while (source[to] === ' ' || source[to] === '\t') to++;
    if (to === marker.to) return;

    if (node.name === 'TermItem') {
      const colon = child(node, 'Colon');
      out.push({ kind: 'item', name: 'term', from: node.from, to, body: { from: to, to: colon ? colon.from : lineEnd(source, to, source.length) } });
    } else {
      out.push({ kind: 'item', name: node.name === 'ListItem' && source[node.from] === '-' ? 'bullet' : 'number', from: node.from, to });
    }
  };

  const rawToken = (node: SyntaxNode): Token | null => {
    const open = node.firstChild;
    const close = node.lastChild;
    if (!open || !close || open === close || open.name !== 'RawDelim' || close.name !== 'RawDelim') return null;
    const n = open.to - open.from;
    const pos = node.from;
    const end = node.to;
    if (n < 3) return { kind: 'raw', name: 'inline', from: pos, to: end, body: { from: pos + n, to: end - n } };

    const lang = child(node, 'RawLang');
    const name = lang ? text(lang) : '';
    const rest = source.slice(pos + n + name.length, end - n);
    const from = pos + n + name.length + (/^\r?\n/.exec(rest)?.[0].length ?? 0);
    const to = Math.max(from, end - n - (/\r?\n$/.exec(rest)?.[0].length ?? 0));
    return { kind: 'container', name: 'codeblock', from: pos, to: end, body: { from, to }, meta: name ? { lang: name } : undefined };
  };

  const linkToken = (node: SyntaxNode): Token | null => {
    const url = text(node).replace(/[.,;:!?]+$/, '');
    if (url.length <= url.indexOf('://') + 3) return null;
    return { kind: 'command', name: 'url', from: node.from, to: node.from + url.length, args: [{ from: node.from, to: node.from + url.length }] };
  };

  const refToken = (node: SyntaxNode): Token | null => {
    const marker = child(node, 'RefMarker');
    if (!marker || marker.to - marker.from < 2 || isWord(source[node.from - 1])) return null;
    const token: Token = {
      kind: 'command',
      name: 'ref',
      from: node.from,
      to: marker.to,
      meta: { key: source.slice(marker.from + 1, marker.to) }
    };
    const supplement = child(node, 'ContentBlock');
    if (supplement && supplement.lastChild?.name === 'RightBracket' && supplement.lastChild !== supplement.firstChild) {
      token.options = [{ from: supplement.from + 1, to: supplement.to - 1 }];
      token.to = supplement.to;
    }
    return token;
  };

  const quoteToken = (node: SyntaxNode): Token => {
    const pos = node.from;
    const before = source[pos - 1];
    const opening = before === undefined || isSpace(before) || /[([{\-–—]/.test(before);
    const glyph = source[pos] === '"'
      ? (opening ? '“' : '”')
      : (opening ? '‘' : '’');
    return { kind: 'ligature', name: 'quote', from: pos, to: pos + 1, meta: { text: glyph } };
  };

  const headingToken = (node: SyntaxNode, depth: number): Token | null => {
    const marker = node.firstChild;
    if (!marker || marker.name !== 'HeadingMarker' || !atLineStart(source, node.from)) return null;
    let from = marker.to;
    while (source[from] === ' ' || source[from] === '\t') from++;
    if (from === marker.to) return null;
    const to = Math.max(node.to, from);
    return {
      kind: 'heading',
      level: marker.to - marker.from,
      from: node.from,
      to,
      body: { from, to },
      children: markup(child(node, 'Markup'), from, to, depth + 1)
    };
  };

  const emphasisToken = (node: SyntaxNode, depth: number): Token | null => {
    const open = node.firstChild;
    const close = node.lastChild;
    if (!open || !close || open === close) return null;
    if (open.name !== (node.name === 'Strong' ? 'Star' : 'Underscore') || close.name !== open.name) return null;
    const body = { from: node.from + 1, to: node.to - 1 };
    if (body.to <= body.from) return null;
    return {
      kind: 'command',
      name: node.name === 'Strong' ? 'strong' : 'emph',
      from: node.from,
      to: node.to,
      body,
      children: markup(child(node, 'Markup'), body.from, body.to, depth + 1)
    };
  };

  // Markup mode: the children of `node` become tokens in `out`. A node that is not a token of
  // its own is looked through, so a list item's text sits beside its marker.
  function emitMarkup(node: SyntaxNode, out: Token[], depth: number): void {
    for (let c = node.firstChild; c; c = c.nextSibling) {
      let token: Token | null = null;
      switch (c.name) {
        case 'Hash': {
          const target = c.nextSibling;
          if (target && target.from === c.to) {
            const made = hashToken(c, target, depth);
            if (made) out.push(made);
            c = target;
          }
          continue;
        }
        case 'LineComment':
        case 'BlockComment':
          token = commentToken(c);
          break;
        case 'Linebreak':
          token = { kind: 'linebreak', from: c.from, to: c.from + 1 };
          break;
        case 'Escape':
          token = escapeToken(c);
          break;
        case 'Heading':
          token = headingToken(c, depth);
          break;
        case 'Strong':
        case 'Emph':
          token = emphasisToken(c, depth);
          break;
        case 'ListItem':
        case 'EnumItem':
        case 'TermItem':
          if (MARKERS[c.name]) itemTokens(c, out);
          emitMarkup(c, out, depth);
          continue;
        case 'Raw':
          token = rawToken(c);
          break;
        case 'Equation':
          token = mathToken(c);
          break;
        case 'Shorthand': {
          const value = text(c);
          if (value === '~') token = { kind: 'tie', from: c.from, to: c.to };
          else if (LIGATURES[value]) token = { kind: 'ligature', from: c.from, to: c.to, meta: { text: LIGATURES[value] } };
          break;
        }
        case 'SmartQuote':
          token = quoteToken(c);
          break;
        case 'Link':
          token = linkToken(c);
          break;
        case 'Ref':
          token = refToken(c);
          break;
        case 'Label':
          token = { kind: 'command', name: 'label', from: c.from, to: c.to, meta: { key: source.slice(c.from + 1, c.to - 1) } };
          break;
        default:
          emitMarkup(c, out, depth);
          continue;
      }
      if (token) out.push(token);
      else emitMarkup(c, out, depth);
    }
  }

  // Code mode, the arguments of a call: whitespace and commas, `name:` labels and strings are
  // tokens of their own, and so are nested calls and content blocks; the rest stays text.
  function emitCode(node: SyntaxNode, out: Token[], depth: number): void {
    let token: Token | null = null;
    switch (node.name) {
      case 'Space':
      case 'Comma': {
        const last = out[out.length - 1];
        if (last && last.name === 'separator' && last.to === node.from) last.to = node.to;
        else out.push({ kind: 'raw', name: 'separator', from: node.from, to: node.to });
        return;
      }
      case 'LineComment':
      case 'BlockComment':
        token = commentToken(node);
        break;
      case 'Str': {
        if (node.to - node.from < 2 || !text(node).endsWith('"')) return;
        out.push({ kind: 'raw', name: 'string', from: node.from, to: node.to, body: { from: node.from + 1, to: node.to - 1 } });
        return;
      }
      case 'Named': {
        const key = node.firstChild;
        const colon = child(node, 'Colon');
        if (key?.name === 'Ident' && colon) {
          out.push({ kind: 'raw', name: 'argument', from: key.from, to: colon.to });
          for (let c = colon.nextSibling; c; c = c.nextSibling) emitCode(c, out, depth);
          return;
        }
        break;
      }
      case 'FuncCall':
        token = callToken(node, null, depth);
        break;
      case 'ContentBlock':
        token = groupToken(node, node.from, depth);
        break;
      case 'Equation':
        token = mathToken(node);
        break;
      case 'Escape':
        token = escapeToken(node);
        break;
      default:
        break;
    }
    if (token) {
      out.push(token);
      return;
    }
    if (node.name === 'Str' || node.name === 'ContentBlock' || node.name === 'Equation') return;
    for (let c = node.firstChild; c; c = c.nextSibling) emitCode(c, out, depth);
  }

  const root = child(tree.topNode, 'Markup');
  return markup(root, 0, source.length, 0);
}
