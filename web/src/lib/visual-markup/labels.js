// Numbers the labels of a LaTeX project the way the article, report and book
// classes would, so that a \ref can show the number a reader sees. The scan is
// one forward pass: the counters only move on, and a \label binds to the
// numbered thing it most recently follows. The same pass reports the citation
// keys in reading order and the numbers of the theorem-like environments.

// Commands and environments the scan reacts to. Groups: 1 and 2 are an include
// and its name; 3 and 4 are begin or end and the environment; 5 and 6 are a
// sectioning command and its star; 7 is appendix or caption; 8 is a label key;
// 9 is an item; 10 is a citation command.
const TOKENS = new RegExp(
  [
    String.raw`\\(input|include|subfile)\s*\{([^{}]*)\}`,
    String.raw`\\(begin|end)\s*\{([^{}]*)\}`,
    String.raw`\\(part|chapter|section|subsection|subsubsection)(\*?)(?![a-zA-Z])`,
    String.raw`\\(appendix|caption)(?![a-zA-Z])`,
    String.raw`\\label\s*\{([^{}]*)\}`,
    String.raw`\\(item)(?![a-zA-Z])`,
    String.raw`\\([A-Za-z]*[Cc]ite[A-Za-z]*)\*?`,
  ].join("|"),
  "g",
);

// Sectioning depth: 0 is the chapter (books), 1 the section, 2 the subsection.
const LEVELS = new Map([
  ["chapter", 0],
  ["section", 1],
  ["subsection", 2],
  ["subsubsection", 3],
]);

// Environments numbered once per equation, or once per row for the split ones.
const EQUATIONS = new Set(["equation", "align", "gather", "multline", "eqnarray"]);
const SPLIT_EQUATIONS = new Set(["align", "gather", "eqnarray"]);

// Floats and the kind of number their captions take.
const FLOATS = new Map([
  ["figure", "figure"],
  ["figure*", "figure"],
  ["wrapfigure", "figure"],
  ["table", "table"],
  ["table*", "table"],
]);

// Theorem-like environments numbered without any \newtheorem. A document that
// defines one of these names again replaces the default.
const DEFAULT_THEOREMS = [
  ["theorem", "Theorem"],
  ["lemma", "Lemma"],
  ["proposition", "Proposition"],
  ["corollary", "Corollary"],
  ["definition", "Definition"],
  ["assumption", "Assumption"],
  ["remark", "Remark"],
  ["example", "Example"],
  ["claim", "Claim"],
  ["conjecture", "Conjecture"],
  ["axiom", "Axiom"],
  ["hypothesis", "Hypothesis"],
];

// Lists whose items the scan numbers. Only an enumerate counts its items.
const LISTS = new Set(["enumerate", "itemize", "description"]);

const NEWTHEOREM = /\\newtheorem(\*?)\s*\{([^{}]*)\}\s*(?:\[([^\]]*)\])?\s*\{((?:[^{}]|\{[^{}]*\})*)\}\s*(?:\[([^\]]*)\])?/g;
const ITEM_OPTION = /\s*\[([^\]]*)\]/y;
const CITE_ARGUMENTS = /\s*(?:\[[^\]]*\]\s*)*\{([^{}]*)\}/y;

const ROW_TOKENS = /\\\\\*?(?:\[[^\]]*\])?|\\(begin|end)\s*\{[^{}]*\}/g;
const UNNUMBERED = /\\(?:nonumber|notag)(?![a-zA-Z])/;
const TAG = /\\tag\s*\{([^{}]*)\}/;
const LABEL = /\\label\s*\{([^{}]*)\}/g;

/// Strips % comments, keeping \% and an escaped backslash before a comment.
export function stripComments(text) {
  return text.replace(/((?:^|[^\\])(?:\\\\)*)%[^\n]*/gm, "$1");
}

// The same comments as stripComments, blanked instead of removed, so that an
// offset into the blanked text is an offset into the source.
function blankComments(text) {
  return text.replace(/((?:^|[^\\])(?:\\\\)*)%[^\n]*/gm, (match, kept) => kept + " ".repeat(match.length - kept.length));
}

