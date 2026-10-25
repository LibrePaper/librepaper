// Underlines the findings of Harper, the English grammar and spelling checker,
// and offers its fixes on hover. It replaces the browser's spellcheck, so the
// content attribute turns that off. The lints come from client.js, one
// paragraph at a time, and the prose and markup split comes from prose.js.
import { EditorView, Decoration, ViewPlugin, closeHoverTooltips, hoverTooltip } from "@codemirror/view";
import { StateEffect } from "@codemirror/state";
import { addWord, lintProse } from "./client.js";
import { keepLint, paragraphs, proseText } from "./prose.js";

// Above this length a pass costs more than the underlines are worth, so
// nothing is checked.
const MAX_SCAN_LENGTH = 2_000_000;

// An edit moves the underlines already drawn, which is cheap, and the visible
// paragraphs are checked again this many milliseconds after the last one.
const RESCAN_DELAY = 400;

// The cache holds a few thousand paragraphs, and the oldest are dropped first.
const MAX_CACHE = 3000;

// A card shows at most this many suggestions.
const MAX_SUGGESTIONS = 5;

// Dispatched after a pass, or after a word is added to the dictionary, so
// that update() rebuilds the decorations from the cache.
const checked = StateEffect.define();

// One extension per format and dialect. The editor reconfigures its switches
// whenever one changes or a file is shown, and handing CodeMirror the same
// plugin keeps it rather than checking the text again.
const extensions = new Map();

/**
 * Harper's grammar and spelling underlines for a prose file, with a card of
 * fixes on hover.
 * @param {string} format
 * @param {string} dialect
 * @returns {import("@codemirror/state").Extension}
 */
export function harperExtension(format, dialect) {
  const key = `${format}:${dialect}`;
  if (!extensions.has(key)) extensions.set(key, build(format, dialect));
  return extensions.get(key);
}

function isSpelling(lint) {
  return lint.kind === "Spelling" || lint.kind === "Typo";
}

