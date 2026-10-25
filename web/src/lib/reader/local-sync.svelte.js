// The Sync panel's state for a folder project (SPEC-sync.md): where the folder
// stands with its web project, and what the last Pull or Push did.
//
// This module owns that state; the panel only reads it and calls the actions.
// The desktop app does the work. Pull and Push are the `librepaper` commands
// run behind routes, so this file decides nothing about files, only what to
// ask and how to say the answer. Pull and Push first settle the folder through
// `flush`, so the disk holds what the editor holds when the command reads it.
import * as routes from "../desktop/sync.js";
import { day } from "../dates.js";

// A count the desktop app may send as a number or as the list it counted.
const count = (value) => (Array.isArray(value) ? value.length : Number(value) || 0);

/** @typedef {{ loaded: boolean, status: any, busy: string, error: string, pull: { written: number, trashed: number, trimmed: boolean } | null, conflicts: string[], refusal: { reason: "changed", paths: string[] } | { reason: "suggestion", path: string } | null, pushed: { nothing: true } | { files: number } | null }} SyncState */

/** @param {{ id: string, api?: typeof routes, flush?: () => Promise<boolean>, now?: () => Date }} options */
export function createLocalSync({ id, api = routes, flush = async () => true, now = () => new Date() }) {
  /** @type {SyncState} */
  let current = $state.raw({ loaded: false, status: null, busy: "", error: "", pull: null, conflicts: [], refusal: null, pushed: null });
  let generation = 0;
  let closed = false;

  const set = (/** @type {Partial<SyncState>} */ patch) => {
    if (!closed) current = { ...current, ...patch };
  };

  /// What a Push is called when the person does not name it: today's date.
  const defaultMessage = () => day(now());

  /// Reads where the folder stands. An answer that arrives after a newer
  /// request was made is dropped.
  async function refresh() {
    const request = ++generation;
    try {
      const status = await api.syncStatus(id);
      if (request === generation) set({ loaded: true, status });
    } catch (error) {
      if (request === generation) set({ loaded: true, error: error?.message || "The desktop app could not read the sync state." });
    }
  }

  /// Runs one action while the panel is busy. A 401 means the sign-in is
  /// gone, so the panel goes back to asking for it; any other failure is said
  /// as the desktop app said it.
  async function run(name, work) {
    if (current.busy || closed) return;
    set({ busy: name, error: "" });
    try {
      await work();
    } catch (error) {
      if (error?.status === 401) {
        set({ error: "Sign in to the web project to continue.", status: { ...current.status, signed_in: false } });
      } else {
        set({ error: error?.message || "The desktop app could not do that." });
      }
    } finally {
      set({ busy: "" });
    }
  }

  /// Settles the folder, so a pull finds the disk as the editor has it and a
  /// push sends it. Says so when the folder could not be written.
  async function saved() {
    if (await flush()) return true;
    set({ error: "The folder could not be saved, so nothing was done." });
    return false;
  }

  const forgetResults = () => ({ pull: null, conflicts: [], refusal: null, pushed: null });

  const login = () => run("login", async () => {
    await api.syncLogin(id);
    await refresh();
  });

  const link = (/** @type {string} */ url) => run("link", async () => {
    await api.syncLink(id, url.trim());
    set(forgetResults());
    await refresh();
  });

  const publish = (/** @type {string} */ title) => run("publish", async () => {
    await api.syncPublish(id, title.trim());
    set(forgetResults());
    await refresh();
  });

  const unlink = () => run("unlink", async () => {
    await api.syncUnlink(id);
    set(forgetResults());
    await refresh();
  });

  /// Takes in the web project's changes. `choices` settles files that changed
  /// on both sides, and its counts are added to the pull they belong to.
  const pull = (/** @type {{ mine?: string[], theirs?: string[] }} */ choices = {}) => run("pull", async () => {
    if (!(await saved())) return;
    const settling = Boolean(choices.mine?.length || choices.theirs?.length);
    const result = await api.syncPull(id, choices);
    const earlier = settling ? current.pull : null;
    set({
      pull: {
        written: (earlier?.written ?? 0) + count(result?.written),
        trashed: (earlier?.trashed ?? 0) + count(result?.trashed),
        trimmed: Boolean(result?.trimmed) || Boolean(earlier?.trimmed),
      },
      conflicts: Array.isArray(result?.conflicts) ? result.conflicts.map(String) : [],
      refusal: null,
      pushed: null,
    });
    await refresh();
  });

  const keepMine = (/** @type {string} */ path) => pull({ mine: [path] });
  const takeTheirs = (/** @type {string} */ path) => pull({ theirs: [path] });

  /// Sends the folder's changes with a short message. A refusal is not a
  /// failure: it says which files changed on the web, or which suggestion is
  /// waiting, and the person settles that and tries again.
  const push = (/** @type {string} */ message) => run("push", async () => {
    set({ refusal: null, pushed: null });
    if (!(await saved())) return;
    let result;
    try {
      result = await api.syncPush(id, message.trim() || defaultMessage());
    } catch (error) {
      const body = error?.body;
      if (error?.status === 409 && body?.reason === "changed") {
        set({ ...forgetResults(), refusal: { reason: "changed", paths: Array.isArray(body.paths) ? body.paths.map(String) : [] } });
        return;
      }
      if (error?.status === 409 && body?.reason === "suggestion") {
        set({ ...forgetResults(), refusal: { reason: "suggestion", path: String(body.path ?? "") } });
        return;
      }
      throw error;
    }
    set({ ...forgetResults(), pushed: result?.nothing ? { nothing: true } : { files: count(result?.files) } });
    await refresh();
  });

  function dispose() {
    closed = true;
  }

  return {
    get state() { return current; },
    defaultMessage,
    refresh,
    login,
    link,
    publish,
    unlink,
    pull,
    keepMine,
    takeTheirs,
    push,
    dispose,
  };
}
