// The Git panel of a web project: what the server knows
// about pushes. A web project cannot run git, so this panel runs nothing. It
// reads the history the way the History panel does and finds the newest
// version a push made.
//
// This module owns that state; the panel only reads it and draws it.
import { keyHeaders } from "../api.js";
import { loadPage } from "../history.js";

/// What the panel asks the server, so a check can stand in for it.
const server = {
  /** @param {string} slug @param {string} key @param {number | null} after */
  history: (slug, key, after) => loadPage(slug, keyHeaders(key), { after, named: true }),
};

// A push is looked for in this many pages of saved versions, newest first,
// before the panel says there has been none.
const PAGES = 5;

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
/// Who made a version, where the history can say. A machine save names
/// nobody, and neither does an identifier that is not a name.
const personOf = (/** @type {any} */ point) => (!point?.by || point.by === "system" || uuid.test(point.by) ? "" : String(point.by));

/** @typedef {{ by: string, at: string, label: string }} LastPush */
/** @typedef {{ loaded: boolean, push: LastPush | null, pushProblem: string }} WebGitState */

/** @param {{ api?: typeof server }} [options] */
export function createWebGit({ api = server } = {}) {
  /** @type {WebGitState} */
  let current = $state.raw({ loaded: false, push: null, pushProblem: "" });
  let generation = 0;
  let closed = false;

  const set = (/** @type {Partial<WebGitState>} */ patch) => {
    if (!closed) current = { ...current, ...patch };
  };

  /// The newest version a push made, or null.
  async function lastPush(/** @type {string} */ slug, /** @type {string} */ key) {
    let after = null;
    for (let page = 0; page < PAGES; page += 1) {
      const answer = await api.history(slug, key, after);
      const found = answer.labels.find((point) => point.why === "push");
      if (found) return { by: personOf(found), at: String(found.at ?? ""), label: String(found.label ?? "") };
      if (answer.next === null) return null;
      after = answer.next;
    }
    return null;
  }

  /// Reads the last push. A read that arrives after a newer request was made
  /// is dropped.
  async function refresh(/** @type {string} */ slug, /** @type {{ key?: string }} */ { key = "" } = {}) {
    const request = ++generation;
    const [push] = await Promise.allSettled([lastPush(slug, key)]);
    if (request !== generation) return;
    set({
      loaded: true,
      push: push.status === "fulfilled" ? push.value : null,
      pushProblem: push.status === "rejected" ? (push.reason?.message || "The history could not be read.") : "",
    });
  }

  function dispose() {
    closed = true;
    generation += 1;
  }

  return {
    get state() { return current; },
    refresh,
    dispose,
  };
}
