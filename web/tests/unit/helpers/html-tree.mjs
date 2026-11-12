// A small HTML parser for tests only. Node has no DOMParser, so this turns a
// clipboard fragment into the neutral tree that paste-html.js consumes:
// a text node is a string, an element is { tag, attrs, children }.
//
// It is not a full HTML parser. It handles what clipboard HTML from Word,
// Google Docs, LibreOffice and browsers actually contains: quoted and unquoted
// attributes, void elements, comments (including Word's conditional comments),
// raw-text <style> and <script> bodies, named and numeric entities, implied
// closes for <p>, <li>, <td>, <th>, <tr> and friends, and stray end tags.
//
// Word's `<![if !supportLists]>` and `<![endif]>` markers are not comments (they
// have no `--`). This parser drops the markers and keeps what sits between them,
// so the bullet span inside them stays in the tree. The converter has its own
// guard for that span, because the browser's DOMParser keeps the markers as
// comment nodes and leaves the span in place.

const VOID = new Set(["br", "img", "hr", "meta", "link", "input", "col", "wbr", "area", "base", "embed", "source", "track", "param"]);
const RAW_TEXT = new Set(["style", "script"]);
// A start tag of the key closes any open element listed in its value on the stack.
const IMPLIED_CLOSE = {
  p: ["p"],
  li: ["li"],
  td: ["td", "th"],
  th: ["td", "th"],
  tr: ["tr", "td", "th"],
  thead: ["tbody", "tfoot", "tr", "td", "th"],
  tbody: ["thead", "tbody", "tfoot", "tr", "td", "th"],
  tfoot: ["thead", "tbody", "tr", "td", "th"],
  dt: ["dt", "dd"],
  dd: ["dt", "dd"],
  option: ["option"],
};
// Block starts that close an open <p>, as the HTML parser does.
const CLOSES_P = new Set(["address", "article", "aside", "blockquote", "div", "dl", "fieldset", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "main", "nav", "ol", "pre", "section", "table", "ul"]);

const NAMED = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  ldquo: "“",
  rdquo: "”",
  lsquo: "‘",
  rsquo: "’",
  sbquo: "‚",
  bdquo: "„",
  laquo: "«",
  raquo: "»",
  mdash: "—",
  ndash: "–",
  hellip: "…",
  middot: "·",
  bull: "•",
  copy: "©",
  reg: "®",
  trade: "™",
  deg: "°",
  plusmn: "±",
  times: "×",
  divide: "÷",
  euro: "€",
  pound: "£",
  yen: "¥",
  cent: "¢",
  sect: "§",
  para: "¶",
  shy: "­",
  zwj: "‍",
  zwnj: "‌",
  thinsp: " ",
  ensp: " ",
  emsp: " ",
  iexcl: "¡",
  iquest: "¿",
  szlig: "ß",
  aacute: "á",
  eacute: "é",
  iacute: "í",
  oacute: "ó",
  uacute: "ú",
  agrave: "à",
  egrave: "è",
  igrave: "ì",
  ograve: "ò",
  ugrave: "ù",
  acirc: "â",
  ecirc: "ê",
  icirc: "î",
  ocirc: "ô",
  ucirc: "û",
  auml: "ä",
  euml: "ë",
  iuml: "ï",
  ouml: "ö",
  uuml: "ü",
  ccedil: "ç",
  ntilde: "ñ",
  Aacute: "Á",
  Eacute: "É",
  Iacute: "Í",
  Oacute: "Ó",
  Uacute: "Ú",
  Agrave: "À",
  Egrave: "È",
  Igrave: "Ì",
  Ograve: "Ò",
  Ugrave: "Ù",
  Acirc: "Â",
  Ecirc: "Ê",
  Icirc: "Î",
  Ocirc: "Ô",
  Ucirc: "Û",
  Auml: "Ä",
  Euml: "Ë",
  Iuml: "Ï",
  Ouml: "Ö",
  Uuml: "Ü",
  Ccedil: "Ç",
  Ntilde: "Ñ",
  nbsp: " " };

/// Decodes character references in text or an attribute value.
export function decodeEntities(text) {
  return text.replace(/&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z][a-zA-Z0-9]*);/g, (match, body) => {
    if (body[0] === "#") {
      const code = body[1] === "x" || body[1] === "X" ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : match;
    }
    return Object.hasOwn(NAMED, body) ? NAMED[body] : match;
  });
}

