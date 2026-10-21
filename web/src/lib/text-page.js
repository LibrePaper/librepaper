// The page a text file previews as. It is painted on the documents origin,
// where the application's theme does not reach, so its colours travel with it.

/// The page a text file previews as: its source in a preformatted block, with
/// no script. The source is highlighted where a language is known and shown
/// plain otherwise. Previews are light paper, so the page stays light whatever
/// the system's colour scheme.
export async function textPage(source, path, title) {
  let body = escapeMarkup(source);
  try {
    body = (await highlightedSource(source, path)) ?? body;
  } catch {
    // A preview never fails because highlighting did: the plain source stands.
  }
  return `<!doctype html><html><head><meta charset="utf-8"><title>${escapeMarkup(title || "")}</title><style>${textPageStyle}</style></head><body><pre>${body}</pre></body></html>`;
}

const textPageStyle = `:root { color-scheme: light; } html, body { background: #fff; color: #24292e; } body { margin: 0; padding: 16px; } pre { margin: 0; font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; } .tok-comment { color: #6a737d; font-style: italic; } .tok-meta { color: #6a737d; } .tok-keyword { color: #d73a49; } .tok-string { color: #032f62; } .tok-number, .tok-bool, .tok-atom { color: #005cc5; } .tok-propertyName, .tok-definition { color: #6f42c1; } .tok-typeName, .tok-className { color: #e36209; } .tok-invalid { color: #b31d28; }`;

/// Sources larger than this are shown without highlighting.
const HIGHLIGHT_LIMIT = 512 * 1024;

function escapeMarkup(value) {
  return String(value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/// The source as markup with a class on each token, or null when the file has
/// no language worth colouring or is too large to be worth highlighting. The
/// languages load on first use, so the reader bundle does not carry them.
async function highlightedSource(source, path) {
  const base = (path || "").split("/").pop() || "";
  const lower = base.toLowerCase();
  // Plain text, tables and bibliographies have no grammar worth colouring.
  if (/\.(?:txt|csv|bib|bst)$/.test(lower) || source.length > HIGHLIGHT_LIMIT) return null;
  const [{ LanguageDescription, StreamLanguage }, { highlightCode, classHighlighter }, { languages }] = await Promise.all([
    import("@codemirror/language"),
    import("@lezer/highlight"),
    import("@codemirror/language-data"),
  ]);
  let language = null;
  if (/\.(?:sty|cls)$/.test(lower)) {
    const { stex } = await import("@codemirror/legacy-modes/mode/stex");
    language = StreamLanguage.define(stex);
  } else {
    const description = LanguageDescription.matchFilename(languages, base)
      || (lower.endsWith(".csl") ? LanguageDescription.matchLanguageName(languages, "xml", false) : null);
    if (description) language = (await description.load()).language;
  }
  if (!language) return null;
  let out = "";
  highlightCode(source, language.parser.parse(source), classHighlighter, (text, classes) => {
    out += classes ? `<span class="${classes}">${escapeMarkup(text)}</span>` : escapeMarkup(text);
  }, () => {
    out += "\n";
  });
  return out;
}
