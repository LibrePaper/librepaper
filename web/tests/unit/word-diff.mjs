import assert from "node:assert/strict";
import { wordDiff } from "../../src/lib/word-diff.js";

const cfg = { scanLimit: 20000, timeout: 1000 };
const diff = (a, b) => wordDiff(a, b, cfg);
const apply = (a, b, changes) => {
  let out = "";
  let pos = 0;
  for (const c of changes) {
    out += a.slice(pos, c.fromA) + b.slice(c.fromB, c.toB);
    pos = c.toA;
  }
  return out + a.slice(pos);
};
const spans = (a, b, changes) => changes.map((c) => [a.slice(c.fromA, c.toA), b.slice(c.fromB, c.toB)]);

{
  const a = "second blue draft";
  const b = "current purple draft";
  assert.deepEqual(spans(a, b, diff(a, b)), [["second blue", "current purple"]]);
}

{
  const a = "The quick brown fox jumps over the lazy dog";
  const b = "The quick red fox jumps over the lazy dog";
  assert.deepEqual(spans(a, b, diff(a, b)), [["brown", "red"]]);
}

{
  const a = "one two three";
  const b = "one two extra three";
  const ch = diff(a, b);
  assert.equal(apply(a, b, ch), b);
  assert.equal(ch.length, 1);
  assert.ok(b.slice(ch[0].fromB, ch[0].toB).includes("extra"));
  assert.ok(!a.slice(ch[0].fromA, ch[0].toA).includes("one"));
}

{
  const a = "one two extra three";
  const b = "one two three";
  const ch = diff(a, b);
  assert.equal(apply(a, b, ch), b);
  assert.equal(ch.length, 1);
  assert.ok(a.slice(ch[0].fromA, ch[0].toA).includes("extra"));
  assert.ok(!b.slice(ch[0].fromB, ch[0].toB).includes("one"));
}

{
  const a = "Hello, world";
  const b = "Hello; world";
  assert.deepEqual(spans(a, b, diff(a, b)), [[",", ";"]]);
}

{
  const a = "first line\nsecond blue line\nthird";
  const b = "first line\nsecond purple line\nthird";
  const ch = diff(a, b);
  assert.equal(apply(a, b, ch), b);
  for (const c of ch) {
    assert.ok(!a.slice(c.fromA, c.toA).includes("\n"));
    assert.ok(!b.slice(c.fromB, c.toB).includes("\n"));
  }
}

{
  const a = "alpha beta gamma";
  const b = "alpha BETA gamma";
  assert.deepEqual(spans(a, b, diff(a, b)), [["beta", "BETA"]]);
  const c = "ab cd ef";
  const d = "xb cx ef";
  const ch = diff(c, d);
  assert.equal(ch.length, 1);
  assert.deepEqual(spans(c, d, ch), [["ab cd", "xb cx"]]);
}

{
  // Seeded generator, words + spaces + newlines + punctuation + an emoji.
  let seed = 12345;
  const rand = (n) => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed % n;
  };
  const vocab = ["alpha", "beta", "gamma", "delta", "x", "résumé", "42", " ", " ", "\n", ",", ".", "😀", "_id"];
  const make = () => Array.from({ length: 5 + rand(40) }, () => vocab[rand(vocab.length)]).join("");
  for (let i = 0; i < 40; i++) {
    const a = make();
    const b = rand(2) ? make() : a.split(" ").map((w) => (rand(4) ? w : w + "z")).join(" ");
    const ch = diff(a, b);
    assert.equal(apply(a, b, ch), b);
    let prev = 0;
    for (const c of ch) {
      assert.ok(c.fromA >= prev);
      prev = c.toA;
    }
  }
}
