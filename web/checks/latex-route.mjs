// Walks every row of SPEC "Product behavior/Routing" and the per-snapshot/
// per-identity retry limits from "Complete native fallback" against the pure
// state machine in `latex/route.js`. No worker, no fetch, no real backend:
// `decide` is a function of an event and a state, so every row is one call.
import assert from "node:assert/strict";
import * as route from "../src/lib/latex/route.js";

function fresh(overrides = {}) {
  return route.initialState({ snapshot: "s1", localStatus: "unknown", vmSupported: true, ...overrides });
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

// Biber needed, local known unreachable, VM eligible -> VM.
{
  const state = fresh({ localStatus: "unreachable" });
  const { action, state: next } = route.decide({ type: "biber-needed", identity: "id1", validBcf: true }, state);
  assert.equal(action, "try-vm");
  assert.equal(next.attempts.vm.id1, 1);
}

// Biber needed, local unreachable, VM not eligible (unsupported browser or
// no valid bcf yet) -> show the browser output with "Local Komodoc is
// unavailable".
{
  const state = fresh({ localStatus: "unreachable", vmSupported: false });
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

// try-local-biber actually turned out unreachable/denied: VM only when
// browser TeX already succeeded (only bibliography work remains).
{
  const state = fresh();
  const unreachable = route.decide({ type: "local-unreachable", identity: "id1", validBcf: true, onlyBibliography: true }, state);
  assert.equal(unreachable.action, "try-vm");
  const denied = route.decide({ type: "local-denied", identity: "id2", validBcf: true, onlyBibliography: true }, state);
  assert.equal(denied.action, "try-vm");
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
  assert.equal(action, "try-vm");
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
// prefer a complete native build over the VM when local TeX exists.
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
  assert.equal(action, "try-vm");
}
{
  const state = fresh({ vmSupported: false });
  const { action, failure } = route.decide(
    { type: "local-incompatible", identity: "id1", validBcf: true, localTexAvailable: false },
    state,
  );
  assert.equal(action, "stop");
  assert.equal(failure.kind, "incompatible");
}

// A real Biber input error never cascades into native or VM.
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

// The VM cannot load, cannot run, or returns a real diagnostic: stop, no
// further automatic cycling through the same failed backends.
{
  const state = fresh();
  const unavailable = route.decide({ type: "vm-unavailable", message: "WebAssembly unsupported" }, state);
  assert.equal(unavailable.action, "stop");
  assert.equal(unavailable.failure.kind, "vm");
  const failed = route.decide({ type: "vm-failed", message: "guest crashed" }, state);
  assert.equal(failed.action, "stop");
  assert.equal(failed.failure.kind, "vm");
}

// Cancellation is silent and spends no attempt budget.
{
  const state = fresh();
  const { action, failure, state: next } = route.decide({ type: "canceled" }, state);
  assert.equal(action, "stop");
  assert.equal(failure, null);
  assert.deepEqual(next.attempts, state.attempts);
}

// VM attempted at most once per bibliography identity, independent of
// snapshot: a second `biber-needed` for the same identity from local-unknown
// still tries local first (identity budget only matters once local has
// already been ruled out for this decision).
{
  let state = fresh({ localStatus: "unreachable" });
  const first = route.decide({ type: "biber-needed", identity: "shared", validBcf: true }, state);
  assert.equal(first.action, "try-vm");
  state = first.state;
  const second = route.decide({ type: "local-unreachable", identity: "shared", validBcf: true, onlyBibliography: true }, state);
  assert.equal(second.action, "stop", "the VM identity budget for 'shared' is already spent");
  assert.equal(second.failure.kind, "local-unavailable");
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