function build(format, dialect) {
  const plugin = ViewPlugin.fromClass(class {
    constructor(view) {
      this.view = view;
      this.timer = null;
      this.running = false;
      this.again = false;
      this.destroyed = false;
      this.failed = false;
      // Paragraph text to its lints, with offsets from the paragraph start.
      this.cache = new Map();
      this.decorations = Decoration.none;
      this.schedule(0);
    }

    update(update) {
      if (update.docChanged) {
        // The underlines follow the edit now, and the next pass corrects them.
        this.decorations = this.decorations.map(update.changes);
        this.schedule(RESCAN_DELAY);
      } else if (update.viewportChanged) {
        this.schedule(RESCAN_DELAY);
      }
      if (update.transactions.some((tr) => tr.effects.some((e) => e.is(checked)))) {
        this.decorations = this.build();
      }
    }

    destroy() {
      this.destroyed = true;
      clearTimeout(this.timer);
    }

    schedule(delay) {
      if (this.failed || this.destroyed) return;
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.pass(), delay);
    }

    // Passes run one at a time. A request that arrives during a pass runs one
    // more pass after it, because the text it saw may be out of date.
    async pass() {
      this.timer = null;
      if (this.failed || this.destroyed) return;
      if (this.running) {
        this.again = true;
        return;
      }
      this.running = true;
      try {
        await this.check();
      } catch (error) {
        this.failed = true;
        console.warn("Harper could not check the text.", error);
      } finally {
        this.running = false;
      }
      if (this.destroyed) return;
      this.view.dispatch({ effects: checked.of(null) });
      if (this.again && !this.failed) {
        this.again = false;
        this.schedule(0);
      }
    }

    async check() {
      const doc = this.view.state.doc;
      if (doc.length > MAX_SCAN_LENGTH) return;
      const text = doc.toString();
      // The markup scanners need the whole document for context, so the text
      // is masked once and each paragraph is cut out of the masked copy.
      const { text: masked, skips } = proseText(text, format);
      for (const { from, to } of this.visibleParagraphs(text)) {
        const original = text.slice(from, to);
        if (this.cache.has(original)) continue;
        const found = await lintProse(masked.slice(from, to), dialect);
        if (this.destroyed) return;
        const kept = found.filter((lint) =>
          keepLint({ ...lint, from: lint.from + from, to: lint.to + from }, skips));
        this.cache.set(original, kept);
        while (this.cache.size > MAX_CACHE) this.cache.delete(this.cache.keys().next().value);
      }
    }

    // The paragraphs that touch the screen, each once. Neighbouring visible
    // ranges can share a paragraph.
    visibleParagraphs(text) {
      const seen = new Set();
      const list = [];
      for (const range of this.view.visibleRanges) {
        for (const paragraph of paragraphs(text, range.from, range.to)) {
          if (seen.has(paragraph.from)) continue;
          seen.add(paragraph.from);
          list.push(paragraph);
        }
      }
      return list;
    }

    build() {
      const doc = this.view.state.doc;
      if (doc.length > MAX_SCAN_LENGTH) return Decoration.none;
      const text = doc.toString();
      const marks = [];
      for (const { from, to } of this.visibleParagraphs(text)) {
        const lints = this.cache.get(text.slice(from, to));
        if (!lints) continue;
        for (const lint of lints) {
          const start = from + lint.from;
          const end = from + lint.to;
          if (end <= start || end > doc.length) continue;
          const spec = { class: isSpelling(lint) ? "cm-harper-spelling" : "cm-harper-grammar", lint };
          marks.push(Decoration.mark(spec).range(start, end));
        }
      }
      return Decoration.set(marks, true);
    }

    // Forgets every paragraph with a finding on this word, so the next pass
    // checks it again with the word in the dictionary.
    forget(word) {
      for (const [key, lints] of this.cache) {
        if (lints.some((lint) => lint.problem === word)) this.cache.delete(key);
      }
    }

    // The current position of a lint, which edits may have moved.
    locate(lint) {
      let found = null;
      this.decorations.between(0, this.view.state.doc.length, (from, to, value) => {
        if (value.spec.lint !== lint) return undefined;
        found = { from, to };
        return false;
      });
      return found;
    }
  }, {
    decorations: (instance) => instance.decorations,
  });

  const hover = hoverTooltip((view, pos) => {
    const instance = view.plugin(plugin);
    if (!instance) return null;
    let hit = null;
    instance.decorations.between(pos, pos, (from, to, value) => {
      if (from > pos || pos >= to) return undefined;
      hit = { from, to, lint: value.spec.lint };
      return false;
    });
    if (!hit) return null;
    const { from, to, lint } = hit;
    return { pos: from, end: to, above: true, create: () => ({ dom: card(view, instance, lint) }) };
  });

  const theme = EditorView.baseTheme({
    ".cm-harper-spelling": { textDecoration: "underline wavy var(--color-error-solid)", textUnderlineOffset: "3px" },
    ".cm-harper-grammar": { textDecoration: "underline wavy var(--color-link)", textUnderlineOffset: "3px" },
    ".cm-harper-card": { padding: "0.4rem 0.6rem", maxWidth: "28rem" },
    ".cm-harper-card-message": { marginBottom: "0.4rem" },
    ".cm-harper-card-actions": { display: "flex", flexWrap: "wrap", gap: "0.3rem" },
  });

  return [EditorView.contentAttributes.of({ spellcheck: "false" }), plugin, hover, theme];
}

function card(view, instance, lint) {
  const dom = document.createElement("div");
  dom.className = "cm-harper-card";
  const message = document.createElement("div");
  message.className = "cm-harper-card-message";
  message.textContent = lint.message;
  const actions = document.createElement("div");
  actions.className = "cm-harper-card-actions";
  dom.append(message, actions);

  // A button reads the position when it is clicked, since the text may have
  // changed while the card was open.
  function button(label, onClick) {
    const element = document.createElement("button");
    element.type = "button";
    element.className = "btn btn-sm";
    element.textContent = label;
    element.addEventListener("click", () => {
      const at = instance.locate(lint);
      if (at) onClick(at);
      view.dispatch({ effects: closeHoverTooltips });
      view.focus();
    });
    actions.append(element);
  }

  function apply(from, to, insert) {
    view.dispatch({ changes: { from, to, insert }, userEvent: "input.complete" });
  }

  for (const suggestion of (lint.suggestions ?? []).slice(0, MAX_SUGGESTIONS)) {
    if (suggestion.kind === "remove") {
      button("Remove", (at) => apply(at.from, at.to, ""));
    } else if (suggestion.kind === "insert") {
      button(`Insert ${suggestion.text}`, (at) => apply(at.to, at.to, suggestion.text));
    } else {
      button(suggestion.text, (at) => apply(at.from, at.to, suggestion.text));
    }
  }

  if (isSpelling(lint)) {
    const element = document.createElement("button");
    element.type = "button";
    element.className = "btn btn-sm";
    element.textContent = "Add to dictionary";
    element.addEventListener("click", () => {
      addWord(lint.problem);
      instance.forget(lint.problem);
      view.dispatch({ effects: [checked.of(null), closeHoverTooltips] });
      instance.schedule(0);
      view.focus();
    });
    actions.append(element);
  }
  return dom;
}
