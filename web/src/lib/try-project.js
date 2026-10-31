// Loads the built-in article template in each demo format and seeds an empty
// session with the result. The renaming and the welcome notes are in
// try-files.js, which node can test.
import { fillTemplate } from "./starter.js";
import { templateFiles } from "./templates.js";
import { buildTryFiles } from "./try-files.js";

export { TRY_DOCUMENT_URL, TRY_SLUG } from "./try-files.js";

const DEMO_TITLE = "Try LibrePaper";
const DEMO_AUTHOR = "A. Visitor";
const DEMO_FORMATS = ["typst", "latex", "markdown", "html"];

/** The starter files: [{ path, text }]. */
export async function tryFiles() {
  const byFormat = {};
  for (const formatId of DEMO_FORMATS) {
    const files = await templateFiles("article", formatId);
    byFormat[formatId] = fillTemplate(files, formatId, { title: DEMO_TITLE, author: DEMO_AUTHOR });
  }
  return buildTryFiles(byFormat);
}

/** Writes the starter files into an empty session. Returns the main path. */
export async function seedTryProject(session) {
  for (const { path, text } of await tryFiles()) {
    if (session.idOf(path)) continue;
    session.addText(path, text);
  }
  return "main.typ";
}
