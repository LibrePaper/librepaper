// A folder project's meeting with its web project, through the `projects/<id>/sync`
// routes of the desktop app's local service (SPEC-sync.md). The desktop app
// holds the sign-in and runs the two commands, pull and push; this file only
// sends. A refusal is a `Refused` error carrying `status` and the parsed
// `body`, which is where a push says which paths changed.
import { send } from "./client.js";

const route = (id, step = "") => `projects/${encodeURIComponent(String(id))}/sync${step ? `/${step}` : ""}`;

/** Where the folder stands with its web project.
 * @param {string} id
 * @returns {Promise<{ signed_in: boolean, linked: boolean, site?: string, slug?: string, pulled?: number, changes?: { here: number, web: boolean } }>} */
export async function syncStatus(id) {
  const response = await send("GET", route(id));
  return response.json();
}

/** Signs in to the web server through the system browser. Resolves once the
 * sign-in has finished.
 * @param {string} id */
export async function syncLogin(id) {
  await send("POST", route(id, "login"));
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
 * changed on both sides, one path at a time.
 * @param {string} id @param {{ mine?: string[], theirs?: string[] }} [choices]
 * @returns {Promise<{ written: number | string[], trashed: number | string[], kept?: unknown, conflicts: string[], trimmed?: boolean }>} */
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
