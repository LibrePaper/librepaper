import assert from "node:assert/strict";
import test from "node:test";
import { BUILDERS, buildersFor, capabilityFor, operationsFor, supportsOperation } from "../../src/lib/build-catalog.js";

const caps = {
  builders: [
    { id: "typst", available: true, outputs: ["pdf"], operations: [{ kind: "build", workspace_modes: ["snapshot"] }] },
    { id: "quarto", available: true, outputs: ["html"], operations: [{ kind: "preview", workspace_modes: ["bound"] }] },
  ],
};

test("a builder advertises only the operations it reports", () => {
  assert.deepEqual(operationsFor(caps, "quarto"), [{ kind: "preview", workspace_modes: ["bound"] }]);
  assert.equal(supportsOperation(caps, "quarto", "build", "snapshot"), false);
  assert.equal(supportsOperation(caps, "quarto", "preview", "bound"), true);
  assert.equal(supportsOperation(caps, "typst", "build", "snapshot"), true);
});

test("an unreported builder has no capability and no operations", () => {
  assert.equal(capabilityFor(caps, "pandoc"), null);
  assert.deepEqual(operationsFor(caps, "pandoc"), []);
  assert.equal(supportsOperation(caps, "pandoc", "build", "snapshot"), false);
  // A companion below the protocol floor never reaches "connected", so its
  // tool-specific fields must not be read as builder capabilities.
  assert.equal(capabilityFor({ quarto: { tool: { available: true } } }, "quarto"), null);
});

test("LaTeX is a browser format and offers no local builder", () => {
  const tex = BUILDERS.find((entry) => entry.id === "tex");
  assert.deepEqual(buildersFor("latex").map((entry) => entry.id), ["tex"]);
  assert.deepEqual(tex.backend, ["browser"]);
  assert.deepEqual(tex.engines, ["pdflatex", "xelatex"]);
  // The companion advertises TeX adapters for the automatic Biber and native
  // fallback that `latex.js` routes itself. They are deliberately not menu
  // choices, so the catalog must not name them.
  assert.equal(BUILDERS.some((entry) => ["latexmk", "tectonic"].includes(entry.id)), false);
  assert.equal(BUILDERS.some((entry) => entry.formats.includes("latex") && entry.backend.includes("local")), false);
});

test("the formats with a local builder offer one", () => {
  assert.deepEqual(buildersFor("typst").map((entry) => entry.id), ["typst", "calepin"]);
  assert.deepEqual(buildersFor("markdown").map((entry) => entry.id), ["markdown", "pandoc", "quarto"]);
  assert.deepEqual(buildersFor("quarto").map((entry) => entry.id), ["markdown", "quarto"]);
});
