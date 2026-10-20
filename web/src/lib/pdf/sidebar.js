// The PDF frame's sidebar: page thumbnails and, when the file has one, the
// outline.
//
// Everything lives in a shadow root. `frame.js` republishes the document's
// text from every text node in the body and rebuilds on any body mutation, and
// a sidebar full of page numbers and headings is not what the PDF says;
// neither its TreeWalker nor its MutationObserver enters a shadow root.
//
// The sidebar owns no document. `viewer.js` opens and destroys the pdf.js
// document and hands it over with `setDocument`; this module only cancels what
// it started when the document changes. Colours are the custom properties
// `viewer.html` defines on `html`, which inherit into the shadow root, so the
// sidebar follows the frame's light and dark modes without knowing about them.

import { resolveDest, safeUrl } from "./links.js";

/// Width in CSS pixels. `viewer.js` offsets the pages by this much while the
/// sidebar is open.
export const SIDEBAR_WIDTH = 200;

/// Thumbnails are drawn this many CSS pixels wide. Small enough to be cheap on
/// a long document, large enough to recognise a page by its layout.
const THUMB_WIDTH = 150;

const STYLE = `
  :host { position: fixed; left: 0; top: 0; bottom: 0; width: ${SIDEBAR_WIDTH}px;
          display: none; z-index: 20; box-sizing: border-box; }
  :host([open]) { display: block; }
  .panel { height: 100%; box-sizing: border-box; display: flex; flex-direction: column;
           background: var(--lp-sidebar-bg, #ffffff); color: var(--lp-sidebar-text, #1a2225);
           border-right: 1px solid var(--lp-sidebar-border, #d6d8d4);
           font: 0.8125rem/1.4 system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; }
  .tabs { display: flex; flex: none; border-bottom: 1px solid var(--lp-sidebar-border, #d6d8d4); }
  .tabs button { flex: 1; padding: 0.5rem 0.25rem; border: 0; background: none; color: var(--lp-sidebar-muted, #5d666a);
                 font: inherit; cursor: pointer; border-bottom: 2px solid transparent; }
  .tabs button[aria-selected="true"] { color: var(--lp-sidebar-text, #1a2225); border-bottom-color: var(--lp-sidebar-active, #2f6fdb); }
  button:focus-visible { outline: 2px solid var(--lp-sidebar-active, #2f6fdb); outline-offset: -2px; }
  .body { flex: 1; min-height: 0; overflow-y: auto; position: relative; }
  .body[hidden] { display: none; }
  ol, ul { list-style: none; margin: 0; padding: 0; }
  .pages { padding: 12px 0; }
  .item { display: block; width: 100%; padding: 6px 0; border: 0; background: none; color: inherit; font: inherit; cursor: pointer; text-align: center; }
  .item .thumb { width: ${THUMB_WIDTH}px; margin: 0 auto 4px; box-sizing: border-box; background: #ffffff;
                 border: 1px solid var(--lp-sidebar-border, #d6d8d4); overflow: hidden; }
  .item canvas { display: block; width: 100%; height: auto; filter: var(--lp-thumb-filter, none); }
  .item .num { color: var(--lp-sidebar-muted, #5d666a); }
  .item[aria-current="page"] .thumb { outline: 3px solid var(--lp-sidebar-active, #2f6fdb); outline-offset: 0; }
  .item[aria-current="page"] .num { color: var(--lp-sidebar-text, #1a2225); font-weight: 600; }
  .outline { padding: 6px 0; }
  .outline ul { padding-left: 0.9rem; }
  .row { display: flex; align-items: flex-start; }
  .row .twist { flex: none; width: 1.25rem; padding: 0.2rem 0; border: 0; background: none; color: var(--lp-sidebar-muted, #5d666a); font: inherit; cursor: pointer; }
  .row .spacer { flex: none; width: 1.25rem; }
  .row .link { flex: 1; min-width: 0; padding: 0.2rem 0.5rem 0.2rem 0; border: 0; background: none; color: inherit;
               font: inherit; text-align: left; cursor: pointer; overflow-wrap: anywhere; }
  .row .link:hover { color: var(--lp-sidebar-active, #2f6fdb); }
  li[data-collapsed] > ul { display: none; }
`;

