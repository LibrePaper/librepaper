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

  // One tint per tool, so what a mark means is legible without opening the
  // sidebar. Hue carries the meaning and saturation stays low: these sit under
  // running text for as long as the document is open, and a saturated wash
  // would fight the words it is meant to mark. Kept in step with the tool
  // buttons and the sidebar labels, which use the same hues.
  const TINTS = {
    commenting: [42, 55],
    questioning: [255, 30],
    highlighting: [145, 28],
    editing: [8, 45],
    assessing: [195, 35],
  };
  const NEUTRAL = [220, 12]; // resolved: the colour has served its purpose
  const tintOf = (motivation) => TINTS[motivation] || TINTS.commenting;
  // Each annotation stacked on the same words takes the wash a step deeper,
  // stopping where dark text would start to struggle against it.
  const wash = ([hue, saturation], depth, alpha = 1) =>
    `hsl(${hue} ${saturation}% ${Math.max(70, 90 - (Math.min(depth, 5) - 1) * 5)}% / ${alpha})`;
  const edge = ([hue, saturation]) => `hsl(${hue} ${Math.min(saturation + 10, 60)}% 45%)`;

  function post(message) {
    parent.postMessage({ komodoc: true, ...message }, READER);
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
      if (to > from) pieces.push({ node: table.nodes[i], from, to });
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
  function highlight(ranges) {
    quietly(() => {
      document
        .querySelectorAll("mark[data-komodoc]")
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
        mark.dataset.komodoc = covering.map((item) => item.id).join(" ");
        const live = covering.filter((item) => !item.resolved);
        // The innermost annotation is the one this stretch most specifically
        // belongs to, so its tool decides the colour; the number of annotations
        // stacked here decides how deep the wash goes.
        const inner = (live.length ? live : covering).reduce((a, b) =>
          b.end - b.start < a.end - a.start ? b : a,
        );
        const shade = live.length
          ? wash(tintOf(inner.motivation), live.length)
          : wash(NEUTRAL, 1);
        mark.style.cssText = `background:${shade};color:inherit;cursor:pointer`;
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

  const images = () => [...document.images].filter((img) => img.width > 40 && img.height > 40);

  async function digestOf(image) {
    if (digests.has(image)) return digests.get(image);
    const source = image.currentSrc || image.src || "";
    const bytes = new TextEncoder().encode(source);
    const hash = await crypto.subtle.digest("SHA-256", bytes);
    const hex = [...new Uint8Array(hash)]
      .slice(0, 8)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
    digests.set(image, hex);
    return hex;
  }

  // Every image gets a positioned wrapper once, so boxes drawn over it move
  // with it: no recomputing on scroll, no listening to resize.
  function layerFor(image) {
    let wrap = image.parentElement;
    if (!wrap || wrap.dataset.komodocFigure !== "1") {
      wrap = document.createElement("span");
      wrap.dataset.komodocFigure = "1";
      wrap.style.cssText = "position:relative;display:inline-block;max-width:100%";
      image.replaceWith(wrap);
      wrap.appendChild(image);
    }
    let layer = wrap.querySelector(":scope > .komodoc-regions");
    if (!layer) {
      layer = document.createElement("span");
      layer.className = "komodoc-regions";
      layer.style.cssText = "position:absolute;inset:0;pointer-events:none";
      wrap.appendChild(layer);
    }
    return layer;
  }

  // Paint the rectangles the sidebar could place, one layer per image.
  async function paintRegions(regions) {
    quietly(() => {
      document.querySelectorAll(".komodoc-regions").forEach((layer) => (layer.innerHTML = ""));
    });
    const found = images();
    // The digests touch crypto.subtle, which is the slow part; running them
    // together rather than one at a time in a loop is free concurrency.
    const hexes = await Promise.all(found.map((image) => digestOf(image)));
    const byDigest = new Map(found.map((image, i) => [hexes[i], image]));

    quietly(() => {
      for (const item of regions) {
        const image = byDigest.get(item.digest) || found[item.index];
        if (!image) continue;
        const box = document.createElement("span");
        box.dataset.komodoc = item.id;
        box.style.cssText =
          `position:absolute;left:${item.x}%;top:${item.y}%;width:${item.w}%;height:${item.h}%;` +
          `border:2px solid ${edge(item.resolved ? NEUTRAL : tintOf(item.motivation))};` +
          `background:${wash(item.resolved ? NEUTRAL : tintOf(item.motivation), 1, 0.35)};` +
          "pointer-events:auto;cursor:pointer;box-sizing:border-box";
        box.onclick = () => post({ type: "focus", id: item.id });
        layerFor(image).appendChild(box);
      }
    });
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
    if (!message || message.komodoc !== true) return;
    if (message.type === "highlight") highlight(message.ranges || []);
    if (message.type === "regions") paintRegions(message.regions || []);
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
    if (message.type === "reveal") {
      document
        .querySelector(`mark[data-komodoc~="${CSS.escape(String(message.id))}"]`)
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

  function publish() {
    scan();
    const current = text();
    if (current === published) return;
    published = current;
    // Where each figure sits in the text, so the sidebar can order a note on a
    // figure against the notes on passages instead of guessing. Computed by
    // scan() in the same walk that built the text-node table.
    post({ type: "ready", text: current, images: table.offsets });
  }

  let pending = null;
  const republish = () => {
    clearTimeout(pending);
    pending = setTimeout(publish, 250);
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
