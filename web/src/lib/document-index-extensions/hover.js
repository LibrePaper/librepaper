// Hover for labels and footnotes: a ref to a label shows the kind of thing it
// names ("Figure", "Footnote", ...), that label's title, and the file it is
// defined in. Citations keep their own hover, so cite refs get nothing here.

import { hoverTooltip } from "@codemirror/view";
import { hover, itemAt } from "../document-index/queries.js";
import { m } from "../paraglide/messages.js";

/** @typedef {import("@codemirror/view").Tooltip} Tooltip */
/** @typedef {import("./context.js").IndexContext} IndexContext */

/**
 * The translated word for a label's target kind, or null for a kind with no word.
 * @param {string} kind
 * @returns {string | null}
 */
function kindWord(kind) {
  switch (kind) {
    case "section":
      return m.common_section();
    case "figure":
      return m.common_figure();
    case "table":
      return m.common_table();
    case "equation":
      return m.mathpalette_equation();
    case "listing":
      return m.common_listing();
    case "footnote":
      return m.insert_footnote();
    case "theorem":
      return m.insert_theorem();
    default:
      return null;
  }
}

/**
 * @param {string | undefined} title
 * @param {string} path
 * @param {string | undefined} kind the target kind, shown before the title
 * @returns {HTMLElement}
 */
function card(title, path, kind) {
  const dom = document.createElement("div");
  dom.className = "cm-index-hover";
  const word = kind === undefined ? null : kindWord(kind);
  if (word !== null) {
    const kindLine = document.createElement("div");
    kindLine.className = "cm-index-hover-kind";
    kindLine.textContent = word;
    dom.append(kindLine);
  }
  if (title !== undefined && title !== "") {
    const heading = document.createElement("div");
    heading.className = "cm-index-hover-title";
    heading.textContent = title;
    dom.append(heading);
  }
  const location = document.createElement("div");
  location.className = "cm-index-hover-path";
  location.textContent = m.navigation_defined_in({ path });
  dom.append(location);
  return dom;
}

/**
 * @param {IndexContext} context
 * @returns {import("@codemirror/state").Extension}
 */
export function indexHover(context) {
  return hoverTooltip((view, pos) => {
    const project = context.project();
    const path = context.path();
    const result = hover(project, path, pos);
    if (!result || result.namespace === "cite") return null;
    const [target] = result.targets;
    if (!target) return null;
    const item = itemAt(project, path, pos);
    if (!item) return null;
    const [from, to] = item.nameRange ?? item.range;
    /** @type {Tooltip} */
    const tooltip = {
      pos: from,
      end: to,
      above: true,
      create: () => ({ dom: card(target.title, target.path, target.kind) }),
    };
    return tooltip;
  });
}
