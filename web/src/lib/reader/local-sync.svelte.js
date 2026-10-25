// The LibrePaper section of the Git panel for a folder project (SPEC-sync.md):
// where the folder stands with its web project, and what the last Pull or Push
// did.
//
// This module owns that state; the panel only reads it and calls the actions.
// The desktop app does the work. Pull and Push are the `librepaper` commands
// run behind routes, so this file decides nothing about files, only what to
// ask and how to say the answer. Pull, Push and Publish first settle the
// folder through `flush`, so the disk holds what the editor holds when the
// command reads it.
//
// Signing in is two calls to the desktop app with the page between them. The
// first hands out a code and the second finishes once the code has been
// approved. When the page has a session on the server, `approve` approves the
// code there. A folder project can open without one, so then the page shows the
// code, opens the server's approval page in a new tab, and asks the second call
// every few seconds until the code is approved or the person cancels.
import * as routes from "../desktop/sync.js";
import { day } from "../dates.js";

// How often the second call is repeated while the desktop app has not yet seen
// the approval, and how long to wait between.
const COMPLETE_ATTEMPTS = 5;
const COMPLETE_DELAY_MS = 800;

// The same call while the person approves the code in another tab: how often
// it is asked, and how long the code is waited for before the sign-in is given
// up on.
const APPROVAL_POLL_MS = 3000;
const APPROVAL_LIMIT_MS = 10 * 60 * 1000;

// The files a push sent, as the desktop app counted them or as a list.
const count = (value) => (Array.isArray(value) ? value.length : Number(value) || 0);

const realWait = (/** @type {number} */ ms) => new Promise((resolve) => setTimeout(resolve, ms));
const realOpen = (/** @type {string} */ url) => { globalThis.open?.(url, "_blank", "noopener"); };

/// Whether the desktop app refused `login/complete` only because the code has
/// not been approved yet.
const isPending = (/** @type {any} */ error) => error?.status === 400 && error?.body?.error === routes.LOGIN_PENDING;

/** What a pull did. In a plain folder it wrote and trashed files. In a git
 * repository it fetched web versions as commits and merged them: `versions`
 * counts the commits, `merge` says how the merge went, and `paths` are the
 * conflicted files of a merge that conflicted or the uncommitted files that
 * stopped it.
 * @typedef {{ repository?: undefined, written: number, trashed: number, skipped: string[], trimmed: boolean } | { repository: true, versions: number, merge: "fast-forward" | "merged" | "already" | "skipped" | "conflicts" | "dirty", paths: string[], trimmed: boolean }} PullResult */

/** @typedef {{ loaded: boolean, status: any, busy: string, error: string, waiting: { userCode: string, url: string } | null, pull: PullResult | null, conflicts: string[], refusal: { reason: "changed", paths: string[] } | { reason: "suggestion", path: string } | null, pushed: { nothing: true } | { files: number } | null }} SyncState */

/** `signedIn` is asked each time Sign in is pressed: whether this page has a
 * session on the server, so the code can be approved here. `open` shows a page
 * in a new tab, and `origin` is the server the verification page is found
 * under when the desktop app does not name it.
 * @param {{ id: string, approve: (userCode: string) => Promise<void>, signedIn?: () => boolean | Promise<boolean>, open?: (url: string) => void, origin?: string, api?: typeof routes, flush?: () => Promise<boolean>, now?: () => Date, wait?: (ms: number) => Promise<void> }} options */
