import assert from "node:assert/strict";
import { createRenderDiagnostics } from "../../src/lib/reader/render-diagnostics.js";

let editing = true;
let local = [];
let visible = [];
let delivered = [];
const diagnostics = createRenderDiagnostics({
  active: () => editing,
  local: () => local,
  update: (list) => (visible = list),
  deliver: (list) => (delivered = list),
  painterOptions: { delay: 0, setTimer: (paint) => { paint(); return 1; }, clearTimer: () => {} },
});

diagnostics.painter.rendered({
  page: null,
  diagnostics: [{ severity: "error", message: "render failed", file: "main.qmd", line: 2 }],
});
assert.deepEqual(visible.map((item) => item.message), ["render failed"]);

diagnostics.bibliography({
  diagnostics: [
    { severity: "warning", message: "missing citation", file: "main.qmd", line: 4 },
    { severity: "error", message: "render failed", file: "main.qmd", line: 2 },
  ],
}, { main: "main.qmd", texts: { "main.qmd": "text" } });
assert.deepEqual(visible.map((item) => item.message), ["render failed", "missing citation"]);

local = [{ severity: "error", message: "local tool unavailable", source: "local-app" }];
diagnostics.refresh();
assert.deepEqual(visible.map((item) => item.message), ["render failed", "missing citation", "local tool unavailable"]);
assert.deepEqual(delivered, visible);

editing = false;
local = [{ severity: "error", message: "hidden" }];
diagnostics.refresh();
assert.deepEqual(visible.map((item) => item.message), ["render failed", "missing citation", "local tool unavailable"]);

// --- failure option tests ---------------------------------------------------

// Reset state
editing = true;
visible = [];
delivered = [];

let failureMessage = "";
const diagnostics2 = createRenderDiagnostics({
  active: () => editing,
  local: () => [],
  update: (list) => (visible = list),
  deliver: (list) => (delivered = list),
  painterOptions: { delay: 0, setTimer: (paint) => { paint(); return 1; }, clearTimer: () => {} },
  failure: () => failureMessage,
});

// Failure appended when no error present
failureMessage = "unknown build error";
diagnostics2.render([]);
assert.deepEqual(visible.map((item) => item.message), ["unknown build error"], "failure appended when no error");
assert.equal(visible[0].severity, "error", "failure has error severity");
assert.equal(visible[0].source, "preview", "failure has preview source");

// Failure not appended when error already present
visible = [];
failureMessage = "unknown build error";
diagnostics2.render([{ severity: "error", message: "compile error", file: "main.tex" }]);
assert.deepEqual(visible.map((item) => item.message), ["compile error"], "failure not appended when error exists");

// Failure not appended when empty
visible = [];
failureMessage = "";
diagnostics2.render([]);
assert.deepEqual(visible.length, 0, "no failure appended when failure returns empty");

console.log("render diagnostics tests passed");
