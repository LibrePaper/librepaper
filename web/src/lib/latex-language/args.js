// Command scanning for the consumers that read LaTeX facts (reference numbers,
// macros, metadata). The Lezer parse says where the commands are, so comments,
// \verb spans and verbatim bodies never count and `\begin {x}` is an
// environment. The arguments are then read from the source with comments
// blanked, with balanced braces, so a nested group is part of its argument.

import { latexTree } from "../source-trees.js";

const BEGIN_TAGS = new Set(["BeginEnv", "MathBeginEnv", "VerbBeginEnv", "ListingBeginEnv"]);

/// The index of the first non-blank character at or after `at`.
export function skipSpace(text, at) {
  let i = at;
  while (i < text.length && /\s/.test(text[i])) i++;
  return i;
}

/// The balanced {...} group that opens at `at`: { value, from, to, next }, where
/// `value` is the inner text and `to` and `next` are the index after the close.
/// Null when there is no closed group there.
export function braced(text, at) {
  if (text[at] !== "{") return null;
  let depth = 0;
  for (let i = at; i < text.length; i++) {
    if (text[i] === "\\") i++;
    else if (text[i] === "{") depth++;
    else if (text[i] === "}" && --depth === 0) return { value: text.slice(at + 1, i), from: at, to: i + 1, next: i + 1 };
  }
  return null;
}

/// The [...] argument that opens at `at`, up to the first ] outside braces.
/// Same shape as braced; null when there is none.
export function bracketed(text, at) {
  if (text[at] !== "[") return null;
  let depth = 0;
  for (let i = at + 1; i < text.length; i++) {
    const char = text[i];
    if (char === "\\") i++;
    else if (char === "{") depth++;
    else if (char === "}") depth = Math.max(0, depth - 1);
    else if (char === "]" && depth === 0) return { value: text.slice(at + 1, i), from: at, to: i + 1, next: i + 1 };
  }
  return null;
}

/// The group that follows a command directly, blanks allowed. Null if none.
export function groupAfter(text, at) {
  return braced(text, skipSpace(text, at));
}

/// The arguments that follow a command, in order: a star is skipped, then each
/// {group} or [optional] is read until something else comes. Each is
/// { kind: "group" | "optional", value, from, to }.
export function readArguments(text, at) {
  const args = [];
  let i = skipSpace(text, at);
  if (text[i] === "*") i = skipSpace(text, i + 1);
  for (;;) {
    const group = text[i] === "{";
    const arg = group ? braced(text, i) : bracketed(text, i);
    if (!arg) break;
    args.push({ kind: group ? "group" : "optional", value: arg.value, from: arg.from, to: arg.to });
    i = skipSpace(text, arg.next);
  }
  return args;
}

/// The entries of a key=value list, split at commas outside braces. A value
/// wrapped in one pair of braces loses them. The first entry of a key wins.
export function keyValues(list) {
  const out = new Map();
  let depth = 0;
  let start = 0;
  const add = (end) => {
    const entry = list.slice(start, end);
    const at = entry.indexOf("=");
    if (at === -1) return;
    const key = entry.slice(0, at).trim();
    let value = entry.slice(at + 1).trim();
    if (value.startsWith("{") && braced(value, 0)?.to === value.length) value = value.slice(1, -1).trim();
    if (key && !out.has(key)) out.set(key, value);
  };
  for (let i = 0; i < list.length; i++) {
    if (list[i] === "\\") i++;
    else if (list[i] === "{") depth++;
    else if (list[i] === "}") depth--;
    else if (list[i] === "," && depth === 0) {
      add(i);
      start = i + 1;
    }
  }
  add(list.length);
  return out;
}

/**
 * The commands of a LaTeX source in reading order, and the source with its
 * comments blanked (offsets do not move). Each command is one of
 *   { type: "command", name, star, from, to }  a control word without its backslash
 *   { type: "begin" | "end", name, from, to }  an environment tag
 *   { type: "break", from, to }                a \\ line break
 * `to` is the end of the token, where its arguments start.
 * @param {string} text
 * @param {string} [key] usually the file path; lets an edit reuse the old parse
 */
export function latexCommands(text, key) {
  const tree = latexTree(text, key);
  const comments = [];
  const nodes = [];
  tree.iterate({
    enter(ref) {
      const name = ref.name;
      if (name === "Comment") {
        comments.push([ref.from, ref.to]);
      } else if (name === "ControlSequence" || name === "SectionName" || name === "ControlSymbol") {
        nodes.push({ name, from: ref.from, to: ref.to });
      } else if (name === "EndEnv" || BEGIN_TAGS.has(name)) {
        const env = ref.node.getChild("EnvName");
        nodes.push({ name: name === "EndEnv" ? "end" : "begin", from: ref.from, to: ref.to, env: env ? text.slice(env.from, env.to).trim() : "" });
      }
    },
  });

  let code = "";
  let cursor = 0;
  for (const [from, to] of comments) {
    code += text.slice(cursor, from) + " ".repeat(to - from);
    cursor = to;
  }
  code += text.slice(cursor);

  const commands = [];
  for (const node of nodes) {
    if (node.name === "begin" || node.name === "end") {
      commands.push({ type: node.name, name: node.env, from: node.from, to: node.to });
    } else if (node.name === "ControlSymbol") {
      if (text.slice(node.from, node.to) === "\\\\") commands.push({ type: "break", from: node.from, to: node.to });
    } else {
      let name = text.slice(node.from + 1, node.to);
      let star = code[node.to] === "*";
      if (name.endsWith("*")) {
        name = name.slice(0, -1);
        star = true;
      }
      if (name === "begin" || name === "end") {
        // An \end the parser left as a plain control word still closes.
        const group = groupAfter(code, node.to);
        if (group) commands.push({ type: name, name: group.value.trim(), from: node.from, to: group.to });
        continue;
      }
      commands.push({ type: "command", name, star, from: node.from, to: node.to });
    }
  }
  return { code, commands };
}

/// The text with each % comment blanked to spaces, found on the parse.
export function blankedComments(text, key) {
  return latexCommands(text, key).code;
}
