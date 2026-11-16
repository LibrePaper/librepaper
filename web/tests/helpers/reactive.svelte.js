// Reactive plain data for checks of `.svelte.js` modules.
//
// A module's derived values follow the signals they read. A check that hands
// the module `() => world.editing` over a plain object gives it nothing to
// follow, so a later `world.editing = true` is never seen. Load this with
// `loadRunes` and make the object with `reactive`, and it is state the way the
// page's own would be.

/** @template {object} T @param {T} value @returns {T} */
export function reactive(value) {
  const state = $state(value);
  return state;
}
