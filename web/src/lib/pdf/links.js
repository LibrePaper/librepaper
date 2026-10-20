// The clickable links of a PDF page.
//
// A PDF keeps its links as annotations: a rectangle plus either a URL or a
// destination inside the document. This module turns the Link annotations of
// one page into a layer of `<a>` elements that sits over the page, and
// resolves destinations (which the outline in `sidebar.js` shares) to a page
// and a distance from its top edge.
//
// The anchors hold no text. `frame.js` walks every text node in the body to
// publish the document's text, and a link's URL is not part of what the page
// says. External links are plain hrefs, which `frame.js` already opens in a
// new window; internal ones are `#page=N` hrefs whose click is handled here.
// The pure arithmetic is exported so `tests/unit/pdf-links.mjs` can check it
// without a browser or a PDF.

const EXTERNAL = new Set(["http:", "https:", "mailto:"]);

/// `url` if it is an absolute http, https or mailto address, else null.
///
/// A PDF can carry `javascript:`, `file:` and `data:` links; none of them is
/// something a document should be able to make the reader's browser follow.
export function safeUrl(url) {
  if (typeof url !== "string" || url === "") return null;
  try {
    const parsed = new URL(url);
    return EXTERNAL.has(parsed.protocol) ? parsed.href : null;
  } catch {
    return null;
  }
}

/// The external address of a link annotation, or null when it has none that
/// is safe. pdf.js already sanitises `url`; `unsafeUrl` is the raw one, and is
/// used only when it passes the same filter.
export function externalHref(annotation) {
  return safeUrl(annotation?.url) ?? safeUrl(annotation?.unsafeUrl);
}

/// The `top` a destination's arguments carry, by destination kind.
///
/// XYZ is `[left, top, zoom]`; FitH and FitBH are `[top]`; the rest (Fit,
/// FitV, FitR ...) do not pin a vertical position we can use.
export function destTop(kind, args) {
  if (kind === "XYZ") return args?.[1];
  if (kind === "FitH" || kind === "FitBH") return args?.[0];
  return null;
}

/// The `left` a destination's arguments carry: XYZ is `[left, top, zoom]`, the
/// others pin no horizontal position.
export function destLeft(kind, args) {
  return kind === "XYZ" ? args?.[0] : null;
}

/// The page a named action leads to from page `current` of `count`, or null.
export function namedActionPage(action, current, count) {
  if (!(count >= 1)) return null;
  if (action === "NextPage") return Math.min(count, current + 1);
  if (action === "PrevPage") return Math.max(1, current - 1);
  if (action === "FirstPage") return 1;
  if (action === "LastPage") return count;
  return null;
}

/// A PDF rectangle as `{ left, top, width, height }` in percent of the page
/// box, given the corners already converted to viewport space.
///
/// Percentages, not pixels, because the page is later resized with CSS (fit
/// modes, zoom) and the layer has to follow without being rebuilt.
export function boxPercent(corners, width, height) {
  if (!(width > 0) || !(height > 0)) return null;
  const [x1, y1, x2, y2] = corners;
  const left = Math.min(x1, x2);
  const top = Math.min(y1, y2);
  return {
    left: (left / width) * 100,
    top: (top / height) * 100,
    width: (Math.abs(x2 - x1) / width) * 100,
    height: (Math.abs(y2 - y1) / height) * 100,
  };
}

/// Resolve a PDF destination (a name, or an explicit array) to
/// `{ page, y }`: `page` 1-based, `y` in PDF points from the top edge of the
/// page as displayed (rotation applied) or null. Anything that cannot be resolved gives null; it never throws,
/// because a broken link must not stop the page it is on from drawing.
export async function resolveDest(pdfDocument, dest) {
  try {
    let explicit = dest;
    if (typeof dest === "string") explicit = await pdfDocument.getDestination(dest);
    if (!Array.isArray(explicit) || explicit.length === 0) return null;
    const [ref, mode, ...args] = explicit;
    let index;
    if (Number.isInteger(ref)) index = ref;
    else if (ref && typeof ref === "object") index = await pdfDocument.getPageIndex(ref);
    else return null;
    if (!Number.isInteger(index) || index < 0 || index >= pdfDocument.numPages) return null;
    const top = destTop(mode?.name, args);
    if (typeof top !== "number") return { page: index + 1, y: null };
    // The page's own viewport, so /Rotate and the user unit are applied: the
    // y of a rotated page is not "height minus top".
    const page = await pdfDocument.getPage(index + 1);
    const viewport = page.getViewport({ scale: 1 });
    const left = destLeft(mode?.name, args);
    const [, y] = viewport.convertToViewportPoint(typeof left === "number" ? left : 0, top);
    if (!Number.isFinite(y)) return { page: index + 1, y: null };
    return { page: index + 1, y: Math.max(0, Math.min(viewport.height, y)) };
  } catch {
    return null;
  }
}

/// The link layer for one page: a `div.linkLayer` with one `<a>` per Link
/// annotation. Returns an empty layer when the annotations cannot be read.
export async function linkLayer(page, viewport, pdfDocument, ongo) {
  const layer = document.createElement("div");
  layer.className = "linkLayer";
  layer.style.cssText = "position:absolute;inset:0;";
  let annotations = [];
  try {
    annotations = await page.getAnnotations({ intent: "display" });
  } catch {
    return layer;
  }
  for (const annotation of annotations) {
    if (annotation.subtype !== "Link" || !Array.isArray(annotation.rect)) continue;
    const box = boxPercent(
      viewport.convertToViewportRectangle(annotation.rect),
      viewport.width,
      viewport.height,
    );
    if (!box) continue;
    const anchor = document.createElement("a");
    const href = externalHref(annotation);
    if (href) {
      anchor.href = href;
      anchor.target = "_blank";
      anchor.rel = "noopener noreferrer";
      anchor.setAttribute("aria-label", href);
    } else if (annotation.dest || annotation.action) {
      const target = await internalTarget(page, pdfDocument, annotation);
      if (!target) continue;
      anchor.href = `#page=${target.page}`;
      anchor.dataset.page = String(target.page);
      anchor.setAttribute("aria-label", `Go to page ${target.page}`);
      anchor.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        ongo(target);
      });
    } else {
      continue;
    }
    anchor.style.cssText =
      "position:absolute;display:block;" +
      `left:${box.left}%;top:${box.top}%;width:${box.width}%;height:${box.height}%;`;
    layer.append(anchor);
  }
  return layer;
}

async function internalTarget(page, pdfDocument, annotation) {
  if (annotation.dest) return resolveDest(pdfDocument, annotation.dest);
  const number = namedActionPage(annotation.action, page.pageNumber, pdfDocument.numPages);
  return number ? { page: number, y: null } : null;
}
