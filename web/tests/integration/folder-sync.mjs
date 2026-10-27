// The folder sync contract, using the production client and fake HTTP.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import { formOf } from "../helpers/form-body.mjs";

globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
client._testing.reset();
client._testing.inject({ wait: () => Promise.resolve() });
client.configure({ project: "paper", origin: "https://app.test" });

function response(status, body) { return { ok: status >= 200 && status < 300, status, json: async () => body, clone() { return response(status, body); } }; }

const calls = [];
let answers = [];
client._testing.inject({ fetch: async (url, init = {}) => {
  const form = await formOf(init);
  const body = JSON.parse(await form.get("manifest").text());
  calls.push({ url, method: init.method, manifest: body.manifest, opening: body.opening, files: form.getAll("file").map((file) => file.name).sort() });
  return answers.shift() || response(200, { kept: [], trashed: [], renamed: [] });
} });

import { createHash } from "node:crypto";
const dot = new Uint8Array([1, 2, 3]);
const dotSha = createHash("sha256").update(dot).digest("hex");
const tree = (text) => ({ main: "a.tex", texts: { "a.tex": text, "b.tex": "b" }, digests: { "fig/dot.png": dotSha } });
// The figure store of the page: sizes are known, and bytes are asked for by path.
const asked = [];
const assets = {
  size: (sha) => (sha === dotSha ? dot.length : undefined),
  bytes: async (digests) => { asked.push(Object.keys(digests).sort()); return Object.fromEntries(Object.keys(digests).map((path) => [path, dot])); },
};

// The first call sends every file.
let result = await client.syncFolder({ tree: tree("one"), bindingId: "folder-1", assets });
assert.deepEqual(result, { kept: [], trashed: [], renamed: [] });
assert.equal(calls[0].method, "PUT");
assert.match(calls[0].url, /folder\?project=paper&binding=folder-1$/);
assert.deepEqual(calls[0].files, ["a.tex", "b.tex", "fig/dot.png"]);
assert.deepEqual(calls[0].manifest.map((file) => file.path).sort(), ["a.tex", "b.tex", "fig/dot.png"]);
assert.equal(calls[0].opening, undefined, "a save is not the opening one unless it says so");

// After one edit only that file is sent, and the manifest still lists all.
await client.syncFolder({ tree: tree("two"), bindingId: "folder-1", assets });
assert.deepEqual(calls[1].files, ["a.tex"]);
assert.equal(calls[1].manifest.length, 3);

// A 409 retries once with the first attempt's changed files plus the missing ones.
answers = [response(409, { missing: ["b.tex", "fig/dot.png"] })];
await client.syncFolder({ tree: tree("three"), bindingId: "folder-1", assets });
assert.equal(calls.length, 4);
assert.deepEqual(calls[2].files, ["a.tex"]);
assert.deepEqual(calls[3].files, ["a.tex", "b.tex", "fig/dot.png"]);
assert.deepEqual(calls[3].manifest, calls[2].manifest);

// A second 409 is an error, and the next call starts from nothing.
answers = [response(409, { missing: ["b.tex"] }), response(409, { missing: ["b.tex"] })];
await assert.rejects(client.syncFolder({ tree: tree("three"), bindingId: "folder-1", assets }), { name: "Refused" });
await client.syncFolder({ tree: tree("three"), bindingId: "folder-1", assets });
assert.deepEqual(calls.at(-1).files, ["a.tex", "b.tex", "fig/dot.png"]);

// A folder the client was told about sends a manifest and no file parts.
{
  const { createHash } = await import("node:crypto");
  const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
  const crlf = (text) => new TextEncoder().encode(text.replace(/\n/g, "\r\n"));
  const encode = (path, text) => (path === "w.tex" ? crlf(text) : new TextEncoder().encode(text));
  const open = { main: "w.tex", texts: { "w.tex": "one\ntwo\n", "b.tex": "b\n" }, digests: {} };
  client.acknowledge("folder-2", { "w.tex": sha(crlf("one\ntwo\n")), "b.tex": sha(new TextEncoder().encode("b\n")) });
  const before = calls.length;
  await client.syncFolder({ tree: open, bindingId: "folder-2", encode, assets });
  assert.deepEqual(calls[before].files, [], "an untouched folder is sent no bytes");
  assert.equal(calls[before].manifest.find((file) => file.path === "w.tex").sha256, sha(crlf("one\ntwo\n")), "the digest is of the CRLF bytes");

  // An edit elsewhere leaves the CRLF file alone; an edit to it is sent with CRLF.
  await client.syncFolder({ tree: { ...open, texts: { ...open.texts, "b.tex": "b2\n" } }, bindingId: "folder-2", encode, assets });
  assert.deepEqual(calls.at(-1).files, ["b.tex"]);
  await client.syncFolder({ tree: { ...open, texts: { "w.tex": "one\nthree\n", "b.tex": "b2\n" } }, bindingId: "folder-2", encode, assets });
  assert.deepEqual(calls.at(-1).files, ["w.tex"]);
  assert.equal(calls.at(-1).manifest.find((file) => file.path === "w.tex").sha256, sha(crlf("one\nthree\n")));
}

