// Writes the Quarto option lists that drive YAML and cell completion.
//
// quarto-cli keeps its editor schema in one JSON file, generated from the YAML
// schema it validates documents against. This script reads the copy at a
// pinned commit and keeps only what completion shows: the front matter keys
// (document-*), the code cell options (cell-*), their descriptions and their
// enumerated values. The result is src/lib/completion/quarto-schema.json,
// which is committed, so a build never needs the network. Source and license
// are in NOTICE-quarto.md beside it.
//
// Run by hand when the pin moves: node web/tools/vendor-quarto-schema.mjs
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const commit = "45cc5991938018ca5e69a92aa73d4daa504e05f1";
const source = `https://raw.githubusercontent.com/quarto-dev/quarto-cli/${commit}/src/resources/editor/tools/yaml/yaml-intelligence-resources.json`;
const target = fileURLToPath(new URL("../src/lib/completion/quarto-schema.json", import.meta.url));

// Top-level `format` of a Quarto document. The schema lists it per format
// (and the epub entry wins a first-come lookup), so its values come from here.
const FORMATS = ["beamer", "docx", "epub", "gfm", "html", "latex", "pdf", "pptx", "revealjs", "typst"];

const oneLine = (text) => String(text ?? "").replace(/\s+/g, " ").trim();
const description = (value) => oneLine(value && typeof value === "object" ? value.short : value);

// Collects enumerated values from a schema node. Strings only, deduped.
function collect(node, found, boolean) {
  if (node === "boolean") boolean.yes = true;
  if (!node || typeof node !== "object") return;
  if (Array.isArray(node.enum)) for (const value of node.enum) if (typeof value === "string") found.add(value);
  if (node.maybeArrayOf) collect(node.maybeArrayOf, found, boolean);
  for (const key of ["anyOf", "oneOf"]) {
    if (Array.isArray(node[key])) for (const item of node[key]) collect(item, found, boolean);
  }
}

function entryFor(item) {
  const found = new Set();
  const boolean = { yes: false };
  collect(item.schema, found, boolean);
  if (boolean.yes) { found.add("true"); found.add("false"); }
  return {
    name: item.name,
    description: description(item.description),
    values: [...found].sort(),
  };
}

// Keeps the first entry per name, drops hidden entries, sorts by name. The
// tag named by `tag` (formats or engine) becomes the list under `key`.
function build(entries, { key, tag }) {
  const byName = new Map();
  for (const item of entries) {
    if (!item || typeof item !== "object" || item.hidden === true || !item.name) continue;
    if (byName.has(item.name)) continue;
    const entry = entryFor(item);
    const tags = item.tags && typeof item.tags === "object" ? item.tags : {};
    const scope = tags[tag];
    const list = (Array.isArray(scope) ? scope : scope ? [scope] : []).filter((value) => typeof value === "string");
    byName.set(item.name, { ...entry, [key]: [...new Set(list)].sort() });
  }
  return [...byName.values()].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

const response = await fetch(source);
if (!response.ok) throw new Error(`GET ${source}: ${response.status}`);
const schema = await response.json();

const documentItems = [];
const cellItems = [];
for (const [key, value] of Object.entries(schema)) {
  if (!Array.isArray(value)) continue;
  if (key.startsWith("schema/document-")) documentItems.push(...value);
  else if (key.startsWith("schema/cell-")) cellItems.push(...value);
}

const document = build(documentItems, { key: "formats", tag: "formats" });
const formatEntry = {
  name: "format",
  description: "Output format the document renders to.",
  values: FORMATS,
  formats: [],
};
const documentWithFormat = document.some((entry) => entry.name === "format")
  ? document.map((entry) => (entry.name === "format" ? formatEntry : entry))
  : [...document, formatEntry].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));

const output = {
  source: `quarto-dev/quarto-cli@${commit}`,
  document: documentWithFormat,
  cell: build(cellItems, { key: "engines", tag: "engine" }),
};

const text = `${JSON.stringify(output, null, 2)}\n`;
writeFileSync(target, text);
console.log(`quarto ${commit.slice(0, 7)}: ${output.document.length} document keys, ${output.cell.length} cell options, ${text.length} bytes -> ${target}`);
