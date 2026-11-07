// Hover for labels: a ref to a label shows that label's title and the file it is
// defined in. Citations keep their own hover, so cite refs get nothing here.

import { hoverTooltip } from "@codemirror/view";
import { hover, itemAt } from "../document-index/queries.js";
import { m } from "../paraglide/messages.js";

/** @typedef {import("@codemirror/view").Tooltip} Tooltip */
/** @typedef {import("./context.js").IndexContext} IndexContext */

/**
 * @param {string | undefined} title
 * @param {string} path
 * @returns {HTMLElement}
 */
function card(title, path) {
  const dom = document.createElement("div");
  dom.className = "cm-index-hover";
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
    if (!result || result.namespace !== "label") return null;
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
      create: () => ({ dom: card(target.title, target.path) }),
    };
    return tooltip;
  });
}
