// The half of the reader that lives inside the document.
//
// Documents are served from their own origin, so the sidebar cannot reach into
// them any more. This script is injected into every document and does the work
// that needs the DOM: read the text, paint highlights, report selections. It
// talks to the sidebar over postMessage and holds no opinions of its own --
// anchoring is still decided in the sidebar, which sends back offsets.
//
// It shares an origin with the document, so a hostile document could tamper
// with it. That is why the sidebar treats everything arriving from here as
// untrusted input rather than as fact.

import { createMathTypesetter } from "../lib/math.js";

(() => {
  // Where this agent is allowed to speak. It is injected into a document on
  // the documents origin, and what it sends back is the document's own text:
  // the passage under the caret, the selection, the projection of the page.
  // Falling back to "*" meant that a page framed by anyone at all was handed
  // all of it. There is exactly one reader entitled to hear from this agent,
  // it is named when the script is injected, and without that name the agent
  // says nothing rather than saying it to everybody.
  const readerParameter = new URL(document.currentScript.src).searchParams.get("reader");
  let READER = "";
  try { READER = readerParameter ? new URL(readerParameter).origin : ""; } catch { READER = ""; }
  let table = null; // {nodes, starts, index, joined}

  // The observer that republishes on the document's own edits (armed in
  // watch()). Painting highlights mutates the DOM too, and left running the
  // observer cannot tell our own brushwork
  // from the document changing under us. `quietly` disconnects around such a
  // mutation and discards whatever records piled up meanwhile, so only the
  // document's own changes ever reach republish(). It can be called before
  // watch() has run -- a highlight can arrive before the document settles --
  // in which case there is nothing to suspend.
  let observer = null;
  function quietly(fn) {
    if (!observer) {
      fn();
      return;
    }
    observer.disconnect();
    fn();
    observer.takeRecords();
    observer.observe(document.body, { childList: true, characterData: true, subtree: true });
  }

  // The styles of the page being shown. The frame is an empty shell and the
  // whole document -- head and body -- arrives over
  // the channel, so the head has to be installed here or every document would
  // render as unstyled HTML. Only `<style>` and `<link rel=stylesheet>` are
  // taken, and the title: nothing else in a head is this frame's to run.
  let installed = "";
  let installedNodes = [];
  // Live previews sit in a reading canvas owned by the shell. Keep this style
  // separate from the document's own stylesheet so published pages and
  // typst's generated rules remain untouched. adoptStyles keeps it appended
  // after the document styles and it only applies while previewing flow HTML.
  const previewCanvasStyle = `
    html.librepaper-preview.librepaper-flow { background: #fff; }
    html.librepaper-preview.librepaper-flow body {
      box-sizing: border-box;
      width: 100%;
      max-width: none;
      min-height: 100vh;
      margin: 0;
      padding: 32px 40px;
      background: #fff;
    }
    html.librepaper-preview.librepaper-flow img {
      max-width: 100%;
      height: auto;
    }
    @media (max-width: 640px) {
      html.librepaper-preview.librepaper-flow body {
        padding: 24px 20px;
      }
    }
  `;
  const previewStyle = document.createElement("style");
  previewStyle.dataset.librepaperPreview = "canvas";
  previewStyle.textContent = previewCanvasStyle;

  // Math arrives as TeX in tagged spans and is typeset here, with a KaTeX
  // fetched from this origin the first time a document needs one. The path is
  // written in at build time, beside the copy of KaTeX the build made.
  const typesetMath = createMathTypesetter({ base: __KATEX__, document, window });

  // The one mark this file draws that is not the document's: the ring around
  // whichever annotation the sidebar has singled out. Installed once, up
  // front: unlike the document's own styles, nothing here ever needs to
  // change, and it survives every body replacement `adoptStyles` does for a
  // live preview.
  //
  // A proposal used to be generated content on the mark itself, so that it
  // added no text node; it is a real span now -- the synthetic span below --
  // kept out of the offset tables by the text walk instead.
  const markStyle = document.createElement("style");
  markStyle.dataset.librepaperMarks = "1";
  markStyle.textContent = `
    /* The annotation the sidebar has singled out: a ring around every piece
       of its passage, so that it stands out from the wash the others sit
       under. */
    mark[data-librepaper][data-librepaper-selected] {
      outline: 2px solid hsl(220 85% 50%);
      outline-offset: 1px;
      border-radius: 2px;
      box-decoration-break: clone;
      -webkit-box-decoration-break: clone;
    }
  `;
  document.head.appendChild(markStyle);

  // Suggestions painted inline: a proposed deletion is a mark over the words
  // themselves, and a proposed insertion has no words in the document to wrap,
  // so it is a span carrying the proposed text. `data-librepaper-synthetic`
  // is what keeps it out of the walk below, so the offset tables never see it
  // as a character of real content.
  //
  // A paged document draws this same span differently -- beside the passage
  // rather than inline, in `pages/viewer.html` -- because a PDF page has no
  // inline to give it and the text layer over the glyphs is transparent.
  const suggestionStyle = document.createElement("style");
  suggestionStyle.dataset.librepaperSuggestions = "1";
  suggestionStyle.textContent = `
    mark.librepaper-suggestion-del {
      background: hsl(0 45% 93%);
      color: hsl(0 55% 40%);
      text-decoration: line-through;
    }
    .librepaper-suggestion-synthetic {
      background: hsl(0 35% 93%);
      color: hsl(0 55% 40%);
      text-decoration: underline;
      text-decoration-style: dotted;
    }
  `;
  document.head.appendChild(suggestionStyle);

  function adoptStyles(parsed, presentation) {
    // Quarto's layout selectors depend on attributes of both root elements.
    // Keep the body itself (and its observer), copying only presentation
    // attributes, never event handlers. Remove these again for the draft.
    for (const [target, incoming] of [[document.documentElement, parsed.documentElement], [document.body, parsed.body]]) {
      for (const name of ["class", "id", "lang", "dir", "style"]) {
        if (incoming.hasAttribute(name)) target.setAttribute(name, incoming.getAttribute(name));
        else target.removeAttribute(name);
      }
    }
    document.documentElement.classList.add("librepaper-preview");
    document.documentElement.classList.toggle("librepaper-flow", presentation !== "document" && !parsed.body.querySelector(":scope > svg.typst-doc"));
    const wanted = [...parsed.head.querySelectorAll("style, link[rel~='stylesheet' i]")];
    const markup = wanted.map((node) => node.outerHTML).join("");
    if (markup !== installed) {
      installed = markup;
      for (const node of installedNodes) node.remove();
      installedNodes = wanted.map((node) => document.head.appendChild(node.cloneNode(true)));
    }
    const title = parsed.head.querySelector("title");
    if (title) document.title = title.textContent || "";
    // Re-append after replacing document styles, and also on the fast path
    // above, so this override always wins the cascade.
    document.head.appendChild(previewStyle);
  }

  // One tint per tool, so what a mark means is legible without opening the
  // sidebar. Hue carries the meaning and saturation stays low: these sit under
  // running text for as long as the document is open, and a saturated wash
  // would fight the words it is meant to mark. Kept in step with the tool
  // buttons and the sidebar labels, which use the same hues.
  const TINTS = {
    commenting: [42, 55],
    highlighting: [145, 28],
    // A suggestion: low saturation, hue near red, so a proposed replacement
    // reads as provisional beside an ordinary comment's warmer tint.
    editing: [0, 20],
  };
  const NEUTRAL = [220, 12]; // resolved: the colour has served its purpose
  const tintOf = (motivation) => TINTS[motivation] || TINTS.commenting;
  // Each annotation stacked on the same words takes the wash a step deeper,
  // stopping where dark text would start to struggle against it.
  const wash = ([hue, saturation], depth, alpha = 1) =>
    `hsl(${hue} ${saturation}% ${Math.max(70, 90 - (Math.min(depth, 5) - 1) * 5)}% / ${alpha})`;
  const edge = ([hue, saturation]) => `hsl(${hue} ${Math.min(saturation + 10, 60)}% 45%)`;

  function post(message) {
    if (!READER) return;
    parent.postMessage({ librepaper: true, ...message }, READER);
  }

  // One walk of the document builds the text-node table (with a node->index
  // map for fast lookup) and the cumulative-offset table.
  // Rebuilt whenever the highlights change the node structure underneath us.
  // The joined text is a separate, lazy step: a repaint needs the table but
  // not the string, and joining a large document is the most expensive thing
  // here, so it is only paid for on the first `text()` call after a scan.
  function scan() {
    const walker = document.createTreeWalker(
      document.body,
      NodeFilter.SHOW_TEXT,
    );
    const nodes = [];
    const starts = [];
    const index = new Map();
    let total = 0;
    let node;
    while ((node = walker.nextNode())) {
      if (node.nodeType === Node.TEXT_NODE) {
        const parent = node.parentElement;
        if (parent && ["SCRIPT", "STYLE", "NOSCRIPT"].includes(parent.tagName)) continue;
        if (parent?.closest("[data-librepaper-synthetic], [data-librepaper-deletion]")) continue;
        index.set(node, nodes.length);
        starts.push(total);
        nodes.push(node);
        total += node.data.length;
      }
    }
    table = { nodes, starts, index, joined: null };
  }

  const text = () => table.joined ?? (table.joined = table.nodes.map((node) => node.data).join(""));

  // Index of the node containing `offset`, by binary search over the table.
  function nodeAt(offset) {
    let lo = 0;
    let hi = table.nodes.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (table.starts[mid] <= offset) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  // The stretch of each text node a segment covers. Painting never spans
  // nodes, so a passage that crosses element boundaries is highlighted piece
  // by piece rather than dropped.
  function piecesFor(start, end) {
    const pieces = [];
    for (let i = nodeAt(start); i < table.nodes.length; i++) {
      const nodeStart = table.starts[i];
      if (nodeStart >= end) break;
      const nodeEnd = nodeStart + table.nodes[i].data.length;
      if (nodeEnd <= start) continue;
      const from = Math.max(start, nodeStart) - nodeStart;
      const to = Math.min(end, nodeEnd) - nodeStart;
      if (to > from) pieces.push({ node: table.nodes[i], from, to, end: nodeStart + to });
    }
    return pieces;
  }

  // Paint, from a list of {id, start, end, resolved} the sidebar worked out.
  //
  // Comments overlap: one passage sits inside another, or the two cross. Marks
  // cannot nest through surroundContents, and painting one range after another
  // would leave the second reading offsets that the first has already split.
  // So the ranges are cut into elementary segments -- every stretch covered by
  // the same set of comments -- and each segment is painted once, whatever
  // order the comments arrive in. Painting runs right to left, which leaves the
  // node and offset of every piece still to come untouched.
  // The ranges last asked for, so a repaint of the document can put the marks
  // back in the same breath rather than a round trip later.
  let lastRanges = [];
  // The annotation singled out, by either side. Marks are rebuilt on every
  // repaint, so the ring is put back after each one rather than kept on a node.
  let selectedId = "";
  function applySelected() {
    document.querySelectorAll("[data-librepaper-selected]").forEach((node) => delete node.dataset.librepaperSelected);
    if (!selectedId) return;
    const id = CSS.escape(selectedId);
    document
      .querySelectorAll(`mark[data-librepaper~="${id}"]`)
      .forEach((node) => (node.dataset.librepaperSelected = ""));
  }
  function select(id) {
    selectedId = id == null ? "" : String(id);
    quietly(applySelected);
  }

  const annotationColor = (item) =>
    typeof item?.color === "string" && /^#[0-9a-f]{6}$/i.test(item.color)
      ? item.color.toLowerCase() : null;

  // Those ranges are offsets into the text as it was, and the text has just
  // changed. Typing is one edit at one place, so the difference is entirely
  // described by where the two texts stop agreeing and how much longer or
  // shorter the new one is: everything after that point moves by exactly that
  // much. Without this the marks are painted a few characters off until the
  // sidebar's own answer lands, which reads as a twitch on every keystroke.
  function shiftRanges(ranges) {
    const before = published || "";
    scan();
    const after = text();
    let same = 0;
    while (same < before.length && same < after.length && before[same] === after[same]) same++;
    const delta = after.length - before.length;
    if (!delta) return ranges;
    return ranges.map((range) =>
      range.start >= same
        ? { ...range, start: range.start + delta, end: range.end + delta }
        : range,
    );
  }

  function highlight(ranges) {
    lastRanges = ranges;
    quietly(() => {
      document.querySelectorAll(".librepaper-suggestion-synthetic").forEach((node) => node.remove());
      document
        .querySelectorAll("mark[data-librepaper]")
        .forEach((mark) => mark.replaceWith(...mark.childNodes));
      document.body.normalize(); // restore the pristine text-node structure
    });
    scan();

    const painted = ranges.filter((item) => item.end > item.start);
    const pendingSuggestions = painted.filter((item) => item.motivation === "editing" && !item.resolved);
    const overlapping = new Set();
    for (const one of pendingSuggestions) for (const other of pendingSuggestions) {
      if (one !== other && one.start < other.end && other.start < one.end) overlapping.add(one.id);
    }
    for (const id of overlapping) post({ type: "suggestion-review", id, reason: "overlap" });
    const edges = [...new Set(painted.flatMap((item) => [item.start, item.end]))].sort(
      (a, b) => a - b,
    );
    // A sweep, rather than asking every comment about every segment: walk the
    // edges in order, opening each comment at its start and closing it at its
    // end, so the covering set is carried along instead of recomputed.
    const opening = new Map();
    for (const item of painted) {
      if (!opening.has(item.start)) opening.set(item.start, []);
      opening.get(item.start).push(item);
    }
    const active = new Set();
    const plan = [];
    for (let i = 0; i + 1 < edges.length; i++) {
      const [start, end] = [edges[i], edges[i + 1]];
      for (const item of opening.get(start) || []) active.add(item);
      for (const item of active) if (item.end <= start) active.delete(item);
      if (!active.size) continue;
      const covering = [...active];
      for (const piece of piecesFor(start, end)) plan.push({ piece, covering });
    }

    quietly(() => {
      for (const { piece, covering } of plan.reverse()) {
        const range = document.createRange();
        range.setStart(piece.node, piece.from);
        range.setEnd(piece.node, piece.to);
        const mark = document.createElement("mark");
        // Every comment covering this stretch is named, so a click can pick the
        // most specific one and `reveal` can find any of them.
        mark.dataset.librepaper = covering.map((item) => item.id).join(" ");
        const live = covering.filter((item) => !item.resolved);
        // The innermost annotation is the one this stretch most specifically
        // belongs to, so its tool decides the colour; the number of annotations
        // stacked here decides how deep the wash goes.
        const inner = (live.length ? live : covering).reduce((a, b) =>
          b.end - b.start < a.end - a.start ? b : a,
        );
        const custom = live.length && inner.motivation !== "editing" ? annotationColor(inner) : null;
        const shade = custom
          ? `color-mix(in srgb, ${custom} 42%, transparent)`
          : live.length
          ? wash(tintOf(inner.motivation), live.length, 0.42)
          : wash(NEUTRAL, 1, 0.24);
        mark.style.cssText = `background:${shade};color:inherit;cursor:pointer`;
        // A pending suggestion is struck through along its whole extent, and
        // its proposal is drawn once, after the last segment it covers -- the
        // segment whose own end lands on the suggestion's end offset. A
        // decided suggestion falls out of `resolved` and is painted like any
        // other resolved comment, above.
        const suggestion = live.find((item) => item.motivation === "editing" && !overlapping.has(item.id));
        if (suggestion) {
          mark.style.textDecoration = "line-through";
          mark.style.textDecorationColor = edge(tintOf("editing"));
        }
        range.surroundContents(mark);
        if (suggestion && piece.end === suggestion.end && suggestion.proposed) {
          const inserted = document.createElement("span");
          inserted.className = "librepaper-suggestion-synthetic";
          inserted.dataset.librepaperSynthetic = "1";
          inserted.dataset.librepaperSuggestion = String(suggestion.id);
          inserted.setAttribute("aria-label", `Suggested insertion: ${suggestion.proposed}`);
          inserted.appendChild(document.createTextNode(suggestion.proposed));
          inserted.onclick = () => { select(suggestion.id); post({ type: "focus", id: suggestion.id }); };
          mark.after(inserted);
        }
        // The same innermost annotation the colour came from is the one a click
        // on this stretch means.
        mark.onclick = () => { select(inner.id); post({ type: "focus", id: inner.id }); };
      }
    });
    quietly(applySelected);
    // surroundContents splits the text nodes it wraps, so the table built above
    // no longer describes the document. A selection made after a highlight
    // would land in a node the table has never seen, and report offsets against
    // text that is missing whatever the splits left behind. The marks add no
    // text, so the rescan still matches the reader's copy.
    //
    // This rescan is not a third scan by the time republish() gets involved:
    // the painting above ran inside `quietly`, so the observer never saw it
    // and there is no queued republish() to race with this one.
    scan();
  }

  // A selection becomes a W3C TextQuoteSelector: the quoted text plus the
  // context each side, which is what the sidebar anchors with.

  // Resolves a Range boundary to a {node, offset} pair inside a text node.
  // Almost always the container already is one. A triple-click, though, hands
  // back an element with a child offset: the boundary sits between two of its
  // children, so it is the start of the child after it or the end of the one
  // before, whichever is a text node. Anything less direct is left alone, and
  // the selection is given up quietly, as before.
  function textPointOf(container, offset) {
    if (container.nodeType === Node.TEXT_NODE) return { node: container, offset };
    const after = container.childNodes[offset];
    if (after && after.nodeType === Node.TEXT_NODE) return { node: after, offset: 0 };
    const before = container.childNodes[offset - 1];
    if (before && before.nodeType === Node.TEXT_NODE) return { node: before, offset: before.data.length };
    return null;
  }

  // Where in the published text the reader has got to, as the offset of the
  // first words they can see.
  //
  // Asked for when a newer version of the document is about to replace this
  // page. The offset itself is worth nothing afterwards -- the next rendering
  // starts from different text -- but the words at it are how the sidebar puts
  // the reader back where they were, so this only has to be right about which
  // words those are.
  //
  // Found by asking the document what sits at a point, rather than by
  // measuring text nodes until one is on screen: a paper has thousands of them
  // and this is a question about exactly one. The point is a little below the
  // top edge, because the line straddling the edge is half cut off and nobody
  // is reading it, and it is tried across the width, because the middle of a
  // page can be a figure, a margin, or the gap between two columns.
  function readingPosition() {
    if (!table?.nodes.length) return 0;
    const y = Math.min(Math.max(1, innerHeight - 1), 80);
    for (const fraction of [0.5, 0.25, 0.75]) {
      const at = offsetAtPoint(Math.round(innerWidth * fraction), y);
      if (at !== null) return at;
    }
    return 0;
  }

  function offsetAtPoint(x, y) {
    let container = null;
    let offset = 0;
    if (document.caretPositionFromPoint) {
      const position = document.caretPositionFromPoint(x, y);
      if (!position) return null;
      container = position.offsetNode;
      offset = position.offset;
    } else if (document.caretRangeFromPoint) {
      const range = document.caretRangeFromPoint(x, y);
      if (!range) return null;
      container = range.startContainer;
      offset = range.startOffset;
    } else {
      return null;
    }
    const point = container && textPointOf(container, offset);
    if (!point) return null;
    const index = table.index.get(point.node);
    if (index === undefined) return null;
    return table.starts[index] + point.offset;
  }

  function captureSelection() {
    const selection = document.getSelection();
    if (!selection || selection.isCollapsed) {
      post({ type: "selection", selector: null });
      return;
    }

    const range = selection.getRangeAt(0);
    // Draft Quarto inserts cached figures, tables, and text as generated
    // content.  Their visible words are not source prose, so a text quote
    // taken from them cannot safely be backfilled to a .qmd span.  Until an
    // artifact identity selector is available, make generated output
    // explicitly non-commentable instead of attaching a note to nearby prose.
    const generated = (node) => {
      const element = node?.nodeType === Node.ELEMENT_NODE ? node : node?.parentElement;
      return element?.closest?.("[data-librepaper-generated='quarto']");
    };
    if (generated(range.startContainer) || generated(range.endContainer)) {
      post({ type: "selection", selector: null });
      return;
    }
    const startPoint = textPointOf(range.startContainer, range.startOffset);
    const endPoint = textPointOf(range.endContainer, range.endOffset);
    if (!startPoint || !endPoint) return;
    const startIndex = table.index.get(startPoint.node);
    const endIndex = table.index.get(endPoint.node);
    if (startIndex === undefined || endIndex === undefined) return;
    let start = table.starts[startIndex] + startPoint.offset;
    let end = table.starts[endIndex] + endPoint.offset;

    const all = text();
    // The quote is cut from the same string the sidebar anchors against, not
    // from selection.toString(): that one collapses runs of whitespace and
    // inserts a break at every block boundary, so a passage spanning two
    // elements came back as text that appears nowhere in the document and
    // could never be re-anchored. Trimming moves the ends in rather than
    // rewriting what lies between them, which keeps the offsets true.
    while (start < end && /\s/.test(all[start])) start++;
    while (end > start && /\s/.test(all[end - 1])) end--;
    const exact = all.slice(start, end);
    if (!exact) return;

    const box = range.getBoundingClientRect();
    post({
      type: "selection",
      selector: {
        exact,
        prefix: all.slice(Math.max(0, start - 64), start),
        suffix: all.slice(end, end + 64),
        // A W3C TextPositionSelector alongside the quote. The quote stays the
        // authority; this only says which copy was meant when a document
        // repeats itself and the context cannot tell them apart.
        position: start,
      },
      // Viewport coordinates inside the frame; the sidebar adds the frame's
      // own offset to place its button.
      rect: { top: box.top, left: box.left, right: box.right, bottom: box.bottom },
    });
  }


  addEventListener("message", (event) => {
    if (!READER || event.source !== parent || event.origin !== READER) return;
    const message = event.data;
    if (!message || message.librepaper !== true) return;
    // The frame can finish loading and publish before Svelte has bound the
    // iframe element used to authenticate its message source. The reader
    // acknowledges its own load event so that readiness is never a one-shot
    // race.
    if (message.type === "reader-ready") publish(true);
    if (message.type === "highlight") highlight(message.ranges || []);
    // The editor's live preview. The document being previewed is not yet
    // published, so it arrives as HTML over this channel rather than as a
    // page to load -- which keeps it on this origin, where a document belongs,
    // instead of inside the reader's. Only the body is replaced: the styles
    // came from the same template that rendered this, and the observer is
    // attached to the body element, which has to survive.
    //
    // innerHTML does not run scripts, so a preview never executes anything.
    if (message.type === "preview") {
      // A LaTeX document arrives as PDF bytes rather than as HTML, and the
      // page it arrives on -- the PDF viewer -- draws
      // it itself into a text layer of ordinary spans. Nothing below applies
      // to that: there is no markup to parse and no body to replace, and
      // doing either would wipe the pages out from under the viewer. That is
      // the whole of the agent's knowledge of PDFs. Drawing the pages mutates
      // the body, so the observer at the bottom of this file republishes the
      // text exactly as it does for a document that builds itself in
      // JavaScript -- one code path for "the document changed", not two.
      if (message.pdf) return;
      const parsed = new DOMParser().parseFromString(String(message.html || ""), "text/html");
      // The frame this arrives in is an empty shell -- nothing rendered is
      // stored any more, so there is no page whose styles the body could
      // inherit. The page's own head comes with it, and is installed once:
      // it is the same template on every keystroke, so it is replaced only
      // when it actually differs.
      adoptStyles(parsed, message.presentation);
      quietly(() => {
        document.body.innerHTML = parsed.body.innerHTML;
        // In the same breath when KaTeX is already here, so the text published
        // below is the typeset text. The first time it is not, and the
        // rendering lands as a mutation the observer republishes.
        typesetMath();
      });
      // Replacing the body throws away every mark on it, and the ranges to
      // paint again only arrive after the sidebar has seen the new text and
      // worked them out. Between the two the document would show no
      // highlights at all -- which, at one repaint per keystroke, is a flicker
      // over the whole document while you type.
      //
      // So the marks go straight back on, in the same breath as the text, at
      // the offsets they had a moment ago. Typing shifts them by however much
      // was typed, which is a few characters for a few milliseconds, and then
      // the sidebar's own answer arrives and corrects them.
      if (lastRanges.length) highlight(shiftRanges(lastRanges));
      // Directly, not through the observer: the observer waits a quarter of a
      // second before republishing, and a preview should keep up with typing.
      publish(true);
    }

    // Show the reader where a place in the text is. The offset is into the
    // text this frame published, which is the only thing both sides agree on:
    // the editor works out which offset a caret in the source corresponds to,
    // and this end knows which node holds it.
    if (message.type === "locate") {
      const start = Number(message.start) || 0;
      const [piece] = piecesFor(start, start + Math.max(1, Number(message.length) || 1));
      if (!piece) return;
      const range = document.createRange();
      range.setStart(piece.node, piece.from);
      range.setEnd(piece.node, piece.to);
      const box = range.getBoundingClientRect();
      // Scrolled to a third of the way down rather than to the very top: a
      // line pinned to the edge of the frame reads as cut off.
      scrollTo({ top: scrollY + box.top - innerHeight / 3, behavior: "smooth" });
      return;
    }
    if (message.type === "reading-position") {
      post({ type: "reading-position", start: readingPosition() });
      return;
    }
    if (message.type === "synctex-locate") {
      globalThis.librepaperViewer?.locatePoint?.(message.page, message.x, message.y);
      return;
    }

    if (message.type === "select") select(message.id);
    if (message.type === "reveal") {
      select(message.id);
      const id = CSS.escape(String(message.id));
      document
        .querySelector(`mark[data-librepaper~="${id}"]`)
        ?.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  });

  // A single pending timer for selection capture, whichever event armed it
  // last: a drag fires mouseup once but selectionchange dozens of times, and
  // without a shared, re-armed timer each of those would queue its own call.
  let selectionTimer = null;
  function scheduleSelection(delay) {
    clearTimeout(selectionTimer);
    selectionTimer = setTimeout(captureSelection, delay);
  }

  // A click in the document, reported as an offset into the published text, so
  // the editor can put its caret in the same place. Only the position is sent;
  // a click that lands on nothing textual says nothing.
  document.addEventListener("click", (event) => {
    const pdf = globalThis.librepaperViewer?.pointFromClient?.(event.clientX, event.clientY) || null;
    if (!table.nodes.length) {
      if (pdf) post({ type: "pdf-caret", pdf });
      return;
    }
    const caret = document.caretPositionFromPoint
      ? document.caretPositionFromPoint(event.clientX, event.clientY)
      : null;
    const node = caret?.offsetNode;
    if (!node || node.nodeType !== Node.TEXT_NODE) {
      if (pdf) post({ type: "pdf-caret", pdf });
      return;
    }
    const index = table.index.get(node);
    if (index === undefined) return;
    const offset = table.starts[index] + (caret.offset || 0);
    post({ type: "caret", offset, pdf });
  });

  // The keys that act on a selection have to be heard here: the selection is
  // in this frame, and a keystroke aimed at it never reaches the page around
  // it. Only two, and both are about the gesture already under way -- "c" for
  // the passage in hand, Escape for the mode the sidebar armed -- so neither
  // takes a key away from a document that wants one.
  document.addEventListener("keydown", (event) => {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const target = event.target;
    if (target?.isContentEditable || target?.closest?.("input,textarea,select,[contenteditable]")) return;
    if (event.key === "Escape") {
      post({ type: "disarm" });
      return;
    }
    if (event.key !== "c" && event.key !== "C") return;
    const selection = document.getSelection();
    if (!selection || selection.isCollapsed) return;
    event.preventDefault();
    post({ type: "annotate" });
  });

  // A selection is only reported once the gesture that made it is over. Mid
  // drag the passage is still growing, and a bar that follows it is a target
  // that moves under the pointer and lands somewhere else the moment the
  // button comes up -- so while the pointer is down nothing is reported, and
  // the bar from the last selection goes away as soon as the new one starts.
  //
  // `selectionchange` still speaks for the keyboard, which has no drag: a
  // selection extended with shift and the arrows reports as it grows.
  let dragging = false;
  function beginDrag() {
    dragging = true;
    clearTimeout(selectionTimer);
    post({ type: "selection", selector: null });
  }
  function endDrag(delay) {
    dragging = false;
    scheduleSelection(delay);
  }

  document.addEventListener("mousedown", beginDrag);
  document.addEventListener("mouseup", () => endDrag(0));
  document.addEventListener("touchstart", beginDrag, { passive: true });
  document.addEventListener("touchend", () => endDrag(120), { passive: true });
  document.addEventListener("selectionchange", () => {
    if (!dragging) scheduleSelection(80);
  });

  // The agent is injected before </body>, so the markup has parsed by the time
  // it runs -- but a document that builds itself in JavaScript has not. Its own
  // scripts run on DOMContentLoaded and load, and whatever they add arrives
  // after this snapshot would have been taken. Anchoring against a text the
  // document has since outgrown puts every highlight in the wrong place, so the
  // text is published when the document has settled, and again whenever it
  // changes. Painting adds no text, so a repaint never triggers a round trip --
  // and now that painting runs inside `quietly`, the observer never even sees
  // it happen.
  let published = null;

  function publish(force = false) {
    scan();
    const current = text();
    if (!force && current === published) return;
    published = current;
    post({ type: "ready", text: current });
  }

  let pending = null;
  const republish = () => {
    clearTimeout(pending);
    // A DOM change can leave visible text equal while still destroying
    // highlights. Signal the shell to repaint remembered annotations.
    pending = setTimeout(() => publish(true), 250);
  };

  function watch() {
    publish();
    // The first bundle runs in the child frame's load handler. The
    // parent's iframe load handler, and therefore Svelte's authenticated
    // frame binding, can settle just afterward. Repeat once on the next task;
    // later bundles remain mutation-driven.
    setTimeout(() => publish(true), 0);
    observer = new MutationObserver(republish);
    observer.observe(document.body, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  }

  if (document.readyState === "complete") watch();
  else addEventListener("load", watch, { once: true });
})();