export function createLocalSync({ id, approve, signedIn = () => true, open = realOpen, origin = globalThis.location?.origin ?? "", api = routes, flush = async () => true, now = () => new Date(), wait = realWait }) {
  /** @type {SyncState} */
  let current = $state.raw({ loaded: false, status: null, busy: "", error: "", waiting: null, pull: null, conflicts: [], refusal: null, pushed: null });
  let generation = 0;
  let closed = false;
  // The sign-in that is waiting for an approval in another tab, if any.
  /** @type {{ stopped: boolean, wake: () => void, woken: Promise<void> } | null} */
  let approval = null;

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
      // The desktop answers 200 with `error` when the site could not be
      // reached; the counts are then unknown, not zero.
      if (request === generation) set({ loaded: true, status, error: typeof status?.error === "string" ? status.error : "" });
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

  /// Asks the desktop app to finish the sign-in. It refuses while the
  /// approval has not reached it yet, so that refusal is tried again a few
  /// times. Any other refusal means the sign-in expired.
  async function complete(/** @type {string} */ authorizationId) {
    for (let attempt = 1; ; attempt += 1) {
      try {
        return await api.syncLoginComplete(id, authorizationId);
      } catch (error) {
        if (!isPending(error) || closed || attempt === COMPLETE_ATTEMPTS) throw error;
        await wait(COMPLETE_DELAY_MS);
      }
    }
  }

  /// Asks the desktop app to finish the sign-in every few seconds while the
  /// person approves the code in another tab. Answers null when the person
  /// cancelled or the panel is gone, and throws when the code is refused or
  /// not approved within the limit.
  async function awaitApproval(/** @type {string} */ authorizationId, /** @type {NonNullable<typeof approval>} */ waiting) {
    const began = now().getTime();
    for (;;) {
      await Promise.race([wait(APPROVAL_POLL_MS), waiting.woken]);
      if (waiting.stopped || closed) return null;
      try {
        const status = await api.syncLoginComplete(id, authorizationId);
        return waiting.stopped || closed ? null : status;
      } catch (error) {
        if (!isPending(error)) throw error;
      }
      if (waiting.stopped || closed) return null;
      if (now().getTime() - began >= APPROVAL_LIMIT_MS) throw new Error("The code was not approved in time. Sign in again to get a new one.");
    }
  }

  /// Where the server asks the person to approve a code, when the desktop app
  /// did not say.
  const verificationUrl = (/** @type {string} */ userCode) => `${origin.replace(/\/+$/, "")}/auth/device?code=${encodeURIComponent(userCode)}`;

  const login = () => run("login", async () => {
    const approving = Boolean(await signedIn());
    const started = await api.syncLogin(id);
    if (approving) {
      await approve(started.user_code);
      const status = await complete(started.authorization_id);
      generation += 1;
      set({ status });
      return;
    }
    // No session here to approve with, so the server's own page does it.
    const url = started.verification_url || verificationUrl(started.user_code);
    /** @type {() => void} */
    let wake = () => {};
    /** @type {Promise<void>} */
    const woken = new Promise((resolve) => { wake = () => resolve(); });
    const waiting = { stopped: false, wake, woken };
    approval = waiting;
    set({ waiting: { userCode: started.user_code, url } });
    open(url);
    try {
      const status = await awaitApproval(started.authorization_id, waiting);
      if (status) {
        generation += 1;
        set({ status });
      }
    } finally {
      approval = null;
      set({ waiting: null });
    }
  });

  /// Stops waiting for an approval. The panel goes back to the Sign in button.
  const cancelLogin = () => {
    if (!approval) return;
    approval.stopped = true;
    approval.wake();
  };

  const link = (/** @type {string} */ url) => run("link", async () => {
    await api.syncLink(id, url.trim());
    set(forgetResults());
    await refresh();
  });

  /// Makes a web project from the folder, so the folder is settled first and
  /// the project is not made from stale files.
  const publish = (/** @type {string} */ title) => run("publish", async () => {
    if (!(await saved())) return;
    await api.syncPublish(id, title.trim());
    set(forgetResults());
    await refresh();
  });

  const unlink = () => run("unlink", async () => {
    await api.syncUnlink(id);
    set(forgetResults());
    await refresh();
  });

  /// What a pull in a git repository did, from the desktop app's answer:
  /// `{ repository: true, versions, merge, trimmed }`, where `merge` is a word
  /// or `{ conflicts: paths }` or `{ dirty: paths }`.
  function repositoryPull(/** @type {any} */ result) {
    const merge = result.merge;
    const word = typeof merge === "string" ? merge : merge?.conflicts ? "conflicts" : merge?.dirty ? "dirty" : "skipped";
    return {
      repository: /** @type {true} */ (true),
      versions: Array.isArray(result.versions) ? result.versions.length : 0,
      merge: /** @type {"fast-forward" | "merged" | "already" | "skipped" | "conflicts" | "dirty"} */ (word),
      paths: Array.isArray(merge?.conflicts) ? merge.conflicts.map(String) : Array.isArray(merge?.dirty) ? merge.dirty.map(String) : [],
      trimmed: Boolean(result.trimmed),
    };
  }

  /// Takes in the web project's changes. `choices` settles files that changed
  /// on both sides, and its counts are added to the pull they belong to. In a
  /// git repository there is nothing to settle one file at a time: the
  /// versions become commits and git merges them.
  const pull = (/** @type {{ mine?: string[], theirs?: string[] }} */ choices = {}) => run("pull", async () => {
    if (!(await saved())) return;
    const settling = Boolean(choices.mine?.length || choices.theirs?.length);
    const result = await api.syncPull(id, choices);
    if (result?.repository) {
      set({ pull: repositoryPull(result), conflicts: [], refusal: null, pushed: null });
      await refresh();
      return;
    }
    const earlier = settling && !current.pull?.repository ? current.pull : null;
    set({
      pull: {
        written: (earlier?.written ?? 0) + result.written.length,
        trashed: (earlier?.trashed ?? 0) + result.trashed.length,
        // Files excluded here that the web also holds. Left alone, and said so.
        skipped: [...new Set([...(earlier?.skipped ?? []), ...(result.skipped ?? []).map(String)])],
        trimmed: Boolean(result?.trimmed) || Boolean(earlier?.trimmed),
      },
      conflicts: result.conflicts.map(String),
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
    approval?.wake();
  }

  return {
    get state() { return current; },
    defaultMessage,
    refresh,
    login,
    cancelLogin,
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
