// Behavioural checks for the browser's render-tree identity.
//
// This used to be the digest a bundle named itself by, computed from a
// hand-rolled JSON object that included compile settings and compared
// against a hash Rust asserted independently. That contract is gone with the
// bundle it named (SPEC-server-is-a-log §12): a reader now renders from the
// projection it fetched rather than from a stored page, and the only digest
// that crosses the wire is the projection digest `projection.js` computes
// straight off a `LoroDoc`, covered by `web/tests/unit/projection.mjs` and
// the shared fixture corpus.
//
// What is left here is `renderers.js`'s `snapshotDigest`: a transient
// browser-only cache key for a render tree (`{ main, texts, digests }`),
// used to tell whether a local build's job or a remembered preview still
// matches the source on screen. It borrows `projection.js`'s canonical
// serialization -- main path, then one line per file in UTF-8 order with its
// kind and content digest -- so there is one digest algorithm in this
// browser rather than two, but it is never compared against a server digest
// and never sent as one.
import { createHash } from "node:crypto";
import { snapshotDigest } from "../../src/lib/renderers.js";
import { canonicalBytes } from "../../src/lib/projection.js";

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
// that pins the shape rather than re-deriving it: main path, then one line
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

if (!process.exitCode) console.log("projection-digest: all checks passed");
