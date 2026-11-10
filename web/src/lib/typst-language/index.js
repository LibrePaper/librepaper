// Typst language support from typst-syntax.wasm: Typst's own parser as a Lezer
// tree, for highlighting, folding and indentation. `typst()` starts with the
// small stream mode and swaps the real tree in when the module has loaded; if
// it never does, the stream mode simply stays.
import { Compartment, Prec } from "@codemirror/state";
import { ViewPlugin, keymap } from "@codemirror/view";
import { Language, LanguageSupport, StreamLanguage, foldService, syntaxTree } from "@codemirror/language";
import { Parser } from "@lezer/common";
import { typstLanguage as streamMode } from "../typst-mode.js";
import { loadTypstSyntax, typstSyntax } from "./module.js";
import { typstData } from "./props.js";
import { typstTableKeymap } from "./table-keys.js";

class TypstParser extends Parser {
  constructor(api) {
    super();
    this.api = api;
  }

  createParse(input) {
    const api = this.api;
    let done = false;
    return {
      parsedPos: 0,
      stoppedAt: null,
      stopAt() {},
      advance() {
        if (done) return null;
        done = true;
        const tree = api.tree(input.read(0, input.length));
        this.parsedPos = input.length;
        return tree;
      },
    };
  }
}

/** The level of a heading node: the length of its `=` marker. */
function headingLevel(state, heading) {
  const marker = heading.firstChild;
  return Math.max(1, (marker ? marker.to - marker.from : 1));
}

/** The heading node that starts on the line beginning at lineStart, if any. */
function headingOn(state, lineStart) {
  const line = state.doc.lineAt(lineStart);
  const at = lineStart + (line.text.length - line.text.trimStart().length);
  for (let node = syntaxTree(state).resolveInner(at, 1); node; node = node.parent) {
    if (node.name === "Heading" && node.from === at) return node;
  }
  return null;
}

/** A heading's section runs to the end of its last content, before the next heading of its level or above. */
const headingSections = foldService.of((state, lineStart, lineEnd) => {
  const heading = headingOn(state, lineStart);
  if (!heading) return null;
  const level = headingLevel(state, heading);
  let limit = state.doc.length;
  for (let next = heading.nextSibling; next; next = next.nextSibling) {
    if (next.name === "Heading" && headingLevel(state, next) <= level) { limit = next.from; break; }
  }
  if (limit === state.doc.length && heading.parent) limit = Math.min(limit, heading.parent.to);
  const body = state.doc.sliceString(lineEnd, limit);
  const to = lineEnd + body.trimEnd().length;
  return to > lineEnd ? { from: lineEnd, to } : null;
});

const supports = new WeakMap();

/** The language support for a loaded module. */
export function typstSupport(api) {
  let support = supports.get(api);
  if (!support) {
    const language = new Language(typstData, new TypstParser(api), [], "typst");
    support = new LanguageSupport(language, [headingSections, Prec.high(keymap.of(typstTableKeymap))]);
    supports.set(api, support);
  }
  return support;
}

/** The Typst language: the stream mode now, the wasm tree once it loads. */
export function typst() {
  // Already loaded: no stream-mode paint first.
  const loaded = typstSyntax();
  if (loaded) return typstSupport(loaded);
  const compartment = new Compartment();
  const upgrade = ViewPlugin.define((view) => {
    let alive = true;
    loadTypstSyntax().then((api) => {
      if (api && alive) view.dispatch({ effects: compartment.reconfigure(typstSupport(api)) });
    });
    return { destroy() { alive = false; } };
  });
  return [compartment.of(StreamLanguage.define(streamMode)), upgrade];
}
