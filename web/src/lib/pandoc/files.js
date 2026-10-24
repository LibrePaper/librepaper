// Pandoc reads a project through a WASI file system that lives in the
// worker's memory. These helpers decide which project paths may enter it and
// lay them out as folders, so the conversion sees the same tree the editor
// does and a relative image or .bib path resolves as it would on disk.

// The files the Pandoc module itself reads and writes at the root, and the one
// the client names for the converted result. A project file may not shadow them.
export const OUTPUT_NAME = ".librepaper-pandoc-output";
export const RESERVED = ["stdin", "stdout", "stderr", "warnings", OUTPUT_NAME];

/// The folders and file name of a project path. Throws on a path that could
/// leave the project or collide with the module's own files.
export function splitPath(path) {
  if (typeof path !== "string" || !path) throw new Error("Pandoc file path is empty");
  if (path.startsWith("/") || path.includes("\\")) throw new Error(`Pandoc file path ${JSON.stringify(path)} must be relative with forward slashes`);
  const segments = path.split("/");
  if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
    throw new Error(`Pandoc file path ${JSON.stringify(path)} has an empty, "." or ".." segment`);
  }
  if (RESERVED.includes(segments[0])) throw new Error(`Pandoc file path ${JSON.stringify(path)} is reserved`);
  return segments;
}

/// Bytes of a project file: strings are UTF-8, bytes are used as they are.
export function fileBytes(value) {
  if (typeof value === "string") return new TextEncoder().encode(value);
  if (value instanceof Uint8Array) return value;
  throw new Error("Pandoc file contents must be a string or Uint8Array");
}

/// Replaces everything in `root` (a Map of name to File or Directory) with the
/// project `files`. `makeFile(bytes)` and `makeDirectory(map)` build the nodes,
/// so this stays independent of the WASI library.
export function fillTree(root, files, makeFile, makeDirectory) {
  root.clear();
  for (const [path, value] of Object.entries(files || {})) {
    const segments = splitPath(path);
    const name = segments.pop();
    let folder = root;
    for (const segment of segments) {
      let next = folder.get(segment);
      if (!next) {
        next = makeDirectory(new Map());
        folder.set(segment, next);
      }
      if (!(next.contents instanceof Map)) throw new Error(`Pandoc file path ${JSON.stringify(path)} uses a file as a folder`);
      folder = next.contents;
    }
    if (folder.get(name)?.contents instanceof Map) throw new Error(`Pandoc file path ${JSON.stringify(path)} is also a folder`);
    folder.set(name, makeFile(fileBytes(value)));
  }
}
