// Behavioural checks for the reader's LaTeX status text -- the pure mapping
// from a `Status`/`Provenance`/`Attempt[]` to what a reader sees, kept in
// web/src/lib/latex/status-text.js so it can be checked without a Svelte
// runtime and without `latex.js` itself.
import assert from "node:assert/strict";
import {
  actionsFor,
  backendChip,
  fallbackExplanation,
  failureHint,
  provenanceSentence,
} from "../../src/lib/latex/status-text.js";

const idle = { phase: "idle", backend: null, route: "browser", local: {} };

// backendChip

assert.equal(backendChip(null), "");
assert.equal(backendChip(idle), "");
assert.equal(backendChip({ ...idle, phase: "compiling", backend: "browser" }), "browser");
assert.equal(backendChip({ ...idle, phase: "compiling", backend: "local" }), "local");
assert.equal(backendChip({ ...idle, phase: "browser-biber", backend: "browser" }), "browser Biber");

// actionsFor: local connection states

assert.deepEqual(actionsFor(null), []);
assert.deepEqual(actionsFor({ ...idle, phase: "local-needed" }), ["connect", "retry"]);
assert.deepEqual(actionsFor({ ...idle, local: { state: "unreachable" } }), ["connect", "retry"]);
assert.deepEqual(actionsFor({ ...idle, local: { state: "denied" } }), ["connect", "retry"]);
assert.deepEqual(actionsFor({ ...idle, local: { state: "unauthorized" } }), ["open-app"]);
assert.deepEqual(actionsFor({ ...idle, local: { state: "incompatible" } }), ["open-app"]);
assert.deepEqual(actionsFor({ ...idle, local: { state: "connected" } }), []);

// actionsFor: failure kinds, only once the job has actually failed

assert.deepEqual(
  actionsFor({ ...idle, phase: "compiling", lastResult: { failure: { kind: "local-unavailable" } } }),
  [],
  "a failure on an earlier result must not offer actions while a newer job is running",
);
assert.deepEqual(
  actionsFor({ ...idle, phase: "failed", lastResult: { failure: { kind: "local-unavailable" } } }),
  ["connect", "retry"],
);
assert.deepEqual(
  actionsFor({ ...idle, phase: "failed", lastResult: { failure: { kind: "tool-missing" } } }),
  ["doctor"],
);
assert.deepEqual(
  actionsFor({ ...idle, phase: "failed", lastResult: { failure: { kind: "incompatible" } } }),
  ["connect"],
);
assert.deepEqual(
  actionsFor({ ...idle, phase: "failed", lastResult: { failure: { kind: "native" } } }),
  ["diagnostics"],
);

// actionsFor: the session-native route always offers a way back, and
// actions never repeat when more than one condition applies at once.

assert.deepEqual(actionsFor({ ...idle, route: "native" }), ["try-browser"]);
assert.deepEqual(
  actionsFor({ ...idle, phase: "failed", route: "native", local: { state: "unreachable" },
    lastResult: { failure: { kind: "local-unavailable" } } }),
  ["connect", "retry", "try-browser"],
);

// failureHint: one line per kind, never invented for a result with none.

assert.equal(failureHint(null), "");
assert.equal(
  failureHint({ kind: "local-unavailable" }),
  "Local LibrePaper is unavailable. Connect it or retry the connection.",
);
assert.match(failureHint({ kind: "tool-missing", message: "biber was not found" }), /biber was not found/);
assert.match(failureHint({ kind: "tool-missing", message: "biber was not found" }), /librepaper local doctor/);
assert.match(failureHint({ kind: "incompatible", message: "biblatex 3.21 needs Biber 2.21" }), /biblatex 3\.21/);
assert.equal(failureHint({ kind: "native" }), "The local build failed too; see Diagnostics.");
assert.equal(failureHint({ kind: "tex", message: "Undefined control sequence" }), "Undefined control sequence");

// fallbackExplanation: silent for one clean attempt, one line for a fallback,
// and never invented from a reason-less attempt (a canceled/superseded one).

assert.equal(fallbackExplanation([]), "");
assert.equal(fallbackExplanation([{ backend: "browser", ok: true }]), "");
assert.equal(
  fallbackExplanation([{ backend: "browser", ok: false, reason: undefined }, { backend: "local", ok: true }]),
  "",
);
assert.equal(
  fallbackExplanation([
    { backend: "browser", ok: false, reason: "Biber is required" },
    { backend: "local", ok: true },
  ]),
  "Used local because Biber is required.",
);
assert.equal(
  fallbackExplanation([
    { backend: "browser", ok: false, reason: "the format failed to load" },
    { backend: "local", ok: true },
  ]),
  "Used local because the format failed to load.",
);

// provenanceSentence

assert.equal(provenanceSentence(null), "");
assert.equal(
  provenanceSentence({ backend: "browser", engine: "pdflatex", release: "2026-abc", bibliography: null, tools: {} }),
  "Compiled in the browser with pdfLaTeX (release 2026-abc).",
);
assert.equal(
  provenanceSentence({
    backend: "local", engine: "pdflatex", release: null, bibliography: "local-biber",
    tools: { tex: "pdfTeX 1.40.27 (TeX Live 2025)", biber: "2.21" },
  }),
  "Compiled locally with pdfLaTeX, pdfTeX 1.40.27 (TeX Live 2025); bibliography via local Biber.",
);
assert.equal(
  provenanceSentence({ backend: "browser", engine: "xelatex", release: "2026-abc", bibliography: "browser-biber", tools: {} }),
  "Compiled in the browser with XeLaTeX (release 2026-abc); bibliography via browser Biber.",
);

console.log("latex-reader: all checks passed");
