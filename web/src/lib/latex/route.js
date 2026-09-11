// The routing table, as a pure state machine.
//
// SPEC "Product behavior/Routing" and "Complete native fallback" write the
// fallback order out as a table of situations and required behavior. This
// file is that table, minus everything that actually runs a backend: `decide`
// takes one event and the routing state and returns one action and the next
// state, so it can be exercised in `latex-route.mjs` against every row and
// every retry limit with no worker, no fetch and no real Biber anywhere in
// reach. `latex.js` is the only caller and the only place an action turns
// into a job.
//
// Two counters carry the spec's "at most once" rules across the whole
// session: `attempts.native` is keyed by snapshot ("full native is attempted
// at most once automatically for that snapshot"), `attempts.vm` is keyed by
// bibliography identity ("attempted at most once automatically for a
// bibliography input identity"). Both survive across compiles of the same
// project until a new snapshot or identity resets the count back to zero by
// simply not having an entry yet -- there is nothing to "reset", a snapshot
// that has not been tried has no key.

/// A fresh routing state for a session (a project, for one page load).
/// `localStatus` mirrors `LocalStatus.state` from `latex/local.js`;
/// `vmSupported` mirrors `latex/vm.js`'s `supported().ok`. Both may be
/// `"unknown"`/`false` before either module has answered -- the first
/// `biber-needed` decision is allowed to be optimistic about local (SPEC:
/// "check the local app" happens concurrently with compilation, not before
/// it) and is never optimistic about the VM, which is opt-in only once its
/// eligibility is actually known.
export function initialState({ snapshot = null, localStatus = "unknown", vmSupported = false } = {}) {
  return {
    snapshot,
    localStatus,
    vmSupported,
    route: "browser",
    attempts: { native: {}, vm: {} },
  };
}

function reachableOrUnknown(localStatus) {
  return localStatus === "unknown" || localStatus === "reachable" || localStatus === "connected";
}

// `event.vmSupported`, when present, overrides `state.vmSupported`. The
// controller only ever learns real VM eligibility (which requires importing
// `latex/vm.js`, the thing the SPEC says must not happen for an ordinary
// successful document) at the moment a decision might actually need it --
// so it is supplied per event rather than pre-loaded into the session state
// `initialState` starts everyone at (`false`, harmless: a state used with no
// per-event override simply never routes to the VM, which is exactly right
// for a check that never mentions Biber at all).
function vmEligible(state, event, validBcf) {
  return Boolean(validBcf) && Boolean(event.vmSupported ?? state.vmSupported);
}

function nativeAttempted(state, snapshot) {
  return Boolean(state.attempts.native[snapshot]);
}

function markNative(state, snapshot) {
  return {
    ...state,
    attempts: { ...state.attempts, native: { ...state.attempts.native, [snapshot]: (state.attempts.native[snapshot] || 0) + 1 } },
  };
}

function vmAttempted(state, identity) {
  return Boolean(state.attempts.vm[identity]);
}

function markVm(state, identity) {
  return {
    ...state,
    attempts: { ...state.attempts, vm: { ...state.attempts.vm, [identity]: (state.attempts.vm[identity] || 0) + 1 } },
  };
}

/// The one place the "may the VM take this?" policy is decided, so the four
/// call sites below cannot drift apart: eligible (a valid BCF and VM support,
/// per `vmEligible`) and not already attempted for this bibliography
/// identity. Returns the `try-vm` result, with `state` updated to record the
/// attempt, or `null` when the caller must fall through to whatever it does
/// when the VM cannot be used.
function vmStep(state, event) {
  if (vmEligible(state, event, event.validBcf) && !vmAttempted(state, event.identity)) {
    return { action: "try-vm", state: markVm(state, event.identity), failure: null };
  }
  return null;
}

const LOCAL_UNAVAILABLE = { kind: "local-unavailable", message: "Local LibrePaper is unavailable" };