/// Parses one start tag's attribute text into a lowercase-keyed record.
function parseAttrs(source) {
  const attrs = {};
  const pattern = /([^\s"'>\/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
  let match;
  while ((match = pattern.exec(source))) {
    const value = match[2] ?? match[3] ?? match[4] ?? "";
    attrs[match[1].toLowerCase()] = decodeEntities(value);
  }
  return attrs;
}

/// Parses an HTML fragment. Returns the <body> element's tree when the input
/// has one, otherwise a body element wrapping the whole fragment.
export function parseHtmlTree(html) {
  const root = { tag: "body", attrs: {}, children: [] };
  const stack = [root];
  const top = () => stack[stack.length - 1];
  const append = (child) => top().children.push(child);

  const closeTo = (tag) => {
    for (let i = stack.length - 1; i > 0; i--) {
      if (stack[i].tag === tag) {
        stack.length = i;
        return;
      }
    }
  };
  const closeImplied = (tag) => {
    const closes = IMPLIED_CLOSE[tag];
    if (closes) {
      for (let i = stack.length - 1; i > 0; i--) {
        if (closes.includes(stack[i].tag)) {
          stack.length = i;
          return;
        }
        // A list or table is a boundary: an item never closes one outside it.
        if (["table", "ul", "ol"].includes(stack[i].tag)) break;
      }
    }
    if (CLOSES_P.has(tag)) closeTo("p");
  };

  let pos = 0;
  while (pos < html.length) {
    const lt = html.indexOf("<", pos);
    if (lt < 0) {
      append(decodeEntities(html.slice(pos)));
      break;
    }
    if (lt > pos) append(decodeEntities(html.slice(pos, lt)));
    pos = lt;

    if (html.startsWith("<!--", pos)) {
      const end = html.indexOf("-->", pos + 4);
      pos = end < 0 ? html.length : end + 3;
      continue;
    }
    if (html.startsWith("<![", pos)) {
      // Word's <![if ...]> and <![endif]> markers: drop the marker, keep the content.
      const end = html.indexOf("]>", pos + 3);
      pos = end < 0 ? html.length : end + 2;
      continue;
    }
    if (html.startsWith("<!", pos) || html.startsWith("<?", pos)) {
      const end = html.indexOf(">", pos);
      pos = end < 0 ? html.length : end + 1;
      continue;
    }
    const endTag = /^<\/([a-zA-Z][^\s/>]*)[^>]*>/.exec(html.slice(pos, pos + 500));
    if (endTag) {
      closeTo(endTag[1].toLowerCase());
      pos += endTag[0].length;
      continue;
    }
    const startTag = /^<([a-zA-Z][^\s/>]*)((?:[^>"']|"[^"]*"|'[^']*')*)>/.exec(html.slice(pos));
    if (!startTag) {
      // A lone "<" that starts no tag is text.
      append("<");
      pos += 1;
      continue;
    }
    const tag = startTag[1].toLowerCase();
    const selfClosing = /\/\s*$/.test(startTag[2]);
    pos += startTag[0].length;

    closeImplied(tag);
    const element = { tag, attrs: parseAttrs(startTag[2].replace(/\/\s*$/, "")), children: [] };
    if (tag === "body" && root.children.length === 0 && stack.length === 1) {
      // The body's own tag is the root: its attributes are kept, its children go in.
      root.attrs = element.attrs;
      continue;
    }
    append(element);
    if (VOID.has(tag) || selfClosing) continue;
    if (RAW_TEXT.has(tag)) {
      const close = html.toLowerCase().indexOf(`</${tag}`, pos);
      pos = close < 0 ? html.length : html.indexOf(">", close) + 1 || html.length;
      continue;
    }
    stack.push(element);
  }

  // If the fragment has a body element, the tree is that body. The root body
  // above is only a container; a nested <body> is the one the tests want.
  const body = findBody(root);
  return body ?? root;
}

/// The first <body> element under `node`, or null.
function findBody(node) {
  for (const child of node.children) {
    if (typeof child === "string") continue;
    if (child.tag === "body") return child;
    const found = findBody(child);
    if (found) return found;
  }
  return null;
}
