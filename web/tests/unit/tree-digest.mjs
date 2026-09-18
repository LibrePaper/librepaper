// Behavioural checks for the browser's rendering snapshot identity.
//
// The digest is a contract with the server: a bundle names the source
// checkpoint it was rendered from by this value, and the server looks that
// checkpoint up by the digest its own `Tree::to_bytes` produces. The pinned
// hashes below are asserted from Rust as well, in `document::history`.
import { createHash } from "node:crypto";
import { snapshotDigest } from "../../src/lib/tree-digest.js";
import { join } from "../../src/lib/collab.js";

function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function check(what, condition, detail = "") {
  if (!condition) {
    console.error(`tree-digest: ${what}${detail ? ` (${detail})` : ""}`);
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
  assets: { "图.png": image },
  files: {
    "é.tex": { kind: "text", id: "text-id" },
    "图.png": { kind: "asset", id: "", size: image.byteLength },
  },
};

// Rust's BTreeMap orders UTF-8 bytes, and each entry is the `(kind, sha)`
// tuple `Tree::to_bytes` writes -- never the stored entry, whose `id` is
// minted per session and whose `size` follows from the bytes it names.
const canonical = JSON.stringify({
  main: "main.tex",
  files: {
    "é.tex": ["text", textSha],
    "图.png": ["asset", imageSha],
  },
});
const actual = await snapshotDigest(tree);
const canonicalHash = "0c041c37dc28ea9fa74f58ca267b637f57afa50098568e7425e49245cc5063e4";
check("the canonical fixture has the Rust hash", sha(Buffer.from(canonical)) === canonicalHash, sha(Buffer.from(canonical)));
check(
  "matches the canonical Rust tree serialization",
  actual === canonicalHash,
  actual,
);

// The stored entry's id and size are not what the document says: two sessions
// holding the same bytes must produce the same name for them.
const changedId = await snapshotDigest({ ...tree, files: { ...tree.files, "é.tex": { kind: "text", id: "another-id" } } });
check("text identity stays out of the digest", changedId === actual);

const changedText = await snapshotDigest({ ...tree, texts: { "é.tex": `${text}!` } });
check("source bytes participate in the digest", changedText !== actual);

const numeric = {
  main: "2",
  texts: { "10": "ten", "2": "two", ["__proto__"]: "prototype" },
  digests: {},
  files: {
    "10": { id: "ten-id" },
    "2": { id: "two-id" },
    ["__proto__"]: { id: "proto-id" },
  },
};
const numericDigest = await snapshotDigest(numeric);
const numericCanonical = `{"main":"2","files":{"10":${JSON.stringify(["text", sha(Buffer.from("ten"))])},"2":${JSON.stringify(["text", sha(Buffer.from("two"))])},"__proto__":${JSON.stringify(["text", sha(Buffer.from("prototype"))])}}}`;
const numericHash = "7ccdf7d3941897d88c4db258a9a4397c66f3982757e25171da439ec6a8bce533";
check(
  "preserves Rust ordering for numeric and prototype paths",
  sha(Buffer.from(numericCanonical)) === numericHash && numericDigest === numericHash,
  numericDigest,
);

// Exercise the producer used by Reader, including the tree a live session
// hands out, rather than only handing the helper a hand-written tree.
const sent = [];
const session = join({ send: (message) => sent.push(message), slug: "digest-check", mayEdit: true });
session.addText("10", "ten");
const twoId = session.addText("2", "two");
session.setMain(twoId);
const liveTree = session.tree();
const liveDigest = await snapshotDigest(liveTree);
const liveCanonical = `{"main":"2","files":{"10":${JSON.stringify(["text", sha(Buffer.from("ten"))])},"2":${JSON.stringify(["text", sha(Buffer.from("two"))])}}}`;
check("a live collab tree names itself as the server would", liveDigest === sha(Buffer.from(liveCanonical)), liveDigest);
session.leave();

// Compile settings, mirroring Rust's `Tree.settings: Option<CompileSettings>`.
// Absent, empty and unset must all serialize identically -- a checkpoint
// taken before settings existed must keep its sha -- and only the engine
// field is written. Compiler releases are selected from the mirror default
// and never participate in a document snapshot.
const noSettings = await snapshotDigest(tree);
const emptySettings = await snapshotDigest({ ...tree, settings: { engine: "", release: "" } });
check("an empty settings object changes nothing", emptySettings === noSettings);

const engineOnly = await snapshotDigest({ ...tree, settings: { engine: "pdflatex", release: "" } });
const engineOnlyCanonical = `{"main":"main.tex","files":{"é.tex":${JSON.stringify(["text", textSha])},"图.png":${JSON.stringify(["asset", imageSha])}},"settings":{"engine":"pdflatex"}}`;
check(
  "an engine alone is written without a release key",
  engineOnly === sha(Buffer.from(engineOnlyCanonical)),
  engineOnly,
);

const both = await snapshotDigest({
  ...tree,
  settings: { engine: "xelatex", release: "2026-8b7946970153c52e+2026-ba38749b8714505a" },
});
const bothCanonical = `{"main":"main.tex","files":{"é.tex":${JSON.stringify(["text", textSha])},"图.png":${JSON.stringify(["asset", imageSha])}},"settings":{"engine":"xelatex"}}`;
check(
  "the release pin is ignored in the snapshot",
  both === sha(Buffer.from(bothCanonical)),
  both,
);
check("engine participates in the digest", both !== noSettings && engineOnly !== noSettings);

if (!process.exitCode) console.log("tree-digest: all checks passed");
