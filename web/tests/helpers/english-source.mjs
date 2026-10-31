// A component's source with each argument-free message call put back as its
// English text, for the tests that read a source file and check its shape.
//
// The interface text lives in messages/en.json now, so a check like "the
// menu offers Project" would otherwise have to name a message key. Markup
// `{m.key()}` becomes the text, an attribute `={m.key()}` becomes `="text"`,
// and any other `m.key()` becomes a quoted string. Calls with arguments are
// left alone.
import { readFileSync } from "node:fs";

const english = JSON.parse(readFileSync(new URL("../../messages/en.json", import.meta.url), "utf8"));
const text = (key) => (typeof english[key] === "string" ? english[key] : null);

export function englishSource(source) {
  return source
    .replace(/=\{m\.([a-z][a-z0-9_]*)\(\)\}/g, (all, key) => (text(key) == null ? all : `="${text(key)}"`))
    .replace(/\{m\.([a-z][a-z0-9_]*)\(\)\}/g, (all, key) => text(key) ?? all)
    .replace(/\bm\.([a-z][a-z0-9_]*)\(\)/g, (all, key) => (text(key) == null ? all : JSON.stringify(text(key))));
}
