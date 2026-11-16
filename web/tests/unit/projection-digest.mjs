// Behavioural checks for the browser's render-tree identity.
//
// This used to be the digest a bundle named itself by, computed from a
// hand-rolled JSON object that included compile settings and compared
// against a hash Rust asserted independently. That contract is gone with the
// bundle it named: a reader now renders from the
// projection it fetched rather than from a stored page, and the only digest
// that crosses the wire is the projection digest `projection.js` computes
// straight off a `LoroDoc`, covered by `web/tests/unit/projection.mjs` and
// the shared fixture corpus.
//
// What is left here is `renderers.js`'s `snapshotDigest`: a transient
// browser-only cache key for a render tree (`{ main, texts, digests }`),
// used to tell whether a local build's job or a remembered preview still
// matches the source on screen. It borrows `projection.js`'s canonical
// serialization -- entry line, then one line per file in UTF-8 order with its
// kind and content digest -- so there is one digest algorithm in this
// browser rather than two, but it is never compared against a server digest
// and never sent as one.
import { createHash } from "node:crypto";
import { snapshotDigest } from "../../src/lib/renderers.js";
import { canonicalBytes } from "../../src/lib/projection.js";
import { __resetTextDigests, compareUtf8 } from "../../src/lib/projection-digest.js";

function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function check(what, condition, detail = "") {
  if (!condition) {
    console.error(`projection-digest: ${what}${detail ? ` (${detail})` : ""}`);
    process.exitCode = 1;
  }
}

const text = "Résumé — 𝜆\n";
const image = new Uint8Array([0, 1, 2, 255]);
const textSha = sha(Buffer.from(text));
const imageSha = sha(image);
const tree = {
  main: "main.tex",
  texts: { "é.tex": text },
  digests: { "图.png": imageSha },
};

// The canonical bytes are `projection.js`'s own, so this is the one place
// that pins the shape rather than re-deriving it: entry line, then one line
// per file in UTF-8 path order, tab-separated kind and digest.
const canonical = canonicalBytes({
  main: "main.tex",
  files: [
    ["é.tex", { kind: "text", digest: textSha }],
    ["图.png", { kind: "asset", digest: imageSha }],
  ],
});
const actual = await snapshotDigest(tree);
check("matches projection.js's canonical serialization", actual === sha(canonical), actual);

// Source bytes participate; nothing else about a text container -- an id
// minted per session, a byte count that follows from the text -- does.
const changedText = await snapshotDigest({ ...tree, texts: { "é.tex": `${text}!` } });
check("source bytes participate in the digest", changedText !== actual);

const unchanged = await snapshotDigest({ ...tree });
check("the same tree names itself the same way twice", unchanged === actual);

// JSON.stringify reorders integer-looking keys; the canonical form does not,
// because it is not JSON. Numeric and prototype-shaped paths must still sort
// and serialize by their UTF-8 bytes, exactly as `projection.js` does.
const numeric = {
  main: "2",
  texts: { "10": "ten", "2": "two", ["__proto__"]: "prototype" },
  digests: {},
};
const numericDigest = await snapshotDigest(numeric);
const numericCanonical = canonicalBytes({
  main: "2",
  files: [
    ["10", { kind: "text", digest: sha(Buffer.from("ten")) }],
    ["2", { kind: "text", digest: sha(Buffer.from("two")) }],
    ["__proto__", { kind: "text", digest: sha(Buffer.from("prototype")) }],
  ],
});
check(
  "preserves projection.js's UTF-8 ordering for numeric and prototype paths",
  numericDigest === sha(numericCanonical),
  numericDigest,
);

// Compile settings are gone from this digest along with the bundle they used
// to identify: a job's actual settings still travel with the tree sent to
// build it, so a settings-only change is deliberately allowed to share a
// snapshot with the tree it changed nothing else about (renderers.js).
const withSettings = await snapshotDigest({ ...tree, settings: { engine: "pdflatex" } });
check("compile settings do not participate in the digest", withSettings === actual);

