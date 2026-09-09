import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { readRelease } from "./release.mjs";

const directory = mkdtempSync(join(tmpdir(), "librepaper-release-"));
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const names = [
  "wasmtex-pdftex.worker.js", "wasmtex-pdftex.js", "wasmtex-pdftex.wasm",
  "wasmtex-pdftex-resolver-evidence.js", "wasmtex-kpse-resolve.js", "wasmtex-bundle-mode.js",
  "wasmtex-pdftex.fmt",
  "wasmtex-bibtex.worker.js", "wasmtex-bibtex.js", "wasmtex-bibtex.wasm",
  "LICENSE", "THIRD_PARTY_NOTICES.md", "SOURCE.md", "SOURCE-RECEIPT.json", "RELINK.md",
  "LICENSES/GPL-2.0.txt",
];

// SPEC-latex.md "The index": a release may carry `bundles/bundles.json` plus
// digest-named bundle tars under `bundles/b/<sha256>/<slug>.tar`, all listed
// generically in `manifest.files` like every other payload file -- these two
// tiny "tars" are just bytes for hashing purposes here, since `readRelease`
// never opens them, only verifies size and digest.
const bundleTars = {
  core: Buffer.from("fixture core.tar bytes"),
  tikz: Buffer.from("fixture tex-latex-tikz.tar bytes"),
};
function bundlesIndex() {
  return JSON.stringify({
    schemaVersion: 1,
    snapshot: "texlive-20260301",
    sourceDateEpoch: 1,
    bundles: {
      core: { url: `b/${sha(bundleTars.core)}/core.tar`, size: bundleTars.core.length, sha256: sha(bundleTars.core), files: 1 },
      "tex/latex/tikz": { url: `b/${sha(bundleTars.tikz)}/tex-latex-tikz.tar`, size: bundleTars.tikz.length, sha256: sha(bundleTars.tikz), files: 1 },
    },
    files: { "tex/latex/base/article.cls": "core", "tex/generic/pgf/basiclayer/pgfcore.code.tex": "tex/latex/tikz" },
  });
}
function fixture({ bundles = false } = {}) {
  const extra = [];
  if (bundles) {
    const index = bundlesIndex();
    extra.push({ name: "bundles/bundles.json", bytes: Buffer.byteLength(index), content: Buffer.from(index) });
    extra.push({ name: `bundles/b/${sha(bundleTars.core)}/core.tar`, content: bundleTars.core });
    extra.push({ name: `bundles/b/${sha(bundleTars.tikz)}/tex-latex-tikz.tar`, content: bundleTars.tikz });
  }
  const files = [...names.map((name) => ({ name, content: Buffer.from(`fixture ${name}`) })), ...extra].map(
    ({ name, content }) => {
      mkdirSync(dirname(join(directory, name)), { recursive: true });
      writeFileSync(join(directory, name), content);
      return { name, bytes: content.length, sha256: sha(content) };
    },
  );
  return {
    schemaVersion: 1, releaseGate: "passed", files,
    artifacts: files.filter(({ name }) => name.startsWith("wasmtex-")),
    families: [{ family: "pdftex", combinedTerms: "GPL-2.0-only" }],
    correspondingSource: { url: "https://example.org/source.tar.xz", sha256: "a".repeat(64) },
    bundles: bundles
      ? { index: "bundles/bundles.json", sha256: sha(Buffer.from(bundlesIndex())), snapshot: "texlive-20260301", count: 2, bytes: bundleTars.core.length + bundleTars.tikz.length, receipt: "bundles/RECEIPT-FILES.json" }
      : null,
  };
}
function pin(manifest) {
  const raw = JSON.stringify(manifest);
  writeFileSync(join(directory, "MANIFEST.json"), raw);
  return sha(raw);
}
try {
  let manifest = fixture();
  let digest = pin(manifest);
  const release = readRelease(directory, digest);
  assert.deepEqual(Object.keys(release.engines), ["pdftex", "bibtex"]);
  assert.ok(release.files.has("LICENSES/GPL-2.0.txt"));
  assert.ok(release.files.has("MANIFEST.json"));
  assert.throws(() => readRelease(directory, "0".repeat(64)), /manifest digest mismatch/);
  assert.throws(() => readRelease(directory, ""), /reviewed.*digest/);
  writeFileSync(join(directory, "LICENSE"), "changed notice");
  assert.throws(() => readRelease(directory, digest), /digest mismatch: LICENSE/);
  manifest = fixture();
  delete manifest.releaseGate;
  assert.throws(() => readRelease(directory, pin(manifest)), /release gate failures/);
  manifest = fixture();
  manifest.files.push({ ...manifest.files[0], name: "../escape" });
  assert.throws(() => readRelease(directory, pin(manifest)), /invalid or duplicate/);
  manifest = fixture();
  manifest.files.push(manifest.files[0]);
  assert.throws(() => readRelease(directory, pin(manifest)), /invalid or duplicate/);
  manifest = fixture();
  manifest.artifacts = manifest.artifacts.filter(({ name }) => !name.endsWith(".fmt"));
  assert.throws(() => readRelease(directory, pin(manifest)), /complete pdfTeX/);

  // A release built before the bundle-mode controller shipped (no
  // `wasmtex-bundle-mode.js` in its verified payload) is not advertised as
  // pdfTeX-capable at all -- intended behaviour, since this host's worker
  // protocol unconditionally importScripts()s it and an old release cannot
  // answer `loadbundleindex`.
  manifest = fixture();
  manifest.artifacts = manifest.artifacts.filter(({ name }) => name !== "wasmtex-bundle-mode.js");
  assert.throws(() => readRelease(directory, pin(manifest)), /complete pdfTeX/);
  manifest = fixture();
  manifest.correspondingSource.sha256 = null;
  assert.throws(() => readRelease(directory, pin(manifest)), /corresponding source/);

  // A release carrying bundles: the two tars and their index verify and
  // read back byte-for-byte through the same generic payload check as every
  // other file, and `manifest.bundles` (wasmtex.mjs's job to interpret, not
  // readRelease's) passes through unexamined.
  manifest = fixture({ bundles: true });
  digest = pin(manifest);
  const bundled = readRelease(directory, digest);
  assert.ok(bundled.files.has("bundles/bundles.json"));
  assert.equal(bundled.manifest.bundles.snapshot, "texlive-20260301");
  assert.equal(bundled.manifest.bundles.count, 2);
  const coreTarName = `bundles/b/${sha(bundleTars.core)}/core.tar`;
  assert.deepEqual(bundled.files.get(coreTarName), bundleTars.core);
  // A bundle tar whose bytes were tampered with after staging is rejected
  // exactly like any other payload file (SPEC-latex.md: "the same thing on
  // the other side, as it does for engines today").
  writeFileSync(join(directory, coreTarName), "corrupted");
  assert.throws(() => readRelease(directory, digest), new RegExp(`digest mismatch: ${coreTarName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));

  console.log("wasm-latex release: pinned payload, notices, engine availability, bundles and invalid releases checked");
} finally {
  rmSync(directory, { recursive: true, force: true });
}
