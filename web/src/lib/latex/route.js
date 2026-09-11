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
// The native counter carries the spec's "at most once" rule across the whole
// session and is keyed by snapshot. A snapshot that has not been tried has no
// entry yet, so no explicit reset is needed.

/// A fresh routing state for a session (a project, for one page load).
/// `localStatus` mirrors `LocalStatus.state` from `latex/local.js`;
export function initialState({ snapshot = null, localStatus = "unknown" } = {}) {
  return {
    snapshot,
    localStatus,
    route: "browser",
    attempts: { native: {} },
  };
}

function reachableOrUnknown(localStatus) {
  return localStatus === "unknown" || localStatus === "reachable" || localStatus === "connected";
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

const LOCAL_UNAVAILABLE = { kind: "local-unavailable", message: "Local LibrePaper is unavailable" };

/// `event` is `{ type, ...context }`. `state` is whatever the previous
/// `decide` returned (or `initialState` for the first call of a session).
/// Returns `{ action, state, failure }`: `action` is one of
/// `"try-local-biber" | "try-native" | "stop" | "show-browser"`;
/// `failure` is `null | { kind, message }` in `Result.failure`'s vocabulary,
/// set whenever the action is terminal (`stop`/`show-browser`) and the
/// situation is not a clean success or a silent cancellation.
export function decide(event, state) {
  const snapshot = event.snapshot ?? state.snapshot;

  switch (event.type) {
    // "Biber is required" -> local first when it might be reachable at all;
    // a definite non-local state shows the browser result with a local hint.
    case "biber-needed": {
      if (reachableOrUnknown(state.localStatus)) {
        return { action: "try-local-biber", state, failure: null };
      }
      return { action: "show-browser", state, failure: LOCAL_UNAVAILABLE };
    }

    // The local app turned out to be unreachable or the connection was
    // denied, discovered only after `try-local-biber` actually tried it (the
    // optimistic "unknown" case above resolves here). The browser result is
    // retained when only bibliography work remains.
    case "local-unreachable":
    case "local-denied": {
      return { action: event.onlyBibliography ? "show-browser" : "stop", state, failure: LOCAL_UNAVAILABLE };
    }

    // The app answered but the required tool is missing on that machine.
    case "local-tool-missing": {
      return {
        action: event.onlyBibliography ? "show-browser" : "stop",
        state,
        failure: { kind: "tool-missing", message: event.message || "The required tool was not found on this machine" },
      };
    }

    // Local Biber exists but cannot read the browser release's control-file
    // version. A complete native build, using its own matching bibliography
    // tool, is preferred when local TeX exists.
    case "local-incompatible": {
      if (event.localTexAvailable && !nativeAttempted(state, snapshot)) {
        return { action: "try-native", state: markNative(state, snapshot), failure: null };
      }
      return {
        action: event.onlyBibliography ? "show-browser" : "stop",
        state,
        failure: { kind: "incompatible", message: event.message || "Local Biber is incompatible with this release" },
      };
    }

    // An actual Biber run failed on real input -- a citation error, a
    // malformed database. SPEC: "a real Biber input error is a diagnostic,
    // not a reason to run identical invalid input through every backend."
    // No native retry: the same bad input would just fail there too.
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
