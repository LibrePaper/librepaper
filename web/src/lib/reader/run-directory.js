// What the Run directory adds to a Quarto render on this computer: the notices
// the desktop app sends back with a job, and the hint a missing data file gets.
import { m } from "../paraglide/messages.js";
import { storageBytes } from "../quota-preferences.js";

// The wording R, Python, Julia and the shell use when a file is not where the
// code looked for it.
const MISSING_FILE = /cannot open file|No such file or directory|Cannot find the file|FileNotFoundError|does not exist in current working directory/i;

/** Whether a Quarto log says some file could not be found. @param {string} [log] */
export function logMissesAFile(log) {
  return MISSING_FILE.test(String(log || ""));
}

/** The project files a log says could not be found: the quoted names on
 * its missing-file lines that are paths in the project. Code runs in the
 * Run directory, so a file that is only in LibrePaper is not there.
 * @param {string} [log] @param {Iterable<string>} [projectPaths]
 * @returns {string[]} */
export function missingProjectFiles(log, projectPaths) {
  const known = new Set(projectPaths || []);
  const found = [];
  if (!known.size) return found;
  for (const line of String(log || "").split(/\r?\n/)) {
    if (!MISSING_FILE.test(line)) continue;
    for (const match of line.matchAll(/'([^']+)'|"([^"]+)"/g)) {
      const path = (match[1] ?? match[2]).replace(/\\/g, "/").replace(/^\.\//, "");
      if (known.has(path) && !found.includes(path)) found.push(path);
    }
  }
  return found;
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
 * @param {string | null} [runDirectoryPath] The Run directory, when it is known.
 * @param {string[]} [projectPaths] The paths of the project's files. */
export function runDirectoryDiagnostics(result, runDirectoryPath = null, projectPaths = []) {
  if (result.format !== "quarto") return [];
  const items = noticeDiagnostics(result.notices);
  if (result.ok === false && runDirectoryPath && logMissesAFile(result.log)) {
    for (const path of missingProjectFiles(result.log, projectPaths)) {
      items.push({ severity: "warning", message: m.preview_render_run_directory_project_only({ path }), source: "local-app" });
    }
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

// What a Run directory holds and a project does not: scripts and data files.
const LOCAL_ONLY = new Set([".r", ".py", ".jl", ".ipynb", ".csv", ".tsv", ".rds", ".rda", ".rdata", ".parquet", ".feather", ".xlsx", ".xls", ".dta", ".sav"]);

/** Whether a path names a script or data file, which belongs in the Run directory.
 * @param {string} [path] */
export function belongsInRunDirectory(path) {
  const name = String(path || "");
  const dot = name.lastIndexOf(".");
  return dot > name.lastIndexOf("/") && LOCAL_ONLY.has(name.slice(dot).toLowerCase());
}

/** The line under Share results: what happened to the last shared run, or ""
 * when there is nothing to say.
 * @param {{ share?: boolean, pdfOutput?: boolean, shareStatus?: { kind?: string, bytes?: number, reason?: string } }} [input]
 * @returns {string} */
export function shareLineOf({ share = true, pdfOutput = false, shareStatus = { kind: "idle" } } = {}) {
  if (!share) return "";
  if (pdfOutput) return m.rundirectorycontrol_html_only();
  if (shareStatus.kind === "shared") return m.rundirectorycontrol_shared({ size: storageBytes(shareStatus.bytes ?? 0) });
  if (shareStatus.kind === "failed") {
    if (shareStatus.reason === "storage") return m.rundirectorycontrol_not_shared_full();
    if (shareStatus.reason === "unmatched") return m.rundirectorycontrol_not_shared_unmatched();
    return m.rundirectorycontrol_not_shared_failed();
  }
  return "";
}
