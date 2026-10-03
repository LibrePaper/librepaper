// The built-in templates: files under src/templates, one directory per
// template and one per format inside it. This is the half of the template code
// that needs Vite (`import.meta.glob`); the pure half, which node tests, is
// starter.js.
//
// The metadata is bundled, because the picker draws from it at once. The
// files are not: they are fetched as separate chunks when a template is
// actually chosen, so the sixty-odd files nobody picked are never downloaded.

import { FORMATS } from "./starter.js";

/** @type {Record<string, {name: string, description?: string, keywords?: string[]}>} */
const metadata = import.meta.glob("../templates/*/template.json", { eager: true, import: "default" });
/** @type {Record<string, () => Promise<string>>} */
const sources = import.meta.glob(["../templates/*/*/**", "!../templates/*/template.json"], { query: "?raw", import: "default" });

// Blank leads because it is what "New project" meant before there were
// templates; the rest are in the order somebody scanning for one expects.
const ORDER = ["blank", "article", "book", "letter", "cv", "homework"];

// "../templates/<id>/<format>/<path...>"
const parts = (key) => key.slice("../templates/".length).split("/");

const formatsOf = (id) => FORMATS
  .map((format) => format.id)
  .filter((formatId) => Object.keys(sources).some((key) => {
    const [owner, directory] = parts(key);
    return owner === id && directory === formatId;
  }));

export const BUILT_IN = Object.entries(metadata)
  .map(([key, meta]) => {
    const id = parts(key)[0];
    return { id, name: meta.name, description: meta.description ?? "", keywords: meta.keywords ?? [], formats: formatsOf(id) };
  })
  .sort((a, b) => {
    const rank = (id) => (ORDER.includes(id) ? ORDER.indexOf(id) : ORDER.length);
    return rank(a.id) - rank(b.id) || a.id.localeCompare(b.id);
  });

/// A template's files for one format: [{ path, text }], paths relative to the
/// format's directory (main.typ, chapters/01-introduction.typ).
export async function templateFiles(id, formatId) {
  const keys = Object.keys(sources).filter((key) => {
    const [owner, directory] = parts(key);
    return owner === id && directory === formatId;
  });
  return Promise.all(keys.map(async (key) => ({
    path: parts(key).slice(2).join("/"),
    text: await sources[key](),
  })));
}
