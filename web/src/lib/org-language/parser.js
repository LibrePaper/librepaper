// Org mode as a Lezer tree, from a hand-written line-based parser. Org is
// line-oriented and indentation-sensitive, so this is not an LR grammar: it
// splits the text into lines, recognises block-level elements line by line,
// and scans inline markup inside the text of paragraphs, titles, list items,
// table cells and some keyword values. It is written from the Org syntax
// description, not ported from any other parser. A parse is linear in the text
// (failed searches are remembered so a run of unclosed openers is not rescanned).
//
// NODE VOCABULARY (the contract for everything that reads this tree).
// Offsets are UTF-16 offsets into the text. "text" below means the node covers
// only that piece, not its delimiters. Siblings never overlap.
//
//   Document                      the whole text; children are the elements
//                                 before the first headline, then Sections
//   Section                       a headline and everything up to the next
//                                 headline of the same or a higher level:
//                                 [Headline, element..., Section...]
//   Headline                      the headline line:
//                                 [Stars, TodoKeyword?, Priority?, Title?, Tags?]
//     Stars                       the leading `*`s (level = length)
//     TodoKeyword                 TODO, DONE or a state declared by #+TODO:
//     Priority                    `[#A]`
//     Title                       the headline text; holds inline nodes
//     Tags                        `:a:b:` including colons; holds Tag
//     Tag                         one tag name, no colons
//
//   Planning                      a SCHEDULED/DEADLINE/CLOSED line:
//                                 [PlanningKeyword | Timestamp... ]
//     PlanningKeyword             `SCHEDULED:` etc.
//   Clock                         a CLOCK line: [ClockKeyword, Timestamp..., ClockDuration?]
//     ClockKeyword                `CLOCK:`
//     ClockDuration               `=> 1:30`
//   PropertyDrawer                `:PROPERTIES:` ... `:END:`:
//                                 [DrawerName, Property..., DrawerEnd]
//     Property                    [PropertyKey, PropertyValue?]
//     PropertyKey                 the key between the colons, including a `+`
//     PropertyValue               the raw value, trimmed
//   Drawer                        any other `:NAME:` ... `:END:`:
//                                 [DrawerName, element..., DrawerEnd]
//     DrawerName                  the name between the colons
//     DrawerEnd                   `:END:`
//
//   Keyword                       `#+KEY: value` (includes #+NAME, #+CAPTION,
//                                 #+ATTR_*, #+INCLUDE, #+BIBLIOGRAPHY, #+TITLE,
//                                 #+TODO...): [KeywordKey, KeywordValue?]. Affiliated
//                                 keywords are siblings of the element they precede.
//     KeywordKey                  the name, plus an optional `[option]`
//     KeywordValue                the value, trimmed. Inline nodes only for
//                                 TITLE, SUBTITLE, AUTHOR, DATE, CAPTION; raw otherwise
//   Block                         `#+begin_NAME` ... `#+end_NAME`:
//                                 [BlockBegin, body?, BlockEnd?]. While typing, a block
//                                 may lack its BlockEnd: it then runs to the end of the
//                                 section (a headline still ends it) or of the document.
//                                 A stray end line with no open block is a BlockEnd
//                                 alone, outside any Block.
//     BlockBegin                  the begin line: [BlockName, BlockParameters?]
//       BlockName                 NAME as written (compare case-insensitively)
//       BlockParameters           the rest of the line, trimmed; for src and
//                                 export it starts with [BlockLanguage]
//       BlockLanguage             the first word of the parameters (src, export)
//     VerbatimBody                the lines of a src, example or export block (no children)
//     CommentBody                 the lines of a comment block (no children)
//     BlockBody                   the lines of any other block; holds elements
//                                 (quote, verse, center, dynamic blocks, custom names)
//     BlockEnd                    the end line
//   LatexEnvironment              `\begin{x}` ... `\end{x}` at the start of a
//                                 line: [LatexEnvBegin, LatexEnvEnd?]
//     LatexEnvBegin               `\begin{x}` : [LatexEnvName]
//     LatexEnvName                x
//     LatexEnvEnd                 `\end{x}` (absent for a single-line environment)
//   Comment                       one `# text` line
//   FixedWidth                    one `: text` line
//   HorizontalRule                five or more dashes
//   Table                         consecutive `|` lines: [TableRow | TableRule | TableFormula]...
//     TableRow                    [TableDelimiter, TableCell, TableDelimiter, ...]
//     TableCell                   trimmed cell text; holds inline nodes (empty cells are zero-width)
//     TableDelimiter              one `|`
//     TableRule                   a `|---+---|` line
//     TableFormula                a `#+TBLFM:` line after the rows: [KeywordKey, KeywordValue?]
//   List                          [ListItem...] at one indentation
//     ListItem                    [ListBullet, Checkbox?, ListTag?, element...]; the
//                                 text after the bullet is the first Paragraph
//     ListBullet                  `-`, `+`, `*` (indented) or `1.` / `1)`
//     Checkbox                    `[ ]`, `[X]`, `[-]`
//     ListTag                     the term of `- term :: description`; holds inline nodes
//   FootnoteDefinition            `[fn:label] text` at column 0: [FootnoteLabel, element...]
//     FootnoteLabel               the label (also used in FootnoteReference)
//   Paragraph                     everything else, up to a blank line or the next
//                                 element; holds inline nodes
//
// Inline nodes (inside Title, Paragraph, TableCell, ListTag, KeywordValue,
// LinkDescription, Planning):
//   Bold `*x*`, Italic `/x/`, Underline `_x_`, StrikeThrough `+x+`:
//                                 [EmphasisMark, inline..., EmphasisMark]
//   Verbatim `=x=`, Code `~x~`:   [EmphasisMark, EmphasisMark]; the text between is not parsed
//   Link                          `[[target][desc]]`, `[[target]]`, `<scheme:target>` or a
//                                 bare `https://...`: [LinkTarget, LinkDescription?]
//     LinkTarget, LinkDescription (description holds inline nodes)
//   FootnoteReference             `[fn:label]`, `[fn:label:definition]`, `[fn::definition]`:
//                                 [FootnoteLabel?, inline...]
//   Citation                      `[cite/style:...@key;...]`: [CitationKey...]
//     CitationKey                 the key without its `@`
//   TimestampActive               `<2024-01-31 Wed 10:00>`
//   TimestampInactive             `[2024-01-31 Wed]`
//   Macro                         `{{{name(args)}}}`: [MacroName?]
//   Target `<<x>>`, RadioTarget `<<<x>>>`
//   InlineMath                    `$x$` or `\(x\)`
//   DisplayMath                   `$$x$$` or `\[x\]`
//   Entity                        `\alpha`, `\to` (a built-in subset of Org's entity table)
//   LatexCommand                  any other `\name`
//   InlineSrc                     `src_lang[header]{code}`: [InlineSrcLanguage, InlineSrcBody]
//   InlineBabelCall               `call_name[h](args)[h]`: [InlineBabelName]
//   LineBreak                     `\\` at the end of a line
//
// Not recognised (they read as plain text): sub/superscripts, statistics
// cookies, `[@n]` list counters, table.el tables, inline tasks, diary sexps,
// plain links other than a short list of schemes.

