import assert from "node:assert/strict";
import { test } from "node:test";
import { packageNames, nearestPackage } from "../../src/lib/latex/near-miss.js";

const enc = new TextEncoder();

test("packageNames lists sorted unique .sty and .cls basenames", () => {
  const index = {
    schemaVersion: 1,
    files: {
      "tex/latex/graphics/graphicx.sty": "bundle-a",
      "tex/latex/base/article.cls": "bundle-b",
      "tex/latex/other/graphicx.sty": "bundle-a",
      "tex/generic/misc/readme.txt": "bundle-c",
      "tex/latex/fonts/cmr10.tfm": "bundle-d",
      "tex/latex/base/size10.clo": "bundle-b",
    },
  };
  assert.deepEqual(packageNames(enc.encode(JSON.stringify(index))), ["article.cls", "graphicx.sty"]);
});

test("packageNames of an index with no files is empty", () => {
  assert.deepEqual(packageNames(enc.encode("{}")), []);
});

test("a missing letter gives the nearest package", () => {
  assert.equal(nearestPackage("graphix.sty", ["graphicx.sty", "hyperref.sty"]), "graphicx.sty");
});

test("a transposed pair of letters counts as one edit", () => {
  assert.equal(nearestPackage("xcolro.sty", ["xcolor.sty"]), "xcolor.sty");
});

test("a transposition in a longer name is suggested", () => {
  assert.equal(nearestPackage("hyperfer.sty", ["hyperref.sty", "graphicx.sty"]), "hyperref.sty");
});

test("an extension mismatch is not suggested", () => {
  assert.equal(nearestPackage("article.sty", ["article.cls"]), null);
});

test("a name with no extension is treated as .sty", () => {
  assert.equal(nearestPackage("graphix", ["graphicx.cls", "graphicx.sty"]), "graphicx.sty");
  assert.equal(nearestPackage("graphix", ["graphicx.cls"]), null);
});

test("a name too far away gives null", () => {
  assert.equal(nearestPackage("zzqqxy.sty", ["graphicx.sty", "hyperref.sty"]), null);
});

test("a stem shorter than eight characters allows only one edit", () => {
  assert.equal(nearestPackage("tabl.sty", ["tabu.sty"]), "tabu.sty");
  assert.equal(nearestPackage("abc.sty", ["abxy.sty"]), null);
  assert.equal(nearestPackage("mystyle.sty", ["sistyle.sty"]), null);
});

test("an exact match gives null", () => {
  assert.equal(nearestPackage("graphicx.sty", ["graphicx.sty", "graphics.sty"]), null);
});

test("the match is case-insensitive", () => {
  assert.equal(nearestPackage("GraphiCX.sty", ["graphicx.sty"]), "graphicx.sty");
  assert.equal(nearestPackage("Graphicx.STY", ["graphicx.sty"]), "graphicx.sty");
});

test("ties go to the candidate that sorts first", () => {
  assert.equal(nearestPackage("abcdefg.sty", ["abcdefi.sty", "abcdefh.sty"]), "abcdefh.sty");
});
