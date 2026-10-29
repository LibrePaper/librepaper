// The metadata of a LaTeX document: the commands that set its title block, and
// the plain text of their arguments. Nothing here imports CodeMirror, so the
// rules can be read and reasoned about on their own.

/// The commands that set the title block, or that the preamble fold reveals as a line.
export const METADATA_COMMANDS = [
  "title",
  "author",
  "date",
  "thanks",
  "affil",
  "affiliation",
  "address",
  "institute",
  "email",
  "subtitle",
];

// The commands whose argument is drawn as a line under the authors.
const AFFIL_COMMANDS = new Set(["affil", "affiliation", "address", "institute", "email"]);

// A metadata command, its optional arguments, and the opening brace of its mandatory one.
const METADATA_PATTERN = new RegExp(
  `\\\\(${METADATA_COMMANDS.join("|")})\\*?(?![a-zA-Z])\\s*(?:\\[[^\\]]*\\]\\s*)*\\{`,
  "g",
);

// The same comments as labels.js stripComments, blanked instead of removed, so
// that an offset into the blanked text is an offset into the source.
/// Blanks each % comment with spaces, keeping every offset where it was.
export function blankComments(text) {
  return text.replace(/((?:^|[^\\])(?:\\\\)*)%[^\n]*/gm, (match, kept) => kept + " ".repeat(match.length - kept.length));
}

/// The metadata commands in a source, in source order, that start before `before`.
/// Each is { name, from, to, argFrom, argTo }: `from` and `to` cover the whole
/// command, optional arguments included; `argFrom` and `argTo` are inside the braces.
export function findMetadata(source, before = source.length) {
  const blanked = blankComments(source);
  const found = [];
  for (const match of blanked.matchAll(METADATA_PATTERN)) {
    if (match.index >= before) break;
    const argFrom = match.index + match[0].length;
    const argTo = closingBrace(blanked, argFrom);
    if (argTo === -1) continue;
    found.push({ name: match[1], from: match.index, to: argTo + 1, argFrom, argTo });
  }
  return found;
}

/// The pieces of an author list, split at each top-level \and. Each piece is
/// { from, to } with the whitespace around it left out, and empty pieces are dropped.
export function authorSegments(source, argFrom, argTo) {
  const pieces = [];
  const add = (start, end) => {
    let from = start;
    let to = end;
    while (from < to && /\s/.test(source[from])) from++;
    while (to > from && /\s/.test(source[to - 1])) to--;
    if (to > from) pieces.push({ from, to });
  };

  let depth = 0;
  let start = argFrom;
  for (let i = argFrom; i < argTo; i++) {
    const character = source[i];
    if (character === "\\") {
      // A \and in the list's own braces splits it; a \and inside a group does not.
      if (depth === 0 && source.startsWith("\\and", i) && !/[a-zA-Z]/.test(source[i + 4] ?? "")) {
        add(start, i);
        start = i + 4;
        i += 3;
      } else {
        // The escaped character is skipped, so \{ and \} do not move the depth.
        i++;
      }
    } else if (character === "{") {
      depth++;
    } else if (character === "}") {
      depth--;
    }
  }
  add(start, argTo);
  return pieces;
}

/// The parts of the title block that \maketitle draws, in source order: the title,
/// each author, each affiliation line, and the date. Each part is
/// { kind, index, from, text }. `from` is where a click puts the caret (the start of
/// that command's argument), or null for a default that has no source to go to.
/// Only commands before `before` count.
export function titleParts(source, before = source.length) {
  const blanked = blankComments(source);
  const found = findMetadata(source, before);
  const lastOf = (name) => found.filter((entry) => entry.name === name).at(-1);
  const argument = (entry) => cleanTitle(blanked.slice(entry.argFrom, entry.argTo));

  const title = lastOf("title");
  const date = lastOf("date");

  const authors = found
    .filter((entry) => entry.name === "author")
    .flatMap((entry) => authorSegments(blanked, entry.argFrom, entry.argTo))
    .map((piece) => ({ from: piece.from, text: cleanTitle(blanked.slice(piece.from, piece.to)) }))
    .filter((piece) => piece.text !== "");

  const affils = found
    .filter((entry) => AFFIL_COMMANDS.has(entry.name))
    .map((entry) => ({ from: entry.argFrom, text: argument(entry) }))
    .filter((piece) => piece.text !== "");

  return [
    // A title with no \title command is the placeholder, and has nothing to click.
    { kind: "title", index: 0, from: title ? title.argFrom : null, text: title ? argument(title) : "" },
    ...authors.map((piece, index) => ({ kind: "author", index, ...piece })),
    ...affils.map((piece, index) => ({ kind: "affil", index, ...piece })),
    // LaTeX prints today's date when there is no \date; an empty \date prints nothing.
    { kind: "date", index: 0, from: date ? date.argFrom : null, text: date ? argument(date) : longToday() },
  ];
}

// The index of the brace that closes one opened just before `from`, or -1.
export function closingBrace(source, from) {
  let depth = 1;
  for (let i = from; i < source.length; i++) {
    const character = source[i];
    if (character === "\\") i++;
    else if (character === "{") depth++;
    else if (character === "}" && --depth === 0) return i;
  }
  return -1;
}

/// The text without every \command{...} of this name, braces and contents included.
export function withoutCommand(text, command) {
  const pattern = new RegExp(`\\\\${command}\\s*\\{`, "g");
  let out = "";
  let kept = 0;
  let match;
  while ((match = pattern.exec(text))) {
    const close = closingBrace(text, match.index + match[0].length);
    if (close === -1) break;
    out += text.slice(kept, match.index);
    kept = close + 1;
    pattern.lastIndex = kept;
  }
  return out + text.slice(kept);
}

/// Today's date in the reader's locale, in long form.
export function longToday() {
  return new Date().toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
}

/// Turns LaTeX source into plain text: formatting is unwrapped, commands and braces
/// are dropped, and ~ is a space. Titles, notes and quotations all read this way.
export function cleanTitle(text) {
  let out = withoutCommand(text, "thanks");
  out = out.replace(/\\\\\*?(?:\[[^\]]*\])?/g, "\n");
  out = out.replace(/\\and(?![a-zA-Z])/g, ", ");
  out = out.replace(/\\today(?![a-zA-Z])/g, longToday());

  // Formatting wrappers can nest, so they are unwrapped until none is left.
  const formatting = /\\(?:textbf|textit|emph|textsc|textrm|textsf|texttt|textsl|underline|mbox)\s*\{([^{}]*)\}/g;
  let previous;
  do {
    previous = out;
    out = out.replace(formatting, "$1");
  } while (out !== previous);

  out = out.replace(/\\([&%$#_{}])/g, "$1");
  out = out.replace(/\\[a-zA-Z]+\*?/g, "");
  out = out.replace(/[{}]/g, "");
  out = out.replace(/~/g, " ");
  return out
    .split("\n")
    .map((line) => line.replace(/\s+/g, " ").trim())
    .join("\n")
    .trim();
}
