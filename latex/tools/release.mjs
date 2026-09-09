// Consume a complete wasm-latex release without its build/source checkout.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

const ENGINE_FILE_SETS = {
  pdftex: {
    worker: "wasmtex-pdftex.worker.js",
    format: "wasmtex-pdftex.fmt",
    files: [
      "wasmtex-pdftex.worker.js",
      "wasmtex-pdftex.js",
      "wasmtex-pdftex.wasm",
      "wasmtex-pdftex-resolver-evidence.js",
      "wasmtex-kpse-resolve.js",
      "wasmtex-pdftex.fmt",
    ],
  },
  xetex: {
    worker: "wasmtex-xetex.worker.js",
    format: "wasmtex-xetex.fmt.gz",
    files: [
      "wasmtex-xetex.worker.js",
      "wasmtex-xetex.js",
      "wasmtex-xetex.wasm",
      "wasmtex-xetex-resolver-evidence.js",
      "wasmtex-xetex.fmt.gz",
    ],
  },
  dvipdfm: {
    worker: "wasmtex-dvipdfm.worker.js",
    files: ["wasmtex-dvipdfm.worker.js", "wasmtex-dvipdfm.js", "wasmtex-dvipdfm.wasm"],
  },
  luatex: {
    worker: "wasmtex-luatex.worker.js",
    format: "wasmtex-luatex.fmt.gz",
    files: [
      "wasmtex-luatex.worker.js",
      "wasmtex-luatex.js",
      "wasmtex-luatex.wasm",
      "wasmtex-luatex-resolver-evidence.js",
      "wasmtex-luatex.fmt.gz",
    ],
  },
  bibtex: {
    worker: "wasmtex-bibtex.worker.js",
    files: ["wasmtex-bibtex.worker.js", "wasmtex-bibtex.js", "wasmtex-bibtex.wasm"],
  },
  bibtex8: {
    worker: "wasmtex-bibtex8.worker.js",
    files: ["wasmtex-bibtex8.worker.js", "wasmtex-bibtex8.js", "wasmtex-bibtex8.wasm"],
  },
  makeindex: {
    worker: "wasmtex-makeindex.worker.js",
    files: ["wasmtex-makeindex.worker.js", "wasmtex-makeindex.js", "wasmtex-makeindex.wasm"],
  },
};

export function readRelease(directory, expectedDigest) {
  if (!directory || !/^[a-f0-9]{64}$/.test(expectedDigest || "")) {
    throw new Error("wasmtex: --release needs a staged wasm-latex directory and --sha256 its reviewed MANIFEST.json digest");
  }
  const raw = readFileSync(join(directory, "MANIFEST.json"));
  if (sha256(raw) !== expectedDigest) throw new Error("wasmtex: release manifest digest mismatch");
  const manifest = JSON.parse(raw);
  if (manifest.schemaVersion !== 1 || manifest.releaseGate !== "passed" || !Array.isArray(manifest.files) || !manifest.files.length || !Array.isArray(manifest.artifacts) || !Array.isArray(manifest.families)) {
    throw new Error("wasmtex: stage this release with wasm-latex/tools/stage-release.mjs and resolve its release gate failures first");
  }
  if (!/^https:\/\//.test(manifest.correspondingSource?.url || "") ||
      !/^[a-f0-9]{64}$/.test(manifest.correspondingSource?.sha256 || "")) {
    throw new Error("wasmtex: release must name and hash its published corresponding source");
  }
  const files = new Map();
  for (const spec of manifest.files) {
    if (typeof spec.name !== "string" || !/^[A-Za-z0-9_.\/-]+$/.test(spec.name) ||
        spec.name.split("/").some((part) => !part || part === "." || part === "..") || files.has(spec.name)) {
      throw new Error("wasmtex: invalid or duplicate release file path");
    }
    const bytes = readFileSync(join(directory, spec.name));
    if (bytes.length !== spec.bytes || sha256(bytes) !== spec.sha256) {
      throw new Error(`wasmtex: release file size or digest mismatch: ${spec.name}`);
    }
    files.set(spec.name, bytes);
  }
  for (const artifact of manifest.artifacts || []) {
    const bytes = files.get(artifact.name);
    if (!bytes || bytes.length !== artifact.bytes || sha256(bytes) !== artifact.sha256) {
      throw new Error(`wasmtex: artifact is not in the verified payload: ${artifact.name}`);
    }
  }
  for (const name of ["LICENSE", "THIRD_PARTY_NOTICES.md", "SOURCE.md", "SOURCE-RECEIPT.json", "RELINK.md"]) {
    if (!files.has(name)) throw new Error(`wasmtex: release is missing ${name}`);
  }
  const artifacts = new Set(manifest.artifacts.map((file) => file.name));
  const engines = {};
  for (const [name, spec] of Object.entries(ENGINE_FILE_SETS)) {
    if (spec.files.every((file) => artifacts.has(file))) engines[name] = { ...spec };
  }
  if (!engines.pdftex) throw new Error("wasmtex: release has no complete pdfTeX engine and format");
  // Keep the manifest itself beside the payload for provenance. Its digest
  // names the mirror directory, so different releases never overwrite it.
  files.set("MANIFEST.json", raw);
  return { manifest, files, digest: expectedDigest, engines };
}
