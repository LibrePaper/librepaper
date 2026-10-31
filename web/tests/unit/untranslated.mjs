// Interface text goes through messages/en.json, never straight into markup.
//
// Every text node and every text attribute a person reads (title, aria-label,
// placeholder, alt, label) in a component must be a message call, or a word
// that is the same in every language: a product name, a format name, a
// keyboard key. A literal sentence here would show in English to everybody,
// whatever language they chose.
//
// The messages files are checked too: the French has every English key, with
// the same placeholders.
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "svelte/compiler";

const ROOT = fileURLToPath(new URL("../../", import.meta.url));
const SRC = join(ROOT, "src");

// Text that reads the same in every language.
const SAME_EVERYWHERE = new Set([
  "LibrePaper", "LaTeX", "Typst", "Quarto", "Markdown", "HTML", "PDF", "DOCX", "Word", "Zotero", "GitHub",
  "Git", "GitHub Action", "Harper", "Pandoc", "BibTeX", "Biber", "Vim", "Emacs", "Claude Code", "Codex", "TeX Live", "KaTeX",
  "Ctrl", "Shift", "Alt", "Cmd", "Enter", "Esc", "Tab", "F2", "OK", "B", "I", "U", "i", "x", "MB", "px", "min",
]);
const ATTRIBUTES = new Set(["title", "aria-label", "aria-description", "placeholder", "alt", "label"]);

const files = (dir) => readdirSync(dir).flatMap((name) => {
  const path = join(dir, name);
  return statSync(path).isDirectory() ? files(path) : [path];
});

// A string needs translating when it holds a run of two or more letters that
// is not one of the shared words.
const needsMessage = (text) => {
  if (/^https?:\/\/\S+$/.test(text)) return false;
  let rest = text;
  for (const word of [...SAME_EVERYWHERE].sort((a, b) => b.length - a.length)) rest = rest.split(word).join(" ");
  return /\p{L}{2,}/u.test(rest);
};

const found = [];
for (const path of files(join(SRC, "components")).filter((p) => p.endsWith(".svelte"))) {
  const source = readFileSync(path, "utf8");
  const where = relative(ROOT, path).split(sep).join("/");
  const walk = (node) => {
    if (!node || typeof node !== "object") return;
    if (Array.isArray(node)) return node.forEach(walk);
    if (node.type === "Text" && node.data && needsMessage(node.data.trim())) {
      found.push(`${where}:${source.slice(0, node.start).split("\n").length}: ${JSON.stringify(node.data.trim().slice(0, 60))}`);
    }
    if (node.type === "Attribute" || node.type?.endsWith("Directive") || node.type === "SpreadAttribute") {
      if (node.type === "Attribute" && ATTRIBUTES.has(node.name) && Array.isArray(node.value)) {
        const text = node.value.filter((part) => part.type === "Text").map((part) => part.data).join(" ").trim();
        if (needsMessage(text)) found.push(`${where}:${source.slice(0, node.start).split("\n").length}: ${node.name}=${JSON.stringify(text.slice(0, 60))}`);
      }
      return;
    }
    for (const [key, value] of Object.entries(node)) if (key !== "parent" && key !== "instance" && key !== "module" && key !== "css") walk(value);
  };
  walk(parse(source, { modern: true }).fragment);
}
assert.deepEqual(found, [], `interface text written straight into components; move it to messages/en.json:\n${found.join("\n")}`);

// The two catalogues agree.
const english = JSON.parse(readFileSync(join(ROOT, "messages/en.json"), "utf8"));
const french = JSON.parse(readFileSync(join(ROOT, "messages/fr.json"), "utf8"));
const placeholders = (value) => [...new Set([...JSON.stringify(value).matchAll(/\{([A-Za-z_]\w*)\}/g)].map((m) => m[1]))].sort().join(",");
const problems = [];
for (const key of Object.keys(english)) {
  if (!(key in french)) problems.push(`${key}: no French`);
  else if (placeholders(english[key]) !== placeholders(french[key])) problems.push(`${key}: placeholders differ`);
}
for (const key of Object.keys(french)) if (!(key in english)) problems.push(`${key}: French only`);
assert.deepEqual(problems, [], problems.join("\n"));

console.log("untranslated: every component string is a message, and the French matches the English");
