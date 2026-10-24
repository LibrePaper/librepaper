// How a download is made self-contained, and what the other downloads carry:
// the project's files, one selected entry, a Word file and the archive.
//
// Which renderings a document can be exported as, and how each is made, is
// `exports.js`. Nothing here touches the DOM; the Reader supplies what it
// knows and this decides.

import { basename, inside } from "../file-manager.js";
import { toBase64 } from "../figures.js";

/// A page painted in this browser names its figures by object URL
/// (`figures.gather`), which is dead the moment the page leaves the tab.
/// The download carries the bytes instead: every `blob:` URL is fetched and
/// written in place as a data URL. One that cannot be fetched is left as it
/// was -- a broken image is what a missing figure looks like everywhere.
export async function inlineBlobUrls(html, fetcher = fetch) {
  const pattern = /blob:[^"'\s)>]+/g;
  const found = [...new Set(html.match(pattern) || [])];
  if (!found.length) return html;
  const replacements = await Promise.all(
    found.map(async (url) => {
      try {
        const response = await fetcher(url.split("#")[0]);
        if (!response.ok) return null;
        const type = response.headers?.get?.("content-type") || "application/octet-stream";
        const bytes = new Uint8Array(await response.arrayBuffer());
        return [url, `data:${type};base64,${toBase64(bytes)}`];
      } catch {
        return null;
      }
    }),
  );
  let out = html;
  for (const pair of replacements) {
    if (!pair) continue;
    const [url, data] = pair;
    out = out.split(url).join(data);
  }
  return out;
}

/// Hand the browser a file to save. The object URL is revoked on a later
/// turn: revoking it now would race the download the click has only started.
export function saveBlob(blob, name) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/// The files of the whole project, ready to be archived.
///
/// `tree` and `folders` arrive as values rather than as getters, and that is
/// the point: the directory has to be snapshotted before the assets are
/// fetched, or a folder added during the fetch lands in an archive cut from
/// an older tree. A caller that passes what it has already read cannot get
/// that order wrong.
///
/// `gather` is the Reader's figure fetch: it answers `{ held, missing }`, and
/// a project archive tolerates no hole -- a missing figure fails the download
/// rather than producing a zip somebody discovers is incomplete later.
export async function projectFiles({ tree, folders, gather }) {
  const files = { ...tree.texts };
  for (const path of folders) files[`${path}/`] = new Uint8Array();
  if (Object.keys(tree.digests || {}).length) {
    const { held, missing } = await gather(tree.digests);
    if (missing.length) throw new Error(`could not download ${missing.join(", ")}`);
    Object.assign(files, held.assets);
  }
  return files;
}

/// What one selected file or folder downloads as: a map of files to archive,
/// or the bytes of a single file. The same snapshot rule as `projectFiles`.
export async function entryDownload({ entry, tree, folders, gather }) {
  const selected = (path) => (entry.kind === "folder" ? inside(path, entry.path) : path === entry.path);
  const content = Object.fromEntries(Object.entries(tree.texts).filter(([path]) => selected(path)));
  const digests = Object.fromEntries(Object.entries(tree.digests || {}).filter(([path]) => selected(path)));
  if (Object.keys(digests).length) {
    const { held, missing } = await gather(digests);
    if (missing.length) throw new Error("Could not download all selected files.");
    Object.assign(content, held.assets);
  }
  if (entry.kind === "folder") {
    for (const path of folders) if (path === entry.path || selected(path)) content[`${path}/`] = new Uint8Array();
    content[`${entry.path}/`] = new Uint8Array();
    return { name: `${basename(entry.path)}.zip`, files: content };
  }
  if (!(entry.path in content)) throw new Error("This file is no longer available.");
  return { name: basename(entry.path), bytes: content[entry.path] };
}

const DOCX_TYPE = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// The one-shot DOCX artifact this tab is holding, as a file to save.
export function docxFile(bytes, slug) {
  return { blob: new Blob([bytes], { type: DOCX_TYPE }), name: `${slug}.docx` };
}

/// The archive itself. `zip` is loaded when it is asked for: a reader who
/// never downloads a document should not carry the code that would build one.
export async function archive(files) {
  const { zip } = await import("../zip.js");
  return zip(files);
}
