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
  "wasmtex-pdftex-resolver-evidence.js", "wasmtex-kpse-resolve.js", "wasmtex-pdftex.fmt",
  "wasmtex-bibtex.worker.js", "wasmtex-bibtex.js", "wasmtex-bibtex.wasm",
  "LICENSE", "THIRD_PARTY_NOTICES.md", "SOURCE.md", "SOURCE-RECEIPT.json", "RELINK.md",
  "LICENSES/GPL-2.0.txt",
];
function fixture() {
  const files = names.map((name) => {
    const bytes = Buffer.from(`fixture ${name}`);
    mkdirSync(dirname(join(directory, name)), { recursive: true });
    writeFileSync(join(directory, name), bytes);
    return { name, bytes: bytes.length, sha256: sha(bytes) };
  });
  return {
    schemaVersion: 1, releaseGate: "passed", files,
    artifacts: files.filter(({ name }) => name.startsWith("wasmtex-")),
    families: [{ family: "pdftex", combinedTerms: "GPL-2.0-only" }],
    correspondingSource: { url: "https://example.org/source.tar.xz", sha256: "a".repeat(64) },
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
  manifest = fixture();
  manifest.correspondingSource.sha256 = null;
  assert.throws(() => readRelease(directory, pin(manifest)), /corresponding source/);
  console.log("wasm-latex release: pinned payload, notices, engine availability and invalid releases checked");
} finally {
  rmSync(directory, { recursive: true, force: true });
}
