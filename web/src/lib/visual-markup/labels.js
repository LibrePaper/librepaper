// Numbers the labels of a LaTeX project the way the article, report and book
// classes would, so that a \ref can show the number a reader sees. The scan is
// one forward pass: the counters only move on, and a \label binds to the
// numbered thing it most recently follows.

// Commands and environments the scan reacts to. Groups: 1 and 2 are an include
// and its name; 3 and 4 are begin or end and the environment; 5 and 6 are a
// sectioning command and its star; 7 is appendix or caption; 8 is a label key.
const TOKENS = new RegExp(
  [
    String.raw`\\(input|include|subfile)\s*\{([^{}]*)\}`,
    String.raw`\\(begin|end)\s*\{([^{}]*)\}`,
    String.raw`\\(chapter|section|subsection|subsubsection)(\*?)(?![a-zA-Z])`,
    String.raw`\\(appendix|caption)(?![a-zA-Z])`,
    String.raw`\\label\s*\{([^{}]*)\}`,
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

const ROW_TOKENS = /\\\\\*?(?:\[[^\]]*\])?|\\(begin|end)\s*\{[^{}]*\}/g;
const UNNUMBERED = /\\(?:nonumber|notag)(?![a-zA-Z])/;
const TAG = /\\tag\s*\{([^{}]*)\}/;
const LABEL = /\\label\s*\{([^{}]*)\}/g;

/// Strips % comments, keeping \% and an escaped backslash before a comment.
export function stripComments(text) {
  return text.replace(/((?:^|[^\\])(?:\\\\)*)%[^\n]*/gm, "$1");
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

/// Numbers every \label in a LaTeX project.
/// `texts` maps each project path (no leading slash) to its source, `main` is
/// the main file's path. Returns a Map from label key to { number, kind }, where
/// kind is 'section', 'equation', 'figure' or 'table'. Labels with nothing
/// numbered before them are left out.
export function numberLabels(texts, main) {
  const root = normalize(main);
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
  };

  // The top level is the chapter in books and the section in articles.
  const top = () => (state.hasChapter ? 0 : 1);

  // A counter that resets per chapter carries the chapter number in front.
  const scoped = (n) => {
    if (!state.hasChapter) return String(n);
    const chapter = state.appendix ? letter(state.counts[0]) : String(state.counts[0]);
    return `${chapter}.${n}`;
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
    const parts = [];
    for (let k = top(); k <= depth; k++) {
      parts.push(k === top() && state.appendix ? letter(state.counts[k]) : String(state.counts[k]));
    }
    state.section = { number: parts.join("."), kind: "section" };
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

  // As in LaTeX, a \label in a float before its \caption takes the section.
  const label = (key) => {
    const target = state.float?.current ?? state.section;
    if (target) state.labels.set(key, target);
  };

  const caption = () => {
    if (!state.float || state.subfigures > 0) return;
    const float = state.float;
    const n = float.kind === "figure" ? ++state.figures : ++state.tables;
    float.current = { number: scoped(n), kind: float.kind };
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
    scan(path, stripComments(texts[path]), stack);
    stack.pop();
  };

  const scan = (path, text, stack) => {
    const tokens = new RegExp(TOKENS.source, "g");
    let match;
    while ((match = tokens.exec(text))) {
      if (match[1]) {
        include(match[2], path, stack);
      } else if (match[3] === "end") {
        if (match[4] === "subfigure") state.subfigures = Math.max(0, state.subfigures - 1);
        else if (state.float && FLOATS.has(match[4]) && state.float.env === match[4]) state.float = null;
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
        }
      } else if (match[5]) {
        if (!match[6]) sectioning(match[5], LEVELS.get(match[5]));
      } else if (match[7] === "appendix") {
        appendix();
      } else if (match[7] === "caption") {
        caption();
      } else if (match[8] !== undefined) {
        label(match[8].trim());
      }
    }
  };

  if (typeof texts[root] !== "string") return state.labels;

  let body = stripComments(texts[root]);
  const start = body.indexOf("\\begin{document}");
  if (start !== -1) body = body.slice(start + "\\begin{document}".length);
  scan(root, body, [root]);
  return state.labels;
}