import { NodeSet, NodeType, Parser, Tree } from "@lezer/common";
import { nodeProps } from "./props.js";

export const NODE_NAMES = `Document Section Headline Stars TodoKeyword Priority Title Tags Tag
Planning PlanningKeyword Clock ClockKeyword ClockDuration
PropertyDrawer Property PropertyKey PropertyValue Drawer DrawerName DrawerEnd
Keyword KeywordKey KeywordValue
Block BlockBegin BlockName BlockParameters BlockLanguage VerbatimBody CommentBody BlockBody BlockEnd
LatexEnvironment LatexEnvBegin LatexEnvName LatexEnvEnd
Comment FixedWidth HorizontalRule
Table TableRow TableCell TableDelimiter TableRule TableFormula
List ListItem ListBullet Checkbox ListTag FootnoteDefinition FootnoteLabel Paragraph
Bold Italic Underline StrikeThrough Verbatim Code EmphasisMark
Link LinkTarget LinkDescription FootnoteReference Citation CitationKey
TimestampActive TimestampInactive Macro MacroName Target RadioTarget
InlineMath DisplayMath Entity LatexCommand InlineSrc InlineSrcLanguage InlineSrcBody
InlineBabelCall InlineBabelName LineBreak`.split(/\s+/);

const ID = new Map(NODE_NAMES.map((name, id) => [name, id]));

export const nodeSet = new NodeSet(
  NODE_NAMES.map((name, id) => NodeType.define({ id, name, top: name === "Document", props: nodeProps })),
);

const VERBATIM_BLOCKS = new Set(["src", "example", "export"]);
const INLINE_KEYWORDS = new Set(["TITLE", "SUBTITLE", "AUTHOR", "DATE", "CAPTION"]);
const EMPHASIS = { "*": "Bold", "/": "Italic", "_": "Underline", "+": "StrikeThrough", "=": "Verbatim", "~": "Code" };
const RAW_EMPHASIS = new Set(["=", "~"]);
const ENTITIES = new Set(`alpha beta gamma delta epsilon varepsilon zeta eta theta vartheta iota kappa
lambda mu nu xi pi varpi rho sigma varsigma tau upsilon phi varphi chi psi omega Gamma Delta Theta Lambda
Xi Pi Sigma Upsilon Phi Psi Omega to rightarrow leftarrow Rightarrow Leftarrow leftrightarrow uparrow
downarrow times div pm mp cdot ldots dots hellip nbsp ndash mdash deg ne neq leq geq le ge infty sum prod
int partial nabla in notin subset subseteq supset cap cup forall exists neg land lor copy reg trade euro
pound yen cent sect para dagger ddagger laquo raquo approx equiv sim propto emptyset angle checkmark
star bullet`.split(/\s+/));