// Joins and resolves . and .. so that paths compare as project paths.
function normalize(path) {
  const parts = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

function folderOf(path) {
  return path.slice(0, path.lastIndexOf("/") + 1);
}

// Chapters and appendix letters: A to Z, then the plain number.
function letter(n) {
  return n >= 1 && n <= 26 ? String.fromCharCode(64 + n) : String(n);
}

// The rows of an equation body, split at \\ outside nested environments.
function splitRows(body) {
  const rows = [];
  let depth = 0;
  let start = 0;
  for (const match of body.matchAll(ROW_TOKENS)) {
    if (match[1] === "begin") depth++;
    else if (match[1] === "end") depth--;
    else if (depth === 0) {
      rows.push(body.slice(start, match.index));
      start = match.index + match[0].length;
    }
  }
  rows.push(body.slice(start));
  return rows;
}

// The keys of a cite command, read from the text after its name. Optional
// arguments are skipped. The plural forms (\cites and the like) take one key
// list per mandatory argument, the others just the first.
function citedKeys(text, at, plural) {
  const keys = [];
  let next = at;
  for (;;) {
    CITE_ARGUMENTS.lastIndex = next;
    const match = CITE_ARGUMENTS.exec(text);
    if (!match) break;
    keys.push(...match[1].split(",").map((key) => key.trim()).filter((key) => key && key !== "*"));
    next = CITE_ARGUMENTS.lastIndex;
    if (!plural) break;
  }
  return keys;
}

/// The theorem-like environments of a project: the defaults, then each
/// \newtheorem in any text. A shared counter is the one of the environment it
/// names, and `within` is the sectioning command that resets the counter.
/// Returns a Map from environment name to { title, counter, numbered, within }.
export function theoremDefinitions(texts) {
  const theorems = new Map();
  for (const [name, title] of DEFAULT_THEOREMS) {
    theorems.set(name, { title, counter: name, numbered: true, within: null });
  }
  for (const path of Object.keys(texts)) {
    const text = stripComments(texts[path] ?? "");
    for (const match of text.matchAll(NEWTHEOREM)) {
      const name = match[2].trim();
      const shared = match[3]?.trim();
      const counter = shared ? (theorems.get(shared)?.counter ?? shared) : name;
      theorems.set(name, { title: match[4].trim(), counter, numbered: !match[1], within: match[5]?.trim() || null });
    }
  }
  return theorems;
}

/// Numbers every \label in a LaTeX project.
/// `texts` maps each project path (no leading slash) to its source, `main` is
/// the main file's path. Returns a Map from label key to { number, kind }, where
/// kind is 'section', 'equation', 'figure', 'table', 'theorem' or 'item'. Labels
/// with nothing numbered before them are left out.
///
/// Options: `theorems` (from theoremDefinitions, computed when missing), `open`
/// (the path of the file in the editor), `environments` (a Map that receives
/// the number of each numbered theorem-like \begin in the open file, keyed by
/// its offset in that file), `headings` (a Map that receives the number of each
/// numbered sectioning command in the open file, keyed by the offset of its
/// backslash), and `cited` (called with each citation key in reading order).
export function numberLabels(texts, main, options = {}) {
  const root = normalize(main);
  const theorems = options.theorems ?? theoremDefinitions(texts);
  const focus = options.open == null ? null : normalize(options.open);
  const environments = options.environments;
  const headings = options.headings;
  const cited = options.cited;
  const state = {
    labels: new Map(),
    hasChapter: false,
    appendix: false,
    counts: [0, 0, 0, 0],
    equations: 0,
    figures: 0,
    tables: 0,
    section: null,
    float: null,
    subfigures: 0,
    counters: new Map(), // theorem counter -> { count, depth }, depth null when global
    theorem: null, // the numbered theorem the scan is in
    theoremEnv: null,
    lists: [], // open lists, innermost last
    parts: 0,
  };

  // The top level is the chapter in books and the section in articles.
  const top = () => (state.hasChapter ? 0 : 1);

  // A counter that resets per chapter carries the chapter number in front.
  const scoped = (n) => {
    if (!state.hasChapter) return String(n);
    const chapter = state.appendix ? letter(state.counts[0]) : String(state.counts[0]);
    return `${chapter}.${n}`;
  };

  // The number of the sectioning level `depth`, with the chapter in front in books.
  const numberAt = (depth) => {
    const parts = [];
    for (let k = top(); k <= depth; k++) {
      parts.push(k === top() && state.appendix ? letter(state.counts[k]) : String(state.counts[k]));
    }
    return parts.join(".");
  };

  const sectioning = (name, depth) => {
    if (depth === 0) {
      state.hasChapter = true;
      state.equations = 0;
      state.figures = 0;
      state.tables = 0;
    }
    state.counts[depth] += 1;
    for (let k = depth + 1; k < 4; k++) state.counts[k] = 0;
    for (const counter of state.counters.values()) {
      if (counter.depth !== null && counter.depth >= depth) counter.count = 0;
    }
    state.section = { number: numberAt(depth), kind: "section" };
  };

  // The number a numbered heading shows: a part counts in Roman numerals, as
  // LaTeX prints it, and is not reset by the sections; a section or chapter
  // takes the number of its level.
  const heading = (name, at, path) => {
    let number;
    if (name === "part") {
      number = roman(++state.parts);
    } else {
      sectioning(name, LEVELS.get(name));
      number = state.section.number;
    }
    if (path === focus) headings?.set(at, number);
  };

  const appendix = () => {
    state.appendix = true;
    const first = top();
    for (let k = first; k < 4; k++) state.counts[k] = 0;
  };

  const displayed = (environment, body) => {
    const rows = SPLIT_EQUATIONS.has(environment) ? splitRows(body) : [body];
    for (const row of rows) {
      if (!row.trim()) continue;
      if (UNNUMBERED.test(row)) continue;
      const tag = TAG.exec(row);
      const number = tag ? tag[1].trim() : scoped(++state.equations);
      for (const match of row.matchAll(LABEL)) {
        state.labels.set(match[1].trim(), { number, kind: "equation" });
      }
    }
  };

  // Text up to the matching \end, and the index just past it.
  const bodyOf = (text, from, environment) => {
    const close = `\\end{${environment}}`;
    const end = text.indexOf(close, from);
    if (end === -1) return { body: text.slice(from), next: text.length };
    return { body: text.slice(from, end), next: end + close.length };
  };

  // A theorem-like environment. Numbered ones advance their counter, which
  // resets with the sectioning level it is within. A starred name, or a
  // \newtheorem*, is unnumbered.
  const theoremAt = (environment) => {
    if (theorems.has(environment)) return theorems.get(environment);
    const base = environment.endsWith("*") ? theorems.get(environment.slice(0, -1)) : null;
    return base ? { ...base, numbered: false } : null;
  };
  const theoremBegins = (environment, index, path) => {
    const definition = theoremAt(environment);
    if (!definition || !definition.numbered) return;
    const counter = state.counters.get(definition.counter) ?? { count: 0, depth: null };
    counter.count += 1;
    counter.depth = LEVELS.get(definition.within) ?? null;
    state.counters.set(definition.counter, counter);
    const head = counter.depth === null ? "" : numberAt(counter.depth);
    const number = head ? `${head}.${counter.count}` : String(counter.count);
    state.theorem = { number, kind: "theorem" };
    state.theoremEnv = environment;
    if (path === focus) environments?.set(index, number);
  };

  // \item counts in an enumerate, and a label after it takes its number. An
  // optional label replaces the number and does not count. Other lists give
  // their items no number, so a label there takes the enclosing number.
  const item = (text, at) => {
    const list = state.lists.at(-1);
    if (!list) return;
    if (list.env !== "enumerate") {
      list.item = null;
      return;
    }
    ITEM_OPTION.lastIndex = at;
    const option = ITEM_OPTION.exec(text);
    if (option) {
      const shown = option[1].trim();
      list.item = shown ? { number: shown, kind: "item" } : null;
      return;
    }
    list.count += 1;
    list.item = { number: String(list.count), kind: "item" };
  };

  const citation = (text, at, name) => {
    // \nocite adds to the bibliography without citing anything in the text.
    if (!cited || name === "nocite") return;
    for (const key of citedKeys(text, at, /cites$/i.test(name))) cited(key);
  };

  const closing = (environment) => {
    if (environment === "subfigure") state.subfigures = Math.max(0, state.subfigures - 1);
    else if (state.float && FLOATS.has(environment) && state.float.env === environment) state.float = null;
    else if (environment === state.theoremEnv) {
      state.theorem = null;
      state.theoremEnv = null;
    } else if (LISTS.has(environment) && state.lists.at(-1)?.env === environment) state.lists.pop();
  };

  const caption = () => {
    if (!state.float || state.subfigures > 0) return;
    const float = state.float;
    const n = float.kind === "figure" ? ++state.figures : ++state.tables;
    float.current = { number: scoped(n), kind: float.kind };
  };

  // As in LaTeX, a \label in a float before its \caption takes the section, and
  // one in a theorem or an item takes the theorem or the item it is in.
  const label = (key) => {
    const target = state.float?.current ?? state.theorem ?? state.lists.at(-1)?.item ?? state.section;
    if (target) state.labels.set(key, target);
  };

  // Resolves an include name against the main file's folder, then the
  // including file's folder. Returns a path, or null when nothing exists.
  const resolve = (name, from) => {
    const target = name.trim();
    const withExtension = /\.tex$/.test(target) ? target : `${target}.tex`;
    for (const folder of [folderOf(root), folderOf(from)]) {
      const path = normalize(folder + withExtension);
      if (typeof texts[path] === "string") return path;
    }
    return null;
  };

  const include = (name, from, stack) => {
    const path = resolve(name, from);
    if (!path || stack.includes(path)) return;
    stack.push(path);
    scan(path, blankComments(texts[path]), stack);
    stack.pop();
  };

  const scan = (path, text, stack, from = 0) => {
    const tokens = new RegExp(TOKENS.source, "g");
    tokens.lastIndex = from;
    let match;
    while ((match = tokens.exec(text))) {
      if (match[1]) {
        include(match[2], path, stack);
      } else if (match[3] === "end") {
        closing(match[4]);
      } else if (match[3] === "begin") {
        const environment = match[4];
        if (EQUATIONS.has(environment)) {
          const { body, next } = bodyOf(text, tokens.lastIndex, environment);
          displayed(environment, body);
          tokens.lastIndex = next;
        } else if (FLOATS.has(environment)) {
          state.float = { env: environment, kind: FLOATS.get(environment), current: null };
        } else if (environment === "subfigure") {
          state.subfigures += 1;
        } else if (LISTS.has(environment)) {
          state.lists.push({ env: environment, count: 0, item: null });
        } else {
          theoremBegins(environment, match.index, path);
        }
      } else if (match[5]) {
        if (!match[6]) heading(match[5], match.index, path);
      } else if (match[7] === "appendix") {
        appendix();
      } else if (match[7] === "caption") {
        caption();
      } else if (match[8] !== undefined) {
        label(match[8].trim());
      } else if (match[9]) {
        item(text, tokens.lastIndex);
      } else if (match[10]) {
        citation(text, tokens.lastIndex, match[10]);
      }
    }
  };

  if (typeof texts[root] !== "string") return state.labels;

  // Only the body is scanned: the preamble holds no numbered text. The text is
  // blanked rather than cut, so that offsets still point into the source.
  const body = blankComments(texts[root]);
  const start = body.indexOf("\\begin{document}");
  scan(root, body, [root], start === -1 ? 0 : start + "\\begin{document}".length);
  return state.labels;
}

/// A number in upper-case Roman numerals, as LaTeX numbers parts and Typst's "I" pattern.
export function roman(n) {
  if (n < 1) return String(n);
  /** @type {Array<[number, string]>} */
  const table = [
    [1000, "M"], [900, "CM"], [500, "D"], [400, "CD"], [100, "C"], [90, "XC"],
    [50, "L"], [40, "XL"], [10, "X"], [9, "IX"], [5, "V"], [4, "IV"], [1, "I"],
  ];
  let rest = n;
  let out = "";
  for (const [value, digits] of table) {
    while (rest >= value) {
      out += digits;
      rest -= value;
    }
  }
  return out;
}
