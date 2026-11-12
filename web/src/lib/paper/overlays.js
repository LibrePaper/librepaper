// Overlays inside rendered paper blocks.
//
// A comment, a tracked change, an authorship tint, a lint or a collaborator's
// caret is a decoration on a range of the source text, and CodeMirror cannot
// draw one inside a block widget: the widget replaces those characters. So
// this plugin paints copies. For each rendered block on screen it gathers the
// overlay decorations that touch the block's source range, aligns the source
// text with the text the block shows (align.js), and wraps the matching
// rendered text in spans with the same classes and attributes. Nothing here
// changes the document, the state or the widgets: it only edits the DOM a
// widget already produced, and a widget that is redrawn arrives unpainted
// and is painted again.
import { EditorView, ViewPlugin } from "@codemirror/view";
import { ephemeralStateField } from "../../../vendor/loro-codemirror/ephemeral.ts";
import { alignText, mapPoint, mapRange, normalizeSpace } from "./align.js";

// The overlays that are painted, by class. Other mark decorations (syntax
// colours, bracket matching) belong to the source and are not repainted.
const WANTED = /(?:^|\s)(?:cm-comment-mark\S*|change-added|cm-authorship|cm-harper-spelling|cm-harper-grammar|cm-citation-missing|cm-searchMatch\S*)(?:\s|$)/;
// A formula is one opaque unit: its TeX stands for it, and it is wrapped whole.
const UNIT = "span[data-math-style], math";
const SKIP = new Set(["style", "script", "template"]);
// A block with more overlays than this is painted with the first of them.
const MAX_ITEMS = 300;

/** @type {WeakMap<Element, string>} what each rendered block was last painted from */
const signatures = new WeakMap();

/** @typedef {{ kind: "mark", from: number, to: number, class: string, attributes: Record<string, string> | null }} MarkItem */
/** @typedef {{ kind: "removed", at: number, widget: any }} RemovedItem */
/** @typedef {{ kind: "caret", at: number, name: string, colour: string }} CaretItem */
/** @typedef {MarkItem | RemovedItem | CaretItem} Item */

/** The decoration sets in force, with the functions resolved against the view. */
function decorationSets(view) {
  const sets = [];
  for (const entry of view.state.facet(EditorView.decorations)) {
    const set = typeof entry === "function" ? entry(view) : entry;
    if (set && set.size !== 0) sets.push(set);
  }
  return sets;
}

/**
 * The overlays that touch [from, to], with offsets relative to `from`.
 * @returns {Item[]}
 */
function itemsIn(view, sets, remote, from, to) {
  /** @type {Item[]} */
  const items = [];
  for (const set of sets) {
    set.between(from, to, (start, end, /** @type {any} */ decoration) => {
      if (!decoration.point) {
        const spec = decoration.spec || {};
        if (typeof spec.class !== "string" || !WANTED.test(spec.class)) return;
        const a = Math.max(start, from);
        const b = Math.min(end, to);
        if (b <= a) return;
        items.push({ kind: "mark", from: a - from, to: b - from, class: spec.class, attributes: spec.attributes || null });
      } else if (decoration.spec?.widget?.kind === "removed" && start >= from && start <= to) {
        items.push({ kind: "removed", at: start - from, widget: decoration.spec.widget });
      }
    });
  }
  if (remote && !remote.isCheckout) {
    for (const [peer, cursor] of remote.remoteCursors) {
      const user = remote.remoteUsers.get(peer);
      const colour = user?.colorClassName || "";
      const { anchor, head } = cursor;
      if (head !== undefined && head !== anchor) {
        const a = Math.max(Math.min(anchor, head), from);
        const b = Math.min(Math.max(anchor, head), to);
        if (b > a) {
          items.push({
            kind: "mark",
            from: a - from,
            to: b - from,
            class: `lp-paper-selection ${colour}`.trim(),
            attributes: { style: "background: color-mix(in srgb, var(--presence, var(--color-brand)) 25%, transparent)" },
          });
        }
      }
      if (anchor >= from && anchor <= to) items.push({ kind: "caret", at: anchor - from, name: user?.name || "unknown", colour });
    }
  }
  return items.slice(0, MAX_ITEMS);
}

