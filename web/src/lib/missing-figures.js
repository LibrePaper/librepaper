// What stands in for a figure that is not there, and where a figure that is
// not a project file can come from.
//
// A figure a Quarto or Typst document draws from its author's Run directory is
// not in the project. It travels with the shared run results (`resultsFiles`,
// the `{ path, base64 }` entries of the last run). A figure that is in neither
// is drawn as a labelled grey box: the page, the PDF and the export all keep
// going, and the reader is told once which file is missing. Nothing here ever
// blocks a render.

import { normalizePath } from "./completion/paths.js";
import { sourceFormatOf } from "./formats.js";

/** @typedef {import("./figure-references.js").FigureReference} FigureReference */
/** @typedef {{ path: string, text?: string, base64?: string }} ResultsFile */

const PLACEHOLDER_BASE64 = {
  png: "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAAAAADhZOFXAAAAEElEQVQI12O8wAABTAwUMQA8OADgU39eYQAAAABJRU5ErkJggg==",
  jpeg: "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDABALDA4MChAODQ4SERATGCgaGBYWGDEjJR0oOjM9PDkzODdASFxOQERXRTc4UG1RV19iZ2hnPk1xeXBkeFxlZ2P/wAALCAAIAAgBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAABv/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AUP/Z",
  gif: "R0lGODlhCAAIAPAAANDQ0AAAACH5BAAAAAAALAAAAAAIAAgAAAIHhI+py+1dAAA7",
  webp: "UklGRh4AAABXRUJQVlA4TBEAAAAvB8ABAAdQ6EIXuv+BiOh/AAA=",
};

