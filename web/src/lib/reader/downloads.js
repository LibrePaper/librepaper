// What of a document's rendering can be downloaded, and how the page it is
// downloaded from is made self-contained.
//
// A document renders to one kind of output: LaTeX and Typst to a paged PDF,
// Markdown, Quarto and authored HTML to flow HTML (`renderers.outputKind`).
// The File menu offers the download for that kind and no other, and offers
// it only once there is something to download -- a PDF this browser has
// been handed or the server is known to hold, an HTML page this browser has
// painted or is itself the source of. Nothing here touches the DOM; the
// Reader supplies what it knows and this decides.

/// Which rendering downloads the File menu can honour right now.
///
/// - `outputKind`: "pdf" or "html", from `renderers.outputKind`.
/// - `deliveredKind`: the kind of the payload the frame was last handed, or
///   "" since the last navigation.
/// - `rendering`: what `/renderings/latest` answered -- `{ sha, missing? }`
///   -- or null. A stored PDF can be fetched even before the frame has it.
/// - `displayedFormat`: the source format on screen; authored HTML is its
///   own rendering, so it is always downloadable.
export function availableDownloads({ outputKind, deliveredKind = "", rendering = null, displayedFormat = "" }) {
  if (outputKind === "pdf") {
    return { pdf: deliveredKind === "pdf" || Boolean(rendering && rendering.sha && !rendering.missing), html: false };
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