/** A description of the items that does not change when the widgets they hold are rebuilt. */
function describe(items) {
  return JSON.stringify(
    items.map((item) => (item.kind === "removed" ? { at: item.at, text: item.widget.text, change: item.widget.change, className: item.widget.className } : item)),
  );
}

/** The TeX a formula element stands for. */
function texOf(element) {
  return element.getAttribute("data-equation-source") ?? element.textContent ?? "";
}

/**
 * The text of a rendered block as a list of segments: a text node, or a
 * formula that counts as its TeX. Painted widgets (removed text, carets) are
 * not part of it; painted wrappers are looked through.
 */
function flatten(root) {
  /** @type {{ node: Node, unit: boolean, start: number, length: number }[]} */
  const segments = [];
  let text = "";
  const walk = (parent) => {
    for (let child = parent.firstChild; child; child = child.nextSibling) {
      if (child.nodeType === 3) {
        const data = /** @type {Text} */ (child).data;
        if (!data) continue;
        segments.push({ node: child, unit: false, start: text.length, length: data.length });
        text += data;
      } else if (child.nodeType === 1) {
        const element = /** @type {Element} */ (child);
        if (element.getAttribute("data-lp-overlay") === "widget" || SKIP.has(element.localName)) continue;
        if (element.matches(UNIT)) {
          const tex = texOf(element);
          if (tex) {
            segments.push({ node: element, unit: true, start: text.length, length: tex.length });
            text += tex;
          }
          continue;
        }
        walk(element);
      }
    }
  };
  walk(root);
  return { segments, text };
}

/** Takes the painted spans and widgets out again and joins the text nodes they split. */
function clear(div) {
  const painted = div.querySelectorAll("[data-lp-overlay]");
  if (!painted.length) return;
  for (const node of Array.from(painted)) {
    if (node.getAttribute("data-lp-overlay") === "widget") node.remove();
    else node.replaceWith(...Array.from(node.childNodes));
  }
  div.normalize();
}

function overlaySpan(item) {
  const span = document.createElement("span");
  span.setAttribute("data-lp-overlay", "");
  let className = item.class;
  for (const [name, value] of Object.entries(item.attributes || {})) {
    if (name === "class") className += ` ${value}`;
    else span.setAttribute(name, String(value));
  }
  span.className = className;
  return span;
}

/** Wraps the rendered range [from, to) of the block in `make()` spans, one per text node or formula. */
function wrapRange(div, from, to, make) {
  const { segments } = flatten(div);
  const ranges = [];
  for (const segment of segments) {
    const a = Math.max(from, segment.start);
    const b = Math.min(to, segment.start + segment.length);
    if (b <= a) continue;
    const range = document.createRange();
    if (segment.unit) range.selectNode(segment.node);
    else {
      const data = /** @type {Text} */ (segment.node).data;
      if (!data.slice(a - segment.start, b - segment.start).trim()) continue;
      range.setStart(segment.node, a - segment.start);
      range.setEnd(segment.node, b - segment.start);
    }
    ranges.push(range);
  }
  // Each range lies in one text node or is one element, so none is cut by another's wrapper.
  for (const range of ranges) range.surroundContents(make());
}

/** The node to insert before or after so the insertion lands outside any painted wrapper that starts or ends there. */
function outside(node, side) {
  let current = node;
  const next = side === "before" ? "previousSibling" : "nextSibling";
  while (current.parentElement?.getAttribute("data-lp-overlay") === "" && current[next] === null) current = current.parentElement;
  return current;
}

