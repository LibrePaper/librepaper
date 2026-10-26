// The Git panel of a web project (SPEC-git-panel.md): what the server knows
// about pushes, and the project tokens that let a GitHub Action push and pull.
//
// A web project cannot run git, so this panel runs nothing. It reads the
// history the way the History panel does, finds the newest version a push
// made, and, for the owner, lists the Edit tokens and makes one. The token is
// shown once, kept only here, and never written to any storage.
//
// This module owns that state; the panel only reads it and calls the actions.
import { getPrivate, keyHeaders, post } from "../api.js";
import { loadPage } from "../history.js";

/// What the panel asks the server, so a check can stand in for it.
const server = {
  /** @param {string} slug @param {string} key @param {number | null} after */
  history: (slug, key, after) => loadPage(slug, keyHeaders(key), { after, named: true }),
  /** @param {string} slug */
  sharing: (slug) => getPrivate(`/api/documents/${slug}/share`),
  /** @param {string} slug @param {unknown} body */
  change: (slug, body) => post(`/api/documents/${slug}/share`, body),
};

// A push is looked for in this many pages of saved versions, newest first,
// before the panel says there has been none.
const PAGES = 5;

/// The token the panel makes: one project, the Edit role, a year.
export const ACTION_TOKEN = { kind: "token", role: "edit", name: "GitHub Action", until: "365d" };

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
/// Who made a version, where the history can say. A machine save names
/// nobody, and neither does an identifier that is not a name.
const personOf = (/** @type {any} */ point) => (!point?.by || point.by === "system" || uuid.test(point.by) ? "" : String(point.by));
const isEdit = (/** @type {any} */ token) => token?.role === "editor" || token?.role === "edit";

/** @typedef {{ by: string, at: string, label: string }} LastPush */
/** @typedef {{ loaded: boolean, push: LastPush | null, pushProblem: string, tokens: any[], tokensProblem: string, busy: string, error: string, reveal: { token: string } | null }} WebGitState */

/** @param {{ api?: typeof server }} [options] */
export function createWebGit({ api = server } = {}) {
  /** @type {WebGitState} */
  let current = $state.raw({ loaded: false, push: null, pushProblem: "", tokens: [], tokensProblem: "", busy: "", error: "", reveal: null });
  let generation = 0;
  let closed = false;
  // The project the actions act on, as the last refresh named it.
  let target = { slug: "", key: "", owner: false };

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

  /// Reads the last push, and the Edit tokens when the reader owns the
  /// project. A read that arrives after a newer request was made is dropped.
  async function refresh(/** @type {string} */ slug, /** @type {{ key?: string, owner?: boolean }} */ { key = "", owner = false } = {}) {
    const request = ++generation;
    target = { slug, key, owner };
    const [push, sharing] = await Promise.allSettled([
      lastPush(slug, key),
      owner ? api.sharing(slug) : Promise.resolve(null),
    ]);
    if (request !== generation) return;
    set({
      loaded: true,
      push: push.status === "fulfilled" ? push.value : null,
      pushProblem: push.status === "rejected" ? (push.reason?.message || "The history could not be read.") : "",
      tokens: sharing.status === "fulfilled" && sharing.value ? (sharing.value.tokens ?? []).filter(isEdit) : [],
      tokensProblem: sharing.status === "rejected" ? (sharing.reason?.message || "The tokens could not be read.") : "",
    });
  }

  /// Runs one change to the project's tokens and keeps the answer's list.
  async function change(/** @type {string} */ name, /** @type {unknown} */ body) {
    if (current.busy || closed || !target.owner) return null;
    const { slug } = target;
    // A read still in flight was asked before this change, so its list may
    // predate it. It is dropped when it lands.
    generation += 1;
    set({ busy: name, error: "" });
    try {
      const answer = await api.change(slug, body);
      if (slug !== target.slug || closed) return null;
      // A read that started during the change may still land with a list the
      // server made before applying it. The answer's list is the current one.
      generation += 1;
      set({ tokens: (answer?.tokens ?? []).filter(isEdit) });
      return answer;
    } catch (error) {
      if (slug === target.slug) set({ error: error?.message || "The tokens could not be changed." });
      return null;
    } finally {
      set({ busy: "" });
    }
  }

  /// Makes the token for a repository and keeps it to show once.
  async function create() {
    const answer = await change("create", { create: ACTION_TOKEN });
    if (answer?.created?.token) set({ reveal: { token: String(answer.created.token) } });
  }

  const revoke = (/** @type {string} */ id) => change("revoke", { revoke: { id } });

  /// Forgets the token that was shown. It cannot be shown again.
  const hide = () => set({ reveal: null });

  function dispose() {
    closed = true;
    generation += 1;
  }

  return {
    get state() { return current; },
    refresh,
    create,
    revoke,
    hide,
    dispose,
  };
}