/// `event` is `{ type, ...context }`. `state` is whatever the previous
/// `decide` returned (or `initialState` for the first call of a session).
/// Returns `{ action, state, failure }`: `action` is one of
/// `"try-local-biber" | "try-vm" | "try-native" | "stop" | "show-browser"`;
/// `failure` is `null | { kind, message }` in `Result.failure`'s vocabulary,
/// set whenever the action is terminal (`stop`/`show-browser`) and the
/// situation is not a clean success or a silent cancellation.
export function decide(event, state) {
  const snapshot = event.snapshot ?? state.snapshot;

  switch (event.type) {
    // "Biber is required" -> local first when it might be reachable at all;
    // a definite non-local state skips straight to the VM eligibility check
    // rather than paying for a probe already known to fail.
    case "biber-needed": {
      if (reachableOrUnknown(state.localStatus)) {
        return { action: "try-local-biber", state, failure: null };
      }
      const vm = vmStep(state, event);
      if (vm) return vm;
      return { action: "show-browser", state, failure: LOCAL_UNAVAILABLE };
    }

    // The local app turned out to be unreachable or the connection was
    // denied, discovered only after `try-local-biber` actually tried it (the
    // optimistic "unknown" case above resolves here). The VM can only stand
    // in when browser TeX already succeeded and bibliography is the only
    // remaining work -- it never substitutes for a failed browser TeX pass.
    case "local-unreachable":
    case "local-denied": {
      if (event.onlyBibliography) {
        const vm = vmStep(state, event);
        if (vm) return vm;
      }
      return { action: "stop", state, failure: LOCAL_UNAVAILABLE };
    }

    // The app answered but the required tool is missing on that machine.
    case "local-tool-missing": {
      if (event.onlyBibliography) {
        const vm = vmStep(state, event);
        if (vm) return vm;
      }
      return {
        action: "stop",
        state,
        failure: { kind: "tool-missing", message: event.message || "The required tool was not found on this machine" },
      };
    }

    // Local Biber exists but cannot read the browser release's control-file
    // version. A complete native build, using its own matching bibliography
    // tool, is preferred over the VM here because it also re-typesets with
    // packages known to match -- the VM only ever returns a BBL for browser TeX
    // to consume, which is exactly the byte stream the incompatible local
    // Biber could not produce.
    case "local-incompatible": {
      if (event.localTexAvailable && !nativeAttempted(state, snapshot)) {
        return { action: "try-native", state: markNative(state, snapshot), failure: null };
      }
      const vm = vmStep(state, event);
      if (vm) return vm;
      return {
        action: "stop",
        state,
        failure: { kind: "incompatible", message: event.message || "Local Biber is incompatible with this release" },
      };
    }

    // An actual Biber run failed on real input -- a citation error, a
    // malformed database. SPEC: "a real Biber input error is a diagnostic,
    // not a reason to run identical invalid input through every backend."
    // No native or VM retry: the same bad input would just fail there too.
    case "local-biber-failed": {
      return { action: "stop", state, failure: { kind: "bibliography", message: event.message || "Biber failed" } };
    }

    // Browser initialization, resource loading, compilation or timeout
    // failure. One automatic native attempt per snapshot when local is
    // usable at all (SPEC "Complete native fallback"); a second browser
    // failure of the same snapshot (e.g. the native attempt itself came back
    // and the caller re-enters routing) must not repeat it.
    case "browser-failed": {
      if (event.localUsable && !nativeAttempted(state, snapshot)) {
        return { action: "try-native", state: markNative(state, snapshot), failure: null };
      }
      return {
        action: "stop",
        state,
        failure: { kind: event.kind || "tex", message: event.message || "Compilation failed" },
      };
    }

    // "A native failure does not bounce back into an automatic browser retry
    // loop." Always terminal.
    case "native-failed": {
      return { action: "stop", state, failure: { kind: "native", message: event.message || "The local build failed" } };
    }

    // "After a successful complete native fallback, keep that project on the
    // native route for the current editing session."
    case "native-ok": {
      return { action: "stop", state: { ...state, route: "native" }, failure: null };
    }

    case "vm-unavailable":
    case "vm-failed": {
      return {
        action: "stop",
        state,
        failure: { kind: "vm", message: event.message || "Browser bibliography support is unavailable" },
      };
    }

    // "Cancellation is not a failure that triggers fallback." No attempt
    // budget is spent and no failure is reported.
    case "canceled": {
      return { action: "stop", state, failure: null };
    }

    // "Provide 'Try browser compilation' to reset the route." A settings
    // change or a new session resets the route the same way, by starting a
    // fresh `initialState()` rather than sending this event.
    case "try-browser": {
      return { action: "stop", state: { ...state, route: "browser" }, failure: null };
    }

    default:
      return { action: "stop", state, failure: { kind: "init", message: `unknown routing event ${event.type}` } };
  }
}
