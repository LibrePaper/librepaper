// What "the text of a PDF" is, so a comment can be anchored into one.
//
// The agent walks text nodes and joins them; on an HTML document that walk
// gives the words in the order they are written, with the whitespace the
// markup put between them. A PDF has no whitespace. pdf.js hands back one
// item per run of glyphs the page painted with the same font and matrix,
// each positioned absolutely, and a run boundary is wherever the typesetter
// changed font or moved the pen -- which is mid-word as often as it is
// between words. Left alone, the joined text of a page reads
// `Weusethesameintervalthroughout`, which anchors nothing.
//
// So the viewer decides, once, what goes *between* the runs, and puts it in
// the DOM as real text nodes. The agent's walk is then unchanged: it still
// just joins text nodes, and what it joins reads as prose. This module holds
// those decisions on their own, away from the DOM, because they are the part
// worth testing and the part worth arguing about.

/// The Latin ligatures a PDF font hands back as one code point.
///
/// A TeX document that writes `fi` in `confidence` is drawn with the `fi`
/// glyph, and pdf.js reports the code point that glyph maps to, U+FB01. The
/// text layer is a transparent overlay for selection; the letters a reader
/// sees are painted on the canvas underneath and are not touched by anything
/// here. So folding the ligature back into its letters changes nothing on
/// screen and makes the text read the way the source wrote it and the way a
/// person quoting it will type it.
///
/// This is done here rather than in `anchor.js`'s tolerance for two reasons.
/// The anchoring is shared with markdown and typst, where a ligature never
/// appears, and widening it would widen it for them too. And the offsets the
/// agent publishes have to be offsets into the text people quote: a selector
/// captured from the text layer must contain `fi`, not U+FB01, or every
/// comment made in a browser is stored in a form the next browser -- or a
/// different font -- may not reproduce.
///
/// Only this block is folded. A general NFKC pass would also rewrite
/// superscripts, fractions and the compatibility forms mathematics is set
/// in, which is a bigger change to a scientific paper than this is meant to
/// be, and one no reader asked for.
const LIGATURES = new Map([
  ["ﬀ", "ff"],
  ["ﬁ", "fi"],
  ["ﬂ", "fl"],
  ["ﬃ", "ffi"],
  ["ﬄ", "ffl"],
  ["ﬅ", "st"],
  ["ﬆ", "st"],
]);

export function foldLigatures(text) {
  if (!/[ﬀ-ﬆ]/.test(text)) return text;
  return text.replace(/[ﬀ-ﬆ]/g, (one) => LIGATURES.get(one));
}

/// A line whose last run ends in a hyphen is a word TeX broke in two. The
/// hyphen is an artefact of the line break, not of the word: nobody reading
/// "inter-\nval" has read anything but "interval", and nobody quoting it will
/// type the hyphen. So the joined text gets `interval`, with no separator at
/// all across the break, and the hyphen the reader can see is put back by CSS
/// -- generated content is not a text node, so it is drawn without being
/// joined.
///
/// The cost is a word that was hyphenated in the source and happened to break
/// at its own hyphen: `well-known` joins as `wellknown`. That is the wrong
/// answer for a minority of breaks, and there is nothing in what pdf.js
/// reports that separates the two cases -- a discretionary hyphen and a typed
/// one are the same glyph in the same place. Hyphenation is much the commoner
/// of the two, so this takes the commoner case and leaves the other to the
/// context re-anchoring, which is what it is for.
export function hyphenatedAtEnd(text) {
  return /[\p{L}\p{N}]-$/u.test(text);
}

