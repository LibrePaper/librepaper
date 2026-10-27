// Folder projects of the desktop app: a folder on this computer opened as a
// project. The absolute path never crosses the wire; a project is its id.
import { send } from "./client.js";

/** Show the native folder chooser and open the chosen folder as a project.
 * A refusal (for example a chooser that is already open) throws with the
 * service's message. Resolves to null when the person cancels.
 * @returns {Promise<{ id: string, project: string, name: string, root_name: string } | null>} */
export async function openFolderProject() {
  const response = await send("POST", "projects/open");
  if (response.status === 204) return null;
  return response.json();
}

/** Show the native folder chooser and keep a web project in the chosen empty
 * folder or in a clone of the repository: the desktop app links the folder,
 * pulls the project into it and opens it as a folder project. Resolves to null
 * when the person cancels. A folder that is neither empty nor a git repository
 * is refused with "choose an empty folder or a git repository".
 *
 * When the app has no sign-in to pull with, it answers 401 after the folder
 * was created and linked. The folder project is real, so that answer resolves
 * with the project and `refused: true` instead of throwing; its Git panel
 * offers Sign in.
 * @param {string} url the project page, `https://<site>/docs/<slug>`
 * @returns {Promise<({ id: string, project: string, name: string, root_name: string } & { refused?: true }) | null>} */
export function keepFolderProject(url) {
  return answerOrRefusal("projects/keep", { url });
}

/** Sends a request that makes a folder project. An error whose body carries an
 * `id` means the folder project exists and only the pull was refused, so it
 * resolves with `refused: true`. A folder linked to another project is refused
 * outright, since opening it would show that other project. Any other error
 * throws.
 * @param {string} path
 * @param {Record<string, string>} body
 * @returns {Promise<any>} */
async function answerOrRefusal(path, body) {
  let response;
  try {
    response = await send("POST", path, { jsonBody: body });
  } catch (error) {
    const failure = /** @type {any} */ (error)?.body;
    if (typeof failure?.id === "string" && failure.reason !== "linked") return { ...failure, refused: true };
    throw error;
  }
  if (response.status === 204) return null;
  return response.json();
}

/** Where a folder project opens. @param {string} id */
export const folderProjectHref = (id) => `/reader.html?local=${encodeURIComponent(id)}`;

/** The folder projects, most recently opened first.
 * @returns {Promise<{ id: string, project: string, name: string, opened_at: string, present: boolean }[]>} */
export async function folderProjects() {
  const response = await send("GET", "projects");
  return response.json();
}

/** Forget a folder project. The files in the folder are untouched.
 * @param {string} id */
export async function forgetFolderProject(id) {
  await send("DELETE", `projects/${encodeURIComponent(id)}`);
}
