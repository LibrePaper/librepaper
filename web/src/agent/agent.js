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
  const READER = new URL(document.currentScript.src).searchParams.get("reader") || "*";
  let table = null; // {nodes, starts, index, offsets, joined}

  // The observer that republishes on the document's own edits (armed in
  // watch()). Painting -- highlight(), paintRegions(), layerFor() -- mutates
  // the DOM too, and left running the observer cannot tell our own brushwork
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
    html.librepaper-preview.librepaper-flow { background: #f6f6f4; }
    html.librepaper-preview.librepaper-flow body {
      box-sizing: border-box;
      width: min(800px, calc(100vw - 32px));
      max-width: 800px;
      margin: 32px auto 64px;
      padding: 32px 40px;
      background: #fff;
    }
    @media (max-width: 640px) {
      html.librepaper-preview.librepaper-flow body {
        width: calc(100vw - 24px);
        margin: 16px auto 40px;
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

  // A pending suggestion's proposal is drawn after its passage by
  // `::after`, which adds no text node -- the offset tables this file keeps
  // stay true to what the document actually contains. Installed once, up
  // front: unlike the document's own styles, nothing here ever needs to
  // change, and it survives every body replacement `adoptStyles` does for a
  // live preview.
  const proposedStyle = document.createElement("style");
  proposedStyle.dataset.librepaperProposed = "1";
  proposedStyle.textContent = `
    mark[data-proposed]::after {
      content: attr(data-proposed);
      text-decoration: underline;
      color: var(--librepaper-proposed-color, inherit);
      margin-inline-start: 0.2em;
    }
  `;
  document.head.appendChild(proposedStyle);

  // Redlines: what changed since a history checkpoint, painted inline. An
  // insertion is a mark wrapping the words themselves, same as a highlight;
  // a deletion has no words left to wrap, so it is an empty mark whose
  // `::before` draws the struck-through text CSS keeps, not the DOM --
  // exactly the `data-proposed` trick above, so the offset tables below
  // never see it as a character of real content.
  const redlineStyle = document.createElement("style");
  redlineStyle.dataset.librepaperRedlines = "1";
  // A colour per author, the way Google Docs paints its own redlines: five
  // hues, `mark[data-author="0..4"]`, matching -- but not sharing code with,
  // this frame has no access to the sidebar's theme tokens -- the literal
  // hex values `theme.css` gives `--color-primary-500`, `--color-warning-
  // -500`, `--color-error-500`, `--color-tertiary-500` and `--color-surface-
  // 500`. `Reader.svelte`'s `applyRedlines` assigns the index the same way
  // `History.svelte`'s timeline dot does: first appearance in the manifest,
  // oldest first, mod 5. An item with no author (attribution fell back to
  // "several people" or nobody at all) keeps the plain default below.
  const authorHues = ["#2b5748", "#b07d3a", "#a33f3f", "#9cb080", "#273338"];
  const authorRules = authorHues.map((hue, index) => `
    mark.librepaper-ins[data-author="${index}"] {
      background: color-mix(in srgb, ${hue} 22%, white);
      text-decoration-color: ${hue};
    }
    mark.librepaper-del[data-author="${index}"]::before {
      background: color-mix(in srgb, ${hue} 16%, white);
      color: ${hue};
    }
  `).join("\n");
  redlineStyle.textContent = `
    mark.librepaper-ins {
      background: hsl(145 45% 88%);
      color: inherit;
      text-decoration: underline;
      text-decoration-color: hsl(145 45% 32%);
    }
    mark.librepaper-del {
      background: transparent;
      padding: 0;
    }
    mark.librepaper-del::before {
      content: attr(data-deleted);
      background: hsl(0 45% 93%);
      color: hsl(0 55% 40%);
      text-decoration: line-through;
    }
    ${authorRules}
  `;
  document.head.appendChild(redlineStyle);
  function adoptStyles(parsed) {
    document.documentElement.classList.add("librepaper-preview");
    document.documentElement.classList.toggle("librepaper-flow", !parsed.body.querySelector(":scope > svg.typst-doc"));
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
    parent.postMessage({ librepaper: true, ...message }, READER);
  }

  // One walk of the document builds the text-node table (with a node->index
  // map for fast lookup), the cumulative-offset table, and the offsets of the
  // qualifying figures -- all three needed the same walk over the same nodes,
  // so a second pass just to find the images was a second tree walk for free.
  // Rebuilt whenever the highlights change the node structure underneath us.
  // The joined text is a separate, lazy step: a repaint needs the table but
  // not the string, and joining a large document is the most expensive thing
  // here, so it is only paid for on the first `text()` call after a scan.
  function scan() {
    const walker = document.createTreeWalker(
      document.body,
      NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT,
    );
    const nodes = [];
    const starts = [];
    const index = new Map();
    const offsets = [];
    let total = 0;
    let node;
    while ((node = walker.nextNode())) {
      if (node.nodeType === Node.TEXT_NODE) {
        const parent = node.parentElement;
        if (parent && ["SCRIPT", "STYLE", "NOSCRIPT"].includes(parent.tagName)) continue;
        index.set(node, nodes.length);
        starts.push(total);
        nodes.push(node);
        total += node.data.length;
      } else if (node.tagName === "IMG" && node.width > 40 && node.height > 40) {
        offsets.push(total);
      }
    }
    table = { nodes, starts, index, offsets, joined: null };
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
      document
        .querySelectorAll("mark[data-librepaper]")
        .forEach((mark) => mark.replaceWith(...mark.childNodes));
      document.body.normalize(); // restore the pristine text-node structure
    });
    scan();

    const painted = ranges.filter((item) => item.end > item.start);
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
        const shade = live.length
          ? wash(tintOf(inner.motivation), live.length, 0.42)
          : wash(NEUTRAL, 1, 0.24);
        mark.style.cssText = `background:${shade};color:inherit;cursor:pointer`;
        // A pending suggestion is struck through along its whole extent, and
        // its proposal is drawn once, after the last segment it covers -- the
        // segment whose own end lands on the suggestion's end offset. A
        // decided suggestion falls out of `resolved` and is painted like any
        // other resolved comment, above.
        const suggestion = live.find((item) => item.motivation === "editing");
        if (suggestion) {
          mark.style.textDecoration = "line-through";
          mark.style.textDecorationColor = edge(tintOf("editing"));
          if (piece.end === suggestion.end) {
            mark.dataset.proposed = suggestion.proposed || "";
            mark.style.setProperty("--librepaper-proposed-color", edge(tintOf("editing")));
          }
        }
        range.surroundContents(mark);
        // The same innermost annotation the colour came from is the one a click
        // on this stretch means.
        mark.onclick = () => post({ type: "focus", id: inner.id });
      }
    });
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

  // The redlines last asked for, remembered for the same reason `lastRanges`
  // is: a document rebuild (a "preview" replacing the body, a fresh "ready")
  // needs to put them back without waiting on a round trip to the sidebar.
  let lastRedlineItems = [];

  // Marks from a previous `redlines()` call, cleared without disturbing
  // `highlight()`'s marks -- a different class, so the two coexist and
  // either can be repainted without the other flickering. An insert mark
  // wraps real text and is unwrapped the way a highlight mark is; a delete
  // mark wraps nothing and is simply removed.
  function clearRedlineMarks() {
    quietly(() => {
      document.querySelectorAll("mark.librepaper-ins").forEach((mark) => mark.replaceWith(...mark.childNodes));
      document.querySelectorAll("mark.librepaper-del").forEach((mark) => mark.remove());
      document.body.normalize();
    });
  }

  // The redline equivalent of `shiftRanges`: a preview rebuild during typing
  // moves everything after the edit point by however much the text grew or
  // shrank, before the sidebar's own recomputed hunks arrive.
  function shiftRedlineItems(items) {
    const before = published || "";
    scan();
    const after = text();
    let same = 0;
    while (same < before.length && same < after.length && before[same] === after[same]) same++;
    const delta = after.length - before.length;
    if (!delta) return items;
    return items.map((item) =>
      item.kind === "insert"
        ? item.start >= same ? { ...item, start: item.start + delta, end: item.end + delta } : item
        : item.at >= same ? { ...item, at: item.at + delta } : item,
    );
  }

  // Paint, from a list of `{start, end, kind:"insert", who}` and
  // `{at, kind:"delete", text, who}` items the sidebar worked out from the
  // history panel's word diff -- see `redlines.js`'s `itemsFor`. Deletes go
  // in first, since they only ever insert an empty marker and never change
  // any offset an insert range depends on; inserts then reuse `piecesFor`,
  // the same segment machinery `highlight` paints with, so a passage that
  // crosses element boundaries is still covered piece by piece.
  function redlines(items) {
    lastRedlineItems = items;
    clearRedlineMarks();
    scan();

    const deletes = [...items.filter((item) => item.kind === "delete")].sort((a, b) => b.at - a.at);
    quietly(() => {
      for (const item of deletes) {
        const at = Number(item.at) || 0;
        if (at < 0 || at > text().length || !table.nodes.length) continue;
        const index = nodeAt(at);
        const node = table.nodes[index];
        if (!node) continue;
        const range = document.createRange();
        range.setStart(node, at - table.starts[index]);
        range.collapse(true);
        const mark = document.createElement("mark");
        mark.className = "librepaper-del";
        mark.dataset.deleted = item.text || "";
        if (item.author !== undefined && item.author !== null) mark.dataset.author = String(item.author);
        mark.title = [item.who, item.text].filter(Boolean).join(": ");
        range.insertNode(mark);
      }
    });
    // The delete markers above split text nodes at their offsets; the table
    // has to catch up before piecesFor, below, can trust it.
    scan();

    const inserts = items.filter((item) => item.kind === "insert" && item.end > item.start);
    quietly(() => {
      for (const item of [...inserts].reverse()) {
        for (const piece of piecesFor(item.start, item.end)) {
          const range = document.createRange();
          range.setStart(piece.node, piece.from);
          range.setEnd(piece.node, piece.to);
          const mark = document.createElement("mark");
          mark.className = "librepaper-ins";
          if (item.who) mark.title = item.who;
          if (item.author !== undefined && item.author !== null) mark.dataset.author = String(item.author);
          range.surroundContents(mark);
        }
      }
    });
    // Same reasoning as `highlight`'s closing rescan: surroundContents split
    // nodes the table no longer describes.
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

  function captureSelection() {
    const selection = document.getSelection();
    if (!selection || selection.isCollapsed) {
      post({ type: "selection", selector: null });
      return;
    }

    const range = selection.getRangeAt(0);
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


  /* --------------------------------------------------------------- figures */

  // Annotating part of a figure. Which image is the harder question: a figure
  // has no words around it to anchor to. Two identifiers are kept, a digest of
  // the image source and its position among the document's images, and the
  // first that matches wins.

  let tool = "commenting"; // set by the sidebar; only "region" draws on figures
  const digests = new WeakMap();
  let regionsGeneration = 0;

  const images = () => [...document.images].filter((img) => img.width > 40 && img.height > 40);

  // Markdown figures use a blob URL, which is intentionally recreated on each
  // page session. The renderer can carry the immutable store digest either as
  // metadata or in the blob URL fragment; prefer that stable identity and only
  // hash the URL for documents that provide no asset identity.
  function stableImageDigest(image) {
    const metadata =
      image.dataset.librepaperImageDigest ||
      image.dataset.librepaperDigest ||
      image.getAttribute("data-librepaper-image-digest") ||
      image.getAttribute("data-librepaper-digest");
    if (metadata) return metadata.slice(0, 16);
    const source = image.currentSrc || image.src || "";
    try {
      const marker = new URL(source, document.baseURI).hash.match(/(?:^#|[?&])librepaper-asset=([^&#]+)/);
      if (marker) return decodeURIComponent(marker[1]).slice(0, 16);
    } catch {
      /* an invalid source is handled by the URL hash fallback below */
    }
    return null;
  }

  async function digestOf(image) {
    const source = image.currentSrc || image.src || "";
    const stable = stableImageDigest(image);
    const identity = stable || source;
    const cached = digests.get(image);
    if (cached?.identity === identity) return cached.digest;
    if (stable) {
      digests.set(image, { identity, digest: stable });
      return stable;
    }
    const bytes = new TextEncoder().encode(source);
    const hash = await crypto.subtle.digest("SHA-256", bytes);
    const hex = [...new Uint8Array(hash)]
      .slice(0, 8)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
    digests.set(image, { identity, digest: hex });
    return hex;
  }

  // Every image gets a positioned wrapper once, so boxes drawn over it move
  // with it: no recomputing on scroll, no listening to resize.
  function layerFor(image) {
    let wrap = image.parentElement;
    if (!wrap || wrap.dataset.librepaperFigure !== "1") {
      wrap = document.createElement("span");
      wrap.dataset.librepaperFigure = "1";
      wrap.style.cssText = "position:relative;display:inline-block;max-width:100%";
      image.replaceWith(wrap);
      wrap.appendChild(image);
    }
    let layer = wrap.querySelector(":scope > .librepaper-regions");
    if (!layer) {
      layer = document.createElement("span");
      layer.className = "librepaper-regions";
      layer.style.cssText = "position:absolute;inset:0;pointer-events:none";
      wrap.appendChild(layer);
    }
    return layer;
  }

  // Paint the rectangles the sidebar could place, one layer per image.
  async function paintRegions(regions) {
    const mine = ++regionsGeneration;
    quietly(() => {
      document.querySelectorAll(".librepaper-regions").forEach((layer) => (layer.innerHTML = ""));
    });
    const found = images();
    // The digests touch crypto.subtle, which is the slow part; running them
    // together rather than one at a time in a loop is free concurrency.
    const hexes = await Promise.all(found.map((image) => digestOf(image)));
    if (mine !== regionsGeneration) return;
    const byDigest = new Map(found.map((image, i) => [hexes[i], image]));
    const placed = [];
    const unplaceable = [];
    const pdf = Boolean(document.querySelector(".pages"));

    quietly(() => {
      for (const item of regions) {
        // A known identity that no longer exists must not silently fall back to
        // a new image at the old position. Positional matching is reserved for
        // legacy annotations that never had an identity in the first place.
        const image = item.digest ? byDigest.get(item.digest) : found[item.index];
        if (!image) {
          if (item.id != null) unplaceable.push(String(item.id));
          continue;
        }
        const box = document.createElement("span");
        box.dataset.librepaper = item.id;
        box.style.cssText =
          `position:absolute;left:${item.x}%;top:${item.y}%;width:${item.w}%;height:${item.h}%;` +
          `border:2px solid ${edge(item.resolved ? NEUTRAL : tintOf(item.motivation))};` +
          `background:${wash(item.resolved ? NEUTRAL : tintOf(item.motivation), 1, 0.35)};` +
          "pointer-events:auto;cursor:pointer;box-sizing:border-box";
        box.onclick = () => post({ type: "focus", id: item.id });
        layerFor(image).appendChild(box);
        if (item.id != null) placed.push(String(item.id));
      }
    });
    // A PDF has pages, not HTML image elements. Its old figure-region records
    // remain in the comments, but there is no honest page/figure coordinate
    // transform, so report them rather than attaching them to a whole page or
    // to a different image. The same message also labels a missing HTML image
    // until a later mutation makes it placeable.
    if (unplaceable.length) {
      post({ type: "regions-unplaceable", ids: unplaceable, reason: pdf ? "pdf" : "figure-unavailable" });
    }
    if (placed.length) post({ type: "regions-placeable", ids: placed });
  }

  // Dragging a rectangle on a figure, while the region tool is chosen.
  let drawing = null;

  function percentWithin(image, event) {
    const box = image.getBoundingClientRect();
    return {
      x: ((event.clientX - box.left) / box.width) * 100,
      y: ((event.clientY - box.top) / box.height) * 100,
    };
  }

  document.addEventListener(
    "pointerdown",
    (event) => {
      if (tool !== "region" || event.button !== 0) return;
      const image = event.target.closest?.("img");
      if (!image || !images().includes(image)) return;
      event.preventDefault();
      // A touch drag is claimed by the browser as a scroll unless the element
      // has given it up (touch-action, set with the tool) and the drag is
      // captured, which is also what keeps the moves coming when a finger
      // leaves the figure.
      image.setPointerCapture?.(event.pointerId);

      const start = percentWithin(image, event);
      const outline = document.createElement("span");
      outline.style.cssText =
        `position:absolute;border:2px dashed ${edge(tintOf("commenting"))};` +
        `background:${wash(tintOf("commenting"), 1, 0.35)};pointer-events:none;box-sizing:border-box`;
      quietly(() => layerFor(image).appendChild(outline));
      drawing = { image, start, outline };
    },
    true,
  );

  document.addEventListener("pointermove", (event) => {
    if (!drawing) return;
    const now = percentWithin(drawing.image, event);
    const { start } = drawing;
    Object.assign(drawing.outline.style, {
      left: Math.min(start.x, now.x) + "%",
      top: Math.min(start.y, now.y) + "%",
      width: Math.abs(now.x - start.x) + "%",
      height: Math.abs(now.y - start.y) + "%",
    });
  });

  // The browser can take the gesture back mid-drag. Nothing was asked for, so
  // the outline goes with it rather than being left on the figure.
  document.addEventListener("pointercancel", () => {
    if (!drawing) return;
    drawing.outline.remove();
    drawing = null;
  });

  document.addEventListener("pointerup", async (event) => {
    if (!drawing) return;
    const { image, start, outline } = drawing;
    drawing = null;
    const now = percentWithin(image, event);
    outline.remove();

    const rectangle = {
      x: Math.max(0, Math.min(start.x, now.x)),
      y: Math.max(0, Math.min(start.y, now.y)),
      w: Math.min(100, Math.abs(now.x - start.x)),
      h: Math.min(100, Math.abs(now.y - start.y)),
    };
    // A click rather than a drag: nothing was asked for.
    if (rectangle.w < 1 || rectangle.h < 1) return;

    const box = image.getBoundingClientRect();
    post({
      type: "region",
      region: {
        ...rectangle,
        image_digest: await digestOf(image),
        image_index: images().indexOf(image),
      },
      // Where to put the button, in the frame's own coordinates.
      rect: {
        top: box.top + (rectangle.y / 100) * box.height,
        left: box.left + (rectangle.x / 100) * box.width,
        right: box.left + ((rectangle.x + rectangle.w) / 100) * box.width,
        bottom: box.top + ((rectangle.y + rectangle.h) / 100) * box.height,
      },
    });
  });

  addEventListener("message", (event) => {
    if (event.source !== parent) return;
    const message = event.data;
    if (!message || message.librepaper !== true) return;
    if (message.type === "highlight") highlight(message.ranges || []);
    if (message.type === "regions") paintRegions(message.regions || []);
    if (message.type === "redlines") redlines(message.items || []);
    // The tool the sidebar is on: only "region" makes figures draggable, and
    // it also stops text selection fighting the drag.
    if (message.type === "tool") {
      tool = String(message.tool || "commenting");
      document.body.style.userSelect = tool === "region" ? "none" : "";
      // touch-action is what decides a touch drag: without giving it up here,
      // dragging a box on a figure just scrolls the document instead.
      for (const image of images()) {
        image.style.cursor = tool === "region" ? "crosshair" : "";
        image.style.touchAction = tool === "region" ? "none" : "";
      }
    }
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
      adoptStyles(parsed);
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
      if (lastRedlineItems.length) redlines(shiftRedlineItems(lastRedlineItems));
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

    if (message.type === "reveal") {
      // A note on a passage is painted as a <mark>; a note on a figure is a box
      // in that figure's region layer. Either one is what "go to it" means.
      const id = CSS.escape(String(message.id));
      document
        .querySelector(`mark[data-librepaper~="${id}"], .librepaper-regions [data-librepaper~="${id}"]`)
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
    if (!table.nodes.length) return;
    const caret = document.caretPositionFromPoint
      ? document.caretPositionFromPoint(event.clientX, event.clientY)
      : null;
    const node = caret?.offsetNode;
    if (!node || node.nodeType !== Node.TEXT_NODE) return;
    const index = table.index.get(node);
    if (index === undefined) return;
    post({ type: "caret", offset: table.starts[index] + (caret.offset || 0) });
  });

  document.addEventListener("mouseup", () => scheduleSelection(0));
  document.addEventListener("touchend", () => scheduleSelection(120), { passive: true });
  document.addEventListener("selectionchange", () => scheduleSelection(80));

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
    // Where each figure sits in the text, so the sidebar can order a note on a
    // figure against the notes on passages instead of guessing. Computed by
    // scan() in the same walk that built the text-node table.
    post({ type: "ready", text: current, images: table.offsets });
  }

  let pending = null;
  const republish = () => {
    clearTimeout(pending);
    // A style, image source, or other DOM change can leave visible text equal
    // while still destroying region overlays. The shell needs a fresh ready
    // signal so it can repaint remembered annotations and figure positions.
    pending = setTimeout(() => publish(true), 250);
  };

  function watch() {
    publish();
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
