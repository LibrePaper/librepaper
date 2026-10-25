// The repository section of the Git panel for a folder project
// (SPEC-git-panel.md): where the folder's git repository stands, and what the
// last action did.
//
// This module owns that state; the panel only reads it and calls the actions.
// The desktop app runs the person's own git in the folder, so this file
// decides nothing about files, only what to ask and how to say the answer.
// Pull, switch and sync, and commit with them, first settle the folder through
// `flush`, so the disk holds what the editor holds when git reads it.
//
// A refusal is not a failure. A pull over uncommitted changes lists the paths,
// a pull that conflicts lists the conflicted files, a push to a remote that
// moved says to pull first, and a sign-in that git could not make carries what
// the sign-in help needs. Each is state the panel draws; only an answer this
// file cannot read becomes `error`.
import * as routes from "../desktop/git.js";
import { remoteUrlProblem } from "../git-url.js";

/// What Sync commits with when the person has not written a message.
export const SYNC_MESSAGE = "Edits from LibrePaper";

const plural = (/** @type {number} */ count, /** @type {string} */ noun) => `${count} ${noun}${count === 1 ? "" : "s"}`;
const strings = (/** @type {unknown} */ list) => (Array.isArray(list) ? list.map(String) : []);
const short = (/** @type {unknown} */ id) => (typeof id === "string" ? id.slice(0, 7) : "");
const nothingToSettle = async () => true;

/** @typedef {"commit" | "sync"} PendingIdentity */
/** @typedef {{ step: string, text: string, failed?: boolean }} GitStep */
/** @typedef {{ loaded: boolean, status: any, busy: string, error: string, branches: { current: string | null, names: string[] } | null, identity: { then: PendingIdentity, message: string, paths?: string[] } | null, steps: GitStep[], conflicts: string[], refusal: { reason: "dirty", paths: string[] } | { reason: "behind" } | { reason: "network", detail: string } | null, auth: { detail: string, action?: "fetch" | "pull" | "push" | "sync" } | null }} GitState */

/** `flush` settles the folder before git reads it and resolves with whether it
 * could.
 * @param {{ id: string, api?: typeof routes, flush?: () => Promise<boolean> }} options */
