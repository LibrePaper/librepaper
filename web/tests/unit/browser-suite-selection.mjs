import assert from "node:assert/strict";
import { browserSuiteArguments } from "../../tools/browser-suite-selection.mjs";

assert.deepEqual(browserSuiteArguments("web/tests/browser/latex-browser.mjs"), ["chromium"],
  "the browser suite selects Chromium for the multi-browser LaTeX fixture");
assert.deepEqual(browserSuiteArguments("web/tests/browser/viewer.mjs"), [],
  "ordinary fixtures need no browser-selection argument");

console.log("browser suite selection: Chromium-only dispatch checked");