/** @param {string} base64 @returns {Uint8Array} */
function decodeBase64(base64) {
  const binary = atob(base64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

/** @param {string} path @returns {string} lower-case extension without the dot */
function extensionOf(path) {
  const match = /\.([^./]+)$/.exec(path.slice(path.lastIndexOf("/") + 1));
  return match ? match[1].toLowerCase() : "";
}

/** @param {string} path @returns {string} */
function mimeOf(path) {
  switch (extensionOf(path)) {
    case "png": return "image/png";
    case "jpg": case "jpeg": return "image/jpeg";
    case "gif": return "image/gif";
    case "webp": return "image/webp";
    case "svg": return "image/svg+xml";
    case "pdf": return "application/pdf";
    default: return "application/octet-stream";
  }
}

/** @param {string} text @returns {string} */
const escapeXml = (text) => text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;" })[c] ?? c);

/** @param {string} path @returns {string} */
function placeholderSvg(path) {
  const name = escapeXml(path.slice(path.lastIndexOf("/") + 1) || path);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" viewBox="0 0 320 180">`
    + `<rect width="320" height="180" fill="#e6e6e6" stroke="#9a9a9a" stroke-width="2" stroke-dasharray="6 4"/>`
    + `<path d="M0 0 L320 180 M320 0 L0 180" stroke="#cfcfcf" stroke-width="1"/>`
    + `<text x="160" y="94" text-anchor="middle" font-family="sans-serif" font-size="14" fill="#555555">${name}</text></svg>`;
}

/** A one-page PDF with a grey box, its cross-reference table computed. @returns {Uint8Array} */
function placeholderPdf() {
  const content = "0.9 g 0 0 200 120 re f";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 120] /Contents 4 0 R /Resources << >> >>",
    `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
  ];
  let out = "%PDF-1.4\n";
  const offsets = objects.map((body, i) => {
    const at = out.length;
    out += `${i + 1} 0 obj\n${body}\nendobj\n`;
    return at;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const at of offsets) out += `${String(at).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(out);
}

/**
 * Bytes of a valid stand-in image in the format the extension names, so a tool
 * that checks the format (TeX, Typst, Pandoc) accepts it. An unknown extension
 * gets a PNG.
 * @param {string} path
 * @returns {Uint8Array}
 */
export function placeholderBytes(path) {
  switch (extensionOf(path)) {
    case "jpg": case "jpeg": return decodeBase64(PLACEHOLDER_BASE64.jpeg);
    case "gif": return decodeBase64(PLACEHOLDER_BASE64.gif);
    case "webp": return decodeBase64(PLACEHOLDER_BASE64.webp);
    case "pdf": return placeholderPdf();
    case "svg": return new TextEncoder().encode(placeholderSvg(path));
    default: return decodeBase64(PLACEHOLDER_BASE64.png);
  }
}

/**
 * An SVG data URL drawing a neutral grey box labelled with the file name.
 * @param {string} path
 * @returns {string}
 */
export function placeholderUrl(path) {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(placeholderSvg(path))}`;
}

/**
 * The run figures among the results files: base64 entries outside `.calepin/`.
 * @param {ResultsFile[] | null | undefined} resultsFiles
 * @returns {Map<string, string>} normalised path to base64
 */
function runFigures(resultsFiles) {
  /** @type {Map<string, string>} */
  const out = new Map();
  for (const file of resultsFiles ?? []) {
    if (typeof file?.base64 !== "string" || typeof file.path !== "string") continue;
    const path = normalizePath(file.path);
    if (path === ".calepin" || path.startsWith(".calepin/")) continue;
    out.set(path, file.base64);
  }
  return out;
}

/**
 * A data URL for a figure of the last run, or null when the run has none at
 * this path. Used by visual and paper mode, which draw from project files
 * first.
 * @param {ResultsFile[] | null | undefined} resultsFiles
 * @param {string} path
 * @returns {string | null}
 */
export function resultsFigureUrl(resultsFiles, path) {
  const base64 = runFigures(resultsFiles).get(normalizePath(path));
  return base64 === undefined ? null : `data:${mimeOf(path)};base64,${base64}`;
}

/// What an earlier call put into a tree, which a project session may hand out
/// again on the next render. Taken out first, so a figure that has since become
/// a project file, or left the run, is not mistaken for one that is there.
/** @type {WeakMap<object, { assets: Map<string, unknown>, urls: Map<string, string> }>} */
const filled = new WeakMap();

const FLOW = new Set(["markdown", "quarto", "org", "html"]);

/**
 * Make every referenced figure resolvable in `source.assets` and
 * `source.urls`. A project file always wins; then a figure of the last run;
 * then a placeholder. A project file whose blob could not be fetched also gets
 * a placeholder, without being counted as missing. For the flow formats the
 * renderer looks a src up exactly as written, so the name as written is added
 * as an alias of the resolved path.
 * @param {{ main?: string, texts?: Record<string, string>, assets?: Record<string, Uint8Array>, urls?: Record<string, string>, digests?: Record<string, string> }} source
 * @param {{ references: FigureReference[], resultsFiles?: ResultsFile[] | null }} options
 * @returns {{ missing: FigureReference[] }}
 */
export function fillFigures(source, { references, resultsFiles }) {
  const assets = (source.assets ??= {});
  const urls = (source.urls ??= {});
  const earlier = filled.get(source);
  for (const [key, value] of earlier?.assets ?? []) if (assets[key] === value) delete assets[key];
  for (const [key, value] of earlier?.urls ?? []) if (urls[key] === value) delete urls[key];
  const added = { assets: new Map(), urls: new Map() };
  filled.set(source, added);
  /** @param {string} key @param {Uint8Array} bytes @param {string} url */
  const put = (key, bytes, url) => {
    assets[key] = bytes;
    urls[key] = url;
    added.assets.set(key, bytes);
    added.urls.set(key, url);
  };
  const run = runFigures(resultsFiles);
  const flow = FLOW.has(sourceFormatOf(source.main ?? ""));
  const latex = sourceFormatOf(source.main ?? "") === "latex";
  /** @type {FigureReference[]} */
  const missing = [];
  const reported = new Set();
  for (const reference of references) {
    let { path } = reference;
    const inProject = Object.hasOwn(assets, path) || Object.hasOwn(source.texts ?? {}, path);
    if (!inProject) {
      const base64 = run.get(path);
      const held = Object.hasOwn(source.digests ?? {}, path);
      // TeX guesses an extension for a bare name; give it one to find.
      if (latex && !extensionOf(path) && !held && base64 === undefined) path = `${path}.png`;
      if (base64 !== undefined && !held) {
        put(path, decodeBase64(base64), `data:${mimeOf(path)};base64,${base64}`);
      } else {
        put(path, placeholderBytes(path), placeholderUrl(path));
        const key = `${reference.path}\0${reference.file}\0${reference.line}`;
        if (!held && !reported.has(key)) {
          reported.add(key);
          missing.push({ path: reference.path, name: reference.name, file: reference.file, line: reference.line });
        }
      }
    }
    const alias = reference.name;
    if (flow && alias && alias !== path) {
      if (!Object.hasOwn(assets, alias) && Object.hasOwn(assets, path)) {
        assets[alias] = assets[path];
        added.assets.set(alias, assets[alias]);
      }
      if (!Object.hasOwn(urls, alias) && Object.hasOwn(urls, path)) {
        urls[alias] = urls[path];
        added.urls.set(alias, urls[alias]);
      }
    }
  }
  return { missing };
}

const IMAGE_EXTENSIONS = [".png", ".jpg", ".jpeg", ".svg", ".pdf"];

/**
 * The URL for an image a document names, for the editor's picture widgets: a
 * project file first (`imageUrl`, which may be async), then a figure of the
 * last run, trying the image extensions for a bare name. Null when neither has
 * it; the widget then draws `placeholderUrl`.
 * @param {(path: string) => Promise<string | null> | string | null} imageUrl
 * @param {(() => ResultsFile[] | null) | null | undefined} resultsFiles
 * @param {string} file project path, no leading slash
 * @returns {Promise<string | null>}
 */
export async function figureUrlFor(imageUrl, resultsFiles, file) {
  const names = /\.[^/]+$/.test(file) ? [file] : [file, ...IMAGE_EXTENSIONS.map((extension) => file + extension)];
  for (const name of names) {
    const url = await imageUrl(name);
    if (url) return url;
  }
  const files = resultsFiles?.() ?? null;
  for (const name of names) {
    const url = resultsFigureUrl(files, name);
    if (url) return url;
  }
  return null;
}
