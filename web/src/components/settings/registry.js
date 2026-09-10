// The map of the settings dialog: the categories, the group each sits in, and
// when each is offered. The dialog draws its navigation from this and searches
// it; what a category shows is a component beside this file.
//
// The groups are scopes. A setting either belongs to this browser, to the
// document everyone is editing, or to the LibrePaper app on this computer,
// and the question a person asks of every setting -- who else does this
// change? -- is answered once, by where it sits, rather than by a sentence
// under every control.

export const GROUPS = [
  { id: "browser", says: "This browser", note: "Applies only to this browser." },
  { id: "document", says: "This document", note: "Shared with everyone who edits this document." },
  { id: "computer", says: "This computer", note: "The LibrePaper app running on this computer." },
];

// `offered` answers with the document's format and whether this browser may
// edit it. `terms` are the words somebody might type when looking for a row
// and not finding its title.
const everyone = () => true;
const editor = ({ mayEdit }) => mayEdit;
const latex = ({ format, mayEdit }) => format === "latex" && mayEdit;
const quarto = ({ format, mayEdit }) => format === "quarto" && mayEdit;
const local = ({ format, mayEdit }) => (format === "latex" || format === "quarto") && mayEdit;

export const CATEGORIES = [
  {
    id: "editor", group: "browser", says: "Editor", offered: everyone,
    entries: [{ id: "editor-keys", says: "Keys", terms: "vim emacs keymap keyboard bindings modal source standard" }],
  },
  {
    id: "dictation", group: "browser", says: "Dictation", offered: everyone,
    entries: [
      { id: "dictation-backend", says: "Speech recognition", terms: "backend browser device whisper privacy audio" },
      { id: "dictation-model", says: "Model", terms: "whisper parakeet download size" },
      { id: "dictation-language", says: "Language", terms: "detect automatically" },
      { id: "dictation-status", says: "Loaded model", terms: "webgpu cpu wasm device running" },
    ],
  },
  {
    id: "storage", group: "browser", says: "Storage", offered: everyone,
    entries: [
      { id: "storage-latex", says: "Downloaded LaTeX files", terms: "cache clear free space packages compiler", offered: latex },
      { id: "storage-dictation", says: "Speech models", terms: "cache clear remove download whisper" },
    ],
  },
  {
    id: "compiler", group: "document", says: "Compiler", offered: latex,
    entries: [
      { id: "compiler-engine", says: "PDF compiler", terms: "engine pdflatex xelatex lualatex automatic" },
      { id: "compiler-release", says: "Browser compiler version", terms: "release update pin undo" },
    ],
  },
  {
    id: "rendering", group: "document", says: "Rendering", offered: quarto,
    entries: [
      { id: "rendering-preview", says: "Local preview", terms: "open url" },
      { id: "rendering-options", says: "Format and profile", terms: "format profile parameters pdf html" },
    ],
  },
  {
    id: "local", group: "computer", says: "Local app", offered: local,
    entries: [
      { id: "local-status", says: "Connection", terms: "connect disconnect retry status" },
      { id: "local-pairing", says: "Pairing code", terms: "pair code allow site" },
      { id: "local-address", says: "Address", terms: "port url localhost host" },
      { id: "local-binding", says: "Binding ID", terms: "quarto hosted", offered: quarto },
      { id: "local-tools", says: "Available tools", terms: "versions latex quarto biber" },
      { id: "local-doctor", says: "Check local setup", terms: "doctor troubleshoot diagnostics report" },
    ],
  },
];

// The categories this browser is offered for this document, in order.
export function offered(context) {
  return CATEGORIES.filter((category) => category.offered(context));
}

// The rows a query finds, grouped by category: every offered category whose
// name matches, with all its rows, and every category with a row that does.
export function search(query, context) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return null;
  const matches = (text) => words.every((word) => text.toLowerCase().includes(word));
  const found = [];
  for (const category of offered(context)) {
    const rows = category.entries.filter((entry) => !entry.offered || entry.offered(context));
    const entries = matches(category.says) ? rows : rows.filter((entry) => matches(`${entry.says} ${entry.terms}`));
    if (entries.length) found.push({ category, entries });
  }
  return found;
}
