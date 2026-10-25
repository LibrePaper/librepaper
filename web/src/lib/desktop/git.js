// A folder project's git repository, through the `projects/<id>/git` routes of
// the desktop app's local service (SPEC-git-panel.md). The desktop app runs
// the person's own git in the folder; this file only sends. A refusal is a
// `Refused` error carrying `status` and the parsed `body`, which is where a
// pull says which paths are dirty and a push says why it failed.
import { send } from "./client.js";

const route = (id, step = "") => `projects/${encodeURIComponent(String(id))}/git${step ? `/${step}` : ""}`;

/** @typedef {{ path: string, kind: "added" | "modified" | "deleted" | "renamed" | "conflicted", from?: string }} GitChange */
/** @typedef {{ repository: boolean, git: { found: boolean, version?: string, supported?: boolean }, branch?: string, upstream?: string, ahead?: number, behind?: number, remotes: { name: string, url: string }[], changes: GitChange[], merging: boolean, identity: boolean, detached?: boolean }} GitStatus */
/** @typedef {{ status: "fast-forward" | "merged" | "up-to-date" | "conflicts", conflicts: string[] }} GitPull */
/** @typedef {{ step: "commit" | "pull" | "push", ok?: boolean, reason?: string, paths?: string[], detail?: string, commit?: string, status?: string, conflicts?: string[] }} GitSyncStep */

/** Where the folder's repository stands: whether there is one, its branch,
 * remotes and changes.
 * @param {string} id
 * @returns {Promise<GitStatus>} */
export async function gitStatus(id) {
  const response = await send("GET", route(id));
  return response.json();
}

/** Makes the folder a repository on branch `main`.
 * @param {string} id */
export async function gitInit(id) {
  await send("POST", route(id, "init"));
}

/** One file as `HEAD` has it and as the working tree has it. A side that does
 * not exist is null; `binary` says there is no text to show.
 * @param {string} id @param {string} path
 * @returns {Promise<{ head: string | null, work: string | null, binary: boolean }>} */
export async function gitDiff(id, path) {
  const response = await send("GET", `${route(id, "diff")}?path=${encodeURIComponent(path)}`);
  return response.json();
}

/** Commits. `paths` leaves out every other change. A missing identity is a 409
 * whose body is `{ reason: "identity" }`.
 * @param {string} id @param {string} message @param {string[]} [paths]
 * @returns {Promise<{ commit?: string }>} */
export async function gitCommit(id, message, paths) {
  const response = await send("POST", route(id, "commit"), { jsonBody: paths ? { message, paths } : { message } });
  return response.json();
}

/** Writes `user.name` and `user.email` into the repository's own config.
 * @param {string} id @param {string} name @param {string} email */
export async function gitIdentity(id, name, email) {
  await send("POST", route(id, "identity"), { jsonBody: { name, email } });
}

/** The local branches and the one checked out. `remote` lists the branches
 * that only a remote has, by name; `gitSwitch` checks one out as a new local
 * branch that tracks it.
 * @param {string} id
 * @returns {Promise<{ current: string | null, branches: string[], remote: string[] }>} */
export async function gitBranches(id) {
  const response = await send("GET", route(id, "branches"));
  return response.json();
}

/** Checks out an existing branch.
 * @param {string} id @param {string} branch */
export async function gitSwitch(id, branch) {
  await send("POST", route(id, "switch"), { jsonBody: { branch } });
}

/** Makes a branch at the current commit and checks it out.
 * @param {string} id @param {string} branch */
export async function gitBranch(id, branch) {
  await send("POST", route(id, "branch"), { jsonBody: { branch } });
}

/** Sets the URL of a remote, adding the remote when it does not exist.
 * @param {string} id @param {string} name @param {string} url */
export async function gitRemote(id, name, url) {
  await send("POST", route(id, "remote"), { jsonBody: { name, url } });
}

/** Fetches the upstream and answers how far the branch has gone from it.
 * @param {string} id
 * @returns {Promise<{ ahead: number, behind: number }>} */
export async function gitFetch(id) {
  const response = await send("POST", route(id, "fetch"));
  return response.json();
}

/** Merges the upstream into the branch (`git pull --no-rebase`). Tracked
 * changes to project files refuse with a 409 `{ reason: "dirty", paths }`; a
 * merge that conflicts is an answer, not a refusal.
 * @param {string} id
 * @returns {Promise<GitPull>} */
export async function gitPull(id) {
  const response = await send("POST", route(id, "pull"));
  return response.json();
}

/** Pushes the branch, setting the upstream on the first push. A remote that
 * moved is a 409 `{ reason: "behind" }`; a sign-in or a connection that failed
 * carries `{ reason: "auth" | "network", detail }`.
 * @param {string} id */
export async function gitPush(id) {
  await send("POST", route(id, "push"));
}

/** Commits when the tree is dirty, pulls, and pushes. The answer lists the
 * steps that ran, in order, and stops at the first that failed: that step
 * carries the `reason` its own route would have given. A failed run may come
 * as a refusal, whose body is read the same way.
 * @param {string} id @param {string} [message]
 * @returns {Promise<{ steps: GitSyncStep[] }>} */
export async function gitSync(id, message) {
  try {
    const response = await send("POST", route(id, "sync"), { jsonBody: message ? { message } : {} });
    return response.json();
  } catch (error) {
    const body = /** @type {any} */ (error)?.body;
    if (Array.isArray(body?.steps)) return body;
    throw error;
  }
}
