// A folder project's meeting with its web project, through the `projects/<id>/sync`
// routes of the desktop app's local service. The desktop app
// holds the sign-in and runs the two commands, pull and push; this file only
// sends. A refusal is a `Refused` error carrying `status` and the parsed
// `body`, which is where a push says which paths changed.
import { send } from "./client.js";

/// What the desktop app answers to `login/complete` while the code it handed
/// out has not been approved yet. Any other 400 means the sign-in expired.
export const LOGIN_PENDING = "sign in and approve the code, then retry";

const route = (id, step = "") => `projects/${encodeURIComponent(String(id))}/sync${step ? `/${step}` : ""}`;

/** Where the folder stands with its web project. `git` is true when the
 * folder holds a `.git`: pull and push then run the repository form of the
 * sync, where versions become commits and git merges them. `branch` is the
 * branch sync is bound to (null until it is confirmed), `checked_out` is the
 * branch the folder is on now (null when HEAD is detached or the folder is not
 * a repository), and `pending` is true while a send to the web was interrupted.
 * @param {string} id
 * @returns {Promise<{ signed_in: boolean, linked: boolean, git?: boolean, branch?: string | null, checked_out?: string | null, pending?: boolean, other_site?: boolean, site?: string, slug?: string, pulled?: number, changes?: { here: number, web: boolean }, error?: string }>} */
export async function syncStatus(id) {
  const response = await send("GET", route(id));
  return response.json();
}

/** Starts signing in to the web server. The desktop app answers at once with
 * a code, which the page approves in its own session.
 * @param {string} id
 * @returns {Promise<{ authorization_id: string, user_code: string, verification_url?: string }>} */
export async function syncLogin(id) {
  const response = await send("POST", route(id, "login"), { jsonBody: {} });
  return response.json();
}

/** Finishes the sign-in once the code is approved, and answers like the
 * status. While the code is still pending the desktop app refuses with a 400
 * whose error is `LOGIN_PENDING`.
 * @param {string} id @param {string} authorizationId
 * @returns {Promise<{ signed_in: boolean, linked: boolean, git?: boolean, site?: string, slug?: string, pulled?: number, changes?: { here: number, web: boolean } }>} */
export async function syncLoginComplete(id, authorizationId) {
  const response = await send("POST", route(id, "login/complete"), { jsonBody: { authorization_id: authorizationId } });
  return response.json();
}

/** Links the folder to an existing web project.
 * @param {string} id @param {string} url the project page, `https://<site>/docs/<slug>` */
export async function syncLink(id, url) {
  await send("POST", route(id, "link"), { jsonBody: { url } });
}

/** Compares the folder with a web project before the two are linked. Nothing
 * is saved or written. The lists are paths, and `blocked` holds
 * `[path, reason]` pairs.
 * @param {string} id @param {string} url the project page, `https://<site>/docs/<slug>`
 * @returns {Promise<{ project_only: string[], local_only: string[], different: string[], equal: string[], blocked: [string, string][], excluded: string[] }>} */
export async function syncPreview(id, url) {
  const response = await send("POST", route(id, "preview"), { jsonBody: { url } });
  return response.json();
}

/** Makes a web project from the folder and links it.
 * @param {string} id @param {string} title */
export async function syncPublish(id, title) {
  await send("POST", route(id, "publish"), { jsonBody: { title } });
}

/** Forgets the link. The folder and the web project stay as they are.
 * @param {string} id */
export async function syncUnlink(id) {
  await send("POST", route(id, "unlink"));
}

/** Takes in what changed on the web. `mine` and `theirs` settle files that
 * changed on both sides, one path at a time. In a git repository the answer is
 * the repository form instead: the versions taken as commits and how git
 * merged them, where `merge` is a word, `{ conflicts: paths }` or
 * `{ dirty: paths }`. In a git repository `branch` confirms the branch to
 * sync; a refusal about the branch is a 409 whose body has `reason: "branch"`,
 * and one about an interrupted send has `reason: "pending"`.
 * @param {string} id @param {{ mine?: string[], theirs?: string[], branch?: string }} [choices]
 * @returns {Promise<{ written: string[], trashed: string[], kept?: unknown, conflicts: string[], skipped?: string[], trimmed?: boolean, repository?: false } | { repository: true, versions: { sha: string, label: string, by: string, commit: string }[], merge: "fast-forward" | "merged" | "already" | "skipped" | { conflicts: string[] } | { dirty: string[] }, trimmed: boolean }>} */
export async function syncPull(id, choices = {}) {
  const response = await send("POST", route(id, "pull"), { jsonBody: choices });
  return response.json();
}

/** Sends what changed in the folder. A refusal is a 409 whose body is
 * `{ reason: "changed", paths }` or `{ reason: "suggestion", path }`, or
 * `{ reason: "branch", error }` about the branch of a git repository.
 * @param {string} id @param {string} message @param {string} [branch] the branch to sync, to confirm it in a repository
 * @returns {Promise<{ label?: { sha: string, sequence: number }, files?: number | string[], nothing?: boolean }>} */
export async function syncPush(id, message, branch) {
  const response = await send("POST", route(id, "push"), { jsonBody: branch ? { message, branch } : { message } });
  return response.json();
}
