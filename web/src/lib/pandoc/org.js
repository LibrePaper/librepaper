// Org text as Pandoc should read it. Pandoc's Org reader handles prose well,
// but a few things break a paper on the way to a PDF. For Typst, a bare LaTeX
// math environment is dropped, an internal link becomes an external link or a
// Typst call with no body, and a reference to a section fails because Typst
// only numbers headings when asked. For LaTeX, the math is kept as written, but
// an internal link is still best expressed as a cross-reference. This pass
// finds those on the Org syntax tree and rewrites the Org text into the forms
// Pandoc handles: for Typst, math wrapped in `$$` and an internal link as a
// citation (`[cite:@id]`, which Typst resolves as a reference); for LaTeX, an
// internal link as a `\autoref` export snippet. A link with a visible
// description becomes `[[#id][text]]` for both.
// Offsets are UTF-16 offsets into the text. Edits are applied back to front so
// the offsets of the ones still to apply stay valid.

import { orgTree } from "../source-trees.js";

/** @typedef {{ from: number, to: number, insert: string }} Edit */
/** @typedef {{ heading: boolean, title: string }} Target */

/// Environments Pandoc drops unless they sit inside `$$`.
const MATH = new Set(["equation", "align", "gather", "multline", "flalign", "alignat", "eqnarray", "math", "displaymath"]);
/// The math environments Typst numbers as equations once told to.
const NUMBERED = new Set(["equation", "align", "gather", "multline", "flalign", "alignat", "eqnarray"]);
/// Typst numbers equations only when this is set; the caller joins it into the header.
const EQUATION_NUMBERING = '#set math.equation(numbering: "(1)")';
const LABEL = /\\label\{([^}\n]+)\}/g;

/** Every node below `node`, in document order. @param {any} node */
function* descendants(node) {
  for (let child = node.firstChild; child; child = child.nextSibling) {
    yield child;
    yield* descendants(child);
  }
}

/** Whitespace folded and case dropped, so a `*Title` link matches a headline. @param {string} title */
function normalized(title) {
  return title.replace(/\s+/g, " ").trim().toLowerCase();
}

/**
 * The identifier Pandoc gives a heading without a CUSTOM_ID: lowercase, only
 * letters, digits, `_`, `-`, `.` and spaces kept, runs of spaces joined with
 * `-`, leading non-letters dropped, and `section` when nothing is left. A
 * repeat gets `-1`, `-2`, and so on.
 * @param {string} title
 * @param {Set<string>} taken identifiers already given, updated
 * @returns {string}
 */
function headingId(title, taken) {
  const base = title
    .toLowerCase()
    .replace(/[^\p{L}\p{N}_\-. \t]/gu, "")
    .trim()
    .split(/\s+/)
    .join("-")
    .replace(/^[^\p{L}]+/u, "") || "section";
  let id = base;
  for (let n = 1; taken.has(id); n += 1) id = `${base}-${n}`;
  taken.add(id);
  return id;
}

/**
 * Rewrites Org text for Pandoc's Org reader. `text` is one Org document and
 * `to` is "typst" (the default) or "latex", the format Pandoc will write.
 * Returns `{ text, numberSections, variables }`: `numberSections` is false only
 * for `num:nil` or `num:0`, and `variables` holds `sectionnumbering` for Typst
 * (Pandoc's Typst template reads it). For Typst, `headerIncludes` is present
 * only when a numbered equation needs a `#set` line.
 * @param {string} text
 * @param {{ to?: "typst" | "latex" }} [options]
 * @returns {{ text: string, numberSections: boolean, variables: Record<string, string>, headerIncludes?: string }}
 */
