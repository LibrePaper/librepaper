// Reading a folder project through the desktop app: its tree, one file, the
// outside changes, and its name. The routes are the `projects/<id>` ones of
// the desktop app's local service.
import { send } from "./client.js";

const segment = (value) => encodeURIComponent(String(value));
const filePath = (path) => String(path).split("/").map(segment).join("/");

/** The project's files and what was left out of the listing. `extensions` are
 * the file extensions the project's rules name; the desktop app leaves every
 * other file out of the listing.
 * @param {string} id @param {string[]} [extensions]
 * @returns {Promise<{ cursor: string | number, files: Array<{ path: string, size: number, sha256: string, mtime: number }>, skipped: Array<{ path: string, reason: string }> }>} */
export async function projectTree(id, extensions = []) {
  const wanted = [...new Set(extensions.map((one) => String(one).replace(/^\./, "").toLowerCase()).filter(Boolean))];
  const query = wanted.length ? `?extensions=${wanted.map(segment).join(",")}` : "";
  const response = await send("GET", `projects/${segment(id)}/tree${query}`);
  return response.json();
}

/** The bytes of one file of the project.
 * @param {string} id @param {string} path */
export async function projectFile(id, path) {
  const response = await send("GET", `projects/${segment(id)}/files/${filePath(path)}`);
  return new Uint8Array(await response.arrayBuffer());
}

/** A long poll for outside changes, from the cursor of the latest tree read.
 * `reset` says the desktop app lost the thread and the tree has
 * to be read again.
 * @param {string} id @param {string | number} since @param {{ signal?: AbortSignal }} [options]
 * @returns {Promise<{ cursor: string | number, reset?: boolean, changes: Array<{ path: string, kind: "changed" | "added" | "removed", sha256?: string, size?: number }> }>} */
export async function projectChanges(id, since, { signal } = {}) {
  const response = await send("GET", `projects/${segment(id)}/changes?since=${segment(since)}`, { signal });
  return response.json();
}

/** The folder's name as the desktop app lists it, or an empty string when the
 * folder is not listed.
 * @param {string} id */
export async function projectName(id) {
  const response = await send("GET", "projects");
  const list = await response.json();
  return list.find((project) => project.id === id)?.name || "";
}
