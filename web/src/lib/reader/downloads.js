// What of a document's rendering can be downloaded, and how the page it is
// downloaded from is made self-contained.
//
// A document renders to one kind of output: LaTeX and Typst to a paged PDF,
// Markdown, Quarto and authored HTML to flow HTML (`renderers.outputKind`).
// The File menu offers the download for that kind and no other, and offers
// it only once there is something to download -- a PDF or HTML payload this
// browser has been handed, or authored HTML that is itself the source.
// Nothing here touches the DOM; the Reader supplies what it knows and this
// decides.

import { basename, inside } from "../file-manager.js";

/// Which rendering downloads the File menu can honour right now.
///
/// - `outputKind`: "pdf" or "html", from `renderers.outputKind`.
/// - `deliveredKind`: the kind of the payload the frame was last handed, or
///   "" since the last navigation.
/// - `displayedFormat`: the source format on screen; authored HTML is its
///   own rendering, so it is always downloadable.
export function availableDownloads({ outputKind, deliveredKind = "", displayedFormat = "" }) {
  if (outputKind === "pdf") {
    return { pdf: deliveredKind === "pdf", html: false };
  }
  if (outputKind === "html") {
    return { pdf: false, html: deliveredKind === "html" || displayedFormat === "html" };
  }
  return { pdf: false, html: false };
}

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

function toBase64(bytes) {
  if (typeof Buffer !== "undefined") return Buffer.from(bytes).toString("base64");
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return btoa(binary);
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

/// The rendering to export, from what this tab is holding and nothing else.
/// LibrePaper never fetches or uploads a generated result for an export.
///
/// `authored` says the document's own source *is* the rendering, which is
/// true of authored HTML and of nothing else; its text is then exported
/// verbatim rather than the painted page, and a document whose text could not
/// be read is an error rather than a silent fall back to the frame.
export async function renderingFile({ kind, slug, preview, authored = false, authoredHtml = null }) {
  if (kind === "pdf") {
    const bytes = preview?.kind === "pdf" ? preview.bytes : null;
    if (!bytes) throw new Error("Render the document before exporting its PDF.");
    return { blob: new Blob([bytes], { type: "application/pdf" }), name: `${slug}.pdf` };
  }
  const html = authored
    ? authoredHtml
    : preview?.kind === "html" ? await inlineBlobUrls(preview.html) : null;
  if (typeof html !== "string") throw new Error("Render the document before exporting its HTML.");
  return { blob: new Blob([html], { type: "text/html" }), name: `${slug}.html` };
}

/// The archive itself. `zip` is loaded when it is asked for: a reader who
/// never downloads a document should not carry the code that would build one.
export async function archive(files) {
  const { zip } = await import("../zip.js");
  return zip(files);
}
