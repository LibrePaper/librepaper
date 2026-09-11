// Behavioural checks for the browser's rendering snapshot identity.
import { createHash } from "node:crypto";
import { snapshotDigest } from "../src/lib/tree-digest.js";
import { join } from "../src/lib/collab.js";

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

// Rust's BTreeMap orders UTF-8 bytes, and serde omits an empty asset id.
const canonical = JSON.stringify({
  main: "main.tex",
  files: {
    "é.tex": { kind: "text", id: "text-id", sha: textSha, size: Buffer.byteLength(text) },
    "图.png": { kind: "asset", sha: imageSha, size: image.byteLength },
  },
});
const actual = await snapshotDigest(tree);
const canonicalHash = "460596c52b6ba3f77aeddc5b579ad804dce0330ffe0235d44d489cc3819780d8";
check("the canonical fixture has the Rust hash", sha(Buffer.from(canonical)) === canonicalHash);
check(
  "matches the canonical Rust tree serialization",
  actual === canonicalHash,
  actual,
);

const changedId = await snapshotDigest({ ...tree, files: { ...tree.files, "é.tex": { kind: "text", id: "another-id" } } });
check("text identity participates in the digest", changedId !== actual);

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
const ten = { kind: "text", id: "ten-id", sha: sha(Buffer.from("ten")), size: 3 };
const two = { kind: "text", id: "two-id", sha: sha(Buffer.from("two")), size: 3 };
const proto = { kind: "text", id: "proto-id", sha: sha(Buffer.from("prototype")), size: 9 };
const numericCanonical = `{"main":"2","files":{"10":${JSON.stringify(ten)},"2":${JSON.stringify(two)},"__proto__":${JSON.stringify(proto)}}}`;
const numericHash = "551f66ceb8f7358e5e34c9b7752273eb3242862c2e6c5b34cc7d447101895dfd";
check(
  "preserves Rust ordering for numeric and prototype paths",
  sha(Buffer.from(numericCanonical)) === numericHash && numericDigest === numericHash,
  numericDigest,
);

// Exercise the producer used by Reader, including the stable Yjs file ids,
// rather than only handing the helper a hand-written renderer tree.
const sent = [];
const session = join({ send: (message) => sent.push(message), slug: "digest-check", mayEdit: true });
const tenId = session.addText("10", "ten");
const twoId = session.addText("2", "two");
session.setMain(twoId);
const liveTree = session.tree();
const liveDigest = await snapshotDigest(liveTree);
const liveCanonical = `{"main":"2","files":{"10":${JSON.stringify({ kind: "text", id: tenId, sha: sha(Buffer.from("ten")), size: 3 })},"2":${JSON.stringify({ kind: "text", id: twoId, sha: sha(Buffer.from("two")), size: 3 })}}}`;
check("matches the canonical serialization of an actual collab tree", liveDigest === sha(Buffer.from(liveCanonical)));
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
const engineOnlyCanonical = `{"main":"main.tex","files":{"é.tex":${JSON.stringify({ kind: "text", id: "text-id", sha: textSha, size: Buffer.byteLength(text) })},"图.png":${JSON.stringify({ kind: "asset", sha: imageSha, size: image.byteLength })}},"settings":{"engine":"pdflatex"}}`;
check(
  "an engine alone is written without a release key",
  engineOnly === sha(Buffer.from(engineOnlyCanonical)),
  engineOnly,
);

const both = await snapshotDigest({
  ...tree,
  settings: { engine: "xelatex", release: "2026-8b7946970153c52e+2026-ba38749b8714505a" },
});
const bothCanonical = `{"main":"main.tex","files":{"é.tex":${JSON.stringify({ kind: "text", id: "text-id", sha: textSha, size: Buffer.byteLength(text) })},"图.png":${JSON.stringify({ kind: "asset", sha: imageSha, size: image.byteLength })}},"settings":{"engine":"xelatex"}}`;
check(
  "the release pin is ignored in the snapshot",
  both === sha(Buffer.from(bothCanonical)),
  both,
);
check("engine participates in the digest", both !== noSettings && engineOnly !== noSettings);

if (!process.exitCode) console.log("tree-digest: all checks passed");