export function orgForPandoc(text, { to = "typst" } = {}) {
  const tree = orgTree(text);
  if (!tree) return { text, numberSections: true, variables: {} };
  const all = [...descendants(tree.topNode)];
  const of = (name) => all.filter((node) => node.name === name);
  const source = (node) => text.slice(node.from, node.to);

  /** @type {Map<string, Target>} identifier to what it names */
  const targets = new Map();
  /** @type {Map<string, string>} normalized headline title to its identifier, first wins */
  const titles = new Map();
  const taken = new Set();

  /** The CUSTOM_ID of a section's headline, or "" when it has none. */
  function customId(section) {
    for (const drawer of section.getChildren("PropertyDrawer")) {
      for (const property of drawer.getChildren("Property")) {
        const key = property.getChild("PropertyKey");
        if (!key || source(key).toUpperCase() !== "CUSTOM_ID") continue;
        const value = property.getChild("PropertyValue");
        return value ? source(value).trim() : "";
      }
    }
    return "";
  }

  // Headings, in document order, so generated identifiers match Pandoc's.
  for (const section of of("Section")) {
    const headline = section.getChild("Headline");
    if (!headline) continue;
    const titleNode = headline.getChild("Title");
    const title = titleNode ? source(titleNode) : "";
    const explicit = customId(section);
    const id = explicit || headingId(title, taken);
    if (!targets.has(id)) targets.set(id, { heading: true, title });
    const key = normalized(title);
    if (key && !titles.has(key)) titles.set(key, id);
  }

  const options = [];
  for (const keyword of of("Keyword")) {
    const key = source(keyword.getChild("KeywordKey")).replace(/\[.*$/, "").trim().toUpperCase();
    const value = keyword.getChild("KeywordValue");
    const trimmed = value ? source(value).trim() : "";
    // A #+NAME target is a figure, table or other element Pandoc labels.
    if (key === "NAME" && trimmed && !targets.has(trimmed)) targets.set(trimmed, { heading: false, title: "" });
    if (key === "OPTIONS") options.push(trimmed);
  }

  /** @type {Edit[]} */
  const edits = [];
  let numberedMath = false;
  for (const env of of("LatexEnvironment")) {
    const name = source(env.getChild("LatexEnvBegin").getChild("LatexEnvName")).trim();
    const base = name.replace(/\*$/, "");
    if (!MATH.has(base)) continue;
    // Pandoc drops a bare math environment for Typst; inside $$ it becomes display math.
    // LaTeX keeps the environment as written.
    if (to === "typst") {
      edits.push({ from: env.from, to: env.from, insert: "$$" });
      edits.push({ from: env.to, to: env.to, insert: "$$" });
      if (NUMBERED.has(base) && !name.endsWith("*")) numberedMath = true;
    }
    for (const match of source(env).matchAll(LABEL)) {
      if (!targets.has(match[1])) targets.set(match[1], { heading: false, title: "" });
    }
  }

  // Unnumbered sections: Typst cannot reference a heading, so a heading link
  // becomes a link with text. Pandoc's default for sections is numbered.
  const numbered = !options.some((value) => value.split(/\s+/).some((token) => token === "num:nil" || token === "num:0"));

  /** The target a link names, or null when it names nothing in this document. */
  const resolve = (target) => {
    let id = null;
    if (target.startsWith("#")) id = target.slice(1);
    else if (target.startsWith("*")) id = titles.get(normalized(target.slice(1))) ?? null;
    else id = target;
    if (!id || !targets.has(id)) return null;
    return { id, ...targets.get(id) };
  };

  for (const link of of("Link")) {
    // Only the [[...]] form: <scheme:...> and bare URLs are left as written.
    if (!source(link).startsWith("[[")) continue;
    const found = resolve(source(link.getChild("LinkTarget")).trim());
    if (!found) continue;
    const description = link.getChild("LinkDescription");
    let insert;
    if (description) {
      insert = `[[#${found.id}][${source(description)}]]`;
    } else if (found.heading && !numbered) {
      insert = `[[#${found.id}][${found.title || found.id}]]`;
    } else if (to === "latex") {
      insert = `@@latex:\\autoref{${found.id}}@@`;
    } else {
      insert = `[cite:@${found.id}]`;
    }
    edits.push({ from: link.from, to: link.to, insert });
  }

  let out = text;
  for (const edit of [...edits].sort((a, b) => b.from - a.from)) {
    out = out.slice(0, edit.from) + edit.insert + out.slice(edit.to);
  }

  if (to === "latex") return { text: out, numberSections: numbered, variables: {} };
  /** @type {{ text: string, numberSections: boolean, variables: Record<string, string>, headerIncludes?: string }} */
  const result = { text: out, numberSections: numbered, variables: numbered ? { sectionnumbering: "1.1" } : {} };
  if (numberedMath) result.headerIncludes = EQUATION_NUMBERING;
  return result;
}