// The first save after open sends no bytes for files the folder already holds,
// and never asks the page for a figure it does not have to send.
{
  const sha = (text) => createHash("sha256").update(text).digest("hex");
  const held = { "a.tex": sha("one"), "b.tex": sha("b"), "fig/dot.png": dotSha };
  const before = calls.length;
  const asks = asked.length;
  await client.syncFolder({ tree: tree("one"), bindingId: "folder-4", opening: true, assets, onDisk: (path) => held[path] ?? null });
  assert.deepEqual(calls[before].files, [], "nothing the folder holds is uploaded");
  assert.deepEqual(calls[before].manifest.map((file) => file.path).sort(), ["a.tex", "b.tex", "fig/dot.png"]);
  assert.equal(calls[before].manifest.find((file) => file.path === "fig/dot.png").size, dot.length, "the size comes from the store");
  assert.equal(asked.length, asks, "no figure bytes were asked for");

  // An edited text goes, and a figure that changed goes with bytes fetched on demand.
  const other = new Uint8Array([4, 4]);
  const otherSha = createHash("sha256").update(other).digest("hex");
  const edited = { main: "a.tex", texts: { "a.tex": "two", "b.tex": "b" }, digests: { "fig/dot.png": otherSha } };
  const sized = { size: () => other.length, bytes: async (digests) => { asked.push(Object.keys(digests).sort()); return Object.fromEntries(Object.keys(digests).map((path) => [path, other])); } };
  await client.syncFolder({ tree: edited, bindingId: "folder-4", assets: sized, onDisk: (path) => held[path] ?? null });
  assert.deepEqual(calls.at(-1).files, ["a.tex", "fig/dot.png"]);
  assert.deepEqual(asked.at(-1), ["fig/dot.png"]);

  // A figure the folder holds is not asked for even when the desktop app has not acknowledged it,
  // and a 409 for it asks for the bytes after all.
  const again = calls.length;
  answers = [response(409, { missing: ["fig/dot.png"] })];
  await client.syncFolder({ tree: tree("one"), bindingId: "folder-5", assets, onDisk: (path) => held[path] ?? null });
  assert.equal(calls.length, again + 2);
  assert.deepEqual(calls[again].files, []);
  assert.deepEqual(calls[again + 1].files, ["fig/dot.png"]);
  assert.deepEqual(asked.at(-1), ["fig/dot.png"]);
}

// The opening save says so in the manifest part, and so does its retry after a 409.
// The answer carries the paths the desktop app released.
{
  const released = { kept: [], trashed: [], renamed: [], kept_unreadable: [], released: ["data.bin"] };
  answers = [response(409, { missing: ["b.tex"] }), response(200, released)];
  const before = calls.length;
  const result = await client.syncFolder({ tree: tree("one"), bindingId: "folder-3", opening: true, assets });
  assert.deepEqual(result, released);
  assert.equal(calls.length, before + 2);
  assert.equal(calls[before].opening, true);
  assert.equal(calls[before + 1].opening, true, "the retry is still the opening save");
  assert.deepEqual(calls[before].manifest, calls[before + 1].manifest);

  // The save after it carries no flag.
  await client.syncFolder({ tree: tree("two"), bindingId: "folder-3", assets });
  assert.equal(calls.at(-1).opening, undefined);
  await client.syncFolder({ tree: tree("three"), bindingId: "folder-3", opening: false, assets });
  assert.equal(calls.at(-1).opening, undefined, "a false flag is not sent");
}
console.log("folder-sync integration ok");
