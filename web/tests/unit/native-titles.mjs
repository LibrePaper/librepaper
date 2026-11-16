// One tooltip everywhere, so no component hands the browser a title attribute.
//
// A native title looks different from the application's tooltip, appears after
// a delay the browser chooses and never to the keyboard. An element that wants
// a tooltip carries data-tip instead (see lib/tip.js).
//
// Lowercase tags are checked, and so are Skeleton parts written with a dot
// (Menu.Item, Tabs.Trigger, ...), because those forward title to the DOM. A
// component without a dot (Modal, SettingRow, IconButton) takes title as an
// ordinary prop and is not checked. Frames and form options have no tooltip of
// their own to replace, and the documentation site is a separate bundle.
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../../", import.meta.url));
const SRC = join(ROOT, "src");
const SITE = join(SRC, "site");
const EXEMPT = new Set(["iframe", "object", "option", "title"]);

const files = (dir) => readdirSync(dir).flatMap((name) => {
  const path = join(dir, name);
  if (path === SITE || name === "paraglide") return [];
  return statSync(path).isDirectory() ? files(path) : path.endsWith(".svelte") ? [path] : [];
});

// Script, style and comments are blanked, keeping their newlines, so line
// numbers survive and code such as `a <b` is not read as a tag.
const blank = (text) => text.replace(/<!--[\s\S]*?-->|<(script|style)\b[\s\S]*?<\/\1>/g, (part) => part.replace(/[^\n]/g, " "));

// The attribute text of a tag, ending at the first `>` outside quotes and braces.
function attributesFrom(text, start) {
  let depth = 0;
  let quote = null;
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === quote && depth === 0) quote = null;
      else if (c === "{") depth++;
      else if (c === "}") depth--;
    } else if (c === "{") depth++;
    else if (c === "}") depth--;
    else if (depth === 0 && (c === '"' || c === "'")) quote = c;
    else if (depth === 0 && c === ">") return text.slice(start, i);
  }
  return text.slice(start);
}

function nativeTitles(source) {
  const text = blank(source);
  const found = [];
  const tag = /<([A-Za-z][\w.:-]*)/g;
  let match;
  while ((match = tag.exec(text))) {
    const name = match[1];
    const native = /^[a-z]/.test(name) ? !EXEMPT.has(name) && !name.startsWith("svelte:") : name.includes(".");
    if (!native) continue;
    if (/(?:^|\s)title(?:=|\s|$)|(?:^|\s)\{title\}/.test(attributesFrom(text, tag.lastIndex))) {
      found.push({ line: text.slice(0, match.index).split("\n").length, name });
    }
  }
  return found;
}

test("no component puts a native title attribute on an element", () => {
  const found = [];
  for (const path of files(SRC)) {
    const where = relative(ROOT, path).split(sep).join("/");
    for (const { line, name } of nativeTitles(readFileSync(path, "utf8"))) found.push(`${where}:${line}: <${name}> has a title attribute`);
  }
  assert.deepEqual(found, [], `use data-tip instead of title (see lib/tip.js):\n${found.join("\n")}`);
});

test("the scanner reads multi-line tags and component parts", () => {
  const source = '<button\n  class="a"\n  title={m.x({ a: "b" })}\n>x</button>\n<Menu.Item title="t" />\n<Modal title="t" />\n<iframe title="f"></iframe>\n<svelte:head><title>T</title></svelte:head>';
  assert.deepEqual(nativeTitles(source), [{ line: 1, name: "button" }, { line: 5, name: "Menu.Item" }]);
});
