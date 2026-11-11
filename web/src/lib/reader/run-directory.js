// What the Run directory adds to a Quarto render on this computer: the notices
// the desktop app sends back with a job, and the hint a missing data file gets.
import { m } from "../paraglide/messages.js";

// The wording R, Python, Julia and the shell use when a file is not where the
// code looked for it.
const MISSING_FILE = /cannot open file|No such file or directory|Cannot find the file|FileNotFoundError/i;

/** Whether a Quarto log says some file could not be found. @param {string} [log] */
export function logMissesAFile(log) {
  return MISSING_FILE.test(String(log || ""));
}

/** The warnings a job's notices become.
 * @param {{ kind?: string, path?: string }[]} [notices]
 * @returns {{ severity: "warning", message: string, source: "local-app" }[]} */
export function noticeDiagnostics(notices) {
  const items = [];
  for (const notice of notices || []) {
    const path = String(notice?.path || "");
    if (!path) continue;
    if (notice.kind === "clash") items.push(m.preview_render_run_directory_clash({ path }));
    else if (notice.kind === "not-linked") items.push(m.preview_render_run_directory_not_linked({ path }));
  }
  return items.map((message) => ({ severity: "warning", message, source: "local-app" }));
}

/** Every Run directory diagnostic for one local render.
 * @param {{ format: string, ok?: boolean, log?: string, notices?: { kind?: string, path?: string }[] }} result
 * @param {string | null} [runDirectoryPath] The Run directory, when it is known. */
export function runDirectoryDiagnostics(result, runDirectoryPath = null) {
  if (result.format !== "quarto") return [];
  const items = noticeDiagnostics(result.notices);
  if (result.ok === false && runDirectoryPath && logMissesAFile(result.log)) {
    items.push({ severity: "warning", message: m.preview_render_run_directory_missing_file({ path: runDirectoryPath }), source: "local-app" });
  }
  return items;
}

/** The home directory, read off the suggested folder, which is <home>/LibrePaper/<project>.
 * @param {string} [suggested] */
export function homeOf(suggested) {
  const found = /^(.*)[\\/]LibrePaper[\\/][^\\/]+[\\/]?$/.exec(suggested || "");
  return found?.[1] || "";
}

/** A path as shown: the home directory is `~`.
 * @param {string} [path] @param {string} [home] */
export function shorten(path, home = "") {
  if (!home || !path) return path || "";
  if (path === home) return "~";
  return path.startsWith(home) && /[\\/]/.test(path[home.length]) ? "~" + path.slice(home.length) : path;
}

/** The last segment of a path. @param {string} path */
export function leaf(path) {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}

/** Which version of the Run directory page to show: the folder in use
 * ("current"), a stored folder that is gone ("missing"), a suggestion to
 * accept ("suggested"), or nothing to offer but the picker ("none").
 * @param {{ path?: string, exists?: boolean, suggested?: string } | null | undefined} status
 * @param {boolean} usable
 * @returns {"current" | "missing" | "suggested" | "none"} */
export function pageVariant(status, usable) {
  if (usable && status?.path) return "current";
  if (status?.path) return "missing";
  return status?.suggested ? "suggested" : "none";
}
