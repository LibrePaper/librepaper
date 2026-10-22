// Which files of a repository are project files.
//
// A synced file is a blob under the linked folder, with an extension the
// deployment accepts, not named by `.librepaperignore`, and not over the size
// limit. Everything else is left alone in both directions. The ignore files
// only ever remove: a path the built-in rules refuse stays refused whatever
// `!` lines say.

import ignore from "ignore";

import { checkPath, collisionKey, normalisePath } from "../paths.js";

const encoder = new TextEncoder();

const SYMLINK_MODE = "120000";

/// Folders that hold an environment rather than the paper. Their scripts have
/// supported extensions, so the extension rules alone would sync them.
const ENVIRONMENT_FOLDERS = new Set(["renv"]);

function ignorer(text) {
  return text ? ignore().add(text) : null;
}

/// The rules for one linked folder. `rules` is the deployment's path rules as
/// `/api/config` serves them, `folder` is "" or a path without slashes at
/// either end, and `ignoreRoot` and `ignoreFolder` are the texts of
/// `.librepaperignore` at the repository root and in the folder, or null.
///
/// Returns `classify(entries)` for a recursive tree listing, `projectPath` and
/// `repoPath` to move between the two spellings of a path, and
/// `isSynced(projectPath)` for the same filter on a project path.
export function syncRules({ rules, folder, maxFileBytes, ignoreRoot, ignoreFolder }) {
  const prefix = folder ? `${folder}/` : "";
  const maxPath = rules?.max_path || 200;
  const maxFiles = rules?.max_files ?? 200;
  const rootIgnore = ignorer(ignoreRoot);
  const folderIgnore = ignorer(ignoreFolder);
  // Length is judged separately, because a long path is a refusal when the file
  // would otherwise be synced and nothing at all when it would not.
  const lenient = { ...rules, max_path: Infinity };

  function projectPath(repoPath) {
    return prefix && repoPath.startsWith(prefix) ? repoPath.slice(prefix.length) : repoPath;
  }

  function repoPath(path) {
    return prefix + path;
  }

  /// Why a project path is not a synced file, or "" when it is. Size is not
  /// part of this, since a project path carries none.
  function unsyncedReason(path) {
    const checked = checkPath(lenient, path);
    if (checked.error) return checked.error;
    if (path.split("/").some((segment) => ENVIRONMENT_FOLDERS.has(segment))) return "an R environment folder";
    if (folderIgnore?.ignores(path)) return "named by .librepaperignore";
    if (rootIgnore?.ignores(repoPath(path))) return "named by .librepaperignore";
    return "";
  }

  function isSynced(path) {
    return !unsyncedReason(path);
  }

  function classify(entries) {
    const found = [];
    const unsynced = [];
    for (const entry of entries) {
      const path = entry.path;
      if (entry.type !== "blob") continue;
      if (entry.mode === SYMLINK_MODE) {
        unsynced.push({ path, reason: "a symbolic link" });
        continue;
      }
      if (prefix && !path.startsWith(prefix)) {
        unsynced.push({ path, reason: "outside the linked folder" });
        continue;
      }
      const inner = projectPath(path);
      const reason = unsyncedReason(inner);
      if (reason) {
        unsynced.push({ path, reason });
        continue;
      }
      if (entry.size > maxFileBytes) {
        unsynced.push({ path, reason: "over the file size limit" });
        continue;
      }
      found.push({
        path,
        project: normalisePath(inner),
        kind: checkPath(lenient, inner).kind,
        blob: entry.sha,
        size: entry.size,
      });
    }

    const refusals = [];
    const groups = new Map();
    for (const item of found) {
      const key = collisionKey(item.project);
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(item);
    }
    const colliding = new Set();
    for (const group of groups.values()) {
      if (group.length < 2) continue;
      for (const item of group) colliding.add(item);
      refusals.push({
        reason: "these paths would be the same file in LibrePaper",
        paths: group.map((item) => item.path),
      });
    }
    const tooLong = found.filter((item) => encoder.encode(item.project).length > maxPath);
    if (tooLong.length) {
      refusals.push({
        reason: `a path may be at most ${maxPath} bytes`,
        paths: tooLong.map((item) => item.path),
      });
    }

    const kept = found.filter((item) => !colliding.has(item) && !tooLong.includes(item));
    if (kept.length > maxFiles) {
      refusals.push({
        reason: `the repository has ${kept.length} syncable files and a project may hold ${maxFiles}`,
        paths: [],
      });
    }

    const synced = new Map();
    const folders = new Map();
    for (const item of kept) {
      synced.set(item.project, {
        repoPath: item.path,
        blob: item.blob,
        size: item.size,
        kind: item.kind,
      });
      const slash = item.project.indexOf("/");
      const top = slash < 0 ? "." : item.project.slice(0, slash);
      folders.set(top, (folders.get(top) || 0) + 1);
    }
    return { synced, unsynced, refusals, folders };
  }

  return { classify, projectPath, repoPath, isSynced };
}