export function createLocalGit({ id, api = routes, flush = nothingToSettle }) {
  /** @type {GitState} */
  let current = $state.raw({ loaded: false, status: null, busy: "", error: "", branches: null, identity: null, steps: [], conflicts: [], refusal: null, auth: null });
  let generation = 0;
  let branchGeneration = 0;
  let closed = false;
  // Whether the last read of the status failed, so that its message is the one
  // the next good read clears.
  let unread = false;
  // The message of the last Sync, so Try again after a sign-in repeats it.
  let lastSyncMessage = "";

  const set = (/** @type {Partial<GitState>} */ patch) => {
    if (!closed) current = { ...current, ...patch };
  };

  /// What the first Commit offers as its message. A merge in progress has its
  /// own, so Commit finishes it with the words git would have used.
  const defaultMessage = () => (current.status?.merging ? `Merge ${current.status.upstream || "the remote"}` : "");

  /// Reads where the repository stands. An answer that arrives after a newer
  /// request was made is dropped.
  async function refresh() {
    const request = ++generation;
    try {
      const status = await api.gitStatus(id);
      if (request !== generation) return;
      // Only a failed read is cleared by a good one; what an action said stays.
      set({ loaded: true, status, ...(unread ? { error: "" } : {}) });
      unread = false;
    } catch (error) {
      if (request !== generation) return;
      unread = true;
      set({ loaded: true, error: error?.message || "The desktop app could not read the repository." });
    }
  }

  /// Reads the branches, for the menu that switches between them.
  async function loadBranches() {
    const request = ++branchGeneration;
    try {
      const answer = await api.gitBranches(id);
      if (request === branchGeneration) set({ branches: { current: answer?.current ?? null, names: strings(answer?.branches) } });
    } catch (error) {
      if (request === branchGeneration) set({ error: error?.message || "The desktop app could not list the branches." });
    }
  }

  /// Runs one action while the panel is busy. A failure that is not one of
  /// the refusals the panel says is shown as the desktop app said it.
  async function run(/** @type {string} */ name, /** @type {() => Promise<void>} */ work) {
    if (current.busy || closed) return;
    set({ busy: name, error: "" });
    try {
      await work();
    } catch (error) {
      set({ error: error?.message || "The desktop app could not do that." });
    } finally {
      set({ busy: "" });
    }
  }

  /// Settles the folder, so git finds the disk as the editor has it. Says so
  /// when the folder could not be written.
  async function saved() {
    if (await flush()) return true;
    set({ error: "The folder could not be saved, so nothing was done." });
    return false;
  }

  const forget = () => set({ steps: [], conflicts: [], refusal: null, auth: null, identity: null });

  /// Shows a refusal the panel knows how to say, and answers whether it was
  /// one. `action` is what to repeat after a sign-in problem is fixed; the
  /// actions that never sign in leave it out.
  function refused(/** @type {any} */ error, /** @type {"fetch" | "pull" | "push" | "sync" | undefined} */ action = undefined) {
    const body = error?.body;
    switch (body?.reason) {
      case "dirty": set({ refusal: { reason: "dirty", paths: strings(body.paths) } }); return true;
      case "behind": set({ refusal: { reason: "behind" } }); return true;
      case "network": set({ refusal: { reason: "network", detail: String(body.detail ?? "") } }); return true;
      case "auth": set({ auth: { detail: String(body.detail ?? ""), action } }); return true;
      default: return false;
    }
  }

  const init = () => run("init", async () => {
    forget();
    await api.gitInit(id);
    set({ steps: [{ step: "init", text: "Made this folder a git repository on the branch main." }], branches: null });
    await refresh();
  });

  /// Commits. `paths` leaves every other change out; without it everything
  /// is committed. A missing identity opens the prompt, and saving the
  /// identity commits again.
  const commit = (/** @type {string} */ message, /** @type {string[] | undefined} */ paths) => run("commit", async () => {
    forget();
    const text = message.trim();
    if (!text) {
      set({ error: "Write a message for the commit." });
      return;
    }
    if (!(await saved())) return;
    let result;
    try {
      result = await api.gitCommit(id, text, paths);
    } catch (error) {
      if (error?.body?.reason === "identity") {
        set({ identity: { then: "commit", message: text, ...(paths ? { paths } : {}) } });
        return;
      }
      throw error;
    }
    const id7 = short(result?.commit);
    set({ steps: [{ step: "commit", text: id7 ? `Committed ${id7}.` : "Committed." }] });
    await refresh();
  });

  /// Writes the name and email into the repository's own config, then does
  /// what was waiting for them.
  async function saveIdentity(/** @type {string} */ name, /** @type {string} */ email) {
    const waiting = current.identity;
    if (!waiting || current.busy || closed) return;
    const who = name.trim();
    const address = email.trim();
    if (!who || !address.includes("@")) {
      set({ error: "Give a name and an email address." });
      return;
    }
    set({ busy: "identity", error: "" });
    try {
      await api.gitIdentity(id, who, address);
    } catch (error) {
      set({ error: error?.message || "The desktop app could not save that identity.", busy: "" });
      return;
    }
    set({ identity: null, busy: "" });
    if (waiting.then === "sync") await sync(waiting.message);
    else await commit(waiting.message, waiting.paths);
  }

  const cancelIdentity = () => set({ identity: null, error: "" });

  const switchBranch = (/** @type {string} */ branch) => run("switch", async () => {
    forget();
    if (!(await saved())) return;
    try {
      await api.gitSwitch(id, branch);
    } catch (error) {
      if (refused(error)) return;
      throw error;
    }
    set({ steps: [{ step: "switch", text: `Switched to ${branch}.` }] });
    await Promise.all([refresh(), loadBranches()]);
  });

  const createBranch = (/** @type {string} */ branch) => run("branch", async () => {
    forget();
    const name = branch.trim();
    if (!name) {
      set({ error: "Give the branch a name." });
      return;
    }
    if (!(await saved())) return;
    await api.gitBranch(id, name);
    set({ steps: [{ step: "branch", text: `Made the branch ${name} and switched to it.` }] });
    await Promise.all([refresh(), loadBranches()]);
  });

  /// Sets where this repository pushes to and pulls from.
  const setRemote = (/** @type {string} */ url, /** @type {string} */ name = "origin") => run("remote", async () => {
    forget();
    const problem = remoteUrlProblem(url);
    if (problem) {
      set({ error: problem });
      return;
    }
    await api.gitRemote(id, name, url.trim());
    set({ steps: [{ step: "remote", text: `The remote ${name} is ${url.trim()}.` }] });
    await refresh();
  });

  const fetchUpstream = () => run("fetch", async () => {
    forget();
    let result;
    try {
      result = await api.gitFetch(id);
    } catch (error) {
      if (refused(error, "fetch")) return;
      throw error;
    }
    const ahead = Number(result?.ahead) || 0;
    const behind = Number(result?.behind) || 0;
    set({ steps: [{ step: "fetch", text: `Fetched. ${ahead} ahead, ${behind} behind.` }] });
    await refresh();
  });

  /// One sentence for what a pull did.
  const pulledText = (/** @type {string} */ status) => (status === "fast-forward" ? "Pulled and fast-forwarded."
    : status === "merged" ? "Pulled and merged."
      : status === "up-to-date" ? "Already up to date."
        : "Pulled.");

  const pull = () => run("pull", async () => {
    forget();
    if (!(await saved())) return;
    let result;
    try {
      result = await api.gitPull(id);
    } catch (error) {
      if (refused(error, "pull")) return;
      throw error;
    }
    const conflicts = strings(result?.conflicts);
    if (result?.status === "conflicts") {
      set({ conflicts, steps: [{ step: "pull", text: `The pull stopped with ${plural(conflicts.length, "file")} in conflict.`, failed: true }] });
    } else {
      set({ steps: [{ step: "pull", text: pulledText(result?.status) }] });
    }
    await refresh();
  });

  const push = () => run("push", async () => {
    forget();
    try {
      await api.gitPush(id);
    } catch (error) {
      if (refused(error, "push")) return;
      throw error;
    }
    set({ steps: [{ step: "push", text: "Pushed." }] });
    await refresh();
  });

  /// What one step of a Sync says, and whether it stopped the run.
  function stepOf(/** @type {any} */ step) {
    const name = String(step?.step ?? "");
    if (step?.reason) {
      const said = {
        identity: "git needs your name and email.",
        dirty: "uncommitted changes are in the way.",
        behind: "the remote has commits you do not have.",
        auth: "git could not sign in.",
        network: "the connection failed.",
      }[/** @type {string} */ (step.reason)] ?? String(step.detail || "it failed.");
      return { step: name, text: `Stopped at ${name}: ${said}`, failed: true };
    }
    if (name === "commit") return { step: name, text: step?.status === "nothing" ? "Nothing to commit." : short(step?.commit) ? `Committed ${short(step.commit)}.` : "Committed." };
    if (name === "pull") {
      if (step?.status === "conflicts") return { step: name, text: `Stopped at pull: ${plural(strings(step.conflicts).length, "file")} in conflict.`, failed: true };
      return { step: name, text: pulledText(step?.status) };
    }
    if (name === "push") return { step: name, text: step?.status === "up-to-date" ? "Nothing to push." : "Pushed." };
    return { step: name, text: "Done." };
  }

  /// Commits when the tree is dirty, pulls, and pushes. Every step the desktop
  /// app ran is listed, and the one that stopped it is drawn the way its own
  /// action draws a refusal.
  const sync = (/** @type {string} */ message = "") => run("sync", async () => {
    forget();
    const text = message.trim() || SYNC_MESSAGE;
    lastSyncMessage = message;
    if (!(await saved())) return;
    const result = await api.gitSync(id, text);
    const ran = Array.isArray(result?.steps) ? result.steps : [];
    if (!ran.length) {
      set({ error: "The desktop app did not say what happened." });
      return;
    }
    const steps = ran.map(stepOf);
    set({ steps });
    const stopped = ran.find((step) => step?.reason);
    if (stopped?.reason === "identity") set({ identity: { then: "sync", message: text } });
    else if (stopped) {
      if (!refused({ body: stopped }, "sync")) set({ error: String(stopped.detail || `Sync stopped at ${stopped.step}.`) });
    }
    const conflicted = ran.find((step) => step?.step === "pull" && step?.status === "conflicts");
    if (conflicted) set({ conflicts: strings(conflicted.conflicts) });
    await refresh();
  });

  /// Repeats the action whose sign-in failed, once the person has fixed it.
  const retrySignIn = () => {
    const action = current.auth?.action;
    if (action === "fetch") return fetchUpstream();
    if (action === "pull") return pull();
    if (action === "push") return push();
    if (action === "sync") return sync(lastSyncMessage);
    return Promise.resolve();
  };

  const dismiss = () => set({ error: "", refusal: null, auth: null });

  function dispose() {
    closed = true;
  }

  return {
    get state() { return current; },
    defaultMessage,
    refresh,
    loadBranches,
    init,
    commit,
    saveIdentity,
    cancelIdentity,
    switchBranch,
    createBranch,
    setRemote,
    fetch: fetchUpstream,
    pull,
    push,
    sync,
    retrySignIn,
    dismiss,
    dispose,
  };
}
