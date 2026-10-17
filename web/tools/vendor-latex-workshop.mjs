// Writes the LaTeX command and environment lists that completion offers,
// taken from LaTeX-Workshop (MIT, James Yu) and converted to CodeMirror's
// snippet syntax.
//
// Only data/commands.json and data/environments.json are read. Nothing under
// data/packages is used: those files are generated from GPL TeXstudio .cwl
// files, and the output must stay MIT. The commit is pinned, so the output
// changes only when this script is changed on purpose.
//
// Run from the repository root after changing the pin:
// node web/tools/vendor-latex-workshop.mjs
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const commit = "fb1d13108f1947028edb3d9881ed4843b983a3c8";
const base = `https://raw.githubusercontent.com/James-Yu/LaTeX-Workshop/${commit}/data/`;
const target = fileURLToPath(new URL("../src/lib/completion/latex-workshop.json", import.meta.url));
const NAME = /^[A-Za-z]+\*?$/;

async function fetchJson(name) {
  const response = await fetch(base + name);
  if (!response.ok) throw new Error(`${name}: HTTP ${response.status}`);
  return response.json();
}

// Converts one VS Code snippet to CodeMirror syntax, or returns null when it
// needs something CodeMirror cannot express. VS Code escapes only $, } and \.
// CodeMirror reads a backslash before a brace as an escape, so a literal
// backslash followed by a brace makes the snippet unconvertible.
function convertSnippet(template) {
  const text = template
    .replace(/\$\{TM_[A-Z_]+(?::[^{}]*)?\}/g, "")
    .replace(/\$TM_[A-Z_]+/g, "");
  let out = "";
  let slash = false;
  const literal = (char) => {
    if (char === "{" || char === "}") {
      if (slash) return false;
      // A brace right after $ or # would start a placeholder, so it is escaped.
      const previous = out.slice(-1);
      out += previous === "$" || previous === "#" ? `\\${char}` : char;
      slash = false;
    } else if (char === "\\") {
      out += char;
      slash = true;
    } else {
      out += char;
      slash = false;
    }
    return true;
  };
  for (let i = 0; i < text.length;) {
    const char = text[i];
    const next = text[i + 1];
    if (char === "\\" && (next === "$" || next === "}" || next === "\\")) {
      if (!literal(next)) return null;
      i += 2;
    } else if (char === "$" && /\d/.test(next || "")) {
      const [digits] = /^\$\d+/.exec(text.slice(i));
      out += `\${${digits.slice(1)}}`;
      slash = false;
      i += digits.length;
    } else if (char === "$" && next === "{") {
      const end = text.indexOf("}", i + 2);
      if (end < 0) return null;
      const body = text.slice(i + 2, end);
      if (/[{$\\]/.test(body)) return null;
      if (body === "") {
        out += "${}";
      } else {
        const match = /^(\d+)(?::([^{}]*)|\|([^|]*)\|)?$/.exec(body);
        if (!match) return null;
        const label = match[3] !== undefined ? match[3].split(",")[0] : match[2] || "";
        if (/[{}\\$]/.test(label)) return null;
        out += label ? `\${${match[1]}:${label}}` : `\${${match[1]}}`;
      }
      slash = false;
      i = end + 1;
    } else {
      if (!literal(char)) return null;
      i += 1;
    }
  }
  return out;
}

// A command entry is kept when its snippet starts with its own name. LaTeX-
// Workshop also has trigger keys such as "beginend" whose snippet is a
// different command.
function commandEntry(key, value) {
  const name = key.startsWith("\\") ? key.slice(1) : key;
  if (!NAME.test(name)) return null;
  const entry = { name };
  if (value.snippet !== undefined) {
    const snippet = convertSnippet(String(value.snippet));
    if (snippet === null) return null;
    const head = /^[A-Za-z]+\*?/.exec(snippet)?.[0] || "";
    if (head !== name) return null;
    entry.snippet = snippet;
  }
  if (typeof value.detail === "string") entry.detail = value.detail;
  if (typeof value.documentation === "string") entry.documentation = value.documentation;
  return entry;
}

function environmentEntry(item) {
  if (!NAME.test(item.name)) return null;
  const entry = { name: item.name };
  const raw = item.arg?.snippet;
  if (raw !== undefined) {
    const snippet = convertSnippet(String(raw));
    if (snippet === null) return null;
    entry.snippet = snippet;
  }
  return entry;
}

const [commandData, environmentData] = await Promise.all([
  fetchJson("commands.json"),
  fetchJson("environments.json"),
]);

const commands = [];
for (const [key, value] of Object.entries(commandData)) {
  const entry = commandEntry(key, value);
  if (entry) commands.push(entry);
}
commands.sort((a, b) => a.name.localeCompare(b.name));

const environments = [];
for (const item of environmentData) {
  const entry = environmentEntry(item);
  if (entry) environments.push(entry);
}
environments.sort((a, b) => a.name.localeCompare(b.name));

const dataset = {
  source: `James-Yu/LaTeX-Workshop@${commit}`,
  commands,
  environments,
};
writeFileSync(target, JSON.stringify(dataset) + "\n");
console.log(`latex-workshop: ${commands.length} commands, ${environments.length} environments -> ${target}`);
