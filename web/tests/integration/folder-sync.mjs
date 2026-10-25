// The folder sync contract, using the production client and fake HTTP.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import { formOf } from "../helpers/form-body.mjs";

const values = new Map();
const storage = { getItem: (key) => values.get(key) || null, setItem: (key, value) => values.set(key, String(value)), removeItem: (key) => values.delete(key) };
globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
client._testing.reset();
client._testing.inject({ storage, wait: () => Promise.resolve() });
client.configure({ project: "paper", origin: "https://app.test" });
client.setBindingId("folder-1");

function response(status, body) { return { ok: status >= 200 && status < 300, status, json: async () => body, clone() { return response(status, body); } }; }

const calls = [];
let answers = [];
client._testing.inject({ fetch: async (url, init = {}) => {
  const form = await formOf(init);
  const manifest = JSON.parse(await form.get("manifest").text()).manifest;
  calls.push({ url, method: init.method, manifest, files: form.getAll("file").map((file) => file.name).sort() });
  return answers.shift() || response(200, { kept: [], trashed: [], renamed: [] });
} });

const tree = (text) => ({ main: "a.tex", texts: { "a.tex": text, "b.tex": "b" }, assets: { "fig/dot.png": new Uint8Array([1, 2, 3]) } });

// The first call sends every file.
let result = await client.syncFolder({ tree: tree("one"), bindingId: "folder-1" });
assert.deepEqual(result, { kept: [], trashed: [], renamed: [] });
assert.equal(calls[0].method, "PUT");
assert.match(calls[0].url, /folder\?project=paper&binding=folder-1$/);
assert.deepEqual(calls[0].files, ["a.tex", "b.tex", "fig/dot.png"]);
assert.deepEqual(calls[0].manifest.map((file) => file.path).sort(), ["a.tex", "b.tex", "fig/dot.png"]);

// After one edit only that file is sent, and the manifest still lists all.
await client.syncFolder({ tree: tree("two"), bindingId: "folder-1" });
assert.deepEqual(calls[1].files, ["a.tex"]);
assert.equal(calls[1].manifest.length, 3);

// A 409 retries once with the first attempt's changed files plus the missing ones.
answers = [response(409, { missing: ["b.tex", "fig/dot.png"] })];
await client.syncFolder({ tree: tree("three"), bindingId: "folder-1" });
assert.equal(calls.length, 4);
assert.deepEqual(calls[2].files, ["a.tex"]);
assert.deepEqual(calls[3].files, ["a.tex", "b.tex", "fig/dot.png"]);
assert.deepEqual(calls[3].manifest, calls[2].manifest);

// A second 409 is an error, and the next call starts from nothing.
answers = [response(409, { missing: ["b.tex"] }), response(409, { missing: ["b.tex"] })];
await assert.rejects(client.syncFolder({ tree: tree("three"), bindingId: "folder-1" }), { name: "Refused" });
await client.syncFolder({ tree: tree("three"), bindingId: "folder-1" });
assert.deepEqual(calls.at(-1).files, ["a.tex", "b.tex", "fig/dot.png"]);

// A revoked folder is forgotten.
answers = [response(403, { error: "not granted" })];
assert.equal(await client.syncFolder({ tree: tree("four"), bindingId: "folder-1" }), null);
assert.equal(client.bindingId(), client.HOSTED_BINDING);
// A folder the client was told about sends a manifest and no file parts.
{
  const { createHash } = await import("node:crypto");
  const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
  const crlf = (text) => new TextEncoder().encode(text.replace(/\n/g, "\r\n"));
  const encode = (path, text) => (path === "w.tex" ? crlf(text) : new TextEncoder().encode(text));
  const open = { main: "w.tex", texts: { "w.tex": "one\ntwo\n", "b.tex": "b\n" }, assets: {} };
  client.acknowledge("folder-2", { "w.tex": sha(crlf("one\ntwo\n")), "b.tex": sha(new TextEncoder().encode("b\n")) });
  const before = calls.length;
  await client.syncFolder({ tree: open, bindingId: "folder-2", encode });
  assert.deepEqual(calls[before].files, [], "an untouched folder is sent no bytes");
  assert.equal(calls[before].manifest.find((file) => file.path === "w.tex").sha256, sha(crlf("one\ntwo\n")), "the digest is of the CRLF bytes");

  // An edit elsewhere leaves the CRLF file alone; an edit to it is sent with CRLF.
  await client.syncFolder({ tree: { ...open, texts: { ...open.texts, "b.tex": "b2\n" } }, bindingId: "folder-2", encode });
  assert.deepEqual(calls.at(-1).files, ["b.tex"]);
  await client.syncFolder({ tree: { ...open, texts: { "w.tex": "one\nthree\n", "b.tex": "b2\n" } }, bindingId: "folder-2", encode });
  assert.deepEqual(calls.at(-1).files, ["w.tex"]);
  assert.equal(calls.at(-1).manifest.find((file) => file.path === "w.tex").sha256, sha(crlf("one\nthree\n")));
}
console.log("folder-sync integration ok");
