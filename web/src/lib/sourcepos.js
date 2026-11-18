// The `data-sourcepos` attribute a preview element carries names the source
// lines it was drawn from. Two shapes exist:
//
//   T:L:C-T:L:C   a tagged source (LaTeX, Typst). T names an entry of the
//                 `sources` list the render returns beside the page.
//   L:C-L:C       an untagged source (Markdown, Quarto). Its one source is
//                 tag 0.
//
// Lines are 1-based. Columns are checked for shape but not returned: a
// source file is named by its lines alone.

const TAGGED = /^(\d+):(\d+):\d+-(\d+):(\d+):\d+$/;
const UNTAGGED = /^(\d+):\d+-(\d+):\d+$/;

function line(digits) {
  const number = Number(digits);
  return Number.isSafeInteger(number) && number >= 1 ? number : null;
}

/** @returns {{ tag: number, from: number, to: number } | null} */
export function parseSourcepos(value) {
  if (typeof value !== "string" || value.length > 64) return null;
  let tag;
  let from;
  let to;
  const tagged = TAGGED.exec(value);
  if (tagged) {
    // Both ends of one attribute must name the same source.
    if (tagged[1] !== tagged[3]) return null;
    tag = Number(tagged[1]);
    from = line(tagged[2]);
    to = line(tagged[4]);
  } else {
    const untagged = UNTAGGED.exec(value);
    if (!untagged) return null;
    tag = 0;
    from = line(untagged[1]);
    to = line(untagged[2]);
  }
  if (!Number.isSafeInteger(tag) || from === null || to === null) return null;
  return { tag, from, to };
}

const ATTRIBUTE = / data-sourcepos="[^"]*"/g;

/// HTML without its `data-sourcepos` attributes. Paper mode renders one block
/// at a time, so the positions a renderer writes there count lines of that
/// block alone, and nothing reads them; leaving them out keeps a block's HTML
/// what it was before renderers wrote positions at all.
/** @param {string} html @returns {string} */
export function withoutSourcepos(html) {
  return html.replace(ATTRIBUTE, "");
}
