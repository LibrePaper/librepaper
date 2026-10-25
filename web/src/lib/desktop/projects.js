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
