// The page sends a renderer worker only what changed since its last request,
// the worker mirrors the tree, and each module takes only what differs. Every
// step must leave the module holding exactly what a full hand-over would.
import assert from "node:assert/strict";
import { treeDelta } from "../../src/lib/renderer-client.js";
import { applyTree, call, emptyMirror, handOver, handOverChanged } from "../../src/lib/renderer-wasm.js";

const dec = new TextDecoder();
const plain = (map) => ({ ...map });

// A module whose exports record what the hand-over does to it. Strings are
// written into its memory the way `call` expects, and read back here.
function fakeWasm() {
  const memory = new WebAssembly.Memory({ initial: 1 });
  let bump = 0;
  const log = [];
  const files = new Map();
  const urls = new Map();
  const state = { main: "", failNextAdd: false };
  const read = (pointer, length) => new Uint8Array(memory.buffer, pointer, length).slice();
  const wasm = {
    memory,
    alloc(length) {
      const pointer = bump;
      bump += length;
      const pages = Math.ceil(bump / 65536) - memory.buffer.byteLength / 65536;
      if (pages > 0) memory.grow(pages);
      return pointer;
    },
    dealloc() {},
    ok: () => 1,
    output_kind: () => 0,
    output_ptr: () => 0,
    diagnostics: () => 0,
    diagnostics_ptr: () => 0,
    clear_files() {
      log.push("clear_files");
      files.clear();
    },
    add_file(namePointer, nameLength, bodyPointer, bodyLength) {
      const name = dec.decode(read(namePointer, nameLength));
      if (state.failNextAdd) {
        state.failNextAdd = false;
        throw new Error("add_file failed");
      }
      files.set(name, read(bodyPointer, bodyLength));
      log.push(`add_file ${name}`);
    },
    set_asset_url(pathPointer, pathLength, urlPointer, urlLength) {
      const name = dec.decode(read(pathPointer, pathLength));
      urls.set(name, dec.decode(read(urlPointer, urlLength)));
      log.push(`set_asset_url ${name}`);
    },
    set_main(pointer, length) {
      state.main = dec.decode(read(pointer, length));
      log.push("set_main");
    },
  };
  return { wasm, log, files, urls, state, main: () => state.main };
}

// What a module holds, as comparable plain data: each file's bytes as a list.
const contents = (files) => Object.fromEntries([...files].map(([name, bytes]) => [name, [...bytes].join(",")]));

const figure = new Uint8Array([137, 80, 78, 71]);
const base = () => ({
  main: "a.typ",
  settings: { theme: "light" },
  texts: { "a.typ": "A", "b.typ": "B" },
  assets: { "f.png": figure },
  urls: { "f.png": "blob:1" },
});

// treeDelta: what the page sends.
{
  const first = base();
  const one = treeDelta(null, first);
  assert.equal(one.delta.full, true);
  assert.deepEqual(plain(one.delta.texts), { "a.typ": "A", "b.typ": "B" });
  assert.deepEqual(plain(one.delta.assets), { "f.png": figure });
  assert.deepEqual(plain(one.delta.urls), { "f.png": "blob:1" });
  assert.deepEqual(one.delta.removed, { texts: [], assets: [], urls: [] });

  const same = treeDelta(one.sent, base());
  assert.equal(same.delta.full, false);
  assert.deepEqual(plain(same.delta.texts), {});
  assert.deepEqual(plain(same.delta.assets), {});
  assert.deepEqual(plain(same.delta.urls), {});
  assert.deepEqual(same.delta.removed, { texts: [], assets: [], urls: [] });

  const edited = { ...base(), texts: { "a.typ": "A2", "b.typ": "B" } };
  assert.deepEqual(plain(treeDelta(same.sent, edited).delta.texts), { "a.typ": "A2" });

  const added = { ...base(), assets: { "f.png": figure, "g.png": new Uint8Array([1]) } };
  const addedDelta = treeDelta(same.sent, added).delta;
  assert.deepEqual(Object.keys(addedDelta.assets), ["g.png"]);
  assert.deepEqual(addedDelta.removed.assets, []);

  // The same bytes under a new reference are a change: the page does not
  // compare contents, so a reference miss only costs a resend.
  const copied = { ...base(), assets: { "f.png": new Uint8Array(figure) } };
  assert.deepEqual(Object.keys(treeDelta(same.sent, copied).delta.assets), ["f.png"]);

  const removed = { ...base(), texts: { "a.typ": "A" } };
  const removedDelta = treeDelta(same.sent, removed).delta;
  assert.deepEqual(removedDelta.removed.texts, ["b.typ"]);
  assert.deepEqual(plain(removedDelta.texts), {});

  const renamed = { ...base(), texts: { "a.typ": "A", "c.typ": "B" } };
  const renamedDelta = treeDelta(same.sent, renamed).delta;
  assert.deepEqual(renamedDelta.removed.texts, ["b.typ"]);
  assert.deepEqual(plain(renamedDelta.texts), { "c.typ": "B" });

  const moved = { ...base(), urls: { "f.png": "blob:2" } };
  const movedDelta = treeDelta(same.sent, moved).delta;
  assert.deepEqual(plain(movedDelta.urls), { "f.png": "blob:2" });
  assert.deepEqual(movedDelta.removed.urls, []);

  const unlinked = { ...base(), urls: {} };
  assert.deepEqual(treeDelta(same.sent, unlinked).delta.removed.urls, ["f.png"]);

  // `sent` holds copies of the entries, not the caller's objects.
  const source = base();
  const kept = treeDelta(null, source).sent;
  source.texts["a.typ"] = "changed after the call";
  assert.equal(kept.texts.get("a.typ"), "A");
}