const HEADLINE = /^\*+(?=[ \t])/;
const BEGIN = /^[ \t]*#\+begin_(\S+)/i;
const KEYWORD = /^([ \t]*)#\+([A-Za-z0-9_-]+)(\[[^\]\n]*\])?:/;
const DRAWER = /^([ \t]*):([^\s:]+):[ \t]*$/;
const PROPERTY = /^([ \t]*):([^\s:]+):(?:[ \t]+(.*))?$/;
const PLANNING = /^[ \t]*(?:SCHEDULED|DEADLINE|CLOSED):/;
const CLOCK = /^[ \t]*CLOCK:/;
const HRULE = /^[ \t]*-{5,}[ \t]*$/;
const LATEX_BEGIN = /^([ \t]*)\\begin\{([^}\n]+)\}/;
const FOOTNOTE_DEF = /^\[fn:([^\]\s]+)\]/;
const RULE_ROW = /^[ \t]*\|[-+|]*-[-+|]*[ \t]*$/;
const BULLET = /(?:[-+]|\d{1,9}[.)]|\*)(?=[ \t]|$)/y;
const CHECKBOX = /\[[ Xx-]\](?=[ \t]|$)/y;
const PRIORITY = /\[#[A-Za-z0-9]\](?=[ \t]|$)/y;
const INTERRUPT = /^[ \t]*(?:#(?:\+|[ \t]|$)|:(?:[ \t]|$|[^\s:]+:[ \t]*$)|\||-{5,}[ \t]*$|\\begin\{)/;
const DATE = /\d{4}-\d{2}-\d{2}/y;
const PLAIN_LINK = /(?:https?|ftp|file|mailto|doi|irc|news|ssh):[^\s<>[\]{}]+/y;
const ANGLE_LINK = /<[A-Za-z][\w+.-]*:[^\s<>]+>/y;
const RADIO_TARGET = /<<<[^<>\n]+>>>/y;
const TARGET = /<<[^<>\n]+>>/y;
const CITE_OPEN = /\[cite(?:\/[\w-]+)*:/y;
const CITE_KEY = /@([-\p{L}\p{N}_:.+/]+)/gu;
const COMMAND = /\\([A-Za-z]+)/y;
const NAME = /[\w-]+/y;

const isSpace = (code) => code === 32 || code === 9 || code === 10 || code === 13 || code === 12 || code === 11 || code === 160;
const isAlnum = (ch) => ch !== undefined && /[\p{L}\p{N}]/u.test(ch);

function node(name, from, to, kids = []) {
  return { name, from, to, kids };
}

/** A line: absolute offsets, its text, leading blanks, indentation and trimmed end. */
function makeLine(from, to, text) {
  let lead = 0;
  let indent = 0;
  while (lead < text.length && (text.charCodeAt(lead) === 32 || text.charCodeAt(lead) === 9)) {
    indent = text.charCodeAt(lead) === 9 ? indent + 8 - (indent % 8) : indent + 1;
    lead++;
  }
  let end = text.length;
  while (end > lead && isSpace(text.charCodeAt(end - 1))) end--;
  return { from, to, text, lead, indent, blank: end === lead, end: from + end };
}

function splitLines(text) {
  const lines = [];
  let from = 0;
  for (;;) {
    const nl = text.indexOf("\n", from);
    let to = nl < 0 ? text.length : nl;
    const next = to;
    if (to > from && text.charCodeAt(to - 1) === 13) to--;
    lines.push(makeLine(from, to, text.slice(from, to)));
    if (nl < 0) return lines;
    from = next + 1;
  }
}

const TODO_LINE = /^#\+(?:SEQ_|TYP_)?TODO:(.*)$/i;

/** The words #+TODO / #+SEQ_TODO / #+TYP_TODO lines declare, plus TODO and DONE. */
function todoWords(text) {
  const words = new Set(["TODO", "DONE"]);
  const see = (from) => {
    let to = text.indexOf("\n", from);
    if (to < 0) to = text.length;
    if (to > from && text.charCodeAt(to - 1) === 13) to--;
    const m = TODO_LINE.exec(text.slice(from, to));
    if (!m) return;
    for (const word of m[1].split(/\s+/)) {
      const name = word.replace(/\(.*$/, "");
      if (name && name !== "|") words.add(name);
    }
  };
  if (text.charCodeAt(0) === 35) see(0);
  for (let i = text.indexOf("\n#"); i >= 0; i = text.indexOf("\n#", i + 1)) see(i + 1);
  return words;
}

class OrgParse {
  constructor(text, todo) {
    this.text = text;
    this.lines = splitLines(text);
    this.todo = todo;
    /** @type {Map<string, number>} searches that found nothing: key -> range end they covered */
    this.failed = new Map();
  }

  line(i, ov) {
    return ov && ov.i === i ? ov.line : this.lines[i];
  }

  lineEndAt(pos) {
    const lines = this.lines;
    let lo = 0;
    let hi = lines.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (lines[mid].from <= pos) lo = mid;
      else hi = mid - 1;
    }
    return lines[lo].to;
  }

  /** The first line in (from, hi) satisfying test, or -1. A miss is remembered per key. */
  findLine(key, from, hi, test) {
    if ((this.failed.get(key) ?? -1) >= hi) return -1;
    for (let j = from + 1; j < hi; j++) if (test(this.lines[j])) return j;
    this.failed.set(key, hi);
    return -1;
  }

  // ---- document ----

  parse() {
    const { lines } = this;
    const doc = node("Document", 0, this.text.length);
    const stack = [];
    const heads = [];
    for (let i = 0; i < lines.length; i++) if (HEADLINE.test(lines[i].text)) heads.push(i);
    this.elements(0, heads.length ? heads[0] : lines.length, doc.kids);
    const pop = () => {
      const section = stack.pop();
      section.to = section.kids[section.kids.length - 1].to;
    };
    heads.forEach((h, k) => {
      const ln = lines[h];
      const level = HEADLINE.exec(ln.text)[0].length;
      while (stack.length && stack[stack.length - 1].level >= level) pop();
      const headline = this.headline(ln);
      const section = node("Section", ln.from, headline.to, [headline]);
      section.level = level;
      (stack.length ? stack[stack.length - 1].kids : doc.kids).push(section);
      stack.push(section);
      this.elements(h + 1, k + 1 < heads.length ? heads[k + 1] : lines.length, section.kids);
    });
    while (stack.length) pop();
    return doc;
  }

  headline(ln) {
    const t = ln.text;
    const stars = HEADLINE.exec(t)[0].length;
    const at = (k) => ln.from + k;
    const kids = [node("Stars", ln.from, at(stars))];
    const end = ln.end - ln.from;
    let p = stars;
    const skip = () => {
      while (p < end && (t.charCodeAt(p) === 32 || t.charCodeAt(p) === 9)) p++;
    };
    skip();
    let word = p;
    while (word < end && !isSpace(t.charCodeAt(word))) word++;
    if (word > p && this.todo.has(t.slice(p, word))) {
      kids.push(node("TodoKeyword", at(p), at(word)));
      p = word;
      skip();
    }
    PRIORITY.lastIndex = p;
    const priority = PRIORITY.exec(t);
    if (priority) {
      kids.push(node("Priority", at(p), at(p + priority[0].length)));
      p += priority[0].length;
      skip();
    }
    // Tags: a last word like :a:b: after whitespace.
    let tagStart = end;
    while (tagStart > p && !isSpace(t.charCodeAt(tagStart - 1))) tagStart--;
    let tags = null;
    if (tagStart > p && end - tagStart > 2 && /^:(?:[^\s:]+:)+$/.test(t.slice(tagStart, end))) {
      tags = node("Tags", at(tagStart), at(end));
      let k = tagStart + 1;
      while (k < end) {
        const colon = t.indexOf(":", k);
        tags.kids.push(node("Tag", at(k), at(colon)));
        k = colon + 1;
      }
    }
    let titleEnd = tags ? tagStart : end;
    while (titleEnd > p && isSpace(t.charCodeAt(titleEnd - 1))) titleEnd--;
    if (titleEnd > p) {
      const title = node("Title", at(p), at(titleEnd));
      this.inline(title.from, title.to, title.kids);
      kids.push(title);
    }
    if (tags) kids.push(tags);
    return node("Headline", ln.from, ln.end, kids);
  }

  // ---- elements ----

  /** Parse lines [lo, hi) as elements into kids. `ov` replaces line `ov.i` (the text after a list bullet or footnote label). */
  elements(lo, hi, kids, ov) {
    let i = lo;
    while (i < hi) {
      const ln = this.line(i, ov);
      if (ln.blank) {
        i++;
        continue;
      }
      i = this.element(i, hi, ln, kids, ov);
    }
  }

  element(i, hi, ln, kids, ov) {
    const t = ln.text;
    const c = t[ln.lead];
    let next = 0;
    if (c === "#") {
      next = this.block(i, hi, ln, kids) || this.keyword(i, ln, kids) || this.comment(i, ln, kids);
    } else if (c === ":") {
      next = this.drawer(i, hi, ln, kids) || this.fixedWidth(i, ln, kids);
    } else if (c === "|") {
      next = this.table(i, hi, kids, ov);
    } else if (c === "\\") {
      next = this.latexEnvironment(i, hi, ln, kids);
    } else if (c === "[" && ln.lead === 0) {
      next = this.footnoteDefinition(i, hi, ln, kids);
    } else if (c === "C") {
      next = this.clock(i, ln, kids) || this.planning(i, ln, kids);
    } else if (c === "S" || c === "D") {
      next = this.planning(i, ln, kids);
    } else if (c === "-" && HRULE.test(t)) {
      kids.push(node("HorizontalRule", ln.from + ln.lead, ln.end));
      next = i + 1;
    }
    if (!next && this.bulletOf(ln)) next = this.list(i, hi, kids, ov);
    return next || this.paragraph(i, hi, ln, kids);
  }

  bulletOf(ln) {
    BULLET.lastIndex = ln.lead;
    const m = BULLET.exec(ln.text);
    if (!m) return null;
    if (m[0] === "*" && ln.indent === 0) return null;
    return m[0];
  }

  paragraph(i, hi, ln, kids) {
    let j = i + 1;
    while (j < hi) {
      const next = this.lines[j];
      if (next.blank || INTERRUPT.test(next.text) || this.bulletOf(next) || (next.lead === 0 && FOOTNOTE_DEF.test(next.text))) break;
      j++;
    }
    const last = j - 1 === i ? ln : this.lines[j - 1];
    const para = node("Paragraph", ln.from + ln.lead, last.end);
    this.inline(para.from, para.to, para.kids);
    kids.push(para);
    return j;
  }

  comment(i, ln, kids) {
    if (!/^[ \t]*#(?:[ \t]|$)/.test(ln.text)) return 0;
    kids.push(node("Comment", ln.from + ln.lead, ln.end));
    return i + 1;
  }

  fixedWidth(i, ln, kids) {
    if (!/^[ \t]*:(?:[ \t]|$)/.test(ln.text)) return 0;
    kids.push(node("FixedWidth", ln.from + ln.lead, ln.end));
    return i + 1;
  }

  keyword(i, ln, kids) {
    const m = KEYWORD.exec(ln.text);
    if (!m) return 0;
    const name = m[2];
    const keyFrom = ln.from + m[1].length + 2;
    const key = node("KeywordKey", keyFrom, keyFrom + name.length + (m[3] ? m[3].length : 0));
    const kw = node("Keyword", ln.from + m[1].length, ln.end, [key]);
    const value = this.value(ln, m[0].length);
    if (value) {
      kw.kids.push(value);
      if (INLINE_KEYWORDS.has(name.toUpperCase())) this.inline(value.from, value.to, value.kids);
    }
    kids.push(kw);
    return i + 1;
  }

  /** A KeywordValue node for the trimmed text of ln from column `col`, or null. */
  value(ln, col) {
    let from = col;
    while (from < ln.end - ln.from && isSpace(ln.text.charCodeAt(from))) from++;
    if (ln.from + from >= ln.end) return null;
    return node("KeywordValue", ln.from + from, ln.end);
  }

  block(i, hi, ln, kids) {
    if (/^[ \t]*#\+end_\S+[ \t]*$/i.test(ln.text)) {
      // A stray end line is an element of its own.
      kids.push(node("BlockEnd", ln.from + ln.lead, ln.end));
      return i + 1;
    }
    const m = BEGIN.exec(ln.text);
    if (!m) return 0;
    const name = m[1];
    const lower = name.toLowerCase();
    const endText = `#+end_${lower}`;
    const j = this.findLine(`block:${lower}`, i, hi, (l) => l.end - l.from === l.lead + endText.length
      && l.text.slice(l.lead, l.lead + endText.length).toLowerCase() === endText);
    const nameFrom = ln.from + m[0].length - name.length;
    const begin = node("BlockBegin", ln.from + ln.lead, ln.end, [node("BlockName", nameFrom, nameFrom + name.length)]);
    let p = m[0].length;
    while (p < ln.end - ln.from && isSpace(ln.text.charCodeAt(p))) p++;
    if (ln.from + p < ln.end) {
      const params = node("BlockParameters", ln.from + p, ln.end);
      if (lower === "src" || lower === "export") {
        let q = p;
        while (q < ln.end - ln.from && !isSpace(ln.text.charCodeAt(q))) q++;
        params.kids.push(node("BlockLanguage", ln.from + p, ln.from + q));
      }
      begin.kids.push(params);
    }
    // An unclosed block runs to the end of the section or the document.
    let stop = j < 0 ? hi : j;
    if (j < 0) while (stop - 1 > i && this.lines[stop - 1].blank) stop--;
    const endLine = j < 0 ? null : this.lines[j];
    const block = node("Block", begin.from, endLine ? endLine.end : begin.to, [begin]);
    if (stop > i + 1) {
      const from = this.lines[i + 1].from;
      const to = this.lines[stop - 1].to;
      if (VERBATIM_BLOCKS.has(lower)) block.kids.push(node("VerbatimBody", from, to));
      else if (lower === "comment") block.kids.push(node("CommentBody", from, to));
      else {
        const body = node("BlockBody", from, to);
        this.elements(i + 1, stop, body.kids);
        block.kids.push(body);
      }
      if (!endLine) block.to = this.lines[stop - 1].end;
    }
    if (endLine) block.kids.push(node("BlockEnd", endLine.from + endLine.lead, endLine.end));
    kids.push(block);
    return j < 0 ? hi : j + 1;
  }

  drawer(i, hi, ln, kids) {
    const m = DRAWER.exec(ln.text);
    if (!m || m[2].toUpperCase() === "END") return 0;
    const properties = m[2].toUpperCase() === "PROPERTIES";
    const j = this.findLine("drawer", i, hi, (l) => /^[ \t]*:END:[ \t]*$/i.test(l.text));
    if (j < 0) return 0;
    const nameFrom = ln.from + m[1].length + 1;
    const endLine = this.lines[j];
    const drawer = node(properties ? "PropertyDrawer" : "Drawer", ln.from + m[1].length, endLine.end, [
      node("DrawerName", nameFrom, nameFrom + m[2].length),
    ]);
    if (properties) {
      for (let k = i + 1; k < j; k++) {
        const pl = this.lines[k];
        const pm = PROPERTY.exec(pl.text);
        if (!pm) continue;
        const keyFrom = pl.from + pm[1].length + 1;
        const prop = node("Property", pl.from + pm[1].length, pl.end, [node("PropertyKey", keyFrom, keyFrom + pm[2].length)]);
        if (pm[3] !== undefined && pm[3].trim() !== "") {
          const valueFrom = pl.from + pl.text.length - pm[3].length;
          const lead = pm[3].length - pm[3].trimStart().length;
          prop.kids.push(node("PropertyValue", valueFrom + lead, pl.end));
        }
        drawer.kids.push(prop);
      }
    } else {
      this.elements(i + 1, j, drawer.kids);
    }
    drawer.kids.push(node("DrawerEnd", endLine.from + endLine.lead, endLine.end));
    kids.push(drawer);
    return j + 1;
  }

  planning(i, ln, kids) {
    if (!PLANNING.test(ln.text)) return 0;
    const plan = node("Planning", ln.from + ln.lead, ln.end);
    const re = /(?:SCHEDULED|DEADLINE|CLOSED):/g;
    let pos = ln.from + ln.lead;
    let m;
    while ((m = re.exec(ln.text))) {
      const from = ln.from + m.index;
      if (from > pos) this.inline(pos, from, plan.kids);
      plan.kids.push(node("PlanningKeyword", from, from + m[0].length));
      pos = from + m[0].length;
    }
    if (ln.end > pos) this.inline(pos, ln.end, plan.kids);
    kids.push(plan);
    return i + 1;
  }

  clock(i, ln, kids) {
    if (!CLOCK.test(ln.text)) return 0;
    const from = ln.from + ln.lead;
    const clock = node("Clock", from, ln.end, [node("ClockKeyword", from, from + 6)]);
    const restFrom = from + 6;
    const rest = ln.text.slice(ln.lead + 6);
    const dur = /=>[ \t]*[\d:]+/.exec(rest);
    if (dur) {
      if (dur.index > 0) this.inline(restFrom, restFrom + dur.index, clock.kids);
      clock.kids.push(node("ClockDuration", restFrom + dur.index, restFrom + dur.index + dur[0].length));
    } else if (ln.end > restFrom) {
      this.inline(restFrom, ln.end, clock.kids);
    }
    kids.push(clock);
    return i + 1;
  }

  latexEnvironment(i, hi, ln, kids) {
    const m = LATEX_BEGIN.exec(ln.text);
    if (!m) return 0;
    const name = m[2];
    const endText = `\\end{${name}}`;
    const beginFrom = ln.from + m[1].length;
    const begin = node("LatexEnvBegin", beginFrom, ln.from + m[0].length, [
      node("LatexEnvName", beginFrom + 7, beginFrom + 7 + name.length),
    ]);
    const sameLine = ln.text.indexOf(endText, m[0].length);
    if (sameLine >= 0) {
      kids.push(node("LatexEnvironment", beginFrom, ln.from + sameLine + endText.length, [begin]));
      return i + 1;
    }
    const j = this.findLine(`latex:${name}`, i, hi, (l) => l.text.startsWith(endText, l.lead));
    if (j < 0) return 0;
    const endLine = this.lines[j];
    const endFrom = endLine.from + endLine.lead;
    kids.push(node("LatexEnvironment", beginFrom, endFrom + endText.length, [
      begin,
      node("LatexEnvEnd", endFrom, endFrom + endText.length),
    ]));
    return j + 1;
  }

  table(i, hi, kids, ov) {
    const first = this.line(i, ov);
    const table = node("Table", first.from + first.lead, first.end);
    let j = i;
    for (; j < hi; j++) {
      const ln = this.line(j, ov);
      if (!/^[ \t]*\|/.test(ln.text)) break;
      if (RULE_ROW.test(ln.text)) table.kids.push(node("TableRule", ln.from + ln.lead, ln.end));
      else table.kids.push(this.tableRow(ln));
    }
    for (; j < hi && /^[ \t]*#\+TBLFM:/i.test(this.lines[j].text); j++) {
      const ln = this.lines[j];
      const colon = ln.text.indexOf(":");
      const formula = node("TableFormula", ln.from + ln.lead, ln.end, [node("KeywordKey", ln.from + ln.lead + 2, ln.from + colon)]);
      const value = this.value(ln, colon + 1);
      if (value) formula.kids.push(value);
      table.kids.push(formula);
    }
    table.to = table.kids[table.kids.length - 1].to;
    kids.push(table);
    return j;
  }

  tableRow(ln) {
    const t = ln.text;
    const row = node("TableRow", ln.from + ln.lead, ln.end);
    const stop = ln.end - ln.from;
    const cell = (from, to) => {
      while (from < to && isSpace(t.charCodeAt(from))) from++;
      while (to > from && isSpace(t.charCodeAt(to - 1))) to--;
      const c = node("TableCell", ln.from + from, ln.from + to);
      if (to > from) this.inline(c.from, c.to, c.kids);
      row.kids.push(c);
    };
    let pos = ln.lead;
    row.kids.push(node("TableDelimiter", ln.from + pos, ln.from + pos + 1));
    pos++;
    while (pos < stop) {
      const bar = t.indexOf("|", pos);
      if (bar < 0 || bar >= stop) {
        cell(pos, stop);
        break;
      }
      cell(pos, bar);
      row.kids.push(node("TableDelimiter", ln.from + bar, ln.from + bar + 1));
      pos = bar + 1;
    }
    return row;
  }

  footnoteDefinition(i, hi, ln, kids) {
    const m = FOOTNOTE_DEF.exec(ln.text);
    if (!m) return 0;
    const { lines } = this;
    // A definition runs to the next definition or two blank lines.
    let j = i + 1;
    let blanks = 0;
    while (j < hi) {
      const next = lines[j];
      if (next.blank) {
        if (++blanks >= 2) break;
      } else {
        if (next.lead === 0 && FOOTNOTE_DEF.test(next.text)) break;
        blanks = 0;
      }
      j++;
    }
    let last = j;
    while (last - 1 > i && lines[last - 1].blank) last--;
    const def = node("FootnoteDefinition", ln.from, ln.end, [node("FootnoteLabel", ln.from + 4, ln.from + m[0].length - 1)]);
    this.elements(i, last, def.kids, { i, line: this.restLine(ln, m[0].length) });
    def.to = def.kids[def.kids.length - 1].to;
    kids.push(def);
    return j;
  }

  /** ln with its first `col` characters (and the blanks after them) removed. */
  restLine(ln, col) {
    let p = col;
    while (p < ln.text.length && (ln.text.charCodeAt(p) === 32 || ln.text.charCodeAt(p) === 9)) p++;
    return makeLine(ln.from + p, ln.to, ln.text.slice(p));
  }

  list(i, hi, kids, ov) {
    const { lines } = this;
    const first = this.line(i, ov);
    const base = first.indent;
    const list = node("List", first.from + first.lead, first.end);
    let k = i;
    while (k < hi) {
      const ln = this.line(k, ov);
      if (ln.indent !== base || !this.bulletOf(ln)) break;
      // The item runs to the next line indented no more than its bullet; one blank line does not end it.
      let scan = k + 1;
      let blanks = 0;
      let lastContent = k;
      while (scan < hi) {
        const next = lines[scan];
        if (next.blank) {
          if (++blanks >= 2) break;
        } else {
          if (next.indent <= base) break;
          blanks = 0;
          lastContent = scan;
        }
        scan++;
      }
      const itemEnd = lastContent + 1;
      list.kids.push(this.item(k, itemEnd, ln));
      list.to = list.kids[list.kids.length - 1].to;
      k = itemEnd;
      while (k < hi && lines[k].blank) k++;
      // Two blank lines end the list.
      if (k - itemEnd >= 2) break;
    }
    kids.push(list);
    return k;
  }

  item(i, end, ln) {
    const t = ln.text;
    const bullet = this.bulletOf(ln);
    const bFrom = ln.from + ln.lead;
    const item = node("ListItem", bFrom, ln.end, [node("ListBullet", bFrom, bFrom + bullet.length)]);
    let p = ln.lead + bullet.length;
    const skip = () => {
      while (p < t.length && (t.charCodeAt(p) === 32 || t.charCodeAt(p) === 9)) p++;
    };
    skip();
    CHECKBOX.lastIndex = p;
    if (CHECKBOX.exec(t)) {
      item.kids.push(node("Checkbox", ln.from + p, ln.from + p + 3));
      p += 3;
      skip();
    }
    if (!/\d/.test(bullet[0])) {
      // `term :: description`
      let at = t.indexOf("::", p);
      while (at >= 0 && !(at > p && isSpace(t.charCodeAt(at - 1)) && (at + 2 >= t.length || isSpace(t.charCodeAt(at + 2))))) {
        at = t.indexOf("::", at + 1);
      }
      if (at > p) {
        let termEnd = at;
        while (termEnd > p && isSpace(t.charCodeAt(termEnd - 1))) termEnd--;
        const tag = node("ListTag", ln.from + p, ln.from + termEnd);
        this.inline(tag.from, tag.to, tag.kids);
        item.kids.push(tag);
        p = at + 2;
        skip();
      }
    }
    this.elements(i, end, item.kids, { i, line: makeLine(ln.from + p, ln.to, t.slice(p)) });
    item.to = Math.max(item.to, item.kids[item.kids.length - 1].to);
    return item;
  }

  // ---- inline ----

  /** Parse the inline markup of text[from, to) into kids. */
  inline(from, to, kids) {
    const text = this.text;
    const cache = new Map();
    const failed = new Map();

    // The first `ch` at or after `at`, within the line (or the range, when
    // `cross`); -1 when there is none. One scan answers every later ask that
    // lands before the hit, so openers that fail do not rescan.
    const next = (ch, at, cross) => {
      const key = cross ? `${ch}+` : ch;
      const hit = cache.get(key);
      if (hit && at >= hit.start && at <= hit.at) return hit.found ? hit.at : -1;
      const limit = cross ? to : Math.min(to, this.lineEndAt(at));
      const code = ch.charCodeAt(0);
      let k = at;
      while (k < limit && text.charCodeAt(k) !== code) k++;
      const found = k < limit;
      cache.set(key, { start: at, at: k, found });
      return found ? k : -1;
    };

    // The first `ch` after `at` on the line that passes `ok`. A miss is
    // remembered: any later opener on the line would miss too.
    const closer = (key, ch, at, ok) => {
      if ((failed.get(key) ?? -1) > at) return -1;
      let k = at;
      for (;;) {
        const p = next(ch, k, false);
        if (p < 0) {
          failed.set(key, Math.min(to, this.lineEndAt(at)));
          return -1;
        }
        if (ok(p)) return p;
        k = p + 1;
      }
    };

    let i = from;
    while (i < to) {
      const ch = text[i];
      let end = 0;
      switch (ch) {
        case "*": case "/": case "_": case "+": case "=": case "~":
          end = this.emphasis(ch, i, from, to, kids, next);
          break;
        case "\\": end = this.backslash(i, to, kids, closer); break;
        case "$": end = this.dollar(i, to, kids, next, closer); break;
        case "<": end = this.angle(i, to, kids, next); break;
        case "[": end = this.bracket(i, to, kids, next); break;
        case "{":
          if (text.startsWith("{{{", i)) end = this.macro(i, to, kids, next);
          break;
        default:
          if (/[A-Za-z]/.test(ch) && !isAlnum(text[i - 1])) end = this.word(i, to, kids, next);
      }
      i = end > i ? end : i + 1;
    }
  }

  emphasis(marker, i, from, to, kids, next) {
    const text = this.text;
    // Opens at the start of the range or after whitespace or an opening bracket or quote.
    if (i > from && !/[\s({'"]/.test(text[i - 1])) return 0;
    const first = text[i + 1];
    if (i + 1 >= to || first === marker || /\s/.test(first)) return 0;
    const close = next(marker, i + 1, true);
    if (close < 0 || isSpace(text.charCodeAt(close - 1))) return 0;
    // Closes before whitespace, punctuation or the end of the range.
    if (close + 1 < to && !/[\s\p{P}]/u.test(text[close + 1])) return 0;
    const n = node(EMPHASIS[marker], i, close + 1, [node("EmphasisMark", i, i + 1)]);
    if (!RAW_EMPHASIS.has(marker)) this.inline(i + 1, close, n.kids);
    n.kids.push(node("EmphasisMark", close, close + 1));
    kids.push(n);
    return close + 1;
  }

  backslash(i, to, kids, closer) {
    const text = this.text;
    const c = text[i + 1];
    if (c === "(" || c === "[") {
      const [shut, name] = c === "(" ? [")", "InlineMath"] : ["]", "DisplayMath"];
      const p = closer(`\\${c}`, shut, i + 2, (k) => k > i + 2 && text[k - 1] === "\\");
      if (p < 0) return 0;
      kids.push(node(name, i, p + 1));
      return p + 1;
    }
    if (c === "\\") {
      let k = i + 2;
      while (k < to && (text[k] === " " || text[k] === "\t")) k++;
      if (k >= to || text[k] === "\n" || text[k] === "\r") kids.push(node("LineBreak", i, i + 2));
      return i + 2;
    }
    COMMAND.lastIndex = i;
    const m = COMMAND.exec(text);
    if (!m || i + m[0].length > to) return 0;
    kids.push(node(ENTITIES.has(m[1]) ? "Entity" : "LatexCommand", i, i + m[0].length));
    return i + m[0].length;
  }

  dollar(i, to, kids, next, closer) {
    const text = this.text;
    if (text[i + 1] === "$") {
      if (i + 2 >= to || /\s/.test(text[i + 2])) return 0;
      const p = closer("$$", "$", i + 3, (k) => text[k + 1] === "$" && !/\s/.test(text[k - 1]));
      if (p < 0) return 0;
      kids.push(node("DisplayMath", i, p + 2));
      return p + 2;
    }
    if (i + 1 >= to || /\s/.test(text[i + 1])) return 0;
    const p = next("$", i + 1, false);
    if (p < 0 || /\s/.test(text[p - 1]) || (p + 1 < to && isAlnum(text[p + 1]))) return 0;
    kids.push(node("InlineMath", i, p + 1));
    return p + 1;
  }

  angle(i, to, kids, next) {
    const text = this.text;
    const target = text.startsWith("<<<", i) ? RADIO_TARGET : text.startsWith("<<", i) ? TARGET : null;
    if (target) {
      target.lastIndex = i;
      const m = target.exec(text);
      if (!m || i + m[0].length > to) return 0;
      kids.push(node(target === RADIO_TARGET ? "RadioTarget" : "Target", i, i + m[0].length));
      return i + m[0].length;
    }
    DATE.lastIndex = i + 1;
    if (DATE.test(text)) {
      const p = next(">", i + 1, false);
      if (p < 0) return 0;
      kids.push(node("TimestampActive", i, p + 1));
      return p + 1;
    }
    ANGLE_LINK.lastIndex = i;
    const link = ANGLE_LINK.exec(text);
    if (!link || i + link[0].length > to) return 0;
    const end = i + link[0].length;
    kids.push(node("Link", i, end, [node("LinkTarget", i + 1, end - 1)]));
    return end;
  }

  bracket(i, to, kids, next) {
    const text = this.text;
    const c = text[i + 1];
    if (c === "[") {
      const p1 = next("]", i + 2, false);
      if (p1 < 0) return 0;
      const target = node("LinkTarget", i + 2, p1);
      if (text[p1 + 1] === "]") {
        kids.push(node("Link", i, p1 + 2, [target]));
        return p1 + 2;
      }
      if (text[p1 + 1] !== "[") return 0;
      const p2 = next("]", p1 + 2, false);
      if (p2 < 0 || text[p2 + 1] !== "]") return 0;
      const desc = node("LinkDescription", p1 + 2, p2);
      this.inline(desc.from, desc.to, desc.kids);
      kids.push(node("Link", i, p2 + 2, [target, desc]));
      return p2 + 2;
    }
    if (c === "f" && text.startsWith("[fn:", i)) {
      const p = next("]", i + 4, false);
      if (p < 0) return 0;
      const ref = node("FootnoteReference", i, p + 1);
      let k = i + 4;
      while (k < p && text[k] !== ":") k++;
      if (k > i + 4) ref.kids.push(node("FootnoteLabel", i + 4, k));
      if (k + 1 < p) this.inline(k + 1, p, ref.kids);
      kids.push(ref);
      return p + 1;
    }
    if (c === "c") {
      CITE_OPEN.lastIndex = i;
      const open = CITE_OPEN.exec(text);
      if (!open) return 0;
      const bodyFrom = i + open[0].length;
      const p = next("]", bodyFrom, false);
      if (p < 0) return 0;
      const cite = node("Citation", i, p + 1);
      const body = text.slice(bodyFrom, p);
      CITE_KEY.lastIndex = 0;
      for (let m; (m = CITE_KEY.exec(body)); ) {
        const key = m[1].replace(/[.:+/-]+$/, "");
        if (!key) continue;
        const keyFrom = bodyFrom + m.index + 1;
        cite.kids.push(node("CitationKey", keyFrom, keyFrom + key.length));
      }
      kids.push(cite);
      return p + 1;
    }
    DATE.lastIndex = i + 1;
    if (DATE.test(text)) {
      const p = next("]", i + 1, false);
      if (p < 0) return 0;
      kids.push(node("TimestampInactive", i, p + 1));
      return p + 1;
    }
    return 0;
  }

  macro(i, to, kids, next) {
    const text = this.text;
    const p = next("}", i + 3, false);
    if (p < 0 || p + 3 > to || text[p + 1] !== "}" || text[p + 2] !== "}") return 0;
    const m = node("Macro", i, p + 3);
    NAME.lastIndex = i + 3;
    const name = NAME.exec(text);
    if (name && i + 3 + name[0].length <= p) m.kids.push(node("MacroName", i + 3, i + 3 + name[0].length));
    kids.push(m);
    return p + 3;
  }

  /** Inline src, babel calls and bare links: constructs that open with a letter. */
  word(i, to, kids, next) {
    const text = this.text;
    if (text.startsWith("src_", i)) {
      NAME.lastIndex = i + 4;
      const name = NAME.exec(text);
      if (!name) return 0;
      let k = i + 4 + name[0].length;
      if (text[k] === "[") {
        const p = next("]", k + 1, false);
        if (p < 0) return 0;
        k = p + 1;
      }
      if (text[k] !== "{") return 0;
      const q = next("}", k + 1, false);
      if (q < 0) return 0;
      kids.push(node("InlineSrc", i, q + 1, [
        node("InlineSrcLanguage", i + 4, i + 4 + name[0].length),
        node("InlineSrcBody", k + 1, q),
      ]));
      return q + 1;
    }
    if (text.startsWith("call_", i)) {
      NAME.lastIndex = i + 5;
      const name = NAME.exec(text);
      if (!name) return 0;
      let k = i + 5 + name[0].length;
      if (text[k] === "[") {
        const p = next("]", k + 1, false);
        if (p < 0) return 0;
        k = p + 1;
      }
      if (text[k] !== "(") return 0;
      const q = next(")", k + 1, false);
      if (q < 0) return 0;
      k = q + 1;
      if (text[k] === "[") {
        const p = next("]", k + 1, false);
        if (p >= 0) k = p + 1;
      }
      kids.push(node("InlineBabelCall", i, k, [node("InlineBabelName", i + 5, i + 5 + name[0].length)]));
      return k;
    }
    PLAIN_LINK.lastIndex = i;
    const m = PLAIN_LINK.exec(text);
    if (!m) return 0;
    let end = Math.min(i + m[0].length, to);
    while (end > i && /[.,;:!?'")]/.test(text[end - 1])) end--;
    if (end <= i) return 0;
    kids.push(node("Link", i, end, [node("LinkTarget", i, end)]));
    return end;
  }
}

/** Write a node, children first, into the postorder buffer Tree.build reads. */
function emit(n, buffer) {
  const id = ID.get(n.name);
  if (id === undefined) throw new Error(`org-language: unknown node ${n.name}`);
  const start = buffer.length;
  let cursor = n.from;
  for (const kid of n.kids) {
    // Children are nested and ordered by construction; this keeps a slip from corrupting the tree.
    if (kid.from < cursor || kid.to > n.to || kid.to < kid.from) continue;
    emit(kid, buffer);
    cursor = kid.to;
  }
  buffer.push(id, n.from, n.to, buffer.length + 4 - start);
}

/** Write the children of `n`, and nothing else, into a Tree.build buffer. */
function build(n, topID, length) {
  const buffer = [];
  let cursor = n.from;
  for (const kid of n.kids) {
    if (kid.from < cursor) continue;
    emit(kid, buffer);
    cursor = kid.to;
  }
  return Tree.build({ buffer, nodeSet, topID, length });
}

const SECTION = ID.get("Section");
const DOCUMENT = ID.get("Document");
const HEAD_AT = /\*+(?=[ \t])/y;

/**
 * Where the document's top-level sections start: every headline whose level is
 * not deeper than the first headline of the section it would otherwise nest in.
 * @param {string} text
 * @returns {number[]}
 */
function sectionStarts(text) {
  const starts = [];
  let base = 0;
  const see = (at) => {
    HEAD_AT.lastIndex = at;
    const m = HEAD_AT.exec(text);
    if (m && (!starts.length || m[0].length <= base)) {
      starts.push(at);
      base = m[0].length;
    }
  };
  if (text.charCodeAt(0) === 42) see(0);
  for (let i = text.indexOf("\n*"); i >= 0; i = text.indexOf("\n*", i + 1)) see(i + 1);
  return starts;
}

/**
 * What a tree remembers so a later parse can reuse its pieces: the TODO words it
 * was parsed with and its chunks (the text before the first headline, then each
 * top-level section), each with the text span it was parsed from and the
 * children it contributed to the Document, positioned relative to the span start.
 * @typedef {{ from: number, to: number, children: readonly (Tree | import("@lezer/common").TreeBuffer)[], positions: readonly number[] }} Chunk
 * @type {WeakMap<Tree, { todo: Set<string>, chunks: Chunk[] }>}
 */
const memory = new WeakMap();

/** How the last parse went: chunks parsed fresh or taken from the previous tree. */
export const parseStats = { chunks: 0, reused: 0 };

const sameSet = (a, b) => a.size === b.size && [...a].every((w) => b.has(w));

/**
 * The syntax tree of an Org text. A section whose text is unchanged inside one
 * of `fragments` (and parsed with the same TODO words) is taken from the tree
 * the fragment came from instead of being parsed again.
 * @param {string} text
 * @param {readonly import("@lezer/common").TreeFragment[]} [fragments]
 * @returns {import("@lezer/common").Tree}
 */
export function parseOrg(text, fragments = []) {
  const todo = todoWords(text);
  const starts = sectionStarts(text);
  /** @type {Array<[number, number]>} */
  const spans = [];
  if (!starts.length) spans.push([0, text.length]);
  else {
    if (starts[0] > 0) spans.push([0, starts[0]]);
    starts.forEach((from, k) => spans.push([from, k + 1 < starts.length ? starts[k + 1] : text.length]));
  }
  const usable = new Map();
  const old = (fragment) => {
    let meta = usable.get(fragment.tree);
    if (meta === undefined) {
      meta = memory.get(fragment.tree);
      if (meta && !sameSet(meta.todo, todo)) meta = null;
      usable.set(fragment.tree, meta ?? null);
    }
    return meta;
  };
  const children = [];
  const positions = [];
  /** @type {Chunk[]} */
  const chunks = [];
  let fi = 0;
  let reused = 0;
  for (const [from, to] of spans) {
    let chunk = null;
    while (fi < fragments.length && fragments[fi].to < to) fi++;
    const fragment = fragments[fi];
    const meta = fragment && fragment.from <= from ? old(fragment) : null;
    if (meta) {
      const start = from + fragment.offset;
      let lo = 0;
      let hi = meta.chunks.length - 1;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (meta.chunks[mid].from < start) lo = mid + 1;
        else hi = mid;
      }
      const hit = meta.chunks[lo];
      if (hit && hit.from === start && hit.to === to + fragment.offset) {
        chunk = { from, to, children: hit.children, positions: hit.positions };
        reused++;
      }
    }
    if (!chunk) {
      // A chunk ends just before a headline line, so its last character is a line break.
      const part = new OrgParse(to < text.length ? text.slice(from, to - 1) : text.slice(from, to), todo).parse();
      if (part.kids.length === 1 && part.kids[0].name === "Section") {
        const section = build(part.kids[0], SECTION, part.kids[0].to);
        chunk = { from, to, children: [section], positions: [0] };
      } else {
        const tree = build(part, DOCUMENT, part.to);
        chunk = { from, to, children: tree.children, positions: tree.positions };
      }
    }
    chunks.push(chunk);
    chunk.children.forEach((child, i) => {
      children.push(child);
      positions.push(from + chunk.positions[i]);
    });
  }
  parseStats.chunks = spans.length;
  parseStats.reused = reused;
  const tree = new Tree(nodeSet.types[DOCUMENT], children, positions, text.length);
  memory.set(tree, { todo, chunks });
  return tree;
}

class OrgParser extends Parser {
  createParse(input, fragments) {
    let done = false;
    return {
      parsedPos: 0,
      stoppedAt: null,
      stopAt() {},
      advance() {
        if (done) return null;
        done = true;
        const tree = parseOrg(input.read(0, input.length), fragments);
        this.parsedPos = input.length;
        return tree;
      },
    };
  }
}

/** The Org parser: `parser.parse(text, fragments)` returns the Lezer tree. */
export const parser = new OrgParser();
