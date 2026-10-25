// The stage line of the preview's waiting display, for each thing the
// preview can be doing.
import assert from "node:assert/strict";
import { waitStageOf } from "../../src/lib/reader/wait-stage.js";

assert.equal(waitStageOf({}), "Preparing the preview");
assert.equal(waitStageOf({ latexPhase: "loading" }), "Loading the LaTeX engine");
assert.equal(waitStageOf({ latexPhase: "loading", latexScope: "engine and format" }), "Loading the LaTeX engine");
assert.equal(waitStageOf({ latexPhase: "loading", latexScope: "resources" }), "Fetching LaTeX packages");
assert.equal(waitStageOf({ latexPhase: "compiling", latexScope: "resources" }), "Fetching LaTeX packages");
assert.equal(waitStageOf({ latexPhase: "compiling" }), "Compiling");
assert.equal(waitStageOf({ latexPhase: "browser-biber" }), "Updating the bibliography");
assert.equal(waitStageOf({ latexPhase: "compiling", latexScope: "biber" }), "Updating the bibliography");
assert.equal(waitStageOf({ compiling: true }), "Compiling");
assert.equal(waitStageOf({ pageDelivered: true, compiling: true, latexPhase: "compiling" }), "Drawing the pages");
assert.equal(waitStageOf({ latexPhase: "idle" }), "Preparing the preview");

console.log("wait-stage: one stage line for every format");
assert.equal(waitStageOf({ engineLoading: "typst", compiling: true }), "Loading the Typst engine");
assert.equal(waitStageOf({ engineLoading: "quarto" }), "Loading the Markdown engine");