// applyTree: applying a sequence of deltas gives exactly the full tree each time.
{
  const trees = [
    base(),
    { ...base(), texts: { "a.typ": "A2", "b.typ": "B" } },
    { ...base(), texts: { "a.typ": "A2" }, assets: {} , urls: {} },
    { ...base(), texts: { "a.typ": "A2", "z.typ": "Z" }, assets: { "f.png": figure }, urls: { "f.png": "blob:9" } },
    { main: "a.typ", settings: {}, texts: { "a.typ": "A3" }, assets: {}, urls: {} },
  ];
  let sent = null;
  let mirror = emptyMirror();
  for (const tree of trees) {
    const step = treeDelta(sent, tree);
    sent = step.sent;
    const applied = applyTree(mirror, step.delta);
    mirror = applied.mirror;
    assert.equal(applied.tree.main, tree.main);
    assert.deepEqual(plain(applied.tree.texts), plain(tree.texts));
    assert.deepEqual(plain(applied.tree.assets), plain(tree.assets));
    assert.deepEqual(plain(applied.tree.urls), plain(tree.urls));
  }
}

// handOverChanged: what one module receives.
{
  const f = fakeWasm();
  handOverChanged(f.wasm, base());
  assert.equal(f.log[0], "clear_files");
  assert.deepEqual(f.log.slice(1), ["add_file a.typ", "add_file b.typ", "add_file f.png", "set_asset_url f.png", "set_main"]);

  f.log.length = 0;
  handOverChanged(f.wasm, base());
  assert.deepEqual(f.log, ["set_main"]);

  f.log.length = 0;
  handOverChanged(f.wasm, { ...base(), texts: { "a.typ": "A2", "b.typ": "B" } });
  assert.deepEqual(f.log, ["add_file a.typ", "set_main"]);

  f.log.length = 0;
  handOverChanged(f.wasm, { ...base(), texts: { "a.typ": "A2", "b.typ": "B" }, urls: { "f.png": "blob:2" } });
  assert.deepEqual(f.log, ["set_asset_url f.png", "set_main"]);

  f.log.length = 0;
  handOverChanged(f.wasm, { ...base(), texts: { "a.typ": "A2" }, urls: { "f.png": "blob:2" } });
  assert.equal(f.log[0], "clear_files");
  assert.deepEqual(Object.keys(contents(f.files)).sort(), ["a.typ", "f.png"]);
}

// A package the module fetched is not in the tree: it survives changes to the
// tree, and only a path the tree had going away forces a full hand-over.
{
  const f = fakeWasm();
  handOverChanged(f.wasm, base());
  call(f.wasm, "add_file", "pkg/typst.toml", new Uint8Array([1]));
  f.log.length = 0;
  handOverChanged(f.wasm, { ...base(), texts: { "a.typ": "A4", "b.typ": "B" } });
  assert.deepEqual(f.log, ["add_file a.typ", "set_main"]);
  assert.ok(f.files.has("pkg/typst.toml"));
  f.log.length = 0;
  handOverChanged(f.wasm, { ...base(), texts: { "a.typ": "A4", "b.typ": "B" } });
  assert.deepEqual(f.log, ["set_main"]);
  assert.ok(f.files.has("pkg/typst.toml"));
}

// A failed hand-over leaves no record, so the next one is a full hand-over.
{
  const f = fakeWasm();
  handOverChanged(f.wasm, base());
  f.log.length = 0;
  f.state.failNextAdd = true;
  assert.throws(() => handOverChanged(f.wasm, { ...base(), texts: { "a.typ": "A5", "b.typ": "B" } }), /add_file failed/);
  f.log.length = 0;
  handOverChanged(f.wasm, { ...base(), texts: { "a.typ": "A5", "b.typ": "B" } });
  assert.equal(f.log[0], "clear_files");
}

// Equivalence: after each step the incremental module holds what a full
// hand-over would give it.
{
  const trees = [
    base(),
    { ...base(), texts: { "a.typ": "A2", "b.typ": "B" } },
    { ...base(), texts: { "a.typ": "A2", "b.typ": "B", "d.typ": "D" } },
    { ...base(), texts: { "a.typ": "A2", "d.typ": "D" } },
    { ...base(), texts: { "a.typ": "A2", "e.typ": "D" } },
    { ...base(), texts: { "a.typ": "A2", "e.typ": "D" }, assets: { "f.png": figure, "g.png": new Uint8Array([7]) }, urls: { "f.png": "blob:3", "g.png": "blob:4" } },
    { main: "e.typ", settings: {}, texts: { "e.typ": "E" }, assets: {}, urls: {} },
    { main: "e.typ", settings: {}, texts: { "e.typ": "E", "a.typ": "A" }, assets: {}, urls: {} },
  ];
  const incremental = fakeWasm();
  const full = fakeWasm();
  for (const tree of trees) {
    handOverChanged(incremental.wasm, tree);
    handOver(full.wasm, tree);
    assert.deepEqual(contents(incremental.files), contents(full.files));
    assert.deepEqual(plain(Object.fromEntries(incremental.urls)), plain(Object.fromEntries(full.urls)));
    assert.equal(incremental.main(), full.main());
  }
}
