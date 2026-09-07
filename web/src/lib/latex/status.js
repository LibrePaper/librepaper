// The compile status store.
//
// `latex.js` is the only writer; the reader's badge and status line are the
// only readers, through `subscribe`. Keeping the store in its own tiny module
// rather than a handful of `let`s in `latex.js` means a check can import it
// on its own and assert on transitions without dragging in the worker, the
// manifest fetch or either backend -- and it means `latex.js` never has to
// remember to notify every place status can change from, because `set` does
// that once, here.

const listeners = new Set();

/// The fields every consumer expects to exist from the first tick, before
/// `configure` has run: an idle phase, no backend, the session route the
/// spec calls "browser" until a native fallback earns it otherwise, and a
/// `local` shape that mirrors `LocalStatus` (section 2.6) even though
/// `latex/local.js` has not been asked anything yet.
let state = {
  phase: "idle",
  message: "",
  backend: null,
  progress: null,
  route: "browser",
  release: null,
  engine: null,
  local: {
    state: "unknown",
    address: "",
    protocol: null,
    version: null,
    capabilities: null,
    checkedAt: null,
    error: null,
    instructions: "",
  },
  lastResult: null,
};

export function get() {
  return state;
}

/// Merges `partial` into the store and notifies every subscriber with the
/// new whole. A shallow merge, on purpose: `latex.js` always sets `local` and
/// `progress` as whole replacement objects rather than patching into them,
/// so a stale nested value from two calls ago can never survive a merge.
export function set(partial) {
  state = { ...state, ...partial };
  for (const listener of listeners) {
    try {
      listener(state);
    } catch {
      // A subscriber's own bug must not stop the rest from hearing the
      // update, or corrupt the compile that produced it.
    }
  }
  return state;
}

export function subscribe(listener) {
  listeners.add(listener);
  listener(state);
  return () => listeners.delete(listener);
}
