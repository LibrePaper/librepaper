import { checkDirectoryPath, checkPath, collisionKey, normalisePath } from "./paths.js";

export const basename = (path) => path.split("/").at(-1);
export const parentPath = (path) => path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
export const inside = (path, folder) => path.startsWith(`${folder}/`);
export const nodeKey = (entry) => `${entry.kind}:${entry.id ?? entry.path}`;

export function folderPaths(files, explicit = []) {
  const folders = new Set(explicit);
  for (const entry of [...files, ...explicit.map((path) => ({ path }))]) {
    let path = parentPath(entry.path);
    while (path) {
      folders.add(path);
      path = parentPath(path);
    }
  }
  return [...folders].sort((a, b) => a.localeCompare(b));
}

export function fileTree(files, folders = []) {
  const root = { id: "root", path: "", name: "Project", children: [] };
  const directories = new Map([["", root]]);
  for (const path of folderPaths(files, folders)) {
    directories.set(path, { id: `folder:${path}`, path, kind: "folder", name: basename(path), children: [] });
  }
  for (const [path, node] of directories) {
    if (path) directories.get(parentPath(path)).children.push(node);
  }
  for (const file of files) {
    directories.get(parentPath(file.path)).children.push({ ...file, id: nodeKey(file), fileId: file.id, name: basename(file.path) });
  }
  for (const node of directories.values()) {
    node.children.sort((a, b) => Number(b.kind === "folder") - Number(a.kind === "folder") || a.name.localeCompare(b.name));
  }
  return root;
}

// A selected folder already includes its selected descendants.
export function topEntries(entries) {
  return entries.filter((entry, index) => entries.findIndex((other) => other.kind === entry.kind && other.path === entry.path) === index
    && !entries.some((other) => other.kind === "folder" && inside(entry.path, other.path)));
}

export function copyPath(path, files, folders) {
  const dot = path.lastIndexOf(".");
  const split = dot > path.lastIndexOf("/") ? dot : path.length;
  const taken = new Set([...files.map((file) => file.path), ...folderPaths(files, folders)].map(collisionKey));
  for (let n = 2; ; n++) {
    const candidate = `${path.slice(0, split)} (${n})${path.slice(split)}`;
    if (!taken.has(collisionKey(candidate))) return candidate;
  }
}

// Capture entries before yielding: browsers release the drag data after drop.
export async function droppedFiles(transfer) {
  const entries = [...(transfer.items || [])].filter((item) => item.kind === "file").map((item) => item.webkitGetAsEntry?.());
  const fallback = [...(transfer.files || [])];
  const result = { files: [], folders: [] };
  async function visit(entry, prefix = "") {
    const path = prefix + entry.name;
    if (entry.isFile) {
      const file = await new Promise((resolve, reject) => entry.file(resolve, reject));
      result.files.push({ file, path });
    } else if (entry.isDirectory) {
      result.folders.push(path);
      const reader = entry.createReader();
      for (;;) {
        const children = await new Promise((resolve, reject) => reader.readEntries(resolve, reject));
        if (!children.length) break;
        for (const child of children) await visit(child, `${path}/`);
      }
    }
  }
  if (entries.length && entries.every(Boolean)) {
    for (const entry of entries) await visit(entry);
  } else result.files = fallback.map((file) => ({ file, path: file.webkitRelativePath || file.name }));
  return result;
}

export function checkPlacement(rules, entry, files, folders = []) {
  const path = normalisePath(entry.path);
  const answer = entry.kind === "folder" ? { error: checkDirectoryPath(rules, path) } : checkPath(rules, path);
  if (answer.error) throw new Error(answer.error);
  if (entry.kind !== "folder" && answer.kind !== entry.kind) throw new Error("Renaming cannot change a text into a figure or a figure into a text.");
  const key = collisionKey(path);
  const dirs = folderPaths(files, folders);
  if (files.some((file) => collisionKey(file.path) === key) || dirs.some((dir) => collisionKey(dir) === key)) {
    throw new Error(`${path}: there is already a file or folder with that name`);
  }
  let parent = parentPath(path);
  while (parent) {
    if (files.some((file) => collisionKey(file.path) === collisionKey(parent))) throw new Error(`${parent}: a file cannot contain other files`);
    if (dirs.some((dir) => collisionKey(dir) === collisionKey(parent) && dir !== parent)) throw new Error(`${parent}: use the existing folder's capitalization`);
    parent = parentPath(parent);
  }
  return path;
}

// Validate the complete destination namespace before changing any shared maps.
export function relocation(files, explicit, entries, destination, rules, rename = false) {
  const roots = topEntries(entries);
  const folders = folderPaths(files, explicit);
  for (const entry of roots) {
    const exists = entry.kind === "folder" ? folders.includes(entry.path) : files.some((file) => file.id === entry.id && file.kind === entry.kind && file.path === entry.path);
    if (!exists) throw new Error(`${entry.path}: this item changed or was removed. Select it again.`);
  }
  const moves = roots.map((entry) => ({ entry, path: normalisePath(rename ? destination : [destination, basename(entry.path)].filter(Boolean).join("/")) }));
  for (const { entry, path } of moves) {
    if (entry.kind === "folder" && inside(collisionKey(path), collisionKey(entry.path))) throw new Error("A folder cannot be moved inside itself.");
  }
  const owner = (path) => moves.find(({ entry }) => path === entry.path || (entry.kind === "folder" && inside(path, entry.path)));
  const changedFiles = files.filter((file) => owner(file.path));
  const changedFolders = folders.filter((path) => owner(path));
  const remainingFiles = files.filter((file) => !owner(file.path));
  const remainingFolders = folders.filter((path) => !owner(path));
  const target = (path) => {
    const move = owner(path);
    return move.path + path.slice(move.entry.path.length);
  };
  // Roots must not merge with an existing directory, even if its files differ.
  for (const { entry, path } of moves) checkPlacement(rules, { ...entry, path }, remainingFiles, remainingFolders);
  const outputFolders = changedFolders.map(target).sort((a, b) => a.split("/").length - b.split("/").length);
  for (const path of outputFolders) {
    checkPlacement(rules, { kind: "folder", path }, remainingFiles, remainingFolders);
    remainingFolders.push(path);
  }
  const outputFiles = changedFiles.map((file) => ({ ...file, previousPath: file.path, path: target(file.path) }));
  for (const file of outputFiles) {
    checkPlacement(rules, file, remainingFiles, remainingFolders);
    remainingFiles.push(file);
  }
  return { files: outputFiles, oldFolders: changedFolders, folders: outputFolders, parents: roots.map((entry) => parentPath(entry.path)).filter(Boolean) };
}
