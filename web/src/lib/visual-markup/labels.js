// Numbers the labels of a LaTeX project the way the article, report and book
// classes would, so that a \ref can show the number a reader sees. The scan is
// one forward pass: the counters only move on, and a \label binds to the
// numbered thing it most recently follows. The same pass reports the citation
// keys in reading order and the numbers of the theorem-like environments.

import { bracketed, groupAfter, latexCommands, readArguments, skipSpace } from "../latex-language/args.js";

// The control words the scan reacts to. Includes name a file; the sectioning
// commands start a numbered heading (the star is checked apart).
const INCLUDES = new Set(["input", "include", "subfile"]);
const CITE_COMMAND = /^[A-Za-z]*[Cc]ite[A-Za-z]*$/;

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

const UNNUMBERED = new Set(["nonumber", "notag"]);

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

// The keys of a cite command, read from the text after its name. Optional
// arguments are skipped. The plural forms (\cites and the like) take one key
// list per mandatory argument, the others just the first.
function citedKeys(code, at, plural) {
  const keys = [];
  for (const arg of readArguments(code, at)) {
    if (arg.kind !== "group") continue;
    keys.push(...arg.value.split(",").map((key) => key.trim()).filter((key) => key && key !== "*"));
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
    const { code, commands } = latexCommands(texts[path] ?? "", path);
    for (const command of commands) {
      if (command.type !== "command" || command.name !== "newtheorem") continue;
      // \newtheorem*{name}[shared]{title}[within]
      const args = readArguments(code, command.to);
      let k = 0;
      if (args[k]?.kind !== "group") continue;
      const name = args[k++].value.trim();
      const shared = args[k]?.kind === "optional" ? args[k++].value.trim() : "";
      if (args[k]?.kind !== "group") continue;
      const title = args[k++].value.trim();
      const within = args[k]?.kind === "optional" ? args[k].value.trim() : "";
      const counter = shared ? (theorems.get(shared)?.counter ?? shared) : name;
      theorems.set(name, { title, counter, numbered: !command.star, within: within || null });
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
/// backslash), `captions` (a Map that receives the number of each figure's
/// \caption in the open file, keyed by the offset of its backslash), and `cited` (called with each citation key in reading order).
export function numberLabels(texts, main, options = {}) {
  const root = normalize(main);
  const theorems = options.theorems ?? theoremDefinitions(texts);
  const focus = options.open == null ? null : normalize(options.open);
  const environments = options.environments;
  const headings = options.headings;
  const captions = options.captions;
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

  // The numbered equations of an equation environment. `inner` holds the
  // commands between its \begin and \end, `from` and `to` the body's offsets.
  // The split environments take a row per \\ outside nested environments.
  const displayed = (environment, code, from, to, inner) => {
    const rows = [];
    let start = from;
    if (SPLIT_EQUATIONS.has(environment)) {
      let depth = 0;
      for (const command of inner) {
        if (command.type === "begin") depth++;
        else if (command.type === "end") depth--;
        else if (command.type === "break" && depth === 0) {
          rows.push([start, command.from]);
          start = command.to;
          if (code[start] === "*") start++;
          const option = bracketed(code, start);
          if (option) start = option.next;
        }
      }
    }
    rows.push([start, to]);
    for (const [rowFrom, rowTo] of rows) {
      if (!code.slice(rowFrom, rowTo).trim()) continue;
      const here = inner.filter((command) => command.type === "command" && command.from >= rowFrom && command.from < rowTo);
      if (here.some((command) => UNNUMBERED.has(command.name))) continue;
      const tagCommand = here.find((command) => command.name === "tag");
      const tag = tagCommand && groupAfter(code, tagCommand.to);
      const number = tag ? tag.value.trim() : scoped(++state.equations);
      for (const command of here) {
        if (command.name !== "label") continue;
        const key = groupAfter(code, command.to);
        if (key) state.labels.set(key.value.trim(), { number, kind: "equation" });
      }
    }
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
    const option = bracketed(text, skipSpace(text, at));
    if (option) {
      const shown = option.value.trim();
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

  const caption = (at, path) => {
    if (!state.float || state.subfigures > 0) return;
    const float = state.float;
    const n = float.kind === "figure" ? ++state.figures : ++state.tables;
    float.current = { number: scoped(n), kind: float.kind };
    if (float.kind === "figure" && path === focus) captions?.set(at, float.current.number);
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
    scan(path, stack);
    stack.pop();
  };

  // Walks the commands of one file from offset `from`.
  const scan = (path, stack, from = 0) => {
    const { code, commands } = latexCommands(texts[path], path);
    for (let i = 0; i < commands.length; i++) {
      const command = commands[i];
      if (command.from < from) continue;
      if (command.type === "end") {
        closing(command.name);
      } else if (command.type === "begin") {
        const environment = command.name;
        if (EQUATIONS.has(environment)) {
          let end = i + 1;
          while (end < commands.length && !(commands[end].type === "end" && commands[end].name === environment)) end++;
          const to = end < commands.length ? commands[end].from : code.length;
          displayed(environment, code, command.to, to, commands.slice(i + 1, end));
          i = end;
        } else if (FLOATS.has(environment)) {
          state.float = { env: environment, kind: FLOATS.get(environment), current: null };
        } else if (environment === "subfigure") {
          state.subfigures += 1;
        } else if (LISTS.has(environment)) {
          state.lists.push({ env: environment, count: 0, item: null });
        } else {
          theoremBegins(environment, command.from, path);
        }
      } else if (command.type === "command") {
        const name = command.name;
        if (INCLUDES.has(name)) {
          const target = groupAfter(code, command.to);
          if (target) include(target.value, path, stack);
        } else if (name === "part" || LEVELS.has(name)) {
          if (!command.star) heading(name, command.from, path);
        } else if (name === "appendix") {
          appendix();
        } else if (name === "caption") {
          caption(command.from, path);
        } else if (name === "label") {
          const key = groupAfter(code, command.to);
          if (key) label(key.value.trim());
        } else if (name === "item") {
          item(code, command.to);
        } else if (CITE_COMMAND.test(name)) {
          citation(code, command.to, name);
        }
      }
    }
  };

  if (typeof texts[root] !== "string") return state.labels;

  // Only the body is scanned: the preamble holds no numbered text. Commands
  // keep their offsets in the source.
  const begin = latexCommands(texts[root], root).commands.find((command) => command.type === "begin" && command.name === "document");
  scan(root, [root], begin ? begin.to : 0);
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
