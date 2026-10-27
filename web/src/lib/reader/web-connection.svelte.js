// The Git panel's connection of a web project to a git repository, in the
// desktop app. The desktop app keeps a clone of the
// repository on this computer, or works in a repository folder the person
// chose, and pull and push move changes between that repository and the
// project. This module decides nothing about files: it asks the
// desktop app, and says what the answer means.
//
// This module owns that state; the panel only reads it and calls the actions.
//
// Signing in is two calls to the desktop app with the page between them. When the page has a session on the server, `approve`
// approves the code there. Otherwise the page opens the server's approval page
// in a new tab and asks the second call every few seconds until the code is
// approved.
import * as routes from "../desktop/connections.js";
import { LOGIN_PENDING } from "../desktop/connections.js";

// How often the second call is repeated while the desktop app has not yet seen
// the approval, and how long to wait between.
const COMPLETE_ATTEMPTS = 5;
const COMPLETE_DELAY_MS = 800;

// The same call while the person approves the code in another tab: how often
// it is asked, and how many times before the sign-in is given up on.
const APPROVAL_POLL_MS = 3000;
const APPROVAL_ATTEMPTS = 200;

const BEHIND = "The repository has changes that this project does not have. Get repository changes first, then send project changes again.";
const INTERRUPTED = "An earlier send to the project was interrupted before its outcome was known.";
const MERGED = "The merge is done. Get repository changes or send project changes again.";

const realWait = (/** @type {number} */ ms) => new Promise((resolve) => setTimeout(resolve, ms));
const realOpen = (/** @type {string} */ url) => { globalThis.open?.(url, "_blank", "noopener"); };

/// Whether the desktop app refused `login/complete` only because the code has
/// not been approved yet.
const isPending = (/** @type {any} */ error) => error?.status === 400 && error?.body?.error === LOGIN_PENDING;

const plural = (/** @type {number} */ count, /** @type {string} */ noun) => `${count} ${noun}${count === 1 ? "" : "s"}`;
const paths = (/** @type {unknown} */ value) => (Array.isArray(value) ? value.map(String) : []);

/** @typedef {{ loaded: boolean, status: any, busy: "" | "connect" | "connect-local" | "pull" | "push" | "retry" | "resolve" | "login" | "disconnect", result: string | null, refusal: { reason: "changed", paths: string[] } | { reason: "suggestion", path: string, action: "pull" | "push" } | null, problem: string, signInHelp: { remote: string, detail: string } | null, interrupted: string }} WebConnectionState */

/** `flush` settles the editor's own edits on the server before a transfer reads
 * the project. `signedIn` is asked each time Sign in is pressed: whether this
 * page has a session on the server, so the code can be approved here. `open`
 * shows a page in a new tab, and `origin` is the server the approval page is
 * found under when the desktop app does not name it.
 * @param {{ api?: typeof routes, project: string, flush?: () => Promise<void>, signedIn?: () => boolean | Promise<boolean>, approve?: (userCode: string) => Promise<void>, open?: (url: string) => void, origin?: string, wait?: (ms: number) => Promise<void> }} options */
