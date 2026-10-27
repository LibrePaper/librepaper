// A web project's connection to a git repository, through the `connections`
// routes of the desktop app's local service. The desktop app keeps the clone
// on this computer, holds the sign-in and runs the commands; this file only
// sends. A refusal is a `Refused` error carrying `status` and the parsed
// `body`, which is where a pull says which paths changed and a sign-in
// failure says what git reported.
import { send } from "./client.js";

const route = (step = "") => `connections${step ? `/${step}` : ""}`;

// A project is named by its page address, `https://<site>/docs/<slug>`, in the
// query of the status and in the body of every other route.
const forProject = (project) => `${route()}?project=${encodeURIComponent(project)}`;

/** Where the project stands with its repository. Until it is connected the
 * answer has no repository in it.
 * @param {string} project
 * @returns {Promise<{ connected: false, signed_in: boolean } | { connected: true, signed_in: boolean, url: string, branch: string, merging: null | { side: "project" | "repository", conflicts: string[] } }>} */
export async function connectionStatus(project) {
  const response = await send("GET", forProject(project));
  return response.json();
}

/** Connects the project to a repository: the desktop app clones it and links
 * the clone to the project.
 * @param {string} project @param {string} url */
export async function connectRepository(project, url) {
  const response = await send("POST", route(), { jsonBody: { project, url } });
  return response.json();
}

/** Takes in the repository's changes as named versions of the project. A
 * 409 with `reason: "pending"` means an earlier send to the project was
 * interrupted and `retryConnection` settles it first.
 * @param {string} project
 * @returns {Promise<{ status: object, pushed: { files: number } | { nothing: true } } | { stopped: "conflicts", status: object }>} */
export async function pullConnection(project) {
  const response = await send("POST", route("pull"), { jsonBody: { project } });
  return response.json();
}

/** Sends the project's named versions to the repository.
 * @param {string} project
 * @returns {Promise<{ status: object, pushed: "pushed" | "up-to-date" } | { stopped: "conflicts", status: object }>} */
export async function pushConnection(project) {
  const response = await send("POST", route("push"), { jsonBody: { project } });
  return response.json();
}

/** Settles a send to the project that was interrupted, and sends what the
 * copy changed since. It runs only that step: no pull comes first, because a
 * pull is refused while the send is unsettled.
 * @param {string} project
 * @returns {Promise<{ status: object, pushed: { files: number } | { nothing: true } }>} */
export async function retryConnection(project) {
  const response = await send("POST", route("retry"), { jsonBody: { project } });
  return response.json();
}

/** Keeps one side of a file that changed on both sides during a merge.
 * @param {string} project @param {string} path @param {"project" | "repository"} keep */
export async function resolveConnection(project, path, keep) {
  const response = await send("POST", route("resolve"), { jsonBody: { project, path, keep } });
  return response.json();
}

/** Forgets the copy on this computer. The repository and the project stay as
 * they are.
 * @param {string} project */
export async function disconnectConnection(project) {
  const response = await send("POST", route("disconnect"), { jsonBody: { project } });
  return response.json();
}

/** Starts signing in to the web server. The desktop app answers at once with
 * a code to approve.
 * @param {string} project
 * @returns {Promise<{ authorization_id: string, user_code: string, verification_url?: string }>} */
export async function loginConnection(project) {
  const response = await send("POST", route("login"), { jsonBody: { project } });
  return response.json();
}

/** Finishes the sign-in once the code is approved. While the code is still
 * pending the desktop app refuses with a 400.
 * @param {string} project @param {string} authorizationId */
export async function loginConnectionComplete(project, authorizationId) {
  const response = await send("POST", route("login/complete"), { jsonBody: { project, authorization_id: authorizationId } });
  return response.json();
}