// The per-path memo of text digests must never change an answer. Each fixture
// gives the same digest with a cold memo and a warm one.
const fixtures = { tree, numeric };
for (const [name, fixture] of Object.entries(fixtures)) {
  __resetTextDigests();
  const cold = await snapshotDigest(fixture);
  const warm = await snapshotDigest(fixture);
  check(`${name}: cold and warm memo give the same digest`, cold === warm, cold);
  check(`${name}: warm memo matches the canonical digest`, warm === sha(canonicalBytes({
    main: fixture.main,
    files: Object.keys({ ...fixture.texts, ...fixture.digests }).sort((a, b) =>
      Buffer.compare(Buffer.from(a), Buffer.from(b)),
    ).map((path) =>
      Object.hasOwn(fixture.texts, path)
        ? [path, { kind: "text", digest: sha(Buffer.from(fixture.texts[path])) }]
        : [path, { kind: "asset", digest: String(fixture.digests[path]) }],
    ),
  })), warm);
}

// Changing one text changes the digest, and changing it back restores the
// original exactly: a memo that returned a stale hash would fail here.
__resetTextDigests();
const original = await snapshotDigest(tree);
const edited = await snapshotDigest({ ...tree, texts: { "é.tex": `${text}?` } });
const restored = await snapshotDigest({ ...tree, texts: { "é.tex": text } });
check("editing one text changes the digest under a warm memo", edited !== original, edited);
check("restoring the text restores the original digest exactly", restored === original, restored);

// Two trees sharing a path but holding different texts, called alternately,
// must each keep their own correct digest.
__resetTextDigests();
const treeA = { main: "main.tex", texts: { "a.tex": "alpha" }, digests: {} };
const treeB = { main: "main.tex", texts: { "a.tex": "beta" }, digests: {} };
const expectedA = sha(canonicalBytes({ main: "main.tex", files: [["a.tex", { kind: "text", digest: sha(Buffer.from("alpha")) }]] }));
const expectedB = sha(canonicalBytes({ main: "main.tex", files: [["a.tex", { kind: "text", digest: sha(Buffer.from("beta")) }]] }));
for (let round = 0; round < 3; round += 1) {
  check("alternating trees: first tree keeps its own digest", (await snapshotDigest(treeA)) === expectedA, `round ${round}`);
  check("alternating trees: second tree keeps its own digest", (await snapshotDigest(treeB)) === expectedB, `round ${round}`);
}

// compareUtf8 must agree in sign with the byte-wise reference it replaced.
const encoder = new TextEncoder();
function referenceCompare(left, right) {
  const a = encoder.encode(left);
  const b = encoder.encode(right);
  const length = Math.min(a.length, b.length);
  for (let at = 0; at < length; at += 1) {
    if (a[at] !== b[at]) return a[at] - b[at];
  }
  return a.length - b.length;
}
const orderPairs = [
  ["a", "b"],
  ["a", "ab"],
  ["ab", "a"],
  ["", "a"],
  ["same", "same"],
  ["café.tex", "cafe.tex"],
  ["é", "z"],
  ["图.png", "图.pdf"],
  ["中", "a"],
  ["😀.md", ".md"],
  ["😀.md", "�.md"],
  ["￿", "😀"],
  ["", "😀"],
  ["😀", "ﬀ"],
  ["x\uD800", "x"],
  ["x\uDC00y", "x😀"],
  ["\uD800", ""],
  ["\uDC00", "￿"],
  ["\uD800\uD800", "𐀀"],
];
for (const [left, right] of orderPairs) {
  const expected = Math.sign(referenceCompare(left, right));
  const actual = Math.sign(compareUtf8(left, right));
  check(
    `compareUtf8 sign agrees with the byte order for ${JSON.stringify([left, right])}`,
    actual === expected,
    `expected ${expected}, got ${actual}`,
  );
  const reversed = Math.sign(compareUtf8(right, left));
  check(
    `compareUtf8 is antisymmetric for ${JSON.stringify([left, right])}`,
    reversed === -expected,
    `expected ${-expected}, got ${reversed}`,
  );
}

if (!process.exitCode) console.log("projection-digest: all checks passed");
