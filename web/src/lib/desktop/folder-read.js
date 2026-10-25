// Reading a folder project through the desktop app: its tree, one file, the
// outside changes, and its name. The routes are the `projects/<id>` ones of
// the desktop app's local service (SPEC-local-folders.md).
import { send } from "./client.js";

const segment = (value) => encodeURIComponent(String(value));
const filePath = (path) => String(path).split("/").map(segment).join("/");

/** The project's files and what was left out of the listing.
 * @param {string} id
 * @returns {Promise<{ files: Array<{ path: string, size: number, sha256: string, mtime: number }>, skipped: Array<{ path: string, reason: string }> }>} */
export async function projectTree(id) {
  const response = await send("GET", `projects/${segment(id)}/tree`);
  return response.json();
}

/** The bytes of one file of the project.
 * @param {string} id @param {string} path */
export async function projectFile(id, path) {
  const response = await send("GET", `projects/${segment(id)}/files/${filePath(path)}`);
  return new Uint8Array(await response.arrayBuffer());
}

/** A long poll for outside changes. Without `since` it answers at once with the
 * current cursor. `reset` says the desktop app lost the thread and the tree has
 * to be read again.
 * @param {string} id @param {string} [since] @param {{ signal?: AbortSignal }} [options]
 * @returns {Promise<{ cursor: string, reset?: boolean, changes: Array<{ path: string, kind: "changed" | "added" | "removed", sha256?: string, size?: number }> }>} */
export async function projectChanges(id, since, { signal } = {}) {
  const query = since === undefined || since === "" ? "" : `?since=${segment(since)}`;
  const response = await send("GET", `projects/${segment(id)}/changes${query}`, { signal });
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
