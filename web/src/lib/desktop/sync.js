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
 * sync, where versions become commits and git merges them.
 * @param {string} id
 * @returns {Promise<{ signed_in: boolean, linked: boolean, git?: boolean, other_site?: boolean, site?: string, slug?: string, pulled?: number, changes?: { here: number, web: boolean }, error?: string }>} */
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
 * `{ dirty: paths }`.
 * @param {string} id @param {{ mine?: string[], theirs?: string[] }} [choices]
 * @returns {Promise<{ written: string[], trashed: string[], kept?: unknown, conflicts: string[], skipped?: string[], trimmed?: boolean, repository?: false } | { repository: true, versions: { sha: string, label: string, by: string, commit: string }[], merge: "fast-forward" | "merged" | "already" | "skipped" | { conflicts: string[] } | { dirty: string[] }, trimmed: boolean }>} */
export async function syncPull(id, choices = {}) {
  const response = await send("POST", route(id, "pull"), { jsonBody: choices });
  return response.json();
}

/** Sends what changed in the folder. A refusal is a 409 whose body is
 * `{ reason: "changed", paths }` or `{ reason: "suggestion", path }`.
 * @param {string} id @param {string} message
 * @returns {Promise<{ label?: { sha: string, sequence: number }, files?: number | string[], nothing?: boolean }>} */
export async function syncPush(id, message) {
  const response = await send("POST", route(id, "push"), { jsonBody: { message } });
  return response.json();
}