/** Inserts `element` at rendered offset `index`. */
function insertAt(div, index, element) {
  const { segments, text } = flatten(div);
  const range = document.createRange();
  const segment = segments.find((candidate) => index >= candidate.start && index < candidate.start + candidate.length);
  if (segment) {
    if (segment.unit || index === segment.start) range.setStartBefore(outside(segment.node, "before"));
    else range.setStart(segment.node, index - segment.start);
  } else if (segments.length && index >= text.length) {
    range.setStartAfter(outside(segments[segments.length - 1].node, "after"));
  } else return;
  range.collapse(true);
  range.insertNode(element);
}

function cssString(text) {
  return `"${text.replace(/[\\"]/g, "\\$&").replace(/\s/g, " ")}"`;
}

/** Paints `items` into the rendered block `div`, whose source is `source`. */
function paint(view, div, source, items) {
  clear(div);
  if (!items.length) return;
  const { text } = flatten(div);
  const sourceText = normalizeSpace(source);
  const map = alignText(sourceText, normalizeSpace(text));
  if (!map) return;
  for (const item of items) {
    if (item.kind !== "mark") continue;
    const range = mapRange(map, sourceText, item.from, item.to);
    if (range) wrapRange(div, range.from, range.to, () => overlaySpan(item));
  }
  for (const item of items) {
    if (item.kind === "mark") continue;
    const index = mapPoint(map, item.at);
    if (index === null) continue;
    let element;
    if (item.kind === "removed") {
      element = item.widget.toDOM(view);
    } else {
      element = document.createElement("span");
      element.className = `loro-cursor ${item.colour}`.trim();
      element.style.setProperty("--name", cssString(item.name));
    }
    element.setAttribute("data-lp-overlay", "widget");
    insertAt(div, index, element);
  }
}

/**
 * The plugin. `field` is the paper field, whose `rendered` list says which
 * source range each widget stands for.
 * @param {import("@codemirror/state").StateField<{ rendered: { from: number, to: number }[] }>} field
 */
export function paperOverlays(field) {
  return ViewPlugin.define((view) => {
    const plugin = {
      painting: false,
      observer: new MutationObserver((records) => {
        // A formula typeset or an image resolved changes the text a block shows.
        const inBlock = (node) => (node.nodeType === 1 ? node : node.parentElement)?.closest?.(".cm-lp-paper");
        if (records.some((record) => inBlock(record.target))) schedule();
      }),
      destroy() {
        plugin.observer.disconnect();
      },
      update() {
        schedule();
      },
    };

    const measure = {
      key: plugin,
      read() {
        const rendered = view.state.field(field, false)?.rendered;
        if (!rendered?.length) return [];
        const divs = Array.from(view.contentDOM.querySelectorAll(".cm-lp-paper"));
        if (!divs.length) return [];
        const sets = decorationSets(view);
        const remote = view.state.field(ephemeralStateField, false);
        const jobs = [];
        for (const div of divs) {
          let at;
          try {
            at = view.posAtDOM(div);
          } catch {
            continue;
          }
          const block = rendered.find((candidate) => candidate.from === at);
          if (!block) continue;
          const source = view.state.sliceDoc(block.from, block.to);
          const items = itemsIn(view, sets, remote, block.from, block.to);
          const key = `${source}\0${describe(items)}`;
          // The text length changes when a formula is typeset, so the paint is redone then.
          if (signatures.get(div) === `${key}|${div.textContent?.length}`) continue;
          jobs.push({ div, source, items, key });
        }
        return jobs;
      },
      write(jobs) {
        if (!jobs.length) return;
        plugin.observer.disconnect();
        try {
          for (const { div, source, items, key } of jobs) {
            try {
              paint(view, div, source, items);
            } catch (error) {
              console.warn("Paper overlays could not be painted.", error);
              clear(div);
            }
            signatures.set(div, `${key}|${div.textContent?.length}`);
          }
        } finally {
          plugin.observer.observe(view.contentDOM, { childList: true, subtree: true });
        }
      },
    };

    function schedule() {
      view.requestMeasure(measure);
    }

    plugin.observer.observe(view.contentDOM, { childList: true, subtree: true });
    schedule();
    return plugin;
  });
}