function el(tag, className, attributes) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  for (const [name, value] of Object.entries(attributes ?? {})) node.setAttribute(name, value);
  return node;
}

/// Build the sidebar. `ongo({ page, y })` is called when a thumbnail (y null)
/// or an outline entry is clicked.
export function createSidebar({ ongo }) {
  const element = document.createElement("div");
  const root = element.attachShadow({ mode: "open" });
  const style = document.createElement("style");
  style.textContent = STYLE;

  const panel = el("div", "panel");
  const tabs = el("div", "tabs", { role: "tablist" });
  const pagesTab = el("button", "", { type: "button", role: "tab", "aria-selected": "true" });
  pagesTab.textContent = "Pages";
  const outlineTab = el("button", "", { type: "button", role: "tab", "aria-selected": "false" });
  outlineTab.textContent = "Outline";
  outlineTab.hidden = true;
  tabs.append(pagesTab, outlineTab);
  const pagesBody = el("div", "body pages", { role: "tabpanel" });
  const outlineBody = el("div", "body outline", { role: "tabpanel" });
  outlineBody.hidden = true;
  panel.append(tabs, pagesBody, outlineBody);
  root.append(style, panel);

  let open = false;
  let current = 0;
  let generation = 0;
  let doc = null;
  let observer = null;
  let items = []; // index = page - 1: { button, thumb, drawn }
  const tasks = new Set();

  function showTab(name) {
    const outline = name === "outline" && !outlineTab.hidden;
    pagesTab.setAttribute("aria-selected", String(!outline));
    outlineTab.setAttribute("aria-selected", String(outline));
    pagesBody.hidden = outline;
    outlineBody.hidden = !outline;
    if (!outline) keepInView();
  }
  pagesTab.addEventListener("click", () => showTab("pages"));
  outlineTab.addEventListener("click", () => showTab("outline"));

  function cancelWork() {
    observer?.disconnect();
    observer = null;
    for (const task of tasks) {
      try {
        task.cancel();
      } catch {
        // already finished
      }
    }
    tasks.clear();
  }

  // Scroll the list, not the document: `scrollIntoView` would move the page
  // behind the sidebar as well.
  function keepInView() {
    const item = items[current - 1]?.button;
    if (!item || pagesBody.hidden || !open) return;
    const top = item.offsetTop;
    const bottom = top + item.offsetHeight;
    if (top < pagesBody.scrollTop) pagesBody.scrollTop = top;
    else if (bottom > pagesBody.scrollTop + pagesBody.clientHeight) pagesBody.scrollTop = bottom - pagesBody.clientHeight;
  }

  async function draw(number, mine) {
    const item = items[number - 1];
    if (!item || item.drawn || !doc) return;
    item.drawn = true;
    try {
      const page = await doc.getPage(number);
      if (mine !== generation) return;
      const base = page.getViewport({ scale: 1 });
      const viewport = page.getViewport({ scale: THUMB_WIDTH / base.width });
      const ratio = Math.min(globalThis.devicePixelRatio || 1, 2);
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.floor(viewport.width * ratio));
      canvas.height = Math.max(1, Math.floor(viewport.height * ratio));
      canvas.setAttribute("aria-hidden", "true");
      const task = page.render({
        canvasContext: canvas.getContext("2d"),
        viewport,
        transform: ratio === 1 ? null : [ratio, 0, 0, ratio, 0, 0],
      });
      tasks.add(task);
      try {
        await task.promise;
      } finally {
        tasks.delete(task);
      }
      if (mine !== generation) return;
      item.thumb.style.aspectRatio = `${base.width} / ${base.height}`;
      item.thumb.replaceChildren(canvas);
    } catch {
      // A cancelled render, or a document destroyed under us: nothing to show.
      if (mine === generation) item.drawn = false;
    }
  }

  async function buildPages(mine) {
    const first = await doc.getPage(1);
    if (mine !== generation) return;
    // Until a page is drawn its box assumes page 1's shape; drawing corrects it.
    const [x0, y0, x1, y1] = first.view;
    const ratio = `${x1 - x0} / ${y1 - y0}`;
    const list = el("ol");
    items = [];
    for (let number = 1; number <= doc.numPages; number += 1) {
      const li = el("li");
      const button = el("button", "item", { type: "button", "aria-label": `Page ${number}` });
      const thumb = el("div", "thumb");
      thumb.style.aspectRatio = ratio;
      const label = el("div", "num");
      label.textContent = String(number);
      button.dataset.page = String(number);
      button.append(thumb, label);
      button.addEventListener("click", () => ongo({ page: number, y: null }));
      li.append(button);
      list.append(li);
      items.push({ button, thumb, drawn: false });
    }
    pagesBody.replaceChildren(list);
    if (typeof IntersectionObserver === "function") {
      observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) draw(Number(entry.target.dataset.page), mine);
          }
        },
        { root: pagesBody, rootMargin: "300px 0px" },
      );
      for (const item of items) observer.observe(item.button);
    }
    markCurrent();
  }

  function markCurrent() {
    items.forEach((item, index) => {
      if (index + 1 === current) item.button.setAttribute("aria-current", "page");
      else item.button.removeAttribute("aria-current");
    });
    keepInView();
  }

  async function go(entry, mine) {
    const url = safeUrl(entry.url);
    if (url && !url.startsWith("mailto:")) {
      window.open(url, "_blank", "noopener");
      return;
    }
    if (!entry.dest || !doc) return;
    const target = await resolveDest(doc, entry.dest);
    if (target && mine === generation) ongo(target);
  }

  function outlineList(entries, depth, mine) {
    const list = el("ul");
    for (const entry of entries) {
      const li = el("li");
      const row = el("div", "row");
      const children = Array.isArray(entry.items) ? entry.items : [];
      if (children.length > 0) {
        const collapsed = depth >= 1;
        const twist = el("button", "twist", { type: "button", "aria-expanded": String(!collapsed), "aria-label": "Toggle section" });
        twist.textContent = collapsed ? "+" : "-";
        if (collapsed) li.dataset.collapsed = "";
        twist.addEventListener("click", () => {
          const closed = li.toggleAttribute("data-collapsed");
          twist.setAttribute("aria-expanded", String(!closed));
          twist.textContent = closed ? "+" : "-";
        });
        row.append(twist);
      } else {
        row.append(el("span", "spacer"));
      }
      const link = el("button", "link", { type: "button" });
      link.textContent = entry.title || "Untitled";
      link.addEventListener("click", () => go(entry, mine));
      row.append(link);
      li.append(row);
      if (children.length > 0) li.append(outlineList(children, depth + 1, mine));
      list.append(li);
    }
    return list;
  }

  async function setDocument(next) {
    generation += 1;
    const mine = generation;
    cancelWork();
    doc = next ?? null;
    items = [];
    pagesBody.replaceChildren();
    outlineBody.replaceChildren();
    outlineTab.hidden = true;
    showTab("pages");
    if (!doc || !(doc.numPages >= 1)) return;
    try {
      await buildPages(mine);
      if (mine !== generation) return;
      const outline = await doc.getOutline();
      if (mine !== generation || !Array.isArray(outline) || outline.length === 0) return;
      outlineBody.replaceChildren(outlineList(outline, 0, mine));
      outlineTab.hidden = false;
    } catch {
      // The document went away while the sidebar was reading it.
    }
  }

  return {
    element,
    isOpen: () => open,
    setOpen(value) {
      open = Boolean(value);
      element.toggleAttribute("open", open);
      if (open) keepInView();
    },
    setDocument,
    setCurrent(page) {
      current = Number.isInteger(page) ? page : 0;
      markCurrent();
    },
  };
}
