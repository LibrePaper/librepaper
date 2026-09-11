// Walks every row of SPEC "Product behavior/Routing" and the per-snapshot/
// per-identity retry limits from "Complete native fallback" against the pure
// state machine in `latex/route.js`. No worker, no fetch, no real backend:
// `decide` is a function of an event and a state, so every row is one call.
import assert from "node:assert/strict";
import * as route from "../src/lib/latex/route.js";

function fresh(overrides = {}) {
  return route.initialState({ snapshot: "s1", localStatus: "unknown", ...overrides });
}

// Biber needed, local status unknown -> try local first (optimistic: SPEC
// "reuse a healthy authenticated connection if one exists ... otherwise
// probe", which is a concurrent local.js concern this event fires ahead of).
{
  const state = fresh();
  const { action, failure } = route.decide({ type: "biber-needed", identity: "id1", validBcf: true }, state);
  assert.equal(action, "try-local-biber");
  assert.equal(failure, null);
}

// Biber needed, local known reachable -> still local first.
{
  const state = fresh({ localStatus: "reachable" });
  const { action } = route.decide({ type: "biber-needed", identity: "id1", validBcf: true }, state);
  assert.equal(action, "try-local-biber");
}

// Biber needed, local known unreachable -> show the browser result.
{
  const state = fresh({ localStatus: "unreachable" });
  const { action, state: next } = route.decide({ type: "biber-needed", identity: "id1", validBcf: true }, state);
  assert.equal(action, "show-browser");
  assert.deepEqual(next, state);
}

// Biber needed, local unavailable -> show the browser output with
// "Local LibrePaper is unavailable".
{
  const state = fresh({ localStatus: "unreachable" });
  const { action, failure } = route.decide({ type: "biber-needed", identity: "id1", validBcf: true }, state);
  assert.equal(action, "show-browser");
  assert.equal(failure.kind, "local-unavailable");
}
{
  const state = fresh({ localStatus: "denied" });
  const { action, failure } = route.decide({ type: "biber-needed", identity: "id1", validBcf: false }, state);
  assert.equal(action, "show-browser");
  assert.equal(failure.kind, "local-unavailable");
}

// try-local-biber actually turned out unreachable/denied: preserve the
// browser result and report the companion failure.
{
  const state = fresh();
  const unreachable = route.decide({ type: "local-unreachable", identity: "id1", validBcf: true, onlyBibliography: true }, state);
  assert.equal(unreachable.action, "show-browser");
  assert.equal(unreachable.failure.kind, "local-unavailable");
  const denied = route.decide({ type: "local-denied", identity: "id2", validBcf: true, onlyBibliography: true }, state);
  assert.equal(denied.action, "show-browser");
  assert.equal(denied.failure.kind, "local-unavailable");
}
{
  const state = fresh();
  const { action, failure } = route.decide({ type: "local-unreachable", identity: "id1", validBcf: true, onlyBibliography: false }, state);
  assert.equal(action, "stop");
  assert.equal(failure.kind, "local-unavailable");
}

// Local reachable but the tool itself is missing.
{
  const state = fresh();
  const { action } = route.decide(
    { type: "local-tool-missing", identity: "id1", validBcf: true, onlyBibliography: true },
    state,
  );
  assert.equal(action, "show-browser");
}
{
  const state = fresh();
  const { action, failure } = route.decide(
    { type: "local-tool-missing", identity: "id1", validBcf: true, onlyBibliography: false },
    state,
  );
  assert.equal(action, "stop");
  assert.equal(failure.kind, "tool-missing");
}

// Local Biber incompatible with the browser release's control-file version:
// prefer a complete native build when local TeX exists.
{
  const state = fresh();
  const { action, state: next } = route.decide(
    { type: "local-incompatible", identity: "id1", validBcf: true, localTexAvailable: true },
    state,
  );
  assert.equal(action, "try-native");
  assert.equal(next.attempts.native.s1, 1);
}
{
  const state = fresh();
  const { action } = route.decide(
    { type: "local-incompatible", identity: "id1", validBcf: true, localTexAvailable: false },
    state,
  );
  assert.equal(action, "stop");
}
{
  const state = fresh();
  const { action, failure } = route.decide(
    { type: "local-incompatible", identity: "id1", validBcf: true, localTexAvailable: false },
    state,
  );
  assert.equal(action, "stop");
  assert.equal(failure.kind, "incompatible");
}

// A real Biber input error never cascades into another backend.
{
  const state = fresh();
  const { action, failure } = route.decide({ type: "local-biber-failed", message: "duplicate key" }, state);
  assert.equal(action, "stop");
  assert.equal(failure.kind, "bibliography");
  assert.match(failure.message, /duplicate key/);
}

// Browser TeX/init/resources/timeout failure -> native once when local is
// usable, then never again for the same snapshot.
for (const kind of ["init", "resources", "tex", "timeout"]) {
  let state = fresh();
  const first = route.decide({ type: "browser-failed", kind, localUsable: true }, state);
  assert.equal(first.action, "try-native", kind);
  state = first.state;
  const second = route.decide({ type: "browser-failed", kind, localUsable: true }, state);
  assert.equal(second.action, "stop", `${kind}: no second automatic native attempt`);
  assert.equal(second.failure.kind, kind);
}
{
  const state = fresh();
  const { action, failure } = route.decide({ type: "browser-failed", kind: "tex", localUsable: false }, state);
  assert.equal(action, "stop");
  assert.equal(failure.kind, "tex");
}

// A native failure never bounces back into a browser retry.
{
  const state = fresh();
  const { action, failure } = route.decide({ type: "native-failed", message: "pdflatex exited 1" }, state);
  assert.equal(action, "stop");
  assert.equal(failure.kind, "native");
}

// A native success switches the session route and stays there until
// tryBrowser() (represented here as the "try-browser" event).
{
  let state = fresh();
  const ok = route.decide({ type: "native-ok" }, state);
  assert.equal(ok.action, "stop");
  assert.equal(ok.state.route, "native");
  state = ok.state;
  const reset = route.decide({ type: "try-browser" }, state);
  assert.equal(reset.state.route, "browser");
}

// Cancellation is silent and spends no attempt budget.
{
  const state = fresh();
  const { action, failure, state: next } = route.decide({ type: "canceled" }, state);
  assert.equal(action, "stop");
  assert.equal(failure, null);
  assert.deepEqual(next.attempts, state.attempts);
}

// Bibliography input failure is terminal and spends no native retry budget.
{
  const state = fresh();
  const failure = route.decide({ type: "local-biber-failed", message: "duplicate key" }, state);
  assert.equal(failure.action, "stop");
  assert.equal(failure.failure.kind, "bibliography");
  assert.deepEqual(failure.state.attempts, state.attempts);
}

// Native attempted at most once per snapshot even across different failure
// kinds within the same snapshot.
{
  let state = fresh({ snapshot: "s2" });
  const first = route.decide({ type: "browser-failed", kind: "init", localUsable: true, snapshot: "s2" }, state);
  assert.equal(first.action, "try-native");
  state = first.state;
  const second = route.decide({ type: "browser-failed", kind: "timeout", localUsable: true, snapshot: "s2" }, state);
  assert.equal(second.action, "stop");
  // A different snapshot gets its own budget.
  const third = route.decide({ type: "browser-failed", kind: "init", localUsable: true, snapshot: "s3" }, state);
  assert.equal(third.action, "try-native");
}

console.log("latex route: every SPEC routing row and retry limit passed");