export function createWebConnection({ api = routes, project, flush = async () => {}, signedIn = () => true, approve = async () => {}, open = realOpen, origin = globalThis.location?.origin ?? "", wait = realWait }) {
  /** @type {WebConnectionState} */
  let current = $state.raw({ loaded: false, status: null, busy: "", result: null, refusal: null, problem: "", signInHelp: null, interrupted: "" });
  let generation = 0;
  let closed = false;
  // The address last typed into Connect, which names the remote until the
  // status knows it.
  let address = "";
  // Repeats the last action, for Try again.
  let again = /** @type {(() => Promise<void>) | null} */ (null);

  const set = (/** @type {Partial<WebConnectionState>} */ patch) => {
    if (!closed) current = { ...current, ...patch };
  };

  /// Reads where the project stands. An answer that arrives after a newer
  /// request was made is dropped, and so is one that overlaps an action, whose
  /// own answer is newer. No read starts while an action runs.
  async function refresh() {
    if (current.busy) return;
    const request = ++generation;
    try {
      const status = await api.connectionStatus(project);
      if (request === generation) set({ loaded: true, status });
    } catch (error) {
      if (request === generation) set({ loaded: true, problem: error?.message || "The desktop app could not read the connection." });
    }
  }

  /// Runs one action while the panel is busy. A 401 means the sign-in is gone,
  /// so the panel asks for it again. A git failure to sign in fills
  /// `signInHelp`; any other failure is said as the desktop app said it.
  async function run(/** @type {WebConnectionState["busy"]} */ name, /** @type {() => Promise<void>} */ work) {
    if (current.busy || closed) return;
    again = () => run(name, work);
    generation += 1;
    set({ busy: name, problem: "", signInHelp: null });
    try {
      await work();
    } catch (error) {
      if (error?.status === 401) {
        set({ problem: "Sign in to continue.", status: { ...current.status, signed_in: false } });
      } else if (error?.body?.reason === "auth") {
        set({ signInHelp: { remote: current.status?.url || address, detail: String(error.body.detail ?? "") } });
      } else {
        set({ problem: error?.message || "The desktop app could not do that." });
      }
    } finally {
      set({ busy: "" });
    }
  }

  /// A pull or a push that stops at a file the project changed, or at a
  /// suggestion still waiting, is an answer for the panel to show, not a
  /// failure. Returns whether the error was one of those.
  function refused(/** @type {any} */ error, /** @type {"pull" | "push"} */ action) {
    if (error?.status !== 409) return false;
    if (error.body?.reason === "changed") {
      set({ refusal: { reason: "changed", paths: paths(error.body.paths) } });
      return true;
    }
    if (error.body?.reason === "suggestion") {
      set({ refusal: { reason: "suggestion", path: String(error.body.path ?? ""), action } });
      return true;
    }
    return false;
  }

  /// A pull or a push that an interrupted send to the project stops. The
  /// project holds a request that was posted and not confirmed, and every
  /// transfer is refused until it is settled, so the panel offers to retry just
  /// that. Returns whether the error was this one.
  function interruptedSend(/** @type {any} */ error) {
    if (error?.status !== 409 || error.body?.reason !== "pending") return false;
    set({ interrupted: String(error.body.error || error.message || INTERRUPTED) });
    return true;
  }

  /// The status an answer carries. Every route answers with it, but a status
  /// that is missing keeps the one the panel has.
  const statusOf = (/** @type {any} */ answer) => answer?.status ?? current.status;

  /// Names the repository branch an answer is about, when the status knows it.
  const branchOf = (/** @type {any} */ answer) => {
    const branch = statusOf(answer)?.branch;
    return branch ? ` branch ${branch}` : "";
  };

  const importedLine = (/** @type {any} */ answer) => {
    const files = Number(answer.pushed?.files) || 0;
    const from = `Imported from repository${branchOf(answer)}`;
    return files ? `${from}: ${plural(files, "file")} into the project.` : `${from}: nothing new.`;
  };

  const sentLine = (/** @type {any} */ answer) => ((typeof answer.pulled === "boolean" ? answer.pulled : answer.pushed === "pushed")
    ? `Sent to repository${branchOf(answer)}.`
    : `Nothing new to send to repository${branchOf(answer)}.`);

  /// Connects the project to a repository the desktop app clones.
  const connect = (/** @type {string} */ url) => {
    address = url.trim();
    return run("connect", async () => {
      const status = await api.connectRepository(project, address);
      set({ status, result: null, refusal: null });
    });
  };

  /// Connects the project to a git repository on this computer, which the
  /// person chooses in the desktop app's folder dialog. A cancelled dialog
  /// answers the status as it was, so the panel stays as it was.
  const connectLocal = () => run("connect-local", async () => {
    const status = await api.connectLocalRepository(project);
    set({ status, result: null, refusal: null });
  });

  /// Imports the repository's changes into the project. Stops at a conflict,
  /// which the merge section then settles one file at a time. The merge reads
  /// the project, so the editor's own edits are settled first.
  const pull = () => run("pull", async () => {
    set({ refusal: null, result: null, interrupted: "" });
    await flush();
    let answer = /** @type {any} */ (null);
    try {
      answer = await api.pullConnection(project);
    } catch (error) {
      if (refused(error, "pull") || interruptedSend(error)) return;
      throw error;
    }
    set({ status: statusOf(answer), result: answer.stopped ? null : importedLine(answer) });
  });

  /// Sends the project's changes to the repository. The editor's own edits are
  /// settled first, so the project the server holds is the one sent.
  const push = () => run("push", async () => {
    set({ refusal: null, result: null, interrupted: "" });
    await flush();
    let answer = /** @type {any} */ (null);
    try {
      answer = await api.pushConnection(project);
    } catch (error) {
      if (refused(error, "push") || interruptedSend(error)) return;
      if (error?.body?.reason === "behind") {
        set({ problem: BEHIND });
        return;
      }
      throw error;
    }
    set({ status: statusOf(answer), result: answer.stopped ? null : sentLine(answer) });
  });

  /// Settles the interrupted send to the project and nothing else. It does not
  /// restart the pull or push that was stopped, because that one is refused
  /// until this settles. The person starts it again afterwards, and the line
  /// says so.
  const retryTransfer = () => run("retry", async () => {
    set({ refusal: null, result: null });
    let answer = /** @type {any} */ (null);
    try {
      answer = await api.retryConnection(project);
    } catch (error) {
      if (refused(error, "push")) return;
      throw error;
    }
    const files = Number(answer.pushed?.files) || 0;
    const settled = files ? `The interrupted send finished: ${plural(files, "file")} reached the project.` : "The interrupted send finished: nothing was left to send to the project.";
    set({ status: statusOf(answer), interrupted: "", result: `${settled} Get repository changes or send project changes again to continue.` });
  });

  /// Keeps one side of a file that changed on both sides during a merge. The
  /// merge is done when the last file is settled.
  const resolve = (/** @type {string} */ path, /** @type {"project" | "repository"} */ keep) => run("resolve", async () => {
    const status = await api.resolveConnection(project, path, keep);
    set({ status, result: status?.merging ? null : MERGED });
  });

  /// Finishes the sign-in. The page approves the code itself when it has a
  /// session on the server; otherwise it waits for the approval in another tab.
  const login = () => run("login", async () => {
    const approving = Boolean(await signedIn());
    const started = await api.loginConnection(project);
    if (approving) {
      await approve(started.user_code);
      const status = await complete(started.authorization_id);
      generation += 1;
      set({ status });
      return;
    }
    // No session here to approve with, so the server's own page does it.
    open(started.verification_url || verificationUrl(started.user_code));
    const status = await awaitApproval(started.authorization_id);
    if (status) {
      generation += 1;
      set({ status });
    }
  });

  /// Asks the desktop app to finish the sign-in. It refuses while the approval
  /// has not reached it yet, so that refusal is tried again a few times.
  async function complete(/** @type {string} */ authorizationId) {
    for (let attempt = 1; ; attempt += 1) {
      try {
        return await api.loginConnectionComplete(project, authorizationId);
      } catch (error) {
        if (!isPending(error) || closed || attempt === COMPLETE_ATTEMPTS) throw error;
        await wait(COMPLETE_DELAY_MS);
      }
    }
  }

  /// Asks the desktop app to finish the sign-in every few seconds while the
  /// person approves the code in another tab. Answers null when the panel is
  /// gone, and throws when the code is not approved within the limit.
  async function awaitApproval(/** @type {string} */ authorizationId) {
    for (let attempt = 1; attempt <= APPROVAL_ATTEMPTS; attempt += 1) {
      await wait(APPROVAL_POLL_MS);
      if (closed) return null;
      try {
        return await api.loginConnectionComplete(project, authorizationId);
      } catch (error) {
        if (!isPending(error)) throw error;
      }
    }
    throw new Error("The code was not approved in time. Sign in again to get a new one.");
  }

  /// Where the server asks the person to approve a code, when the desktop app
  /// did not say.
  const verificationUrl = (/** @type {string} */ userCode) => `${origin.replace(/\/+$/, "")}/auth/device?code=${encodeURIComponent(userCode)}`;

  /// Forgets the link. The copy on this computer, a local folder included, and
  /// the project stay as they are.
  const disconnect = () => run("disconnect", async () => {
    const status = await api.disconnectConnection(project);
    set({ status, result: null, refusal: null });
  });

  /// Repeats the action that failed, after the sign-in help has been read.
  const retry = () => again?.();

  const dismiss = () => set({ signInHelp: null });

  function dispose() {
    closed = true;
    generation += 1;
  }

  return {
    get state() { return current; },
    refresh,
    connect,
    connectLocal,
    pull,
    push,
    retryTransfer,
    resolve,
    login,
    disconnect,
    retry,
    dismiss,
    dispose,
  };
}