/// What goes between two runs on the same line.
///
/// pdf.js emits no whitespace between runs, but the runs themselves often end
/// or begin with a space that the page really painted, so a blind space would
/// double it. And a run boundary inside a word -- a font change for an italic
/// term, or a kern TeX set separately -- must get nothing at all, or every
/// such word is cut in half.
///
/// Geometry answers both: the gap between where the previous run ended and
/// where this one begins, measured in the font's own size. Below a fifth of
/// the font size the two runs are touching and it is one word; above it, the
/// page put a space there.
export function gapBetween(previous, next) {
  if (!previous.text || !next.text) return "";
  if (/\s$/.test(previous.text) || /^\s/.test(next.text)) return "";
  const gap = next.left - (previous.left + previous.width);
  return gap > 0.2 * Math.max(previous.height, next.height, 1) ? " " : "";
}

/// A folio, or a running head that is only a number: page furniture, not text.
///
/// This is what a quotation across a page break actually runs into, and it is
/// worth being exact about, because it is the case `05-SPEC-latex.md` names as
/// the risk of the whole step. A sentence broken over a page boundary does not
/// meet a gap in the sequence: it meets the page number. pdf.js reports the
/// folio as one more text run in reading order, so the joined text reads
/// `...to re-anchor across, 2 since pdf.js will hand...`, and no context
/// re-anchoring will find a quotation that says `across, since`, because those
/// two words are not adjacent in the document as extracted -- only in the
/// document as read.
///
/// So a run that is alone on the first or last line of a page and is nothing
/// but a number -- arabic or roman -- is dropped from the joined text. It is
/// still painted, because it is painted on the canvas underneath and the text
/// layer is only the selection overlay; the one thing lost is the ability to
/// select the page number itself, which nobody has ever wanted to quote.
///
/// The rule is deliberately narrow. A running head that is the paper's title
/// stays, because it is words and a reader might quote them, and a number in
/// the body of the page stays, because it is the document.
const FOLIO = /^[\s ]*[\divxlcdmIVXLCDM]{1,8}[\s ]*$/;

function isFurniture(runs, i) {
  if (!FOLIO.test(runs[i].text)) return false;
  const alone = (i === 0 || runs[i - 1].eol) && (runs[i].eol || i === runs.length - 1);
  if (!alone) return false;
  const firstLine = i === 0 || runs.slice(0, i).every((run) => !run.text.trim());
  const lastLine = i === runs.length - 1 || runs.slice(i + 1).every((run) => !run.text.trim());
  return firstLine || lastLine;
}

/// The whole joined text of a document, from runs already grouped into pages.
///
/// A line end is a newline, a page break is a blank line, and both are
/// whitespace, so `anchor.js`'s flattened view already treats them as the
/// single space a person typing the quotation would type. That is the whole
/// trick: nothing here needs anchoring it does not already have. A run
/// boundary mid-word is nothing, a run boundary between words is one space,
/// a hyphenated line end is nothing, and everything else is a newline.
///
/// `runs` is an array of pages, each an array of
/// `{text, left, width, height, eol}` in reading order. Returns the pieces to
/// put in the DOM, in order, each `{kind: "run"|"gap", text, page, hyphen}`,
/// so the caller can build spans and separators from one description and the
/// tests can read the same one.
export function piecesOf(pages) {
  const pieces = [];
  for (let page = 0; page < pages.length; page++) {
    if (page > 0) pieces.push({ kind: "gap", text: "\n\n", page });
    const runs = pages[page];
    for (let i = 0; i < runs.length; i++) {
      const run = runs[i];
      if (isFurniture(runs, i)) {
        pieces.push({ kind: "run", text: "", hyphen: false, furniture: true, page, run: i });
        continue;
      }
      const text = foldLigatures(run.text);
      const hyphen = run.eol && hyphenatedAtEnd(text);
      pieces.push({
        kind: "run",
        text: hyphen ? text.slice(0, -1) : text,
        hyphen,
        page,
        run: i,
      });
      const next = runs[i + 1];
      if (!next) continue;
      if (hyphen) continue; // the word carries on; nothing between its halves
      if (run.eol) pieces.push({ kind: "gap", text: "\n", page });
      else {
        const between = gapBetween(run, next);
        if (between) pieces.push({ kind: "gap", text: between, page });
      }
    }
  }
  return pieces;
}
