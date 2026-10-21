// The page a text file previews as. It is painted on the documents origin,
// where the application's theme does not reach, so its colours travel with it.

/// The page a text file previews as: a CSV or TSV file as a table of its rows,
/// and anything else as its source in a preformatted block, with no script.
/// The source is highlighted where a language is known and shown plain
/// otherwise. Previews are light paper, so the page stays light whatever the
/// system's colour scheme.
export async function textPage(source, path, title) {
  let content = null;
  try {
    content = tableMarkup(source, path);
    if (content === null) {
      const markup = await highlightedSource(source, path);
      content = markup === null ? null : `<pre>${markup}</pre>`;
    }
  } catch {
    // A preview never fails because a table or highlighting did: the plain source stands.
    content = null;
  }
  if (content === null) content = `<pre>${escapeMarkup(source)}</pre>`;
  return `<!doctype html><html><head><meta charset="utf-8"><title>${escapeMarkup(title || "")}</title><style>${textPageStyle}</style></head><body>${content}</body></html>`;
}

const textPageStyle = `:root { color-scheme: light; } html, body { background: #fff; color: #24292e; } body { margin: 0; padding: 16px; } pre { margin: 0; font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; } .tok-comment { color: #6a737d; font-style: italic; } .tok-meta { color: #6a737d; } .tok-keyword { color: #d73a49; } .tok-string { color: #032f62; } .tok-number, .tok-bool, .tok-atom { color: #005cc5; } .tok-propertyName, .tok-definition { color: #6f42c1; } .tok-typeName, .tok-className { color: #e36209; } .tok-invalid { color: #b31d28; } table { border-collapse: collapse; min-width: max-content; font: 13px/1.4 ui-sans-serif, system-ui, sans-serif; } th, td { border: 1px solid #d0d7de; padding: 4px 8px; text-align: left; vertical-align: top; white-space: pre-wrap; } thead th { position: sticky; top: 0; background: #f6f8fa; font-weight: 600; } tbody tr:nth-child(even) td { background: #f6f8fa; } td.num { text-align: right; font-variant-numeric: tabular-nums; } .omitted { color: #6a737d; margin: 12px 0 0; }`;

/// Sources larger than this are shown without highlighting.
const HIGHLIGHT_LIMIT = 512 * 1024;

/// Body rows a table preview shows before it stops.
const TABLE_ROW_LIMIT = 5000;

const NUMBER = /^[-+]?(?:\d+(?:[.,]\d+)*|\d*\.\d+)(?:[eE][-+]?\d+)?%?$/;

/// The rows of a delimited text as arrays of fields, following RFC 4180. A
/// field may be wrapped in double quotes, where a doubled quote is a literal
/// quote and the delimiter and line breaks are literal. A quote that is not
/// the first character of a field, or that follows a closing quote, is kept as
/// content, so malformed input still yields rows.
export function parseDelimited(source, delimiter) {
  let text = String(source);
  if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
  const rows = [];
  let row = [];
  let field = "";
  let quoted = false;
  let fieldStart = true;
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (quoted) {
      if (c !== "\"") field += c;
      else if (text[i + 1] === "\"") {
        field += "\"";
        i += 1;
      } else quoted = false;
    } else if (c === "\"" && fieldStart) {
      quoted = true;
      fieldStart = false;
    } else if (c === delimiter) {
      row.push(field);
      field = "";
      fieldStart = true;
    } else if (c === "\n" || (c === "\r" && text[i + 1] === "\n")) {
      if (c === "\r") i += 1;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
      fieldStart = true;
    } else {
      field += c;
      fieldStart = false;
    }
  }
  // A final line without a line break is still a row. A source that ends with
  // a line break adds no empty row after it.
  if (field !== "" || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

/// The table for a CSV or TSV source, or null when the file is not one or has
/// no rows. The first row is the header, and short rows are padded so every
/// column lines up.
function tableMarkup(source, path) {
  const lower = (path || "").toLowerCase();
  if (!/\.(?:csv|tsv)$/.test(lower)) return null;
  const rows = parseDelimited(source, lower.endsWith(".tsv") ? "\t" : ",");
  if (rows.length === 0) return null;
  const width = rows.reduce((widest, row) => Math.max(widest, row.length), 0);
  const pad = (row) => {
    const padded = row.slice();
    while (padded.length < width) padded.push("");
    return padded;
  };
  const [header, ...body] = rows;
  const head = `<thead><tr>${pad(header).map((value) => `<th>${escapeMarkup(value)}</th>`).join("")}</tr></thead>`;
  const rowsMarkup = body.slice(0, TABLE_ROW_LIMIT).map((row) => `<tr>${pad(row).map((value) => {
    const number = NUMBER.test(value.trim()) ? " class=\"num\"" : "";
    return `<td${number}>${escapeMarkup(value)}</td>`;
  }).join("")}</tr>`).join("");
  const omitted = body.length > TABLE_ROW_LIMIT
    ? `<p class="omitted">Showing the first ${TABLE_ROW_LIMIT.toLocaleString("en-US")} of ${body.length.toLocaleString("en-US")} rows.</p>`
    : "";
  return `<table>${head}<tbody>${rowsMarkup}</tbody></table>${omitted}`;
}

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
  if (/\.(?:txt|csv|tsv|bib|bst)$/.test(lower) || source.length > HIGHLIGHT_LIMIT) return null;
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
