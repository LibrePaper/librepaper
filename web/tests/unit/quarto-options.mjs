import assert from "node:assert/strict";
import {
  clearRenderOptions,
  loadRenderOptions,
  parseRenderOptions,
  renderOptionsStorageKey,
  saveRenderOptions,
} from "../../src/lib/quarto-options.js";
import { parameterSha256 } from "../../src/lib/engines/quarto.js";

assert.deepEqual(parseRenderOptions(), { format: "default", profile: "", parameters: {} });
assert.deepEqual(parseRenderOptions({ format: "pdf", profile: "  nightly  ", parameters: '{"seed":1,"enabled":true,"label":"1","missing":null}' }), {
  format: "pdf", profile: "nightly", parameters: { seed: 1, enabled: true, label: "1", missing: null },
});
assert.deepEqual(parseRenderOptions({ format: "revealjs", parameters: { number: 1.0 } }).parameters, { number: 1 });
assert.equal(await parameterSha256(parseRenderOptions({ parameters: '{"A":1,"a":1e-3,"_":true,"Z":"é😀","number":2.5e4}' }).parameters), "aeea5bcdd2f603f88bb8d6387ce656ebe5b6a3560947363a369b736071bb8b3d");
const specialKey = parseRenderOptions({ parameters: '{"__proto__":1,"_ok":true}' }).parameters;
assert.equal(Object.prototype.hasOwnProperty.call(specialKey, "__proto__"), true);
assert.equal(specialKey.__proto__, 1);
assert.deepEqual(parseRenderOptions({ parameters: " \n\t " }).parameters, {});
assert.throws(() => parseRenderOptions({ format: "html; rm -rf" }), /format/);
assert.throws(() => parseRenderOptions({ profile: "unsafe/profile" }), /profile/);
assert.equal(parseRenderOptions({ profile: "\u00a0" }).profile, "");
assert.throws(() => parseRenderOptions({ parameters: "[]" }), /object/);
assert.throws(() => parseRenderOptions({ parameters: '{"nested":{}}' }), /scalar/);
assert.throws(() => parseRenderOptions({ parameters: '{"seed":' }), /valid JSON/);
assert.throws(() => parseRenderOptions({ parameters: { seed: -0 } }), /portable range/);
assert.throws(() => parseRenderOptions({ parameters: { seed: Number.MAX_SAFE_INTEGER + 1 } }), /portable range/);
assert.throws(() => parseRenderOptions({ parameters: { seed: Infinity } }), /portable range/);
assert.throws(() => parseRenderOptions({ parameters: { seed: "x".repeat(16 * 1024 + 1) } }), /too large/);
assert.doesNotThrow(() => parseRenderOptions({ parameters: { seed: "😀".repeat(4 * 1024) } }));
assert.throws(() => parseRenderOptions({ parameters: { ["a".repeat(129)]: true } }), /name/);
assert.throws(() => parseRenderOptions({ parameters: Object.fromEntries(Array.from({ length: 129 }, (_, i) => [`p${i}`, i])) }), /128/);

const values = new Map();
const storage = {
  getItem(key) { return values.get(key) ?? null; },
  setItem(key, value) { values.set(key, value); },
  removeItem(key) { values.delete(key); },
};
const scope = { documentId: "paper", origin: "https://writer.example", storage };
const saved = { format: "docx", profile: "nightly", parameters: { seed: 2 } };
assert.equal(saveRenderOptions({ ...scope, options: saved }), true);
assert.deepEqual(loadRenderOptions(scope), saved);
assert.notEqual(renderOptionsStorageKey("paper", "https://writer.example"), renderOptionsStorageKey("other", "https://writer.example"));
assert.notEqual(renderOptionsStorageKey("paper", "https://other.example"), renderOptionsStorageKey("paper", "https://writer.example"));
assert.deepEqual(loadRenderOptions({ ...scope, documentId: "other" }), { format: "default", profile: "", parameters: {} });

values.set(renderOptionsStorageKey(scope), "not json");
assert.deepEqual(loadRenderOptions(scope), { format: "default", profile: "", parameters: {} });
values.set(renderOptionsStorageKey(scope), JSON.stringify({ version: 1, options: { parameters: '{"nested":[]}' } }));
assert.deepEqual(loadRenderOptions(scope), { format: "default", profile: "", parameters: {} });
values.set(renderOptionsStorageKey(scope), "x".repeat(512 * 1024 + 1));
assert.deepEqual(loadRenderOptions(scope), { format: "default", profile: "", parameters: {} });
assert.equal(clearRenderOptions(scope), true);
assert.deepEqual(loadRenderOptions(scope), { format: "default", profile: "", parameters: {} });

const failingStorage = {
  getItem() { throw new Error("storage denied"); },
  setItem() { throw new Error("quota"); },
  removeItem() { throw new Error("storage denied"); },
};
assert.deepEqual(loadRenderOptions({ ...scope, storage: failingStorage }), { format: "default", profile: "", parameters: {} });
assert.equal(saveRenderOptions({ ...scope, storage: failingStorage, options: saved }), false);
assert.equal(clearRenderOptions({ ...scope, storage: failingStorage }), false);
assert.equal(saveRenderOptions({
  ...scope,
  options: { parameters: Object.fromEntries(Array.from({ length: 128 }, (_, i) => [`p${i}`, "x".repeat(4096)])) },
}), false);

console.log("quarto options: scalar parsing, numeric bounds, scoped persistence, and storage failures passed");
