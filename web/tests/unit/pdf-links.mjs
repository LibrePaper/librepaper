// The arithmetic and filtering behind PDF links, without a browser or a PDF.

import assert from "node:assert/strict";
import {
  boxPercent,
  destTop,
  externalHref,
  namedActionPage,
  resolveDest,
  safeUrl,
  yFromTop,
} from "../../src/lib/pdf/links.js";

assert.equal(safeUrl("https://example.org/a"), "https://example.org/a");
assert.equal(safeUrl("http://example.org/"), "http://example.org/");
assert.equal(safeUrl("mailto:a@b.org"), "mailto:a@b.org");
for (const bad of ["javascript:alert(1)", "file:///etc/passwd", "data:text/html,x", "/relative", "", null, undefined, 3]) {
  assert.equal(safeUrl(bad), null, `refuses ${String(bad)}`);
}

assert.equal(externalHref({ url: "https://a.org/", unsafeUrl: "javascript:x" }), "https://a.org/");
assert.equal(externalHref({ unsafeUrl: "https://b.org/" }), "https://b.org/");
assert.equal(externalHref({ unsafeUrl: "javascript:x" }), null);
assert.equal(externalHref({}), null);

const view = [0, 0, 612, 792];
assert.equal(yFromTop(view, 700), 92);
assert.equal(yFromTop(view, 900), 0, "clamped to the top");
assert.equal(yFromTop(view, -50), 792, "clamped to the bottom");
assert.equal(yFromTop([0, 10, 612, 802], 802), 0, "offset boxes work");
assert.equal(yFromTop(view, null), null);
assert.equal(yFromTop(view, undefined), null);
assert.equal(yFromTop(null, 5), null);

assert.equal(destTop("XYZ", [72, 700, null]), 700);
assert.equal(destTop("XYZ", [72, null, null]), null);
assert.equal(destTop("FitH", [650]), 650);
assert.equal(destTop("FitBH", [650]), 650);
assert.equal(destTop("Fit", []), null);

assert.equal(namedActionPage("NextPage", 3, 10), 4);
assert.equal(namedActionPage("NextPage", 10, 10), 10);
assert.equal(namedActionPage("PrevPage", 1, 10), 1);
assert.equal(namedActionPage("PrevPage", 5, 10), 4);
assert.equal(namedActionPage("FirstPage", 5, 10), 1);
assert.equal(namedActionPage("LastPage", 5, 10), 10);
assert.equal(namedActionPage("Print", 5, 10), null);
assert.equal(namedActionPage("NextPage", 1, 0), null);

// Corners come back from pdf.js in either order.
const box = boxPercent([100, 600, 50, 500], 500, 1000);
assert.deepEqual(box, { left: 10, top: 50, width: 10, height: 10 });
assert.equal(boxPercent([0, 0, 1, 1], 0, 10), null);

const doc = {
  numPages: 3,
  getDestination: async (name) => (name === "intro" ? [{ num: 7, gen: 0 }, { name: "XYZ" }, 0, 700, null] : null),
  getPageIndex: async (ref) => {
    if (ref.num === 7) return 1;
    throw new Error("unknown ref");
  },
  getPage: async () => ({ view }),
};
assert.deepEqual(await resolveDest(doc, "intro"), { page: 2, y: 92 });
assert.deepEqual(await resolveDest(doc, [2, { name: "Fit" }]), { page: 3, y: null });
assert.deepEqual(await resolveDest(doc, [{ num: 7, gen: 0 }, { name: "FitH" }, 400]), { page: 2, y: 392 });
assert.equal(await resolveDest(doc, "missing"), null);
assert.equal(await resolveDest(doc, [{ num: 9, gen: 0 }, { name: "Fit" }]), null, "an unknown ref does not throw");
assert.equal(await resolveDest(doc, [5, { name: "Fit" }]), null, "out of range");
assert.equal(await resolveDest(doc, null), null);
